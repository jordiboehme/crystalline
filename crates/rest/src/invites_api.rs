//! Invite links, made and managed over HTTP: an admin's way to bring a
//! person in, and, where `auth.invites.managers` allows it, a private
//! domain's owner's or manager's way to bring a person into that domain.
//!
//! Who may make which link:
//!
//! * an instance admin makes a link for any instance role below admin, with
//!   or without a private domain and a level there, and may make it a
//!   local-account link;
//! * with `auth.invites.managers` on, the owner or a manager of a private
//!   domain makes a link to that domain only, for instance role `none` and a
//!   level not above their own;
//! * nobody makes a link to a shared domain or to one whose access follows
//!   GitHub.
//!
//! The token is answered once, with `Cache-Control: no-store`, inside the
//! link's fragment so it stays out of access logs and `Referer`. The list
//! never carries a token. A creator revokes their own links, an admin any;
//! a link that is not the caller's answers 404, as one that does not exist.
//! A read-only instance refuses both changes and still serves the list, as
//! the users routes do. Each change logs one INFO line naming who did it;
//! a refusal by a cap names the setting in the log, never in the answer.
//!
//! Redeeming a link is not here: it re-checks the creator's rights at that
//! moment and lives beside the sign-in routes, in [`super::invite_redeem`].

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;

use super::invite_redeem::{InviteSignIn, offered_sign_in};

use super::auth::{Identity, NoStore, no_store};
use super::auth_store::{
    INVITE_DEFAULT_TTL_SECS, INVITE_PENDING_MAX_ADMIN, INVITE_PENDING_MAX_MANAGER, Invite,
    InviteLimits, InviteState, MemberLevel, NewInvite, RefusalKind, Role, StoreRefusal,
};
use super::scope::DomainRight;
use super::{ApiError, ApiJson, ApiPath, ApiQuery, ProblemDetail, RestState, refuse_read_only};

/// The longest life a link may be asked for over the API, in days.
const MAX_EXPIRES_IN_DAYS: u32 = 30;

/// What a non-admin is told while `auth.invites.managers` is off. It names
/// no setting: the person reading it cannot change one.
const ADMINS_ONLY: &str =
    "only an administrator makes invite links on this instance: ask one to invite this person";

/// What `GET /invites` answers with.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct InvitesResponse {
    /// The links, newest first, never with their tokens.
    #[schema(value_type = Vec<super::schemas::Invite>)]
    invites: Vec<Invite>,
    /// The way in this instance offers a link's holder: `proxy` or `oidc`
    /// when an identity provider names people, else `local`. Only where it
    /// is not `local` does a link's `local_account` flag change anything.
    sign_in: InviteSignIn,
}

/// The query string `GET /invites` takes.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListInvitesQuery {
    /// Only links in this state.
    #[serde(default)]
    #[param(value_type = Option<super::schemas::InviteState>)]
    state: Option<InviteState>,
}

/// The body of `POST /invites`.
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInviteBody {
    /// The instance role an account this link creates gets: `none`, `viewer`
    /// or `editor`. Defaults to `viewer` for an admin's link; a domain
    /// owner's or manager's link is always `none`.
    #[schema(value_type = Option<super::schemas::Role>)]
    role: Option<Role>,
    /// A private domain the link gives membership of. Required for a domain
    /// owner's or manager's link.
    #[schema(example = "team")]
    domain: Option<String>,
    /// The level on that domain: `viewer`, `editor` or `manager`. Defaults to
    /// `viewer` when a domain is given; not above the maker's own.
    #[schema(value_type = Option<super::schemas::MemberLevel>)]
    level: Option<MemberLevel>,
    /// A label for the maker's own list.
    #[schema(example = "for Bo")]
    label: Option<String>,
    /// Whether the link may create an account with a password where single
    /// sign-on or a proxy would otherwise be the only way in. Admins only.
    local_account: Option<bool>,
    /// The link's life in days, 1 to 30. Defaults to 7.
    #[schema(example = 7)]
    expires_in_days: Option<u32>,
}

/// What `POST /invites` answers with: the link as stored, its token and the
/// address to copy, this once.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[schema(description = "A freshly made invite link. `token` and `link` are \
                        shown here and nowhere else, ever: only the token's \
                        hash is stored.")]
pub struct CreatedInviteResponse {
    /// The link as stored.
    #[schema(value_type = super::schemas::Invite)]
    invite: Invite,
    /// The token itself, this once.
    #[schema(example = "cin_q2V0...")]
    token: String,
    /// The address to hand over, the token in its fragment. Absolute when
    /// `service.public_url` is set, else a path under the base path that the
    /// browser completes with its own origin.
    #[schema(example = "https://kb.example.com/invite#cin_q2V0...")]
    link: String,
    /// Whether `link` is a path the browser completes rather than a whole
    /// address.
    relative: bool,
}

/// Hand-written so the token never reaches a log line.
impl std::fmt::Debug for CreatedInviteResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedInviteResponse")
            .field("invite", &self.invite)
            .field("token", &"cin_[redacted]")
            .field("relative", &self.relative)
            .finish()
    }
}

/// A store failure as a status, classified by [`RefusalKind`].
fn store_error(e: anyhow::Error) -> ApiError {
    let words = || {
        e.chain()
            .find_map(|cause| cause.downcast_ref::<StoreRefusal>())
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("{e:#}"))
    };
    match StoreRefusal::kind_of(&e) {
        Some(RefusalKind::InviteLimit) => ApiError::conflict(words()),
        Some(RefusalKind::CapReached) => {
            // The store's sentence names the setting; the answer does not,
            // for the reason every provisioning refusal keeps it to the log.
            tracing::warn!(
                "an invite link was refused: the accounts and the pending invite links reach \
                 auth.max_users"
            );
            ApiError::conflict(
                "this instance has no room for another account: revoke a pending invite link \
                 or ask an administrator",
            )
        }
        Some(RefusalKind::InvalidInvite) => ApiError::unprocessable(words()),
        _ => ApiError::internal(format!("{e:#}")),
    }
}

/// The domain right a membership level gives, to compare against the maker's.
fn right_of(level: MemberLevel) -> DomainRight {
    match level {
        MemberLevel::None => DomainRight::None,
        MemberLevel::Viewer => DomainRight::Read,
        MemberLevel::Editor => DomainRight::Write,
        MemberLevel::Manager => DomainRight::Manage,
    }
}

/// The address a token is handed over at: under `service.public_url` when
/// it is set, else the base path alone, for the browser to complete.
fn link_for(state: &RestState, token: &str) -> (String, bool) {
    let config = state.engine.config();
    match crate::settings::public_base(&config) {
        Some(public) => (public.join(&format!("/invite#{token}")), false),
        None => (format!("{}/invite#{token}", state.base_path.as_str()), true),
    }
}

/// `GET /invites` - the invite links, newest first: every one for an admin,
/// the caller's own for anyone else. Never carries a token.
#[utoipa::path(
    get,
    path = "/api/v1/invites",
    tag = "invites",
    operation_id = "list_invites",
    params(ListInvitesQuery),
    responses(
        (status = 200, description = "The links, newest first.", body = InvitesResponse),
        (
            status = 400,
            description = "`state` is not pending, used, revoked or expired.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity, or the anonymous viewer.",
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
    ApiQuery(query): ApiQuery<ListInvitesQuery>,
) -> Result<Json<InvitesResponse>, ApiError> {
    let user = identity.require_account()?;
    let creator = (user.role != Role::Admin).then_some(user.name.as_str());
    let invites = state
        .auth
        .invites(creator, query.state)
        .await
        .map_err(store_error)?;
    Ok(Json(InvitesResponse {
        invites,
        sign_in: offered_sign_in(&state),
    }))
}

/// `POST /invites` - make an invite link, answering 201 with it, its token
/// and the address to copy, this once.
#[utoipa::path(
    post,
    path = "/api/v1/invites",
    tag = "invites",
    operation_id = "create_invite",
    request_body = CreateInviteBody,
    responses(
        (status = 201, description = "The link, its token and its address, this once.", body = CreatedInviteResponse),
        (
            status = 400,
            description = "The body is not JSON.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity, or the anonymous viewer.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin and owners and managers \
                           may not make links here, or the caller is not the \
                           owner or a manager of the domain, asked for a role \
                           other than `none`, a level above their own, no \
                           domain or a local-account link; a cookie session \
                           did not echo its CSRF token, this instance is \
                           read-only, or the trusted-header identity names a \
                           disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The domain is shared or its access follows \
                           GitHub, the caller holds as many pending links as \
                           allowed, or the accounts and the pending links \
                           fill this instance.",
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
            description = "The body is JSON but not a link: an admin role, a \
                           level without a domain, a level of `none`, a life \
                           outside 1 to 30 days, or a label that is too long \
                           or holds a control character.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn create(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<CreateInviteBody>,
) -> Result<(StatusCode, NoStore, Json<CreatedInviteResponse>), ApiError> {
    let user = identity.require_account()?;
    let admin = user.role == Role::Admin;
    if !admin && !state.auth_cfg.invites_managers {
        return Err(ApiError::forbidden(ADMINS_ONLY));
    }
    // A hidden domain answers 404 before anything else is said about it, on a
    // read-only instance too, as the membership routes do.
    if let Some(domain) = body.domain.as_deref() {
        crate::require_domain_read(&state, &identity, domain).await?;
    }
    refuse_read_only(&state)?;
    if body.level.is_some() && body.domain.is_none() {
        return Err(ApiError::unprocessable(
            "a level needs a domain to apply to",
        ));
    }
    if !admin {
        if body.domain.is_none() {
            return Err(ApiError::forbidden(
                "an owner's or a manager's invite link names their private domain",
            ));
        }
        if body.local_account == Some(true) {
            return Err(ApiError::forbidden(
                "only an administrator makes a local-account invite link",
            ));
        }
        if body.role.is_some_and(|role| role != Role::None) {
            return Err(ApiError::forbidden(
                "an owner's or a manager's invite link gives instance role none",
            ));
        }
    }
    let level = body
        .domain
        .as_ref()
        .map(|_| body.level.unwrap_or(MemberLevel::Viewer));
    if let Some(domain) = body.domain.as_deref() {
        if state
            .auth
            .domain_visibility(domain)
            .await
            .map_err(|e| ApiError::internal(format!("reading this domain's visibility: {e:#}")))?
            .is_none()
        {
            return Err(ApiError::conflict(
                "this domain is shared, so an invite link cannot give membership of it: make it \
                 private first",
            ));
        }
        if state.engine.github_governs(domain) {
            return Err(ApiError::conflict(crate::members::FOLLOWS_GITHUB));
        }
        if !admin {
            crate::members::refuse_below(&state, &identity, domain, DomainRight::Manage, "manager")
                .await?;
            let own = state
                .access
                .right(&identity.scope(), domain)
                .await
                .map_err(|e| {
                    ApiError::internal(format!("this domain's membership is unreadable: {e:#}"))
                })?;
            if let Some(level) = level
                && right_of(level) > own
            {
                return Err(ApiError::forbidden(format!(
                    "your membership on this domain is {}, and an invite link gives a level not \
                     above your own",
                    crate::member_level_word(own)
                )));
            }
        }
    }
    let ttl_secs = match body.expires_in_days {
        None => INVITE_DEFAULT_TTL_SECS,
        Some(days @ 1..=MAX_EXPIRES_IN_DAYS) => i64::from(days) * 86_400,
        Some(_) => {
            return Err(ApiError::unprocessable(format!(
                "an invite link lives from 1 to {MAX_EXPIRES_IN_DAYS} days: pick a number in \
                 that range"
            )));
        }
    };
    let role = if admin {
        body.role.unwrap_or(Role::Viewer)
    } else {
        Role::None
    };
    let new = NewInvite {
        role,
        domain: body.domain.as_deref(),
        level,
        label: body.label.as_deref(),
        local_account: admin && body.local_account.unwrap_or(false),
        created_by: &user.name,
        ttl_secs,
    };
    let limits = InviteLimits {
        per_creator: if admin {
            INVITE_PENDING_MAX_ADMIN
        } else {
            INVITE_PENDING_MAX_MANAGER
        },
        max_users: state.auth_cfg.max_users,
    };
    let issued = state
        .auth
        .create_invite(&new, limits)
        .await
        .map_err(store_error)?;
    let invite = issued.invite;
    tracing::info!(
        by = %invite.created_by,
        invite = invite.id,
        role = %invite.role,
        domain = invite.domain.as_deref().unwrap_or(""),
        level = invite.level.map(MemberLevel::as_str).unwrap_or(""),
        local_account = invite.local_account,
        expires_at = %invite.expires_at,
        "invite created"
    );
    let (link, relative) = link_for(&state, &issued.token);
    Ok((
        StatusCode::CREATED,
        no_store(),
        Json(CreatedInviteResponse {
            invite,
            token: issued.token,
            link,
            relative,
        }),
    ))
}

/// `DELETE /invites/{id}` - revoke an unused link, answering 204: an admin
/// any, its maker their own. The link stays listed as revoked.
#[utoipa::path(
    delete,
    path = "/api/v1/invites/{id}",
    tag = "invites",
    operation_id = "revoke_invite",
    params(("id" = i64, Path, description = "The link's id.")),
    responses(
        (status = 204, description = "The link is revoked."),
        (
            status = 401,
            description = "No identity, or the anonymous viewer.",
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
            status = 404,
            description = "No unused, unrevoked link of that id that this \
                           caller may revoke.",
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
    refuse_read_only(&state)?;
    let creator = (user.role != Role::Admin).then_some(user.name.as_str());
    if !state
        .auth
        .revoke_invite(id, creator)
        .await
        .map_err(store_error)?
    {
        return Err(ApiError::not_found(format!(
            "no unused, unrevoked invite link {id} that you may revoke"
        )));
    }
    tracing::info!(by = %user.name, invite = id, "invite revoked");
    Ok(StatusCode::NO_CONTENT)
}
