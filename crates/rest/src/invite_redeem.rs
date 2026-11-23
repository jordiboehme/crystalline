//! Redeeming an invite link: `POST /invites/confirm` reads what a link
//! offers, `POST /invites/redeem` takes it.
//!
//! Both are POST with the token in the body, never in a query or a path, so
//! the token stays out of access logs, and both are served without a session
//! (they are in the guard's public paths): the person holding a link usually
//! has no account yet. Nothing is redeemed by a GET, so a link preview cannot
//! burn a link. The redeem POST is under the guard's one CSRF rule like every
//! other POST: a request with no account behind it has no token to echo, and
//! [`crate::ApiJson`] refuses the form encodings a cross-site page can send;
//! a signed-in person echoes their session's token.
//!
//! **Which sign-in a link offers.** An instance whose identities come from a
//! proxy (`auth.proxy_headers` or `auth.trusted_header`) offers `proxy`, one
//! with single sign-on offers `oidc`, any other `local`. A local account with
//! a password can be made only where `local` is the offer or an admin made
//! the link a local-account link: an invite must not walk around the
//! identity provider.
//!
//! * `local`: the person picks a login name and a password; the account is
//!   created and signed in, in the answer.
//! * `oidc`: the answer is where to send the browser to sign in; the token is
//!   kept server side in the sign-in's pending record (never in a URL) and
//!   the callback redeems it. See [`crate::oidc`].
//! * `proxy`: the forwarded identity is the account. The proxy mode makes the
//!   account on the first request it sees, before any redemption runs, at
//!   `auth.oidc.default_role`, so in this mode a link gives the membership
//!   only and its role never applies.
//!
//! A signed-in person always redeems for their own account (membership
//! only); a different login in the body is refused.
//!
//! **What is said.** `confirm` answers one sentence for a token that is
//! unknown, used, revoked or expired, or whose creator or domain no longer
//! allows it, so a token cannot be probed for which of those it is. `redeem`
//! is specific once the token matched a link. Both count refusals per peer
//! address on a throttle of the login throttle's shape (the same
//! `auth.login.*` settings), so tokens cannot be tried at speed. Behind a
//! reverse proxy every caller shares the proxy's address, so the throttle
//! then slows everybody's wrong tokens together; a right one is never slowed
//! below the free attempts.
//!
//! A read-only instance refuses `redeem`, like the users routes, and still
//! answers `confirm`. One INFO line per redemption names the account, the
//! link, whether the account was made, and the domain and level.

use std::net::IpAddr;
use std::time::Instant;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use super::auth::{Identity, PeerAddr, issue_session_with_csrf, no_store};
use super::auth_store::{
    Invite, InviteRedeemer, MemberLevel, RedeemedInvite, RefusalKind, Role, StoreRefusal, User,
    normalize_account_name,
};
use super::login_throttle::Verdict;
use super::{ApiError, ApiJson, ProblemDetail, RestState, check_password, refuse_read_only};

/// The route that reads what a link offers, relative to the `/api/v1` mount.
pub const CONFIRM_PATH: &str = "/invites/confirm";

/// The route that takes a link, relative to the `/api/v1` mount.
pub const REDEEM_PATH: &str = "/invites/redeem";

/// What `confirm` answers for every token it will not describe, and what
/// `redeem` answers for a token that matches no link.
pub const NOT_VALID: &str = "This invite link is not valid any more.";

/// What a caller past the throttle's ceiling is told.
const TOO_MANY_TRIES: &str = "too many invite links were tried from here: wait and try again";

/// Which way in a link offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum InviteSignIn {
    /// A login name and a password made here.
    Local,
    /// The instance's single sign-on provider.
    Oidc,
    /// The proxy in front of this instance names the person.
    Proxy,
}

/// The sign-in this instance offers a link's holder.
pub(crate) fn offered_sign_in(state: &RestState) -> InviteSignIn {
    if state.auth_cfg.proxy_headers || state.auth_cfg.trusted_header.is_some() {
        InviteSignIn::Proxy
    } else if state.oidc.is_some() {
        InviteSignIn::Oidc
    } else {
        InviteSignIn::Local
    }
}

/// Whether `invite` may make a local account with a password here.
fn local_allowed(state: &RestState, invite: &Invite) -> bool {
    invite.local_account || offered_sign_in(state) == InviteSignIn::Local
}

/// The body of `POST /invites/confirm`.
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfirmBody {
    /// The token from the link's fragment.
    #[schema(example = "cin_q2V0...")]
    token: String,
}

/// Hand-written so the token never reaches a log line.
impl std::fmt::Debug for ConfirmBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfirmBody")
            .field("token", &"cin_[redacted]")
            .finish()
    }
}

/// What `POST /invites/confirm` answers with: what the link gives and how to
/// take it.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ConfirmResponse {
    /// The login name of the person who made the link.
    #[schema(example = "ada")]
    inviter: String,
    /// Their display name.
    #[schema(example = "Ada Lovelace")]
    inviter_display: String,
    /// The private domain the link gives membership of, if any.
    #[schema(example = "team")]
    domain: Option<String>,
    /// The level there.
    #[schema(value_type = Option<super::schemas::MemberLevel>)]
    level: Option<MemberLevel>,
    /// The instance role an account this link creates gets. An existing
    /// account keeps its own.
    #[schema(value_type = super::schemas::Role)]
    role: Role,
    /// When the link stops working, RFC 3339.
    expires_at: String,
    /// The sign-in this instance offers.
    sign_in: InviteSignIn,
    /// Whether the link may make an account with a login name and a
    /// password here.
    local_allowed: bool,
}

/// The body of `POST /invites/redeem`.
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RedeemBody {
    /// The token from the link's fragment.
    #[schema(example = "cin_q2V0...")]
    token: String,
    /// The login name for a new local account. Signed in, it may only name
    /// the caller's own account.
    #[schema(example = "nina")]
    login: Option<String>,
    /// The password for a new local account.
    #[schema(example = "correct horse battery staple")]
    password: Option<String>,
    /// The display name for a new local account. Defaults to the login name
    /// as typed.
    #[schema(example = "Nina")]
    display_name: Option<String>,
}

/// Hand-written so neither the token nor the password reaches a log line.
impl std::fmt::Debug for RedeemBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedeemBody")
            .field("token", &"cin_[redacted]")
            .field("login", &self.login)
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .field("display_name", &self.display_name)
            .finish()
    }
}

/// What a redeem request came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RedeemOutcome {
    /// The link is redeemed.
    Redeemed,
    /// The browser signs in at the provider first; the callback redeems the
    /// link. Navigate the whole page to `location`.
    SignIn,
}

/// What `POST /invites/redeem` answers with.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RedeemResponse {
    /// Redeemed, or a sign-in to go through first.
    outcome: RedeemOutcome,
    /// Where to send the browser, for `sign_in`.
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
    /// The account that redeemed the link, as stored, for `redeemed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<super::schemas::User>)]
    user: Option<User>,
    /// Whether the redemption made that account.
    created: bool,
    /// The domain the account is now a member of, if the link named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<String>,
    /// The level it holds there now: the link's, or a higher one it held.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<super::schemas::MemberLevel>)]
    level: Option<MemberLevel>,
    /// The CSRF token of the session this answer signed a new local account
    /// in with.
    #[serde(skip_serializing_if = "Option::is_none")]
    csrf: Option<String>,
}

impl RedeemResponse {
    fn redeemed(done: RedeemedInvite, csrf: Option<String>) -> RedeemResponse {
        RedeemResponse {
            outcome: RedeemOutcome::Redeemed,
            location: None,
            user: Some(done.user),
            created: done.created,
            domain: done.invite.domain,
            level: done.level,
            csrf,
        }
    }
}

/// The throttle key of a request: its peer's address.
fn throttle_key(peer: &PeerAddr) -> String {
    peer.ip()
        .map_or_else(|| "unknown".to_string(), |ip: IpAddr| ip.to_string())
}

/// Let a request through the invite throttle, napping when it has earned a
/// delay, or refuse it.
async fn admit(state: &RestState, key: &str) -> Result<(), ApiError> {
    let throttle = state.invite_throttle();
    match throttle.check(key, Instant::now()) {
        Verdict::Refuse(seconds) => {
            Err(ApiError::too_many_requests(TOO_MANY_TRIES).retry_after(seconds))
        }
        Verdict::Delay(delay) if !delay.is_zero() => {
            let Some(_permit) = throttle.try_take_sleeper() else {
                return Err(
                    ApiError::too_many_requests(TOO_MANY_TRIES).retry_after(delay.as_secs().max(1))
                );
            };
            tokio::time::sleep(delay).await;
            Ok(())
        }
        Verdict::Delay(_) => Ok(()),
    }
}

/// Count one refused token for `key`.
fn record_miss(state: &RestState, key: &str, peer: &PeerAddr) {
    let throttle = state.invite_throttle();
    throttle.record_failure(key, Instant::now());
    tracing::warn!(
        peer = ?peer.ip(),
        failures = throttle.failures(key),
        "an invite token was refused"
    );
}

/// The checks the store cannot make, run before every confirm and redeem:
/// the domain's access does not follow GitHub, and a link an owner or a
/// manager made still may be redeemed while `auth.invites.managers` is off.
pub(crate) async fn refuse_beyond_store(
    state: &RestState,
    invite: &Invite,
) -> Result<(), ApiError> {
    if let Some(domain) = invite.domain.as_deref()
        && state.engine.github_governs(domain)
    {
        return Err(ApiError::conflict(
            "this domain's access follows GitHub now, so this invite link gives nothing: ask its \
             maker to add you there",
        ));
    }
    if !state.auth_cfg.invites_managers {
        let admin = state
            .auth
            .user(&invite.created_by)
            .await?
            .is_some_and(|maker| maker.role == Role::Admin);
        if !admin {
            return Err(lapsed());
        }
    }
    Ok(())
}

/// What a link whose maker may no longer give it is answered with.
fn lapsed() -> ApiError {
    ApiError::conflict(
        "the person who made this invite link may not give what it gives any more: ask them for \
         a new one",
    )
}

/// A store refusal of a redemption as a status. Specific: the token matched
/// a link, or the refusal is about the account, not the link.
pub(crate) fn redeem_error(e: anyhow::Error) -> ApiError {
    let words = || {
        e.chain()
            .find_map(|cause| cause.downcast_ref::<StoreRefusal>())
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("{e:#}"))
    };
    match StoreRefusal::kind_of(&e) {
        Some(RefusalKind::InviteUnknown) => ApiError::not_found(NOT_VALID),
        Some(RefusalKind::InviteUsed) => ApiError::gone("This invite link was used already."),
        Some(RefusalKind::InviteRevoked) => ApiError::gone("This invite link was revoked."),
        Some(RefusalKind::InviteExpired) => ApiError::gone("This invite link has expired."),
        Some(RefusalKind::InviteCreatorLapsed) => lapsed(),
        Some(RefusalKind::NotPrivate) => ApiError::conflict(
            "this domain is not private any more, so this invite link gives nothing",
        ),
        Some(RefusalKind::InviteNeedsNewAccount) => ApiError::conflict(
            "this invite link makes a new account and names no domain, so it gives an account \
             that exists already nothing: sign out to make a new account with it",
        ),
        Some(RefusalKind::NoSuchAccount) => ApiError::forbidden("this account is disabled"),
        Some(RefusalKind::CapReached) => super::users_api::account_cap_refusal(&e),
        Some(RefusalKind::NameTaken) => ApiError::conflict(words()),
        Some(RefusalKind::InvalidName | RefusalKind::InvalidDisplayName) => {
            ApiError::unprocessable(words())
        }
        Some(RefusalKind::IdentityAlreadyLinked) => ApiError::conflict(
            "this sign-in identity belongs to an account already: sign in with it, then open the \
             invite link again",
        ),
        _ => ApiError::internal(format!("{e:#}")),
    }
}

/// The INFO line a redemption leaves.
pub(crate) fn log_redeemed(done: &RedeemedInvite) {
    tracing::info!(
        by = %done.user.name,
        invite = done.invite.id,
        created = done.created,
        role = %if done.created { done.user.role.as_str() } else { "" },
        domain = done.invite.domain.as_deref().unwrap_or(""),
        level = done.level.map(MemberLevel::as_str).unwrap_or(""),
        made_by = %done.invite.created_by,
        "invite redeemed"
    );
}

/// `POST /invites/confirm` - what an invite link gives and how to take it.
/// Redeems nothing.
#[utoipa::path(
    post,
    path = "/api/v1/invites/confirm",
    tag = "invites",
    operation_id = "confirm_invite",
    request_body = ConfirmBody,
    responses(
        (status = 200, description = "What the link gives and the sign-in offered.", body = ConfirmResponse),
        (
            status = 400,
            description = "The body is not JSON.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "The link is not valid any more: one answer for a \
                           token that is unknown, used, revoked or expired, or \
                           whose maker or domain no longer allows it.",
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
            description = "The body is JSON but carries no token.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 429,
            description = "Too many tokens were tried from this address.",
            body = ProblemDetail,
            content_type = "application/problem+json",
            headers(
                ("retry-after" = String, description = "Seconds to wait before \
                 trying again."),
            ),
        ),
    ),
)]
pub async fn confirm(
    State(state): State<RestState>,
    peer: PeerAddr,
    ApiJson(body): ApiJson<ConfirmBody>,
) -> Result<(super::auth::NoStore, axum::Json<ConfirmResponse>), ApiError> {
    let key = throttle_key(&peer);
    admit(&state, &key).await?;
    let offer = match state.auth.invite_offer(&body.token).await {
        Ok(offer) => offer,
        Err(e) if StoreRefusal::kind_of(&e).is_some() => {
            record_miss(&state, &key, &peer);
            return Err(ApiError::not_found(NOT_VALID));
        }
        Err(e) => return Err(ApiError::internal(format!("{e:#}"))),
    };
    if refuse_beyond_store(&state, &offer.invite).await.is_err() {
        return Err(ApiError::not_found(NOT_VALID));
    }
    let invite = offer.invite;
    Ok((
        no_store(),
        axum::Json(ConfirmResponse {
            local_allowed: local_allowed(&state, &invite),
            sign_in: offered_sign_in(&state),
            inviter: invite.created_by,
            inviter_display: offer.inviter_display,
            domain: invite.domain,
            level: invite.level,
            role: invite.role,
            expires_at: invite.expires_at,
        }),
    ))
}

/// `POST /invites/redeem` - take an invite link.
#[utoipa::path(
    post,
    path = "/api/v1/invites/redeem",
    tag = "invites",
    operation_id = "redeem_invite",
    request_body = RedeemBody,
    responses(
        (
            status = 200,
            description = "Redeemed, or (`outcome: sign_in`) where to send the \
                           browser to sign in first: the callback redeems the \
                           link. A new local account is signed in, with the \
                           session cookie set and its CSRF token in `csrf`.",
            body = RedeemResponse,
            headers(
                ("set-cookie" = String, description = "The `fluid_session` cookie \
                 for a new local account, or the `fluid_oidc_state` cookie for a \
                 sign-in to go through."),
            ),
        ),
        (
            status = 400,
            description = "The body is not JSON.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "The link is taken through the proxy in front of this \
                           instance, and this request carried no identity from \
                           it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "A signed-in caller named another login, this link \
                           may not make a local account here, the account is \
                           disabled, a new account would pass the account cap, \
                           a signed-in caller did not echo the CSRF token, or \
                           this instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No invite link has this token.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The link's maker may no longer give what it gives, \
                           its domain is no longer private or follows GitHub, \
                           the login name is taken, the link names no domain \
                           and the caller has an account, or the sign-on \
                           identity belongs to an account already.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 410,
            description = "The link was used, revoked or has expired.",
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
            description = "A local account needs a login name and a password, \
                           and one was missing, empty or not a login name.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 429,
            description = "Too many tokens were tried from this address.",
            body = ProblemDetail,
            content_type = "application/problem+json",
            headers(
                ("retry-after" = String, description = "Seconds to wait before \
                 trying again."),
            ),
        ),
        (
            status = 502,
            description = "The sign-on provider could not be reached to start \
                           the sign-in.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 503,
            description = "Too many sign-ins are in flight, or the wait for a \
                           password slot ran out.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn redeem(
    State(state): State<RestState>,
    peer: PeerAddr,
    identity: Identity,
    jar: CookieJar,
    headers: HeaderMap,
    ApiJson(body): ApiJson<RedeemBody>,
) -> Result<Response, ApiError> {
    let key = throttle_key(&peer);
    admit(&state, &key).await?;
    refuse_read_only(&state)?;
    let offer = match state.auth.invite_offer(&body.token).await {
        Ok(offer) => offer,
        Err(e) => {
            if StoreRefusal::kind_of(&e) == Some(RefusalKind::InviteUnknown) {
                record_miss(&state, &key, &peer);
            }
            return Err(redeem_error(e));
        }
    };
    refuse_beyond_store(&state, &offer.invite).await?;
    let invite = offer.invite;

    // A signed-in person (a session, or the proxy naming them) redeems for
    // their own account and gets the membership only.
    if let Some(user) = identity.user.as_ref() {
        if let Some(login) = body.login.as_deref()
            && normalize_account_name(login).ok().as_deref() != Some(user.name.as_str())
        {
            return Err(ApiError::forbidden(format!(
                "you are signed in as '{}', so this link is redeemed for that account: sign out \
                 first to make another",
                user.name
            )));
        }
        let done = state
            .auth
            .redeem_invite(
                &body.token,
                InviteRedeemer::Account(&user.name),
                state.auth_cfg.max_users,
            )
            .await
            .map_err(redeem_error)?;
        log_redeemed(&done);
        crate::readers_changed(&state);
        return Ok((no_store(), axum::Json(RedeemResponse::redeemed(done, None))).into_response());
    }

    let sign_in = offered_sign_in(&state);
    if body.login.is_some() || body.password.is_some() {
        if !local_allowed(&state, &invite) {
            return Err(ApiError::forbidden(match sign_in {
                InviteSignIn::Oidc => {
                    "this invite link is taken with single sign-on here: sign in with it"
                }
                _ => "this invite link is taken through the sign-in in front of this instance",
            }));
        }
        let (Some(login), Some(password)) = (body.login.as_deref(), body.password.as_deref())
        else {
            return Err(ApiError::unprocessable(
                "a new account needs a login name and a password",
            ));
        };
        check_password(password)?;
        super::users_api::clear_name_for_new_account(&state, login, false).await?;
        let display = body
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| login.trim());
        let done = state
            .with_login_slot(state.auth.redeem_invite(
                &body.token,
                InviteRedeemer::Local {
                    name: login,
                    display: Some(display),
                    password,
                },
                state.auth_cfg.max_users,
            ))
            .await?
            .map_err(redeem_error)?;
        log_redeemed(&done);
        crate::readers_changed(&state);
        let (jar, csrf) = issue_session_with_csrf(&state, jar, &headers, &done.user).await?;
        return Ok((
            jar,
            no_store(),
            axum::Json(RedeemResponse::redeemed(done, Some(csrf))),
        )
            .into_response());
    }
    match sign_in {
        InviteSignIn::Oidc => {
            let (location, cookie) =
                super::oidc::start_invite_sign_on(&state, &headers, body.token.clone()).await?;
            Ok((
                jar.add(cookie),
                no_store(),
                axum::Json(RedeemResponse {
                    outcome: RedeemOutcome::SignIn,
                    location: Some(location),
                    user: None,
                    created: false,
                    domain: invite.domain,
                    level: invite.level,
                    csrf: None,
                }),
            )
                .into_response())
        }
        InviteSignIn::Proxy => Err(ApiError::unauthorized(
            "this invite link is taken as the person the sign-in in front of this instance \
             names, and this request named nobody: sign in there first",
        )),
        InviteSignIn::Local => Err(ApiError::unprocessable(
            "a new account needs a login name and a password",
        )),
    }
}
