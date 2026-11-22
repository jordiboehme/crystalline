//! The accounts that may sign in to this instance, managed over HTTP.
//!
//! The same operations `crystalline users` performs, behind the same store, so
//! the CLI and the UI are one source of truth rather than two: the listing is
//! the `{"users": [...]}` envelope `users list --json` already prints, and
//! every refusal here is the store's own, surfaced with a status a browser
//! client can branch on. A password reset lives on its own route
//! ([`reset_password`]) rather than as a `PATCH` field, since it is a distinct,
//! higher-stakes action with its own session-revoking side effect.
//!
//! These are the first mutating routes on this surface, so four rules are
//! written down rather than left to be re-derived:
//!
//! 1. **Admin only, in the handler.** [`super::auth::guard`] enforces viewer
//!    and nothing more, so every handler below opens with
//!    [`Identity::require_admin`]. A route added here without it would be
//!    reachable by any account that can log in.
//! 2. **Cross-site protection.** A cookie session must echo its CSRF token on
//!    every unsafe method; the middleware does that. The other two identities
//!    carry no token, so each needs its own reason to be safe:
//!    - The anonymous viewer never gets past `require_admin`. It has no
//!      account, so it is answered 401 rather than served.
//!    - A trusted-header admin is protected by the request shape. `PATCH` and
//!      `DELETE` are not simple methods, so a cross-origin caller cannot send
//!      them at all without a CORS preflight; `POST` it can send, but only with
//!      `application/x-www-form-urlencoded`, `text/plain` or
//!      `multipart/form-data`, and [`create`] takes its body through
//!      [`ApiJson`], which demands `application/json` and refuses all three.
//!      **No CORS layer exists on this surface and one must not be added
//!      without revisiting the CSRF check in [`super::auth`] first**: a
//!      permitted preflight would remove the only thing standing between
//!      another origin and a trusted-header admin's account.
//! 3. **No lockout escape hatch.** The store refuses to remove, disable or
//!    demote the last enabled admin, and that refusal is passed through as a
//!    409 rather than overridden by any flag. An installation must never be
//!    lockable out of its own user management over HTTP; `crystalline users`,
//!    which runs on the machine that holds the database, is the recovery path.
//!    For the same reason an admin may not delete or disable *itself*, which is
//!    a distinct refusal: another admin may well exist, and the account being
//!    protected is the caller's own way back in.
//! 4. **Read-only refuses every mutation.** [`create`], [`update`], [`remove`]
//!    and [`reset_password`] each open with [`refuse_read_only`] right after
//!    their role check: a read-only instance answers 403 rather than touching
//!    the store, and `crystalline users` on the server that holds the database
//!    is the only way to change an account there. `GET /users` carries no such
//!    check - read-only means writes are refused, not that the roster goes
//!    dark.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use crystalline_identity::account_match::{AccountMatcher, fold};
use serde::Deserialize;

use super::auth::{Caller, Identity};
use super::auth_store::{ReflectionLevel, RefusalKind, Role, SignInKind, StoreRefusal, User};
use super::{
    ApiError, ApiJson, ApiPath, ApiQuery, ProblemDetail, RestState, check_password,
    refuse_read_only,
};

/// What `GET /users` answers with. A wrapper rather than a bare array, matching
/// the `{"users": [...]}` envelope `crystalline users list --json` prints.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct UsersResponse {
    /// The accounts of the asked page, or every matching account when no
    /// `limit` was given.
    #[schema(value_type = Vec<super::schemas::User>)]
    users: Vec<User>,
    /// How many accounts match the filters in all, whatever the page.
    total: usize,
}

/// What every writing route but `DELETE` answers with: the account as stored,
/// read back after the write rather than echoed from the request.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct UserResponse {
    /// The account as it now stands.
    #[schema(value_type = super::schemas::User)]
    user: User,
}

/// The state a `GET /users` filter names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StateFilter {
    /// Accounts that may sign in.
    Active,
    /// Disabled accounts.
    Disabled,
}

/// A sign-in kind a `GET /users` filter names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum KindFilter {
    /// A password this instance verifies.
    Password,
    /// A sign-on provider link (OIDC).
    Oidc,
    /// Authenticated by an upstream proxy or trusted header.
    Proxy,
    /// No password and no link, but a personal token.
    TokenOnly,
}

impl KindFilter {
    fn kind(self) -> SignInKind {
        match self {
            KindFilter::Password => SignInKind::Password,
            KindFilter::Oidc => SignInKind::Oidc,
            KindFilter::Proxy => SignInKind::Proxy,
            KindFilter::TokenOnly => SignInKind::TokenOnly,
        }
    }
}

/// The column `GET /users` sorts by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SortColumn {
    /// The login name (the default).
    Name,
    /// The display name, case and accents folded.
    Display,
    /// The role, least to most privileged.
    Role,
    /// Active before disabled.
    State,
    /// When the account was last seen; an account never seen sorts last in
    /// both directions.
    LastSeen,
    /// The sign-in kind, by its name.
    Kind,
}

/// The direction of a sort.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    /// Smallest first (the default).
    Asc,
    /// Largest first.
    Desc,
}

/// The most accounts one page may hold.
const MAX_PAGE_SIZE: usize = 500;

/// The query string `GET /users` takes. Every field is optional; with none of
/// them the answer is every account by name, as it always was.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Search text: a prefix or word prefix of the login or display name (case
    /// and accents folded), or a whole email address.
    #[serde(default)]
    #[param(example = "ada")]
    q: Option<String>,
    /// Only accounts with this role.
    #[serde(default)]
    #[param(value_type = Option<super::schemas::Role>)]
    role: Option<Role>,
    /// Only active or only disabled accounts.
    #[serde(default)]
    state: Option<StateFilter>,
    /// Only accounts that sign in this way.
    #[serde(default)]
    kind: Option<KindFilter>,
    /// Only accounts last seen before this instant (RFC 3339). An account
    /// never seen is not before anything.
    #[serde(default)]
    #[param(example = "2026-11-01T00:00:00Z")]
    seen_before: Option<String>,
    /// Only accounts last seen after this instant (RFC 3339).
    #[serde(default)]
    #[param(example = "2026-11-01T00:00:00Z")]
    seen_after: Option<String>,
    /// `true` for accounts with a GitHub link, `false` for those without.
    #[serde(default)]
    github: Option<bool>,
    /// The column to sort by. Default `name`; `name` breaks every tie.
    #[serde(default)]
    sort: Option<SortColumn>,
    /// The direction. Default `asc`.
    #[serde(default)]
    dir: Option<SortDir>,
    /// The page, from 1. Only read together with `limit`.
    #[serde(default)]
    #[param(minimum = 1)]
    page: Option<usize>,
    /// Accounts per page (1 to 500). Without it every matching account comes
    /// back in one answer.
    #[serde(default)]
    #[param(minimum = 1, maximum = 500)]
    limit: Option<usize>,
}

/// Where a role stands from least to most privileged, for sorting.
fn role_rank(role: Role) -> u8 {
    match role {
        Role::None => 0,
        Role::Viewer => 1,
        Role::Editor => 2,
        Role::Admin => 3,
    }
}

/// An RFC 3339 filter value as an instant, or the 400 that names the filter.
fn instant_filter(name: &str, value: Option<&str>) -> Result<Option<i64>, ApiError> {
    let Some(value) = value else { return Ok(None) };
    crystalline_core::time::parse_rfc3339(value)
        .map(|at| Some(at.stamp().unix_micros()))
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "{name} must be an RFC 3339 time, such as 2026-11-01T00:00:00Z"
            ))
        })
}

/// The instant an account was last seen, in microseconds, if it ever was.
fn seen_micros(user: &User) -> Option<i64> {
    user.last_seen
        .as_deref()
        .and_then(crystalline_core::time::parse_lenient_stamp)
        .map(|at| at.unix_micros())
}

/// Filter, sort and page `users` as `query` asks, returning the page and how
/// many accounts matched before paging.
///
/// Lives apart from the handler so the rules (the `last_seen` NULL order, the
/// `name` tiebreak, the paging) read in one place.
fn select_users(users: Vec<User>, query: &ListQuery) -> Result<(Vec<User>, usize), ApiError> {
    let before = instant_filter("seen_before", query.seen_before.as_deref())?;
    let after = instant_filter("seen_after", query.seen_after.as_deref())?;
    if query.page == Some(0) {
        return Err(ApiError::bad_request("page starts at 1"));
    }
    if let Some(limit) = query.limit
        && !(1..=MAX_PAGE_SIZE).contains(&limit)
    {
        return Err(ApiError::bad_request(format!(
            "limit must be between 1 and {MAX_PAGE_SIZE}"
        )));
    }
    let matcher = query
        .q
        .as_deref()
        .map(|q| AccountMatcher::new(q, true))
        .filter(|matcher| !matcher.is_empty());
    let mut rows: Vec<User> = users
        .into_iter()
        .filter(|user| {
            matcher
                .as_ref()
                .is_none_or(|m| m.matches(&user.name, &user.display, user.email.as_deref()))
                && query.role.is_none_or(|role| user.role == role)
                && query.state.is_none_or(|state| match state {
                    StateFilter::Active => !user.disabled,
                    StateFilter::Disabled => user.disabled,
                })
                && query
                    .kind
                    .is_none_or(|kind| user.sign_in == Some(kind.kind()))
                && query.github.is_none_or(|g| user.github_linked == Some(g))
                && before.is_none_or(|b| seen_micros(user).is_some_and(|seen| seen < b))
                && after.is_none_or(|a| seen_micros(user).is_some_and(|seen| seen > a))
        })
        .collect();
    let total = rows.len();
    let column = query.sort.unwrap_or(SortColumn::Name);
    let descending = query.dir == Some(SortDir::Desc);
    rows.sort_by(|a, b| {
        let by_column = match column {
            SortColumn::Name => a.name.cmp(&b.name),
            SortColumn::Display => fold(&a.display).cmp(&fold(&b.display)),
            SortColumn::Role => role_rank(a.role).cmp(&role_rank(b.role)),
            SortColumn::State => a.disabled.cmp(&b.disabled),
            SortColumn::Kind => {
                let kind = |u: &User| u.sign_in.map(SignInKind::as_str);
                kind(a).cmp(&kind(b))
            }
            SortColumn::LastSeen => {
                // A never-seen account sorts after every seen one whichever
                // way the sort runs, so the direction applies to the seen
                // ones only.
                return match (seen_micros(a), seen_micros(b)) {
                    (Some(x), Some(y)) => {
                        let order = x.cmp(&y);
                        (if descending { order.reverse() } else { order })
                            .then_with(|| a.name.cmp(&b.name))
                    }
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.name.cmp(&b.name),
                };
            }
        };
        (if descending {
            by_column.reverse()
        } else {
            by_column
        })
        .then_with(|| a.name.cmp(&b.name))
    });
    if let Some(limit) = query.limit {
        let skip = (query.page.unwrap_or(1) - 1).saturating_mul(limit);
        rows = rows.into_iter().skip(skip).take(limit).collect();
    }
    Ok((rows, total))
}

/// `GET /users` - the accounts, filtered, sorted and paged as asked; every
/// account, by name, when nothing is asked.
///
/// [`User`] carries no password material, so the rows go out as they come back
/// from the store. Admin only: the account list names every way into the
/// instance and who holds it.
#[utoipa::path(
    get,
    path = "/api/v1/users",
    tag = "users",
    operation_id = "list_users",
    summary = "The accounts, searched, filtered, sorted and paged.",
    description = "An account carries no password material, so the rows go out \
                   as they come back from the store. Admin only: the account \
                   list names every way into the instance and who holds it. \
                   `q` matches a prefix or word prefix of the login or display \
                   name (case and accents folded) or a whole email address. \
                   Sorting is by `sort` and `dir` with the login name as the \
                   tiebreak; an account never seen sorts last by `last_seen` in \
                   both directions. Without `limit` every matching account is \
                   answered, by name, and `page` is ignored. `total` counts the \
                   matches before paging; paging by `last_seen` can shift as \
                   accounts are seen.",
    params(ListQuery),
    responses(
        (status = 200, description = "The matching accounts.", body = UsersResponse),
        (
            status = 400,
            description = "A filter, `page` or `limit` is malformed.",
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
            description = "The caller is not an admin, or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn list(
    State(state): State<RestState>,
    identity: Identity,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<UsersResponse>, ApiError> {
    identity.require_admin()?;
    let users = state
        .auth
        .list_users()
        .await
        .map_err(|e| store_error(e, ""))?;
    let (users, total) = select_users(users, &query)?;
    Ok(Json(UsersResponse { users, total }))
}

/// What `POST /users` takes. `display` defaults to the name as typed, matching
/// `crystalline users add`, and `email` is optional and never used for login.
///
/// [`Debug`] is written by hand rather than derived, here and on
/// [`PasswordBody`]: the derived one would print the plaintext password, and
/// this type is one `tracing::debug!` or one `unwrap` on a rejection away from
/// a log file.
#[derive(Deserialize, utoipa::ToSchema)]
#[schema(description = "A new account. `display` defaults to `name` as typed, \
                        and `email` is optional and never used for login.")]
pub struct CreateBody {
    /// The login name, in any casing: the store folds it.
    #[schema(example = "bob")]
    name: String,
    /// Human-readable name for the UI. Defaults to `name` as typed.
    #[serde(default)]
    #[schema(example = "Bob")]
    display: Option<String>,
    /// Optional contact address.
    #[serde(default)]
    #[schema(example = "bob@example.com")]
    email: Option<String>,
    /// What the new account may do: none (sees no engram on a shared
    /// domain), viewer, editor or admin.
    #[schema(value_type = super::schemas::Role)]
    role: Role,
    /// The Reflection level on every shared domain: default, reader or
    /// contributor. Only raises what the role gives.
    #[serde(default)]
    #[schema(value_type = Option<super::schemas::ReflectionLevel>)]
    reflection: Option<ReflectionLevel>,
    /// The initial password. Never stored in the clear; the store hashes it.
    #[schema(example = "correct horse battery staple")]
    password: String,
}

/// `POST /users` - add an account, answering 201 with the account as stored.
///
/// The response is read back out of the store rather than echoed from the
/// request, so what the client renders is what was written: the folded name,
/// the defaulted display name, and the disabled flag the row starts with.
#[utoipa::path(
    post,
    path = "/api/v1/users",
    tag = "users",
    operation_id = "create_user",
    request_body = CreateBody,
    responses(
        (status = 201, description = "The account as stored.", body = UserResponse),
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
            status = 409,
            description = "That name is already taken.",
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
            description = "The body is JSON but not an account, or the password \
                           is empty.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 503,
            description = "This instance is busy checking other passwords and \
                           the wait for a slot ran out. Hashing a password uses \
                           the same bounded pool signing in does.",
            body = ProblemDetail,
            content_type = "application/problem+json",
            headers(
                ("retry-after" = String, description = "Seconds to wait before \
                 trying again."),
            ),
        ),
    ),
)]
pub async fn create(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<CreateBody>,
) -> Result<(StatusCode, Json<UserResponse>), ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    check_password(&body.password)?;
    clear_name_for_new_account(&state, &body.name, true).await?;
    // The login name as typed makes the better default display name: the store
    // folds the login name but keeps this one as given.
    let display = body.display.unwrap_or_else(|| body.name.trim().to_string());
    // Under the login limiter: `add_user` hashes with argon2id, which costs
    // about 19 MiB on a blocking thread, and an admin client looping over a
    // list of new accounts would otherwise reserve that memory without bound
    // while logins on the same instance are being held to four.
    state
        .with_login_slot(state.auth.add_user_with_reflection(
            &body.name,
            &display,
            body.email.as_deref(),
            body.role,
            body.reflection.unwrap_or_default(),
            &body.password,
        ))
        .await?
        .map_err(|e| store_error(e, &body.name))?;
    let user = read_back(&state, &body.name).await?;
    crate::readers_changed(&state);
    Ok((StatusCode::CREATED, Json(UserResponse { user })))
}

/// The step before an account is made, while the name holds none: see
/// [`crate::engine::Engine::clear_name_for_new_account`]. A name that holds an
/// account, or that no account can have, is left alone, so the insert answers it
/// and nothing saved under the name is touched.
///
/// `admin_reads` says whether an admin is at the other end: the sentence with the
/// domains and the command is theirs, and anybody else (a first sign-in through a
/// proxy or a provider) is told to ask one.
pub(crate) async fn clear_name_for_new_account(
    state: &RestState,
    name: &str,
    admin_reads: bool,
) -> Result<(), ApiError> {
    let Ok(account) = crate::auth_store::normalize_new_account_name(name) else {
        return Ok(());
    };
    if state.auth.user(&account).await?.is_some() {
        return Ok(());
    }
    // The step deletes what is saved under the name, and here that is the bot's
    // token. Only an admin who adds that account may do that, not a stranger's first
    // sign-in. With no token saved there is nothing to protect.
    if !admin_reads
        && state.engine.config().github_agent_identity() == Some(account.as_str())
        && state.engine.personal_token_saved(&account)
    {
        tracing::warn!(
            "a first sign-in under '{account}' was refused: github.agent_identity names it and a GitHub token is saved under it. Add the account first, or unset the key"
        );
        return Err(ApiError::conflict(
            crystalline_core::account::NEW_ACCOUNT_ASK_AN_ADMIN,
        ));
    }
    let Err(refusal) = state.engine.clear_name_for_new_account(&account).await else {
        return Ok(());
    };
    tracing::warn!("no account under '{account}': {}", refusal.logged());
    use crystalline_core::account::NewAccountRefusal as Refusal;
    Err(match (&refusal, admin_reads) {
        (Refusal::Drafts(_), true) => ApiError::conflict(refusal.admin_sentence()),
        (_, true) => ApiError::internal(refusal.admin_sentence()),
        (_, false) => ApiError::conflict(crystalline_core::account::NEW_ACCOUNT_ASK_AN_ADMIN),
    })
}

/// The refusal a person reads at a first sign-in past the account cap. The store's
/// sentence names the setting and the fix, so it goes to the log for the operator.
pub(crate) fn account_cap_refusal(err: &anyhow::Error) -> ApiError {
    tracing::warn!("a first sign-in was refused: {err:#}");
    ApiError::forbidden(crystalline_core::account::NEW_ACCOUNT_CAP_REACHED)
}

/// A first sign-in's provisioning with the same step before it. The store picks
/// the first free name from `desired`, so the step asks about that name. When
/// another account took it in between, the store picks a later one, which was not
/// asked about: that account is taken back out and the person signs in again.
pub(crate) async fn provision_clear(
    state: &RestState,
    desired: &str,
    provision: impl std::future::Future<Output = anyhow::Result<User>>,
) -> Result<anyhow::Result<User>, ApiError> {
    let cleared = match state.auth.free_name_from(desired).await {
        Ok(name) => {
            clear_name_for_new_account(state, &name, false).await?;
            Some(name)
        }
        Err(_) => None,
    };
    match provision.await {
        // Also when the name could not be asked about: an account stands only on a
        // name that was cleared.
        Ok(user) if cleared.as_deref() != Some(user.name.as_str()) => {
            // A sign-in that found an account of its own never reaches here: the
            // caller asks for the linked account first.
            if let Err(e) = state.auth.remove_user_force(&user.name).await {
                tracing::warn!(
                    "the account '{}' could not be taken back out: {e:#}",
                    user.name
                );
            }
            Err(ApiError::conflict(
                "Another account took this name just now. Sign in again.",
            ))
        }
        other => Ok(other),
    }
}

/// What `PATCH /users/{name}` takes: whichever of the four an admin wants to
/// change. All absent is refused rather than served as a no-op, so a client
/// sending the wrong field names hears about it.
///
/// The password lives on its own route ([`reset_password`]) rather than here:
/// a reset is a distinct, higher-stakes action, and keeping it off this body
/// means a client editing a role or a display name can never trip the
/// password's own session-revoking side effect by accident.
#[derive(Deserialize, utoipa::ToSchema)]
#[schema(description = "Whichever of the four an admin wants to change. All \
                        absent is refused with a 422 rather than served as a \
                        no-op, so a client sending the wrong field names hears \
                        about it. `display` clears to the login name when sent \
                        empty or blank; the password lives on its own route, \
                        `POST /users/{name}/password`.")]
pub struct PatchBody {
    /// The new role: none (sees no engram on a shared domain), viewer,
    /// editor or admin.
    #[serde(default)]
    #[schema(value_type = Option<super::schemas::Role>)]
    role: Option<Role>,
    /// The Reflection level on every shared domain: default, reader or
    /// contributor. Only raises what the role gives; `default` clears it.
    #[serde(default)]
    #[schema(value_type = Option<super::schemas::ReflectionLevel>)]
    reflection: Option<ReflectionLevel>,
    /// Whether the account is disabled. Disabling deletes every session the
    /// account holds, so it is a revocation rather than a flag a later
    /// re-enabling hands back; it also stops any new login.
    #[serde(default)]
    disabled: Option<bool>,
    /// The new display name. Empty or blank clears it, resetting the account
    /// to its folded login name - a row is never nameless. Never normalized
    /// like a login name: spaces and casing are kept as sent.
    #[serde(default)]
    display: Option<String>,
}

/// What `POST /users/{name}/password` takes.
///
/// [`Debug`] is hand-written and redacts the password, as on [`CreateBody`].
#[derive(Deserialize, utoipa::ToSchema)]
#[schema(description = "The replacement password. Setting it revokes every \
                        session, MCP token and OAuth grant the account holds, \
                        this admin's own session included when they reset \
                        themselves.")]
pub struct PasswordBody {
    /// The replacement password. Setting it revokes every session, MCP token
    /// and OAuth grant the account holds, this admin's own session included
    /// when they reset themselves.
    #[schema(example = "correct horse battery staple")]
    password: String,
}

impl std::fmt::Debug for CreateBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateBody")
            .field("name", &self.name)
            .field("display", &self.display)
            .field("email", &self.email)
            .field("role", &self.role)
            .field("reflection", &self.reflection)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl std::fmt::Debug for PatchBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatchBody")
            .field("role", &self.role)
            .field("reflection", &self.reflection)
            .field("disabled", &self.disabled)
            .field("display", &self.display)
            .finish()
    }
}

impl std::fmt::Debug for PasswordBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordBody")
            .field("password", &"<redacted>")
            .finish()
    }
}

/// `PATCH /users/{name}` - change a role, a disabled flag, a display name, or
/// any combination, answering with the account as it now stands. A password
/// reset is a separate route, [`reset_password`].
///
/// Disabling an account is a revocation as well as an edit: it deletes every
/// session that account holds, in the store and in the same transaction as the
/// flag. Setting a role or a display name never touches a session.
///
/// Every check runs before the first write, so a request that will be refused
/// changes nothing. The writes themselves are one store call each rather than
/// one transaction: the store's guarded statements each own their own
/// invariant, and a failure part-way leaves the earlier fields applied and says
/// which operation was refused. That is visible to a client only in the
/// pathological case of a concurrent edit racing this one, since the refusals a
/// caller can provoke on purpose - the last-admin guard and an unknown account
/// - are decided identically by all three statements.
#[utoipa::path(
    patch,
    path = "/api/v1/users/{name}",
    tag = "users",
    operation_id = "update_user",
    params(("name" = String, Path, description = "The account, in any casing.")),
    request_body = PatchBody,
    responses(
        (status = 200, description = "The account as it now stands.", body = UserResponse),
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
            status = 404,
            description = "No such account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The change would disable the caller's own account, or \
                           would leave the installation without an enabled admin.",
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
            description = "The body changes nothing: send role, reflection, disabled or display.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn update(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(name): ApiPath<String>,
    ApiJson(body): ApiJson<PatchBody>,
) -> Result<Json<UserResponse>, ApiError> {
    let caller = identity.require_admin()?;
    refuse_read_only(&state)?;
    if body.role.is_none()
        && body.reflection.is_none()
        && body.disabled.is_none()
        && body.display.is_none()
    {
        return Err(ApiError::unprocessable(
            "this request changes nothing: send role, reflection, disabled or display",
        ));
    }
    if body.disabled == Some(true) {
        refuse_self(&caller, &name, "disable")?;
    }
    if let Some(role) = body.role {
        state
            .auth
            .set_role(&name, role)
            .await
            .map_err(|e| store_error(e, &name))?;
    }
    if let Some(level) = body.reflection {
        state
            .auth
            .set_reflection_level(&name, level)
            .await
            .map_err(|e| store_error(e, &name))?;
    }
    if let Some(disabled) = body.disabled {
        state
            .auth
            .set_disabled(&name, disabled)
            .await
            .map_err(|e| store_error(e, &name))?;
    }
    if let Some(display) = &body.display {
        // Blank clears to the folded login name; the store does the trimming
        // and the fallback, so this is a straight pass-through.
        state
            .auth
            .set_display(&name, Some(display.as_str()))
            .await
            .map_err(|e| store_error(e, &name))?;
    }
    crate::readers_changed(&state);
    let user = read_back(&state, &name).await?;
    Ok(Json(UserResponse { user }))
}

/// What `PATCH /me` takes: the one thing a person may change about their own
/// account here, their display name.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(description = "Your own display name. Trimmed; empty or blank \
                        resets it to your login name. At most 100 characters, \
                        and no control or invisible characters.")]
pub struct MyProfileBody {
    /// The new display name. Empty or blank resets it to the login name.
    #[schema(example = "Ada Lovelace")]
    display: String,
}

/// `PATCH /me` - change the caller's own display name, answering with the
/// account as it now stands.
///
/// Self-service in the shape of the other `/me` routes: no name in the path,
/// because the session already names the account, so a caller can only ever
/// reach its own. Every signed-in account may use it, viewers included; the
/// anonymous viewer has no account and is answered 401. Role, Reflection
/// level and the disabled flag stay on the admin route, `PATCH /users/{name}`:
/// the body here has no field for them, and an unknown field is refused.
///
/// The rules are the admin route's, in the store
/// ([`super::auth_store::normalize_display_name`]): trimmed, blank resets to
/// the login name, at most 100 characters, no control or invisible character.
///
/// A read-only instance refuses it, as it refuses the admin route that writes
/// the same column. Unlike an MCP token, which a read-only server needs before
/// an agent can connect at all, a display name is nothing anybody needs there.
/// CSRF is the middleware's, as on every other unsafe route.
#[utoipa::path(
    patch,
    path = "/api/v1/me",
    tag = "users",
    operation_id = "update_my_profile",
    request_body = MyProfileBody,
    responses(
        (status = 200, description = "Your account as it now stands.", body = UserResponse),
        (
            status = 400,
            description = "The body is not JSON.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity, or the anonymous viewer, which has no \
                           account of its own.",
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
            status = 415,
            description = "The body is not `application/json`.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The body is JSON but has no `display` or carries \
                           another field, or the display \
                           name is longer than 100 characters or holds a control \
                           or invisible character.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn update_me(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<MyProfileBody>,
) -> Result<Json<UserResponse>, ApiError> {
    let user = identity.require_account()?;
    refuse_read_only(&state)?;
    state
        .auth
        .set_display(&user.name, Some(body.display.as_str()))
        .await
        .map_err(|e| store_error(e, &user.name))?;
    let user = read_back(&state, &user.name).await?;
    Ok(Json(UserResponse { user }))
}

/// `POST /users/{name}/password` - replace an account's password, answering
/// with the account as it now stands.
///
/// A distinct route rather than a `PATCH` field: setting a password revokes
/// every session, MCP token and OAuth grant the account holds, in the same
/// transaction as the change - the reset is useless against a compromised
/// account otherwise, since a cookie minted before it would keep working for
/// the rest of its life, and a token or grant minted with that cookie would
/// keep an agent connected. An admin resetting their own password signs their
/// own other sessions out too, this one included.
#[utoipa::path(
    post,
    path = "/api/v1/users/{name}/password",
    tag = "users",
    operation_id = "reset_user_password",
    params(("name" = String, Path, description = "The account, in any casing.")),
    request_body = PasswordBody,
    responses(
        (status = 200, description = "The account as it now stands.", body = UserResponse),
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
            status = 404,
            description = "No such account.",
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
            description = "The new password is empty.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 503,
            description = "This instance is busy checking other passwords and \
                           the wait for a slot ran out. Hashing a password uses \
                           the same bounded pool signing in does.",
            body = ProblemDetail,
            content_type = "application/problem+json",
            headers(
                ("retry-after" = String, description = "Seconds to wait before \
                 trying again."),
            ),
        ),
    ),
)]
pub async fn reset_password(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(name): ApiPath<String>,
    ApiJson(body): ApiJson<PasswordBody>,
) -> Result<Json<UserResponse>, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    check_password(&body.password)?;
    // Under the login limiter, for the reason `create` gives.
    state
        .with_login_slot(state.auth.set_password(&name, &body.password))
        .await?
        .map_err(|e| store_error(e, &name))?;
    let user = read_back(&state, &name).await?;
    Ok(Json(UserResponse { user }))
}

/// `DELETE /users/{name}` - remove an account and every session it holds,
/// answering 204.
///
/// An admin may not remove its own account (409), and the store refuses to
/// remove the last enabled admin whoever asks (409 as well, in its own words).
#[utoipa::path(
    delete,
    path = "/api/v1/users/{name}",
    tag = "users",
    operation_id = "delete_user",
    params(("name" = String, Path, description = "The account, in any casing.")),
    responses(
        (status = 204, description = "The account and its sessions are gone."),
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
            status = 404,
            description = "No such account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The account is the caller's own, or is the last \
                           enabled admin.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn remove(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(name): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    let caller = identity.require_admin()?;
    refuse_read_only(&state)?;
    refuse_self(&caller, &name, "delete")?;
    state
        .auth
        .remove_user(&name)
        .await
        .map_err(|e| store_error(e, &name))?;
    // Its GitHub token, cached copy and single-use link go with it, so an
    // account made later under the same name cannot write on the token or open
    // the link.
    if let Ok(account) = crate::auth_store::normalize_account_name(&name) {
        state.engine.forget_removed_account(&account).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /users/{name}/drafts` - end every draft a name still holds.
///
/// Admin only, and never automatic: this is the way out when a removed account
/// left drafts and its name is wanted again. Works on a name with no account,
/// and refuses one that has an account, whose drafts are that person's own.
#[utoipa::path(
    delete,
    path = "/api/v1/users/{name}/drafts",
    tag = "users",
    operation_id = "discard_user_drafts",
    summary = "End every draft a removed account left under its name.",
    description = "Admin only. Drafts of a removed account stay until an admin \
                   asks for this, and a new account under that name is refused \
                   while they do. Logged with who asked. A draft open in a live \
                   editor is left, and named under `left`. Refused for a name \
                   with an account.",
    params(("name" = String, Path, description = "The name, in any casing.")),
    responses(
        (
            status = 200,
            description = "What went, per domain.",
            body = Object,
            example = json!({
                "account": "ada",
                "domains": [{ "domain": "eng", "discarded": 3, "left": [] }]
            }),
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The name has an account, or is the machine owner's.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn discard_drafts(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caller = identity.require_admin()?;
    refuse_read_only(&state)?;
    let account =
        crate::auth_store::normalize_account_name(&name).map_err(|e| store_error(e, &name))?;
    if state.auth.user(&account).await?.is_some() {
        return Err(ApiError::conflict(format!(
            "'{account}' is an account, and its drafts are its own; this is for a name whose account was removed"
        )));
    }
    match state
        .engine
        .discard_account_drafts(&account, caller.name())
        .await
    {
        Ok(report) => Ok(Json(report)),
        Err(crate::engine::EngineError::Conflict(sentence)) => Err(ApiError::conflict(sentence)),
        Err(e) => Err(e.into()),
    }
}

/// `DELETE /users/{name}/github-link` - unlink another account from GitHub.
///
/// Admin only: this is how a GitHub account linked to the wrong Crystalline
/// account is freed, so the right one can link it. The user token is revoked
/// at GitHub, deleted here and the link removed, exactly as when the person
/// unlinks themselves. An admin may unlink its own account here too; nothing
/// about its session depends on the link.
#[utoipa::path(
    delete,
    path = "/api/v1/users/{name}/github-link",
    tag = "users",
    operation_id = "unlink_user_github_link",
    params(("name" = String, Path, description = "The account, in any casing.")),
    responses(
        (status = 200, description = "Unlinked.", body = super::github_link::GithubUnlinkResponse),
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
            status = 404,
            description = "No such account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn unlink_github(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<super::github_link::GithubUnlinkResponse>, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    let name = folded(&name);
    if state.auth.user(&name).await?.is_none() {
        return Err(ApiError::not_found(format!("no account named '{name}'")));
    }
    Ok(Json(super::github_link::unlink_response(
        state.engine.unlink_github(&name).await?,
    )))
}

/// Refuse an admin acting destructively on its own account. `verb` names what
/// was asked for, and the message is deliberately unlike the store's last-admin
/// one: this refusal can fire while other admins exist, and what it protects is
/// the caller's own session rather than the installation.
fn refuse_self(caller: &Caller, target: &str, verb: &str) -> Result<(), ApiError> {
    if folded(target) != caller.name() {
        return Ok(());
    }
    Err(ApiError::conflict(format!(
        "refusing to {verb} your own account ('{}'): ask another admin to do it, \
         or use `crystalline users` on the server",
        caller.name()
    )))
}

/// The account as it now stands, read back out of the store after a write.
///
/// The listing rather than a single-row read because the store exposes no
/// by-name getter, and the cost is one small query over a table with a handful
/// of rows in it. Shared with `super::auth::setup`, which reads its brand new
/// first admin back the same way rather than echoing the request.
pub(super) async fn read_back(state: &RestState, name: &str) -> Result<User, ApiError> {
    let name = folded(name);
    state
        .auth
        .list_users()
        .await
        .map_err(|e| store_error(e, &name))?
        .into_iter()
        .find(|user| user.name == name)
        .ok_or_else(|| {
            // Only reachable if something removed the account between the write
            // and this read. The write did happen, so this is not a failure of
            // the request - but answering with a body that was made up here
            // would be worse than saying the read did not work.
            ApiError::internal(format!(
                "the account '{name}' was written but could not be read back"
            ))
        })
}

/// The form the store keys on: trimmed and lowercased, mirroring the store's
/// own `normalize_account_name`.
///
/// Mirrored rather than shared because the store's folding is private to it and
/// is the authority; nothing here writes with this value, it only compares
/// (`refuse_self`) and looks up (`read_back`). `folding_matches_the_store`
/// pins the two together, so a change on either side fails a test rather than
/// silently opening a way around the self-account check.
fn folded(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Classify an [`AuthStore`](super::AuthStore) failure for the wire.
///
/// The store reports in `anyhow`, so this reads its message. That is a string
/// seam, and it is tested rather than assumed: `store_errors_are_classified`
/// below drives a real store into each of these failures and asserts the status
/// that comes out, so rewording a message on the store side fails here instead
/// of quietly turning a 409 into a 500.
///
/// Anything unrecognized stays a 500: an error this layer cannot name is not
/// the caller's fault by default. `subject` is the account the failed call was
/// about, for the one message that has to be rewritten rather than passed
/// through; pass `""` where there is no single one.
fn store_error(e: anyhow::Error, subject: &str) -> ApiError {
    // A display name the store refused is the caller's to fix, in the store's
    // own words, which name the rule and not the account.
    if StoreRefusal::kind_of(&e) == Some(RefusalKind::InvalidDisplayName) {
        return ApiError::unprocessable(
            e.chain()
                .find_map(|cause| cause.downcast_ref::<StoreRefusal>())
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("{e:#}")),
        );
    }
    let detail = format!("{e:#}");
    if detail.contains("UNIQUE constraint") {
        // The store's message here is the database's, and it names a column.
        // The caller gets product copy instead, naming the account and what to
        // do about it, the way `crystalline users add` does.
        return ApiError::conflict(format!(
            "a user named '{}' already exists; edit that account instead",
            folded(subject)
        ));
    }
    // Before the last-admin branch, not after: `no such user: 'the last admin'`
    // is a legal name for an account nobody created, and reading the refusals
    // in the other order would answer that miss with a 409.
    if detail.contains("no such user") {
        return ApiError::not_found(detail);
    }
    if detail.contains("the last admin") {
        return ApiError::conflict(detail);
    }
    if detail.contains("a user name cannot be empty") {
        return ApiError::unprocessable(detail);
    }
    if detail.contains("cannot contain whitespace") {
        return ApiError::unprocessable(detail);
    }
    ApiError::internal(detail)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::AuthStore;

    async fn store() -> (tempfile::TempDir, AuthStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        (dir, store)
    }

    /// The classifier against the store's real errors rather than against
    /// strings written down twice: every one of these is provoked by asking the
    /// store to do the thing it refuses.
    #[tokio::test]
    async fn store_errors_are_classified() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "s3cret")
            .await
            .unwrap();

        let duplicate = store
            .add_user("ADA", "Ada again", None, Role::Viewer, "other")
            .await
            .expect_err("the primary key refuses a second 'ada'");
        let api = store_error(duplicate, "ADA");
        assert_eq!(api.status, StatusCode::CONFLICT);
        assert!(
            !api.detail.contains("UNIQUE"),
            "in product copy: {}",
            api.detail
        );
        assert!(
            api.detail.contains("'ada'"),
            "naming the account as it is stored: {}",
            api.detail
        );

        let last_admin = store
            .set_role("ada", Role::Viewer)
            .await
            .expect_err("the last admin cannot be demoted");
        let api = store_error(last_admin, "ada");
        assert_eq!(api.status, StatusCode::CONFLICT);
        assert!(
            api.detail.contains("last admin"),
            "the store's own words: {}",
            api.detail
        );

        let removed = store.remove_user("ada").await.expect_err("nor removed");
        let api = store_error(removed, "ada");
        assert_eq!(api.status, StatusCode::CONFLICT);
        assert!(api.detail.contains("last admin"), "{}", api.detail);

        let missing = store
            .set_password("ghost", "s3cret")
            .await
            .expect_err("no such account");
        let api = store_error(missing, "ghost");
        assert_eq!(api.status, StatusCode::NOT_FOUND);
        assert!(api.detail.contains("ghost"), "{}", api.detail);

        // The phrase collision the last-admin branch order guards against
        // ("no such user: 'the last admin'" containing "the last admin") is no
        // longer reachable through a real name: spaces are refused by
        // normalize_account_name before the store ever gets to say "no such user".
        // The branch order stays anyway, as documentation of the hazard it
        // once guarded against.
        let named = store
            .set_password("the last admin", "s3cret")
            .await
            .expect_err("whitespace is refused before the name is looked up");
        assert_eq!(
            store_error(named, "the last admin").status,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let empty = store
            .set_disabled("   ", true)
            .await
            .expect_err("an empty name is not a name");
        assert_eq!(
            store_error(empty, "   ").status,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        assert_eq!(
            store_error(anyhow::anyhow!("the disk fell over"), "ada").status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "an error this layer cannot name is not the caller's fault"
        );
    }

    /// [`folded`] and the store must agree, because the self-account check
    /// compares this layer's folding against the name the store handed back.
    #[tokio::test]
    async fn folding_matches_the_store() {
        let (_dir, store) = store().await;
        for raw in ["  AdA ", "ADA", "ada", "Ada\t"] {
            store
                .add_user(raw, "Ada", None, Role::Viewer, "s3cret")
                .await
                .ok();
            let users = store.list_users().await.unwrap();
            assert_eq!(users.len(), 1, "every spelling is the one account");
            assert_eq!(users[0].name, folded(raw), "'{raw}' folds the same way");
        }
    }

    /// The self-account refusal fires on the folded name, so a differently
    /// spelled path is not a way around it, and leaves another account alone.
    #[test]
    fn an_admin_cannot_target_its_own_account() {
        let caller = Caller::Account(User {
            name: "root".to_string(),
            display: "Root".to_string(),
            email: None,
            role: Role::Admin,
            disabled: false,
            last_seen: None,
            reflection: ReflectionLevel::Default,
            sign_in: None,
            github_linked: None,
        });
        for spelling in ["root", "ROOT", "  Root  "] {
            let refused = refuse_self(&caller, spelling, "delete")
                .expect_err("'{spelling}' is the caller's own account");
            assert_eq!(refused.status, StatusCode::CONFLICT);
            assert!(refused.detail.contains("your own account"));
        }
        assert!(refuse_self(&caller, "ada", "delete").is_ok());
    }

    /// The plaintext password must never be one `tracing::debug!` away, on
    /// either body that carries one.
    #[test]
    fn a_debugged_body_redacts_the_password() {
        let create: CreateBody = serde_json::from_value(json!({
            "name": "bob",
            "role": "viewer",
            "password": "hunter2",
        }))
        .unwrap();
        let text = format!("{create:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");
        assert!(text.contains("bob"), "the rest of the body is still there");

        let reset: PasswordBody = serde_json::from_value(json!({"password": "hunter2"})).unwrap();
        let text = format!("{reset:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");
    }

    /// [`PatchBody`] carries no password, so its `Debug` needs no redaction -
    /// pinned so a field added back to it later is not silently exempt.
    #[test]
    fn a_debugged_patch_body_carries_no_secret() {
        let patch: PatchBody =
            serde_json::from_value(json!({"display": "Eddy the Editor"})).unwrap();
        let text = format!("{patch:?}");
        assert!(text.contains("Eddy the Editor"), "{text}");

        let empty: PatchBody = serde_json::from_value(json!({})).unwrap();
        assert!(
            format!("{empty:?}").contains("None"),
            "an absent field still reads as absent"
        );
    }

    /// An empty password is refused before it reaches the store.
    #[test]
    fn an_empty_password_is_refused() {
        assert_eq!(
            check_password("").unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(check_password(" ").is_ok(), "a space is a character");
    }
    fn user(name: &str, seen: Option<&str>) -> User {
        User {
            name: name.to_string(),
            display: name.to_string(),
            email: None,
            role: Role::Viewer,
            disabled: false,
            last_seen: seen.map(str::to_string),
            reflection: ReflectionLevel::Default,
            sign_in: Some(SignInKind::Password),
            github_linked: Some(false),
        }
    }

    fn query(text: &str) -> ListQuery {
        let pairs: Vec<(String, String)> = text
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (k, v) = p.split_once('=').unwrap();
                (k.to_string(), v.to_string())
            })
            .collect();
        let object: serde_json::Map<String, serde_json::Value> = pairs
            .into_iter()
            .map(|(k, v)| {
                let value = v
                    .parse::<usize>()
                    .map(serde_json::Value::from)
                    .unwrap_or(serde_json::Value::String(v));
                (k, value)
            })
            .collect();
        serde_json::from_value(serde_json::Value::Object(object)).unwrap()
    }

    fn names(users: &[User]) -> Vec<&str> {
        users.iter().map(|u| u.name.as_str()).collect()
    }

    fn roster() -> Vec<User> {
        vec![
            user("a", None),
            user("b", Some("2026-11-02T10:00:00Z")),
            user("c", Some("2026-11-01T10:00:00+00:00")),
            user("d", None),
            // Seen later than "b" by the clock, though it sorts earlier as text.
            user("e", Some("2026-11-02T09:30:00-02:00")),
        ]
    }

    #[test]
    fn never_seen_sorts_last_in_both_directions_and_ties_break_by_name() {
        let (asc, _) = select_users(roster(), &query("sort=last_seen")).unwrap();
        assert_eq!(names(&asc), ["c", "b", "e", "a", "d"]);
        let (desc, _) = select_users(roster(), &query("sort=last_seen&dir=desc")).unwrap();
        assert_eq!(names(&desc), ["e", "b", "c", "a", "d"]);
    }

    #[test]
    fn the_time_filters_compare_instants_and_skip_accounts_never_seen() {
        let (rows, total) =
            select_users(roster(), &query("seen_after=2026-11-01T12:00:00Z")).unwrap();
        assert_eq!(names(&rows), ["b", "e"]);
        assert_eq!(total, 2);
        let (rows, _) = select_users(roster(), &query("seen_before=2026-11-02T09:30:00Z")).unwrap();
        assert_eq!(names(&rows), ["c"]);
    }

    #[test]
    fn no_limit_means_everything_and_paging_reports_the_whole_total() {
        let (all, total) = select_users(roster(), &query("page=3")).unwrap();
        assert_eq!((all.len(), total), (5, 5));
        let (page, total) = select_users(roster(), &query("limit=2&page=3")).unwrap();
        assert_eq!(names(&page), ["e"]);
        assert_eq!(total, 5);
    }
}
