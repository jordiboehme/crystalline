//! GitHub settings (admin): connection status and the poll, device-code
//! connect, PAT connect, disconnect. Everything here is admin-only and
//! token-material-free: mutations are refused under read_only, the status
//! GET is a pure read, and the status carries only where the credential
//! lives (keyring | file | environment) and whose it is.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::auth::Identity;
use super::{ApiError, ApiJson, ProblemDetail, RestState, refuse_read_only};

/// What `POST /settings/github/token` takes.
///
/// [`Debug`] is written by hand rather than derived, as on the password
/// bodies in [`super::users_api`]: the derived one would print the token, and
/// this type is one `tracing::debug!` or one `unwrap` on a rejection away
/// from a log file.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[schema(description = "A GitHub personal access token to connect with. \
                        Write-only: no response on this surface ever echoes \
                        it, and the status shape carries only where the \
                        credential lives and whose it is.")]
pub struct TokenBody {
    /// A GitHub personal access token. Write-only: no response ever echoes it.
    #[schema(write_only = true, example = "ghp_xxxxxxxxxxxxxxxxxxxx")]
    pub token: String,
}

impl std::fmt::Debug for TokenBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenBody")
            .field("token", &"<redacted>")
            .finish()
    }
}

/// The connection as the settings screen renders and polls it. Never carries
/// token material: the engine's own status struct, passed through.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "The GitHub connection as the settings screen renders \
                        and polls it. No token material, ever: only whether \
                        the feature is on, whether a credential is on file, \
                        whose it is and where it lives.")]
pub struct GithubStatusResponse {
    /// Whether `github.enabled` is on: team tools and origin polling.
    #[schema(example = true)]
    pub enabled: bool,
    /// Whether a credential is on file for this instance.
    #[schema(example = true)]
    pub connected: bool,
    /// The account login, when connected.
    #[schema(example = "octo")]
    pub user: Option<String>,
    /// keyring | file | environment; null when disconnected.
    #[schema(example = "keyring")]
    pub token_store: Option<String>,
    /// A device flow waiting for the browser side. Poll this route until it
    /// goes null; the flow either connected or set `error`.
    pub pending: Option<GithubPendingView>,
    /// A device flow's failure (expired, denied), reported on exactly one
    /// status read after the flow ends, then cleared.
    pub error: Option<String>,
}

/// The half of a running device flow a browser has to show.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "The half of a running device flow a browser has to \
                        show: the short code the user types in, where they \
                        type it, and how long the code stays valid.")]
pub struct GithubPendingView {
    /// The short code the user confirms at `verification_url`.
    #[schema(example = "ABCD-1234")]
    pub user_code: String,
    /// Where the user confirms the code.
    #[schema(example = "https://github.com/login/device")]
    pub verification_url: String,
    /// How many seconds from the flow's start the code stays valid.
    #[schema(example = 900)]
    pub expires_in_secs: u64,
}

fn view(c: crate::engine::GithubConnection) -> GithubStatusResponse {
    GithubStatusResponse {
        enabled: c.enabled,
        connected: c.connected,
        user: c.user,
        token_store: c.token_store,
        pending: c.pending.map(|p| GithubPendingView {
            user_code: p.user_code,
            verification_url: p.verification_url,
            expires_in_secs: p.expires_in_secs,
        }),
        error: c.error,
    }
}

/// The settings screen's Connect button IS the intent to use the feature,
/// so both connect paths flip github.enabled on first. (MCP's configure
/// keeps its advisory note instead: a tool call is not necessarily that
/// intent.) A failed flow leaves enabled + disconnected, exactly the state
/// the screen shows and retries from.
async fn ensure_enabled(state: &RestState) -> Result<(), ApiError> {
    if !state.engine.github_enabled() {
        state
            .engine
            .configure(&crate::engine::ConfigureAction::Set {
                key: "github.enabled".to_string(),
                value: "true".to_string(),
            })
            .await?;
    }
    Ok(())
}

/// `GET /settings/github` - the connection, and the device flow's poll.
///
/// A pure read, so a read-only instance serves it: read-only means writes are
/// refused, not that the settings screen goes dark. It doubles as the device
/// flow's poll, because a flow started by [`connect`] runs in the background
/// and lands its outcome in the engine: while it waits, `pending` carries the
/// code; once it ends, `pending` goes null and a failure is reported in
/// `error` on exactly that one read.
#[utoipa::path(
    get,
    path = "/api/v1/settings/github",
    tag = "settings",
    operation_id = "get_github_settings",
    summary = "The GitHub connection, and the device flow's poll.",
    description = "Admin only. A pure read, served even on a read-only \
                   instance. Also the device flow's poll: while one runs, \
                   `pending` carries the short code and its URL; once it \
                   ends, `pending` goes null and a failed flow's reason is \
                   reported in `error` on exactly one read, then cleared.",
    responses(
        (status = 200, description = "The connection. No token material.", body = GithubStatusResponse),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn status(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<GithubStatusResponse>, ApiError> {
    identity.require_admin()?;
    // A pure read: served under read_only (Group A: GET routes untouched).
    Ok(Json(view(state.engine.github_connection().await?)))
}

/// `POST /settings/github/connect` - start a device-code sign-in, answering
/// 202 with the code to confirm in a browser.
///
/// 202 rather than 200: the connection does not exist yet when this answers.
/// The flow runs in the background and its outcome shows up on
/// [`status`], which the screen polls. A second call while one flow is still
/// running reports that same flow rather than starting another (engine
/// behavior), so a double click cannot strand a code.
///
/// A previous flow's unconsumed failure may ride along in `error` beside the
/// fresh `pending` block, since the once-reported slot heals on this very
/// read. Harmless: `pending` is authoritative while a flow runs.
#[utoipa::path(
    post,
    path = "/api/v1/settings/github/connect",
    tag = "settings",
    operation_id = "connect_github_device",
    summary = "Start a device-code sign-in.",
    description = "Admin only, and refused on a read-only instance. Answers \
                   202 with the short code to confirm in a browser; the flow \
                   itself runs in the background and its outcome is read from \
                   `GET /settings/github`. A second call while one flow runs \
                   reports that same flow rather than starting another. \
                   Connecting is the intent to use the feature, so this turns \
                   `github.enabled` on.",
    responses(
        (
            status = 202,
            description = "The flow started (or one was already running): \
                           `pending` carries the code to confirm. Poll \
                           `GET /settings/github` for the outcome.",
            body = GithubStatusResponse,
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, a cookie session did \
                           not echo its CSRF token, this instance is \
                           read-only, or the trusted-header identity names a \
                           disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "This machine's identity is fixed by \
                           `CRYSTALLINE_GITHUB_TOKEN`, so no sign-in can be \
                           started here, or GitHub refused to start the flow.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn connect(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Response, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    ensure_enabled(&state).await?;
    // Starts a flow, or returns the one already pending (engine behavior).
    state.engine.start_device_connect(None).await?;
    let status = view(state.engine.github_connection().await?);
    Ok((StatusCode::ACCEPTED, Json(status)).into_response())
}

/// `POST /settings/github/token` - connect with a personal access token,
/// answering with the connection as it now stands.
///
/// The token is validated against GitHub before it is stored, so the account
/// the status names is the one the token actually belongs to rather than one
/// the caller claimed. Write-only in both directions: the response is the
/// same token-material-free status every other route here answers with.
#[utoipa::path(
    post,
    path = "/api/v1/settings/github/token",
    tag = "settings",
    operation_id = "connect_github_token",
    summary = "Connect with a personal access token.",
    description = "Admin only, and refused on a read-only instance. The token \
                   is validated against GitHub before it is stored, and is \
                   never echoed: the answer is the same token-material-free \
                   status the GET returns. Connecting is the intent to use \
                   the feature, so this turns `github.enabled` on.",
    request_body = TokenBody,
    responses(
        (status = 200, description = "The connection as it now stands.", body = GithubStatusResponse),
        (
            status = 400,
            description = "The body is not JSON.",
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
            description = "The caller is not an admin, a cookie session did \
                           not echo its CSRF token, this instance is \
                           read-only, or the trusted-header identity names a \
                           disabled account.",
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
                           empty, GitHub refused it, or this machine's \
                           identity is fixed by `CRYSTALLINE_GITHUB_TOKEN` \
                           and no token may be stored here.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn token(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<TokenBody>,
) -> Result<Json<GithubStatusResponse>, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    let token = body.token.trim();
    if token.is_empty() {
        return Err(ApiError::unprocessable("the token is empty"));
    }
    ensure_enabled(&state).await?;
    state.engine.connect_with_token(token, None).await?;
    Ok(Json(view(state.engine.github_connection().await?)))
}

/// `DELETE /settings/github` - forget the stored credential, answering with
/// the connection as it now stands.
///
/// Idempotent, like the method it is spelled with: disconnecting an instance
/// that holds no credential succeeds and answers `connected: false` rather
/// than 404. The screen can therefore offer the button without first having
/// to decide whether pressing it would be legal, and a retry after a dropped
/// response is safe.
///
/// `github.enabled` is left alone: turning the feature off is a separate
/// intent from forgetting who this machine signs in as.
#[utoipa::path(
    delete,
    path = "/api/v1/settings/github",
    tag = "settings",
    operation_id = "disconnect_github",
    summary = "Forget the stored credential.",
    description = "Admin only, and refused on a read-only instance. \
                   Idempotent: disconnecting an instance that holds no \
                   credential succeeds and answers `connected: false` rather \
                   than 404. `github.enabled` is left alone - turning the \
                   feature off is a separate intent.",
    responses(
        (status = 200, description = "The connection as it now stands.", body = GithubStatusResponse),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, a cookie session did \
                           not echo its CSRF token, this instance is \
                           read-only, or the trusted-header identity names a \
                           disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The credential comes from \
                           `CRYSTALLINE_GITHUB_TOKEN`: only the environment \
                           that set it can retire it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn disconnect(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<GithubStatusResponse>, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    state.engine.github_disconnect().await?;
    Ok(Json(view(state.engine.github_connection().await?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The token must never be one `tracing::debug!` away.
    #[test]
    fn a_debugged_token_body_redacts_the_token() {
        let body: TokenBody =
            serde_json::from_value(serde_json::json!({"token": "ghp_secret"})).unwrap();
        let text = format!("{body:?}");
        assert!(!text.contains("ghp_secret"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");
    }
}
