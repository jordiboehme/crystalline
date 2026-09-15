//! Share-links on a draft: the one thing that lets somebody see, and then
//! edit, a page another person has not shared with the team yet.
//!
//! A domain in review mode keeps every person's unfolded work in their own
//! overlay, and nothing crosses between two overlays: a search never returns
//! somebody else's draft, a listing never counts one, a reference never
//! resolves onto one, and a read at its path answers the folder's own page or
//! nothing at all. That is the rule the whole mode rests on, and **this
//! surface is its single exception**. An author mints a link on one of their
//! own drafts and hands it to one person; that link opens that draft and
//! nothing else, for that one account and nobody else, until the author takes
//! it back or the draft itself ends.
//!
//! Four properties hold it to that, and each is somewhere different:
//!
//! - **One draft.** A grant names a domain and a path, so there is nothing for
//!   it to widen into. The account it binds to sees the draft where the link
//!   puts them and nowhere else - their search, their listing and their reads
//!   are exactly what they were, which is what
//!   `a_grantees_search_still_excludes_the_owners_draft` pins.
//! - **One account.** The first account to present a live link is its grantee
//!   for good (`AuthStore::redeem_overlay_grant`), so a link forwarded on
//!   opens nothing for whoever it was forwarded to.
//! - **Revocable, and it ends with its draft.** Its author can take it back at
//!   any time, and folding or discarding the draft ends every link on it
//!   (`Engine::end_domain_grants`): a grant lasts as long as the thing it
//!   grants.
//! - **Seeing is not editing.** Redeeming a link lets the grantee READ the
//!   draft. Typing into it needs a second, explicit step - a **join**, which
//!   belongs to a session rather than to the account (see [`crate::join`]) -
//!   and a write at the granted path without one is refused in words that name
//!   both ways forward.
//!
//! Two rules are this surface's own rather than the guard's, and they are the
//! settlement [`super::mcp_tokens`] documents: **every signed-in account may
//! redeem a link and join a draft, viewers included** - a viewer opens the
//! draft read-only, with the server's reason, because reviewing somebody's
//! wording is exactly what a viewer is for - and **a read-only instance serves
//! the account-state half**: a grant is a row in the accounts database beside
//! the tokens, and a join is a record in this process's memory. Neither is
//! knowledge, and `service.read_only` protects the knowledge. What read-only
//! still refuses is the write itself, in the engine, where it always did.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};

use super::auth::Identity;
use super::auth_store::{DRAFT_LINK_PREFIX, OverlayGrant};
use super::{
    ApiError, ApiJson, ApiPath, ApiQuery, ProblemDetail, RestState, require_domain_read,
    require_domain_write,
};
use crate::join::Join;
use crate::scope::DomainRight;

/// The header a request carries its join key in.
///
/// A header rather than a body field or a query parameter, because a join is a
/// property of the *session making the request* rather than of the thing being
/// written: every write verb would otherwise have to grow a field, and every
/// one of them would have to remember to read it. This way the transport
/// carries it and the routes that can be driven from inside a join ask one
/// helper.
pub const JOIN_HEADER: &str = "X-Crystalline-Join";

/// The join this request is being made inside, if any.
///
/// `None` for the overwhelming majority of requests, which carry no header at
/// all, and also for a key that names no open join and for one belonging to
/// another account - deliberately one answer for all three, since none of them
/// is a join and a caller learning which it was would learn whether a key
/// exists.
///
/// Called by the write routes that can be driven from inside a join; see
/// [`crate::join`] for why the answer belongs to the session rather than to
/// the account.
pub(super) fn join_of(state: &RestState, identity: &Identity, headers: &HeaderMap) -> Option<Join> {
    let key = headers.get(JOIN_HEADER)?.to_str().ok()?;
    let account = identity.user.as_ref()?;
    state.engine.joins().get(key, &account.name)
}

/// What minting a link asks for.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[schema(description = "Which of the caller's own drafts to mint a share-link \
                        on, and when the link should stop working.")]
pub struct MintBody {
    /// The domain-relative path of the caller's own draft.
    #[schema(example = "plan.md")]
    pub path: String,
    /// RFC 3339, when the link should stop working, or absent for one that
    /// lasts as long as the draft does.
    pub expires_at: Option<String>,
}

/// The one moment a link is readable. Never sent again.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "A freshly minted share-link. The token is readable \
                        exactly once, in this reply: only its hash is stored, \
                        so the listing can never hand it back.")]
pub struct MintedLinkResponse {
    /// The row id, which is what revokes this link.
    pub id: i64,
    /// The link itself, `dl_` plus 64 hex characters. Hand it to one person.
    pub token: String,
    /// The path it opens, echoed so the reply is self-describing.
    pub path: String,
}

/// Which draft a listing is about.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct LinksQuery {
    /// The domain-relative path of the caller's own draft.
    pub path: String,
}

/// What presenting a link asks for, and what leaving a draft asks for.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[schema(description = "A share-link, as it was handed over.")]
pub struct TokenBody {
    /// The link, `dl_` plus 64 hex characters.
    pub token: String,
}

/// What ending a join asks for.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[schema(description = "The key the join was opened under.")]
pub struct LeaveBody {
    /// The key `POST /draft-links/join` answered with.
    pub key: String,
}

/// A granted draft, as the account that redeemed the link receives it.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(
    description = "One draft, handed over by its author: where it stands, \
                        whose it is, whether this account may edit it, and the \
                        text itself."
)]
pub struct AcceptedDraft {
    /// The domain the draft lives in.
    pub domain: String,
    /// The domain-relative path it stands at.
    pub path: String,
    /// Whose draft it is.
    pub owner: String,
    /// The address it answers to, which is how a save of it is addressed.
    pub permalink: String,
    /// Whether this account may edit it at all: `write_right` on the domain,
    /// which is the same gate every other write on it passes.
    pub editable: bool,
    /// Why not, in the server's own words, or null when it is editable. What
    /// the editor shows above a buffer it opened read-only.
    pub reason: Option<String>,
    /// The markdown as its author last left it, frontmatter and all.
    pub content: String,
    /// The version token a save of this draft presents as its
    /// `expected_checksum`.
    pub checksum: String,
    /// The key this session works inside the draft under, or null when the
    /// caller only accepted the link and has not joined. Never stored beyond
    /// the session that holds it.
    pub join_key: Option<String>,
    /// Whose draft a joined save just landed in, in the server's own words, or
    /// null everywhere else.
    ///
    /// Only a save answers this shape with it filled in - opening a link and
    /// joining one are not writes - and it is here rather than on a receipt
    /// shape of its own because the granted-draft screen speaks this one
    /// shape: a save that landed answers the draft as it now stands, plus the
    /// sentence saying whose work it changed.
    pub joined: Option<String>,
}

/// `POST /domains/{domain}/draft-links` - mint a link on one of the caller's
/// own drafts.
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/draft-links",
    tag = "engrams",
    operation_id = "mint_draft_link",
    params(("domain" = String, Path, description = "The domain the draft is in.")),
    request_body = MintBody,
    summary = "Mint a share-link on one of the caller's own drafts.",
    description = "Answers the link once and never again: only its hash is \
                   stored. The link opens that one draft, for the first \
                   account that presents it and nobody else, until it is \
                   revoked or the draft is folded or discarded. 404 when the \
                   caller holds no draft at that path - including when \
                   somebody else does, because whose drafts exist is exactly \
                   what review mode does not say. Served on a read-only \
                   instance: a grant is account state rather than knowledge.",
    responses(
        (status = 200, description = "The link, readable once.", body = MintedLinkResponse),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "A viewer account, a missing CSRF token, or a \
                           membership below editor on this domain.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain for this caller, or no draft of \
                           theirs at that path.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn mint(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<MintBody>,
) -> Result<Json<MintedLinkResponse>, ApiError> {
    // The role and membership gate first, so a viewer is refused by what they
    // may do rather than by what happens to be drafted: the author check below
    // answers 404, and a 404 reached before the role gate would tell an
    // account with no business here whether a draft exists.
    require_domain_write(&state, &identity, &domain).await?;
    let user = identity.require_account()?;
    let draft = state
        .engine
        .overlay_draft_at(&domain, &user.name, &body.path)
        .await?;
    if draft.is_none() {
        return Err(no_such_draft());
    }
    let minted = state
        .auth
        .mint_overlay_grant(&domain, &body.path, &user.name, body.expires_at)
        .await
        .map_err(|e| ApiError::internal(format!("{e:#}")))?;
    Ok(Json(MintedLinkResponse {
        id: minted.id,
        token: minted.token,
        path: body.path,
    }))
}

/// `GET /domains/{domain}/draft-links?path=` - the links standing on one of
/// the caller's own drafts.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/draft-links",
    tag = "engrams",
    operation_id = "list_draft_links",
    params(
        ("domain" = String, Path, description = "The domain the draft is in."),
        LinksQuery,
    ),
    summary = "The share-links standing on one of the caller's own drafts.",
    description = "Never carries a token: only hashes are stored, so a link \
                   handed over cannot be read back. A row says who redeemed \
                   it, if anybody has, and when it was made. Revoked and \
                   expired links are left out - the list is what still opens \
                   the draft.",
    responses(
        (status = 200, description = "The links on that draft.", body = Vec<OverlayGrant>),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain for this caller, or no draft of \
                           theirs at that path.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn list(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiQuery(query): ApiQuery<LinksQuery>,
) -> Result<Json<Vec<OverlayGrant>>, ApiError> {
    require_domain_read(&state, &identity, &domain).await?;
    let user = identity.require_account()?;
    if state
        .engine
        .overlay_draft_at(&domain, &user.name, &query.path)
        .await?
        .is_none()
    {
        return Err(no_such_draft());
    }
    let grants = state
        .auth
        .overlay_grants_of(&user.name, &domain, &query.path)
        .await
        .map_err(|e| ApiError::internal(format!("{e:#}")))?;
    Ok(Json(grants))
}

/// `DELETE /draft-links/{id}` - take one link back.
///
/// Not domain-addressed, because a link id names its own domain: asking the
/// caller to repeat it would only create a second thing that could disagree
/// with the row.
#[utoipa::path(
    delete,
    path = "/api/v1/draft-links/{id}",
    tag = "engrams",
    operation_id = "revoke_draft_link",
    params(("id" = i64, Path, description = "The link's row id.")),
    summary = "Revoke one share-link.",
    description = "It stops opening anything at once, and the account it was \
                   redeemed by stops seeing the draft on its next request. \
                   404 when the id names no link of the caller's, which is \
                   what somebody else's link and an invented id both answer: \
                   a revoke is never a probe for which links exist. Served on \
                   a read-only instance, like every other account-state \
                   route.",
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
            description = "A viewer account, or a missing CSRF token.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "The caller minted no link with that id.",
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
    identity.require_editor()?;
    let user = identity.require_account()?;
    let removed = state
        .auth
        .revoke_overlay_grant(&user.name, id)
        .await
        .map_err(|e| ApiError::internal(format!("{e:#}")))?;
    if !removed {
        return Err(ApiError::not_found(
            "you minted no draft share-link with that id",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /draft-links/accept` - present a link and read the draft it opens.
#[utoipa::path(
    post,
    path = "/api/v1/draft-links/accept",
    tag = "engrams",
    operation_id = "accept_draft_link",
    request_body = TokenBody,
    summary = "Present a share-link and receive the draft it opens.",
    description = "Binds the link to the caller's account the first time, \
                   whatever their role, and answers the same thing every time \
                   after. The reply carries the draft itself and whether this \
                   account may edit it: a viewer opens it read-only with the \
                   reason beside it, because reading somebody's wording is \
                   exactly what a viewer is for. Editing is a second step - \
                   see `POST /draft-links/join`. 404 for an unknown, revoked, \
                   expired or already-taken link, for a domain this account \
                   may not read, and for a link whose draft is no longer \
                   there: a grant lasts as long as the thing it grants.",
    responses(
        (status = 200, description = "The granted draft.", body = AcceptedDraft),
        (
            status = 401,
            description = "No identity, or an anonymous one: a link binds to \
                           an account, and the anonymous viewer has none.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "A cookie session did not echo its CSRF token.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "The link opens nothing for this account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn accept(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<TokenBody>,
) -> Result<Json<AcceptedDraft>, ApiError> {
    let opened = open_link(&state, &identity, &body.token).await?;
    Ok(Json(opened))
}

/// `POST /draft-links/join` - start editing inside the granted draft.
#[utoipa::path(
    post,
    path = "/api/v1/draft-links/join",
    tag = "engrams",
    operation_id = "join_draft_link",
    request_body = TokenBody,
    summary = "Start working inside the granted draft.",
    description = "Seeing a draft and editing it are two states, and this is \
                   the step between them. The reply carries a key this \
                   SESSION sends back with every write - never the account's, \
                   so a person joining a draft in one window has not joined it \
                   in another and has not joined it for their agent. While it \
                   is held, a save at the granted path lands in the owner's \
                   draft and an upload lands in the owner's files, to be \
                   folded or discarded with it. Refused with 403 when this \
                   account may not write on the domain, in the same words the \
                   read-only editor shows.",
    responses(
        (status = 200, description = "The granted draft, and the key to work in it.", body = AcceptedDraft),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "This account may only read the draft, or the \
                           request did not echo its CSRF token.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "The link opens nothing for this account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "This instance is already holding as many joins as \
                           it will hold at once.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn join(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<TokenBody>,
) -> Result<Json<AcceptedDraft>, ApiError> {
    let mut opened = open_link(&state, &identity, &body.token).await?;
    let user = identity.require_account()?;
    if !opened.editable {
        return Err(ApiError::forbidden(opened.reason.clone().unwrap_or_else(
            || "you may read this draft but not edit it".to_string(),
        )));
    }
    let key = state
        .engine
        .joins()
        .open(Join {
            account: user.name.clone(),
            domain: opened.domain.clone(),
            path: opened.path.clone(),
            owner: opened.owner.clone(),
        })
        .ok_or_else(|| {
            ApiError::conflict(
                "this instance is already holding as many drafts open as it will hold at \
                 once: leave one and try again",
            )
        })?;
    opened.join_key = Some(key);
    Ok(Json(opened))
}

/// `POST /draft-links/leave` - stop working inside the draft.
#[utoipa::path(
    post,
    path = "/api/v1/draft-links/leave",
    tag = "engrams",
    operation_id = "leave_draft_link",
    request_body = LeaveBody,
    summary = "Stop working inside somebody else's draft.",
    description = "Ends the join this session was holding. The link is \
                   untouched: the draft is still readable, and joining again \
                   is one press. Answers 204 for a key that names no open \
                   join of the caller's too - leaving something you are not \
                   inside is not a failure.",
    responses(
        (status = 204, description = "The session is no longer inside the draft."),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "A cookie session did not echo its CSRF token.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn leave(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<LeaveBody>,
) -> Result<StatusCode, ApiError> {
    let user = identity.require_account()?;
    state.engine.joins().close(&body.key, &user.name);
    Ok(StatusCode::NO_CONTENT)
}

/// Present a link and resolve everything both routes need out of it.
///
/// One function, so accepting and joining cannot disagree about what a link
/// opens, about what the draft says, or about whether this account may edit
/// it. The refusals are all 404 and all the same 404, deliberately: an
/// invented link, a revoked one, an expired one, one already bound to somebody
/// else, one into a domain this account may not read, and one whose draft has
/// since been folded, discarded, deleted or renamed away are six different
/// facts and none of them is this caller's to learn.
async fn open_link(
    state: &RestState,
    identity: &Identity,
    token: &str,
) -> Result<AcceptedDraft, ApiError> {
    let user = identity.require_account()?;
    if !token.starts_with(DRAFT_LINK_PREFIX) {
        return Err(dead_link());
    }
    let grant = state
        .auth
        .redeem_overlay_grant(token, &user.name)
        .await
        .map_err(|e| ApiError::internal(format!("{e:#}")))?
        .ok_or_else(dead_link)?;
    // The domain screen, on the grantee rather than on the author: a link is
    // the author's word about one draft, never about a domain, so a private
    // domain this account is not a member of stays a domain it has never heard
    // of. Answered as the dead link above, for the reason every hidden-domain
    // answer on this surface is a 404.
    require_domain_read(state, identity, &grant.domain)
        .await
        .map_err(|_| dead_link())?;
    let draft = state
        .engine
        .overlay_draft_at(&grant.domain, &grant.owner, &grant.path)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "this link was for {}'s draft of '{}', and that draft is no longer there: it \
                 was folded into the domain, discarded, or moved somewhere else. Ask for a \
                 fresh link, or look for the page in the domain itself.",
                grant.owner, grant.path
            ))
        })?;
    let right = state
        .access
        .write_right(&identity.scope(), &grant.domain)
        .await
        .map_err(|e| {
            ApiError::internal(format!("this domain's membership is unreadable: {e:#}"))
        })?;
    let editable = right >= DomainRight::Write;
    Ok(AcceptedDraft {
        domain: grant.domain,
        path: grant.path,
        owner: grant.owner,
        permalink: draft.permalink,
        editable,
        reason: (!editable).then(|| read_only_reason(&right)),
        content: draft.content,
        checksum: draft.checksum,
        join_key: None,
        joined: None,
    })
}

/// Why the editor opened read-only, in words that say what would change it.
fn read_only_reason(right: &DomainRight) -> String {
    format!(
        "you are reading this draft: your access on this domain is {}, and editing somebody's \
         draft needs the same editor access that writing anything else here needs. Suggest \
         changes to whoever shared it, or ask for editor access on the domain.",
        super::member_level_word(*right)
    )
}

/// What an author is told about a path they are not drafting.
///
/// Deliberately the same answer whether nobody holds a draft there or somebody
/// else does: whose drafts exist is exactly what a domain in review mode does
/// not say, and a mint route that said it would be a way to ask.
fn no_such_draft() -> ApiError {
    ApiError::not_found("you are not holding a draft at that path in this domain")
}

/// What every way a link fails to open anything is told.
fn dead_link() -> ApiError {
    ApiError::not_found(
        "this draft link opens nothing: it may have been revoked, it may have expired, it may \
         already belong to somebody else, or the draft it was for may have been folded or \
         discarded. Ask whoever shared it for a fresh one.",
    )
}
