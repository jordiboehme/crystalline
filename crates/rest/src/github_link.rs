//! One account's link to a GitHub account through the instance's GitHub App,
//! made from the browser with the App's web flow.
//!
//! The flow is three hops. `POST /me/github-link/start` (a signed-in session,
//! under the CSRF check like every other state-changing route) mints a state,
//! remembers who started it and answers GitHub's authorize url. GitHub sends
//! the browser back to `GET /github/link/callback` with a code and that
//! state. The callback takes the record the state names, checks that the
//! request comes from the very session that started it, exchanges the code
//! and links the account, then sends the browser to the profile.
//!
//! The callback needs the session, and gets it because the session cookie
//! is `SameSite=Lax`, which rides GitHub's top-level redirect back. That is
//! the whole binding. It is on the guard's public path list only so a
//! browser without a session lands on the profile (which asks for a sign-in)
//! instead of a raw 401; the handler checks the account itself, and every
//! refusal is a redirect to the profile with a code, never a JSON body. A state carries no
//! account; the record does, together with a hash of the session that
//! started the flow (of its CSRF token, the one per-session value every
//! identity path resolves, never the raw session cookie), so a link can only
//! ever land on the account of the session that began it. A callback from
//! any other session, of another account or of the same one, is refused
//! before anything reaches GitHub.
//!
//! The state is random, kept only in this process, single use (taken, not
//! read) and lives ten minutes ([`PENDING_TTL`]). The code is never logged
//! and never echoed.

use std::sync::Mutex;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::auth::{Identity, no_store};
use super::github_settings::GithubPendingView;
use super::pending::{PENDING_TTL, PendingRecord, PendingStore, StateMiss};
use super::{ApiError, ApiQuery, ProblemDetail, RestState, refuse_read_only};
use crate::engine::{EngineError, GithubLinkView, GithubUnlink};

/// Where GitHub sends the browser back to, under `/api/v1`. Registered as the
/// App's callback url as `<service.public_url>/api/v1` + this.
pub const CALLBACK_PATH: &str = "/github/link/callback";

/// The web flows in flight, keyed by state.
#[derive(Default)]
pub struct GithubLinkFlows {
    pending: Mutex<PendingStore<GithubLinkPending>>,
}

/// One link in flight: everything the callback needs and the browser must
/// not be trusted to carry back.
struct GithubLinkPending {
    /// The account the link lands on: the account of the session that
    /// started it.
    account: String,
    /// The sha256 of the starting session's CSRF token (see [`session_hash`]).
    session_hash: String,
    /// The redirect uri that went out with the authorize url. GitHub compares
    /// the one in the code exchange against it, so it is stored rather than
    /// derived a second time.
    redirect_uri: String,
    /// When the flow started, on the engine's link clock.
    started: Instant,
}

impl PendingRecord for GithubLinkPending {
    fn started(&self) -> Instant {
        self.started
    }
}

/// The value a pending link is bound to: a hash of the session's CSRF token,
/// with a label so the digest is not the same value any other surface would
/// derive from it.
fn session_hash(csrf: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"crystalline github link session\0");
    hasher.update(csrf.as_bytes());
    crystalline_index::hex_lower(&hasher.finalize())
}

/// A fresh state: 32 random bytes, lowercase hex.
fn new_state() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS CSPRNG is available");
    crystalline_index::hex_lower(&bytes)
}

/// One account's GitHub link as the profile renders it. Carries no token
/// material.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "One account's link to a GitHub account through the \
                        instance's GitHub App: who it is linked to, since \
                        when, and whether the user token still works. No \
                        token material, ever.")]
pub struct GithubLinkResponse {
    /// Whether the account is linked.
    #[schema(example = true)]
    pub linked: bool,
    /// The GitHub login last seen for the linked id.
    #[schema(example = "octo")]
    pub login: Option<String>,
    /// GitHub's numeric user id.
    #[schema(example = 583231)]
    pub github_id: Option<u64>,
    /// `github.com` or the Enterprise Server host.
    #[schema(example = "github.com")]
    pub host: Option<String>,
    /// When the link was made, RFC 3339.
    #[schema(example = "2026-10-28T09:12:44Z")]
    pub linked_at: Option<String>,
    /// False once the user token is known to be gone (revoked, or its
    /// refresh token expired). The link stays; linking again restores it.
    #[schema(example = true)]
    pub token_live: bool,
    /// This account's link device flow waiting for the browser side, when
    /// one runs (the device flow is started from the CLI).
    pub pending: Option<GithubPendingView>,
    /// The last link device flow's failure, reported on exactly one read.
    pub error: Option<String>,
    /// Whether this instance has a GitHub App to link through. Any account
    /// may read it, unlike the admin-only settings read, so the profile picks
    /// its GitHub card by it.
    #[schema(example = true)]
    pub app_configured: bool,
}

fn view(v: GithubLinkView) -> GithubLinkResponse {
    GithubLinkResponse {
        linked: v.linked,
        login: v.login,
        github_id: v.github_id,
        host: v.host,
        linked_at: v.linked_at.map(|at| at.to_rfc3339()),
        token_live: v.token_live,
        pending: v.pending.map(|p| GithubPendingView {
            user_code: p.user_code,
            verification_url: p.verification_url,
            expires_in_secs: p.expires_in_secs,
        }),
        error: v.error,
        app_configured: v.app_configured,
    }
}

/// What `POST /me/github-link/start` answers: where to send the browser.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct StartGithubLinkResponse {
    /// GitHub's authorize url for this flow. The page navigates the browser
    /// there; GitHub sends it back to the callback.
    #[schema(
        example = "https://github.com/login/oauth/authorize?client_id=Iv23.x&redirect_uri=https%3A%2F%2Fkb.example.com%2Fapi%2Fv1%2Fgithub%2Flink%2Fcallback&state=..."
    )]
    pub authorize_url: String,
}

/// What an unlink answers.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct GithubUnlinkResponse {
    /// Always false: the account is not linked any more.
    #[schema(example = false)]
    pub linked: bool,
    /// Whether there was a link to remove.
    #[schema(example = true)]
    pub was_linked: bool,
    /// Set when the link is gone here but GitHub did not revoke the token:
    /// the person should revoke it in their GitHub settings.
    #[schema(
        example = "unlinked here; GitHub could not be reached to revoke the token, revoke it in your GitHub settings"
    )]
    pub warning: Option<String>,
}

/// The answer for an unlink outcome. Shared with the admin route in
/// [`super::users_api`].
pub(crate) fn unlink_response(outcome: GithubUnlink) -> GithubUnlinkResponse {
    GithubUnlinkResponse {
        linked: false,
        was_linked: outcome != GithubUnlink::NotLinked,
        warning: outcome.warning().map(str::to_string),
    }
}

/// `GET /me/github-link` - the caller's own GitHub link.
#[utoipa::path(
    get,
    path = "/api/v1/me/github-link",
    tag = "settings",
    operation_id = "get_my_github_link",
    summary = "The caller's own link to a GitHub account through the App.",
    description = "Any signed-in account, viewers included: the link says who \
                   the person is at GitHub. A pure read, served even on a \
                   read-only instance. No token material.",
    responses(
        (status = 200, description = "The link.", body = GithubLinkResponse),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
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
pub async fn status(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<GithubLinkResponse>, ApiError> {
    let user = identity.require_account()?;
    Ok(Json(view(
        state.engine.github_link_status(&user.name).await?,
    )))
}

/// `POST /me/github-link/start` - start linking the caller's account through
/// the App's web flow.
#[utoipa::path(
    post,
    path = "/api/v1/me/github-link/start",
    tag = "settings",
    operation_id = "start_my_github_link",
    summary = "Start linking the caller's account to GitHub through the App's web flow.",
    description = "Any signed-in account, and refused on a read-only \
                   instance. Answers GitHub's authorize url; the page sends \
                   the browser there, and GitHub sends it back to \
                   `GET /github/link/callback`, which links the account of \
                   the session that started this and nobody else's. The \
                   state lives ten minutes and is good for one callback.",
    responses(
        (status = 200, description = "Where to send the browser.", body = StartGithubLinkResponse),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "A cookie session did not echo its CSRF token, this \
                           instance is read-only, or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "`service.public_url` is not set, so there is no \
                           callback address to give GitHub, or the GitHub App \
                           has no client secret, so a code cannot be \
                           exchanged. The detail names the fix.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "No GitHub App is configured on this instance, or \
                           it cannot be used. The detail names the fix.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 503,
            description = "Too many links are in flight at once; try again \
                           in a moment.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn start(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Response, ApiError> {
    let user = identity.require_account()?;
    refuse_read_only(&state)?;
    // The guard's CSRF check already required a token for this POST; a
    // missing one here would mean no session to bind the flow to.
    let csrf = identity.csrf.as_deref().ok_or_else(|| {
        ApiError::forbidden("this request carries no session to bind the link to: log in again")
    })?;
    let app = state.engine.github_link_app()?;
    let credentials = app.credentials();
    if credentials.client_secret.is_none() {
        return Err(ApiError::conflict(
            "the GitHub App has no client secret, so the browser link cannot exchange GitHub's \
             code; an admin adds it with: crystalline github app set --client-secret-file <file> (or \
             sets CRYSTALLINE_GITHUB_APP_CLIENT_SECRET when the App comes from the environment)",
        ));
    }
    let config = state.engine.config();
    let Some(public) = crate::settings::public_base(&config) else {
        return Err(ApiError::conflict(
            "linking GitHub from the browser needs service.public_url: GitHub sends the browser \
             back to <service.public_url>/api/v1/github/link/callback, which must also be the \
             GitHub App's callback url",
        ));
    };
    let redirect_uri = public.join(&format!("/api/v1{CALLBACK_PATH}"));
    let flow_state = new_state();
    let now = state.engine.link_clock_now();
    let pending = GithubLinkPending {
        account: user.name.clone(),
        session_hash: session_hash(csrf),
        redirect_uri: redirect_uri.clone(),
        started: now,
    };
    state
        .github_links
        .pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert_at(flow_state.clone(), pending, now)
        .map_err(|_| {
            ApiError::service_unavailable(
                "too many GitHub links are in flight at once; try again in a moment",
            )
            .retry_after(30)
        })?;
    let authorize_url = crystalline_remote::github::auth::authorize_url(
        app.auth_base(),
        &credentials.client_id,
        &redirect_uri,
        &flow_state,
    );
    Ok((no_store(), Json(StartGithubLinkResponse { authorize_url })).into_response())
}

/// The query GitHub sends the browser back with. Every field optional, so a
/// malformed callback is answered by this module rather than by the
/// extractor. No `Debug`: it carries the code.
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CallbackQuery {
    /// The authorization code, exchanged once for a user token.
    code: Option<String>,
    /// The state `start` minted.
    state: Option<String>,
    /// GitHub's refusal, `access_denied` when the person cancelled.
    error: Option<String>,
}

/// A state that could not be used, whatever the reason: the browser lands
/// on the profile with `state`, which says to start again. The reason goes to
/// the log only, so the landing tells a stranger nothing about why.
fn state_refused(state: &RestState, reason: &str, account: &str) -> Response {
    tracing::warn!("a GitHub link callback for {account} was refused: {reason}");
    to_profile(state, "github_error=state")
}

/// A 303 to the profile under the base path.
fn to_profile(state: &RestState, query: &str) -> Response {
    let location = format!("{}profile?{query}", state.base_path.href());
    (
        StatusCode::SEE_OTHER,
        no_store(),
        [(header::LOCATION, location)],
    )
        .into_response()
}

/// `GET /github/link/callback` - where GitHub sends the browser back.
#[utoipa::path(
    get,
    path = "/api/v1/github/link/callback",
    tag = "settings",
    operation_id = "finish_github_link",
    summary = "Where GitHub sends the browser back to finish a link.",
    description = "Needs the session that started the link (the session \
                   cookie is SameSite=Lax, so it rides GitHub's redirect). \
                   The state must name a link this session started in the \
                   last ten minutes and is spent by this call. Then the code \
                   is exchanged and the account linked, and the browser is \
                   sent to `/profile?github=linked`, or to \
                   `/profile?github_error=<code>` with `id_taken` (the GitHub \
                   account is linked to another account), `denied` (the \
                   person cancelled at GitHub), `state` (the state is \
                   unknown, used, expired or another session's), `session` \
                   (no signed-in session; the app asks for a sign-in and \
                   returns to the profile) or `failed` (anything else). It \
                   is a browser landing, so it never answers JSON.",
    params(CallbackQuery),
    responses(
        (
            status = 303,
            description = "To the profile, with `github=linked` or \
                           `github_error=<code>`. `state` is no state, or a \
                           state this session did not start, already used, or \
                           expired, and `session` is no signed-in session; in \
                           both cases nothing reached GitHub.",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn callback(
    State(state): State<RestState>,
    identity: Identity,
    ApiQuery(query): ApiQuery<CallbackQuery>,
) -> Result<Response, ApiError> {
    // A browser landing: a lost session goes to the profile too, which asks
    // for a sign-in and comes back. The state is not taken, since nothing
    // proved who sent it.
    let user = match identity.require_account() {
        Ok(user) => user,
        Err(_) => {
            tracing::warn!("a GitHub link callback arrived without a signed-in session");
            return Ok(to_profile(&state, "github_error=session"));
        }
    };
    let Some(flow_state) = query.state.as_deref() else {
        return Ok(state_refused(&state, "no state", &user.name));
    };
    let now = state.engine.link_clock_now();
    // Taken before anything is compared: a state is spent by the first
    // callback that names it, whoever sends it.
    let taken = state
        .github_links
        .pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take_at(flow_state, now);
    let pending = match taken {
        Ok(pending) => pending,
        Err(StateMiss::Unknown) => {
            return Ok(state_refused(&state, "unknown state", &user.name));
        }
        Err(StateMiss::Expired) => {
            return Ok(state_refused(
                &state,
                &format!(
                    "the link was started more than {} minutes ago",
                    PENDING_TTL.as_secs() / 60
                ),
                &user.name,
            ));
        }
    };
    let this_session = identity.csrf.as_deref().map(session_hash);
    let same_session = this_session.is_some_and(|hash| {
        super::auth::constant_time_eq(hash.as_bytes(), pending.session_hash.as_bytes())
    });
    if !same_session || pending.account != user.name {
        return Ok(state_refused(
            &state,
            "the state belongs to another session",
            &user.name,
        ));
    }
    if let Some(error) = query.error.as_deref() {
        // GitHub's words go to the debug log only: the query is whatever the
        // browser was sent here with, so it is not trusted into a line an
        // operator reads at the default level.
        tracing::info!("GitHub did not authorize the link of {}", user.name);
        tracing::debug!("GitHub's answer to the link of {}: {error}", user.name);
        let code = if error == "access_denied" {
            "denied"
        } else {
            "failed"
        };
        return Ok(to_profile(&state, &format!("github_error={code}")));
    }
    let Some(code) = query.code.as_deref() else {
        tracing::warn!(
            "the GitHub link of {} could not finish: GitHub sent no code",
            user.name
        );
        return Ok(to_profile(&state, "github_error=failed"));
    };
    match finish(&state, &pending, code).await {
        Ok(()) => Ok(to_profile(&state, "github=linked")),
        Err(code) => Ok(to_profile(&state, &format!("github_error={code}"))),
    }
}

/// The exchange and the link, answering the profile's error code on failure.
/// Every failure is logged with its reason; none of them names the code.
async fn finish(
    state: &RestState,
    pending: &GithubLinkPending,
    code: &str,
) -> Result<(), &'static str> {
    let account = &pending.account;
    let app = state.engine.github_link_app().map_err(|e| {
        tracing::warn!("the GitHub link of {account} could not finish: {e}");
        "failed"
    })?;
    let credentials = app.credentials();
    let Some(secret) = credentials.client_secret.as_deref() else {
        tracing::warn!(
            "the GitHub link of {account} could not finish: the GitHub App has no client secret"
        );
        return Err("failed");
    };
    let grant = crystalline_remote::github::auth::exchange_code(
        app.auth_base(),
        &credentials.client_id,
        secret,
        code,
        &pending.redirect_uri,
    )
    .await
    .map_err(|e| {
        tracing::warn!("GitHub refused the code of the GitHub link of {account}: {e}");
        "failed"
    })?;
    match state.engine.link_github(account, grant).await {
        Ok(_) => Ok(()),
        Err(EngineError::Conflict(text)) if text.contains(crate::auth_store::GITHUB_ID_TAKEN) => {
            tracing::warn!(
                "the GitHub link of {account} was refused: the GitHub account is linked to \
                 another account"
            );
            Err("id_taken")
        }
        Err(e) => {
            tracing::warn!("the GitHub link of {account} could not finish: {e}");
            Err("failed")
        }
    }
}

/// `DELETE /me/github-link` - unlink the caller's own account.
#[utoipa::path(
    delete,
    path = "/api/v1/me/github-link",
    tag = "settings",
    operation_id = "unlink_my_github_link",
    summary = "Unlink the caller's own account from GitHub.",
    description = "Any signed-in account, and refused on a read-only \
                   instance. Revokes the user token at GitHub, deletes it \
                   here and removes the link. When GitHub cannot revoke it, \
                   the unlink still happens here and `warning` says so. \
                   Idempotent: an account without a link answers \
                   `was_linked: false`.",
    responses(
        (status = 200, description = "Unlinked.", body = GithubUnlinkResponse),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "A cookie session did not echo its CSRF token, this \
                           instance is read-only, or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn unlink(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<GithubUnlinkResponse>, ApiError> {
    let user = identity.require_account()?;
    refuse_read_only(&state)?;
    Ok(Json(unlink_response(
        state.engine.unlink_github(&user.name).await?,
    )))
}
