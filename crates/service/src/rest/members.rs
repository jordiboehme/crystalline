//! Who is invited to a private domain, and at what level.
//!
//! The administration half of the private-domain feature: Task 10 made every
//! read and write on this surface obey the membership records, and this is
//! where they are written. Four routes, all under `/domains/{domain}`, and one
//! rule they all share - **a domain the caller may not see is answered 404**,
//! exactly as a domain nobody registered is, because the existence of a
//! private domain is the secret it keeps. Every gate below therefore opens
//! with [`require_domain_read`], which is the one call that produces that
//! answer.
//!
//! The ladder these routes read is [`DomainRight`], not the instance role:
//!
//! * **reading the list** needs nothing beyond seeing the domain at all. A
//!   viewer-level member sees who else is here, which is what the domain card
//!   in the UI draws, and a stranger never gets that far - they are refused by
//!   the 404 above;
//! * **inviting somebody, and changing a level**, needs [`DomainRight::Manage`]:
//!   the domain's own manager, its owner, or an instance admin;
//! * **removing somebody** needs the same, EXCEPT that any member may remove
//!   itself. Leaving is not an administrative act, and a person who wants out
//!   of a domain should not have to ask the manager who put them there;
//! * **handing the domain on** needs [`DomainRight::Own`]: the owner or an
//!   instance admin. A manager may invite and change levels and may not do
//!   this, for the same reason it may not change visibility - both hand the
//!   domain to somebody, and who may see a domain at all is not one domain's
//!   administration to decide.
//!
//! An anonymous identity is refused 401 by [`Identity::require_account`] ahead
//! of every MUTATION here. It could only ever resolve to
//! [`DomainRight::None`] on a private domain, so the check costs nothing and
//! buys the same answer every other write on this surface gives the anonymous
//! tier. The LISTING deliberately does not carry it: on an instance serving
//! `auth.anonymous`, an anonymous caller reads the domain list, searches and
//! opens engrams in every shared domain, and a member listing that alone
//! answered 401 would break a domain card that every other call on the page
//! serves. It sees what it is entitled to see - a shared domain has no owner
//! and no members, and a private one is not there at all.
//!
//! Membership only means something while a domain is private (see
//! `crate::scope`, where every caller's right on a shared domain comes from
//! its instance role), so every mutation here is refused on a shared one -
//! `409`, naming the visibility route as the step that comes first.
//!
//! **Why there is no instance-role gate beside the domain right.** Every write
//! route on this surface carries one: `require_domain_write` runs
//! [`Identity::require_editor`] before it reads the membership, because a
//! domain invitation must never widen what an account's INSTANCE role permits
//! *on knowledge*. Nothing here writes knowledge. A membership row is a
//! statement about who may reach a domain, and granting somebody `editor` on
//! one leaves them refused by that very check the moment they try to write, so
//! the non-escalation is enforced one layer down rather than duplicated here.
//! What this does mean is that an instance viewer who owns a private domain
//! administers it - which is the personal-private-domain case working as
//! intended.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use super::auth::Identity;
use super::auth_store::{
    DomainMember, MemberLevel, StoreRefusal, RefusalKind, normalize_account_name,
};
use super::{
    ApiError, ApiJson, ApiPath, ProblemDetail, RestState, refuse_read_only, require_domain_read,
};
use crate::scope::DomainRight;

/// What `GET /domains/{domain}/members` answers with.
///
/// `owner` is `Option` rather than a string because a private domain can
/// genuinely have none: removing an account un-names it on the domains it
/// owned (`AuthStore::remove_user`), leaving a domain only an instance admin
/// can administer. An empty string there would read as an account whose name
/// is empty, so the absence is spelled as one.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "Who may reach one domain. `visibility` is `private` \
                        or `shared`; a shared domain has no owner and no \
                        members, because membership only decides anything \
                        while a domain is private.")]
pub struct MembersResponse {
    /// The account that owns this domain, or `null`: a shared domain has no
    /// owner, and a private one whose owner's account was removed has none
    /// either.
    #[schema(example = "ada")]
    pub owner: Option<String>,
    /// `private` or `shared`.
    #[schema(example = "private")]
    pub visibility: &'static str,
    /// Everyone invited, by name. Empty for a shared domain.
    pub members: Vec<DomainMember>,
}

/// What `PUT /domains/{domain}/members/{principal}` takes.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[schema(description = "The level to invite this account at, or to move it \
                        to: `viewer` reads, `editor` writes, `manager` also \
                        administers the membership.")]
pub struct MemberBody {
    /// viewer | editor | manager
    #[schema(example = "editor")]
    pub level: MemberLevel,
}

/// What `PUT /domains/{domain}/owner` takes.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[schema(description = "The account to hand this private domain to. It must \
                        be an existing, enabled account; its own membership \
                        row, if it had one, is dropped, since an owner holds \
                        every level already.")]
pub struct OwnerBody {
    /// The new owner's login name.
    #[schema(example = "ada")]
    pub owner: String,
}

/// What a principal that names no enabled account is told, in one place.
///
/// **One answer for two states, deliberately.** The store knows whether a name
/// belongs to nobody or to a disabled account, and says which in its own
/// message. This surface must not repeat that: a domain MANAGER is not an
/// instance admin and cannot read `GET /users`, so forwarding the store's words
/// would hand them a probe for which login names exist on this instance and
/// which of those are switched off - an account-existence oracle built out of
/// an error message. The operator-facing surfaces that may legitimately know
/// (the `crystalline` CLI, which is the machine owner, and the admin user
/// screens) read the store's own text; this route says only that the name is
/// not something it can invite.
const NOT_AN_ENABLED_ACCOUNT: &str = "that name is not an enabled account on this instance: check it with an \
     administrator, who can see the account list";

/// Turn a membership store failure into a status.
///
/// Three of the store's refusals are the caller's doing rather than this
/// server's, and each of them would otherwise arrive as a 500 that reads like
/// a bug:
///
/// * a domain that is not private has no membership at all, and the fix is
///   the visibility route - a conflict with the current state, so 409;
/// * the owner is not a member and cannot be made one (the row could only ever
///   say *less* than the truth), so naming it is a 409 too, with the transfer
///   route as the way to change who that is;
/// * a principal that is not an existing, enabled account is an unprocessable
///   body: the name is well-formed and there is nobody behind it. This arm
///   answers [`NOT_AN_ENABLED_ACCOUNT`] and never the store's own words.
///
/// The classification is [`RefusalKind`], read off the error as a TYPE. It was
/// substring matching over the store's prose, which meant a rewording in
/// `auth_store.rs` would silently turn one of these 409s into a 500 - and
/// tempted this function into forwarding `{e:#}` on the one arm that must not.
///
/// Anything else is this server's problem and stays a 500 with the store's own
/// words.
fn store_error(e: anyhow::Error) -> ApiError {
    match StoreRefusal::kind_of(&e) {
        Some(RefusalKind::NotPrivate) => ApiError::conflict(
            "this domain is shared, so it has no membership: make it private \
             first (PUT /domains/{domain}/visibility)",
        ),
        Some(RefusalKind::OwnerIsNotAMember) => ApiError::conflict(
            "that account owns this domain and already holds every level: \
             hand the domain on with PUT /domains/{domain}/owner instead",
        ),
        Some(RefusalKind::NoSuchAccount) => ApiError::unprocessable(NOT_AN_ENABLED_ACCOUNT),
        // No membership statement can refuse for those reasons - they are the
        // identity-link surface's own - so they fall in with the server's
        // problems rather than being given a status here that would be a
        // guess.
        Some(
            RefusalKind::LastCredential
            | RefusalKind::IdentityAlreadyLinked
            | RefusalKind::IssuerAlreadyHeld,
        )
        | None => ApiError::internal(format!("{e:#}")),
    }
}

/// The login name a path segment addresses, folded exactly as the store folds
/// it, or the 422 that says the segment names no login name at all.
///
/// Shared by the two routes that take a `{principal}` so that the comparison
/// they make against the caller's own name, and against the owner's, is
/// against the value the store would key on rather than against a second
/// spelling of the folding rule. The refusal names the shape a login name has
/// and no account, so it is not an oracle.
fn principal_key(principal: &str) -> Result<String, ApiError> {
    normalize_account_name(principal).map_err(|e| ApiError::unprocessable(format!("{e}")))
}

/// The gate every mutation here opens with, in the order the refusals have to
/// happen:
///
/// 1. an identity with no account is 401 - see [`Identity::require_account`];
/// 2. a domain this caller may not see is the 404 an unregistered name gets,
///    decided BEFORE the read-only check, so a hidden domain answers the same
///    way on a read-only instance as on a writable one (the ordering
///    [`refuse_read_only`] documents);
/// 3. a read-only instance refuses;
/// 4. and only then the domain right, which is what a private domain adds.
///
/// The account name comes back rather than the whole [`super::auth_store::User`]:
/// what the callers need it for is the `added_by` audit field.
async fn require_right(
    state: &RestState,
    identity: &Identity,
    domain: &str,
    min: DomainRight,
    needed: &str,
) -> Result<String, ApiError> {
    let user = identity.require_account()?;
    require_domain_read(state, identity, domain).await?;
    refuse_read_only(state)?;
    refuse_below(state, identity, domain, min, needed).await?;
    Ok(user.name)
}

/// The right check on its own, for the one route that decides whether to make
/// it at all (a member leaving needs no level) after the three checks above
/// have already run.
async fn refuse_below(
    state: &RestState,
    identity: &Identity,
    domain: &str,
    min: DomainRight,
    needed: &str,
) -> Result<(), ApiError> {
    let right = state
        .access
        .right(&identity.scope(), domain)
        .await
        // Never a fallback: a change that cannot learn what its caller may do
        // refuses rather than proceeding on an assumption.
        .map_err(|e| {
            ApiError::internal(format!("this domain's membership is unreadable: {e:#}"))
        })?;
    if right < min {
        return Err(ApiError::forbidden(format!(
            "your membership on this domain is {}, and {needed} access is required",
            super::member_level_word(right)
        )));
    }
    Ok(())
}

/// One domain's visibility record, or the 409 that says it has none.
///
/// Every mutation here needs a private domain: membership decides nothing
/// while a domain is shared, so a caller who reached one of these routes on a
/// shared domain asked for something that cannot mean anything yet. The
/// refusal names the step that comes first.
async fn require_private(
    state: &RestState,
    domain: &str,
    what: &str,
) -> Result<super::auth_store::DomainAcl, ApiError> {
    let acl = state
        .auth
        .domain_visibility(domain)
        .await
        .map_err(|e| ApiError::internal(format!("reading this domain's visibility: {e:#}")))?;
    acl.ok_or_else(|| {
        ApiError::conflict(format!(
            "this domain is shared, so it has no {what}: make it private first"
        ))
    })
}

/// `GET /domains/{domain}/members` - who may reach this domain.
///
/// Open to anyone who can see the domain at all, membership viewers and the
/// anonymous tier included: the list is what the domain card draws, and a
/// person invited into a domain may see who else is in it. A stranger never
/// reaches the question - the domain does not exist as far as they are told.
///
/// A pure read, so a read-only instance serves it, and a shared domain answers
/// honestly rather than with a 409: `shared`, no owner, no members.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/members",
    tag = "domains",
    operation_id = "list_domain_members",
    summary = "Who owns this domain and who is invited into it.",
    description = "Served to any account that may see the domain, which on a \
                   private one means its owner, its members at every level, \
                   and instance admins. A caller who may not see the domain \
                   is answered 404, exactly as for a domain nobody \
                   registered.\n\nA shared domain answers `shared` with no \
                   owner and no members: membership only decides anything \
                   while a domain is private.",
    params(("domain" = String, Path, description = "The registered domain.")),
    responses(
        (status = 200, description = "The domain's owner and members.", body = MembersResponse),
        (
            status = 401,
            description = "No identity. The anonymous viewer, where \
                           `auth.anonymous` allows one, is served: it sees the \
                           shared domains it can already read, and no private \
                           one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn list(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<MembersResponse>, ApiError> {
    // `require_viewer`, not `require_account`: this is a read, and the
    // anonymous tier already reads every shared domain on this instance. It
    // learns nothing here it could not learn from `GET /domains`, because a
    // private domain is filtered out by the gate below and a shared one has
    // no owner and no members to report.
    identity.require_viewer()?;
    require_domain_read(&state, &identity, &domain).await?;
    let acl = state
        .auth
        .domain_visibility(&domain)
        .await
        .map_err(|e| ApiError::internal(format!("reading this domain's visibility: {e:#}")))?;
    let Some(acl) = acl else {
        return Ok(Json(MembersResponse {
            owner: None,
            visibility: "shared",
            members: Vec::new(),
        }));
    };
    let members = state
        .auth
        .domain_members(&domain)
        .await
        .map_err(|e| ApiError::internal(format!("listing this domain's members: {e:#}")))?;
    Ok(Json(MembersResponse {
        // The store writes `""` for a domain whose owner's account was
        // removed, which is the one value no live account can hold. It reaches
        // a client as `null` rather than as an empty name.
        owner: (!acl.owner.is_empty()).then_some(acl.owner),
        visibility: "private",
        members,
    }))
}

/// `PUT /domains/{domain}/members/{principal}` - invite an account, or move
/// one to a different level.
///
/// One verb for both, because they are the same row: a `PUT` states what the
/// membership should be, and whether it existed before is not something the
/// caller has to know. Manager and above.
#[utoipa::path(
    put,
    path = "/api/v1/domains/{domain}/members/{principal}",
    tag = "domains",
    operation_id = "set_domain_member",
    summary = "Invite an account to a private domain, or change its level.",
    description = "Needs manager access on the domain: its own manager, its \
                   owner, or an instance admin. The same call invites and \
                   re-levels, since both state what the membership should \
                   be.\n\nRefused on a shared domain (409): membership only \
                   decides anything while a domain is private. The owner \
                   cannot be named here either - it already holds every level \
                   - and an account that does not exist, or is disabled, is a \
                   422 rather than a row waiting for somebody to claim the \
                   name.",
    params(
        ("domain" = String, Path, description = "The registered domain."),
        ("principal" = String, Path, description = "The account's login name."),
    ),
    request_body = MemberBody,
    responses(
        (status = 204, description = "The account is a member at that level."),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not a manager here, the request did \
                           not echo its CSRF token, or this instance is \
                           read-only.",
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
            description = "The domain is shared, or the principal owns it.",
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
            description = "An unknown level, or a principal that names no \
                           enabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn set_member(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, principal)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<MemberBody>,
) -> Result<StatusCode, ApiError> {
    let actor = require_right(&state, &identity, &domain, DomainRight::Manage, "manager").await?;
    let principal = principal_key(&principal)?;
    state
        .auth
        .upsert_domain_member(&domain, &principal, body.level, &actor)
        .await
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /domains/{domain}/members/{principal}` - remove a membership.
///
/// Manager and above, OR the principal itself: leaving a domain is not an
/// administrative act, and needing a manager's permission to stop being a
/// member is a strange thing to make somebody ask for.
///
/// The owner cannot be removed this way at all, whoever asks. It holds no
/// membership row to delete, and what a caller reaching for this actually
/// wants is the transfer route, which the refusal names.
#[utoipa::path(
    delete,
    path = "/api/v1/domains/{domain}/members/{principal}",
    tag = "domains",
    operation_id = "remove_domain_member",
    summary = "Remove a membership, or leave a domain.",
    description = "Needs manager access on the domain - its manager, its \
                   owner, or an instance admin - OR that the principal is the \
                   caller itself, which is how a member leaves.\n\nRefused on \
                   a shared domain (409), which has no membership to remove. \
                   The owner cannot be removed here: hand the domain on with \
                   `PUT /domains/{domain}/owner` first. A name that is not a \
                   member of this domain is a 404.",
    params(
        ("domain" = String, Path, description = "The registered domain."),
        ("principal" = String, Path, description = "The account's login name."),
    ),
    responses(
        (status = 204, description = "That account is no longer a member."),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is neither a manager here nor the \
                           principal itself, the request did not echo its \
                           CSRF token, or this instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, none this caller may see, or an \
                           account that is not a member of it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The domain is shared, or the principal owns it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn remove_member(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, principal)): ApiPath<(String, String)>,
) -> Result<StatusCode, ApiError> {
    // The account, the 404 and the read-only refusal, in that order, before
    // anything decides whether this is a leave or an eviction: those three
    // answers are the same either way, and the leave case must not be a way
    // around them.
    let actor = identity.require_account()?;
    require_domain_read(&state, &identity, &domain).await?;
    refuse_read_only(&state)?;
    // The store folds a login name; a path segment is whatever was typed.
    // Compare on the folded form, or `DELETE /members/ADA` by `ada` would be
    // read as an eviction and refused for a caller who is simply leaving - and
    // fold it with the STORE's own function rather than a second spelling of
    // the rule, since the values this is compared against (the caller's own
    // name, and the owner's) are the store's.
    let principal = principal_key(&principal)?;
    let leaving = principal == actor.name;
    if !leaving {
        refuse_below(&state, &identity, &domain, DomainRight::Manage, "manager").await?;
    }
    // `remove_domain_member` deletes a row and asks no questions, so both
    // refusals below are this handler's own. The store's own privacy check
    // lives on the invite path, where a row would be *created*.
    //
    // These two reads sit OUTSIDE the delete's statement, unlike the invite
    // path's, whose checks share one `BEGIN IMMEDIATE` with its write. Both
    // outcomes of losing that race are benign and neither can widen access: a
    // domain made shared in between has had every membership row deleted
    // already, so the delete removes nothing and answers 404; a transfer in
    // between has dropped the incoming owner's row for the same reason, so the
    // owner check cannot be raced into deleting one. A store method that did
    // the owner check and the delete together would buy a better *message*,
    // not a better guarantee, so it is not worth the second spelling of the
    // rule.
    let acl = require_private(&state, &domain, "membership to remove").await?;
    if acl.owner == principal {
        return Err(ApiError::conflict(
            "that account owns this domain, so it holds no membership row: \
             hand the domain on with PUT /domains/{domain}/owner instead",
        ));
    }
    let removed = state
        .auth
        .remove_domain_member(&domain, &principal)
        .await
        .map_err(store_error)?;
    if !removed {
        return Err(ApiError::not_found(
            "that account is not a member of this domain",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /domains/{domain}/owner` - hand a private domain to somebody else.
///
/// The owner or an instance admin, and nobody else: this is the "person
/// leaves" step, and a manager may not perform it for the reason a manager may
/// not make a domain private - both decide who holds the domain, which is not
/// one domain's administration to settle.
///
/// The old owner keeps nothing. They are a stranger to the domain the moment
/// this returns, unless the new owner invites them back, which is what handing
/// something over means.
#[utoipa::path(
    put,
    path = "/api/v1/domains/{domain}/owner",
    tag = "domains",
    operation_id = "set_domain_owner",
    summary = "Hand a private domain to a different account.",
    description = "The domain's owner or an instance admin. A manager may \
                   not: handing a domain on decides who holds it, which is \
                   the same reason a manager may not change \
                   visibility.\n\nThe old owner keeps nothing - they are a \
                   stranger to the domain afterwards unless the new owner \
                   invites them back. The new owner's own membership row, if \
                   it had one, is dropped, since an owner already holds every \
                   level. Refused on a shared domain (409), which has no \
                   owner to hand on.",
    params(("domain" = String, Path, description = "The registered domain.")),
    request_body = OwnerBody,
    responses(
        (status = 204, description = "The domain has that owner now."),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller neither owns this domain nor is an \
                           admin, the request did not echo its CSRF token, or \
                           this instance is read-only.",
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
            description = "The domain is shared, so it has no owner.",
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
            description = "The owner names no enabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn set_owner(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<OwnerBody>,
) -> Result<StatusCode, ApiError> {
    require_right(&state, &identity, &domain, DomainRight::Own, "owner").await?;
    require_private(&state, &domain, "owner to hand on").await?;
    state
        .auth
        .transfer_domain(&domain, &body.owner)
        .await
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}
