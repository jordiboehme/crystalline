//! Who a REST request is, and what that lets it do.
//!
//! Three ways in, tried in this order and never blended:
//!
//! 1. **The trusted header.** An upstream proxy has already authenticated the
//!    caller and names them in the header `auth.trusted_header` configures. The
//!    account is provisioned at viewer on first sight, so an SSO deployment
//!    needs no separate user creation step. Believed only when configured: an
//!    instance that has not been told to trust a proxy ignores the header
//!    whatever a client sends.
//! 2. **The session cookie.** [`SESSION_COOKIE`] carries a token issued by
//!    `POST /auth/login`; the store resolves it to an account and the session's
//!    CSRF token.
//! 3. **Anonymous.** With `auth.anonymous` on, a request that carries neither
//!    is still served, at viewer level and with no account behind it.
//!
//! Both settings are resolved once, when the HTTP surface is built (see
//! [`AuthCfg`]), matching `service.read_only`: a flip takes effect at the next
//! start, never halfway through a served request.
//!
//! Everything below the auth endpoints is closed by default. [`guard`] runs
//! ahead of routing for every `/api/v1` path, so a caller with no identity is
//! told to authenticate rather than being told which paths exist, and a route
//! added later is guarded the moment it is registered rather than when someone
//! remembers to add an extractor to it.

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, header};
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use crystalline_core::config::{GlobalConfig, ShareIdentityMode};
use tokio::sync::Semaphore;

use super::auth_store::{AuthStore, PasswordCheck, Role, SessionMint, User, dummy_verify};
use super::{ApiError, ApiJson, ProblemDetail, RestState};

/// The session cookie. Named for the UI it serves so it never collides with a
/// cookie another app sets on a shared host.
pub const SESSION_COOKIE: &str = "fluid_session";

/// The header a mutating request echoes its session's CSRF token in.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// How long a session stays valid. Long enough that a browser kept open across
/// a holiday is still logged in, short enough that an abandoned cookie expires
/// within a quarter. Logging out revokes immediately either way.
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// How many password verifications may run at once. Argon2id at the crate's
/// recommended defaults costs about 19 MiB per verification, and every one runs
/// on a `spawn_blocking` thread, of which tokio allows 512: unbounded, a burst
/// of logins would reserve gigabytes of memory before any of them finished.
/// Four is enough that a household of users never queues noticeably and small
/// enough that the worst case is a rounding error.
pub const LOGIN_SLOTS: usize = 4;

/// The one path the CSRF check cannot apply to: there is no session yet, so
/// there is no token to echo.
const LOGIN_PATH: &str = "/auth/login";

/// The first-run path, which is pre-session for the same reason login is: it
/// creates the account every later session belongs to. See [`setup`].
pub const SETUP_PATH: &str = "/auth/setup";

/// Paths served without an identity. The rest of the API is closed by default.
/// `/auth/me` is the capability probe a client calls before it knows whether it
/// is logged in, `/auth/logout` must work with a cookie the server has
/// already forgotten, which is exactly the case a browser retries, and
/// `/auth/setup` runs when no account exists at all, so there is no identity
/// for the guard to find and it would otherwise 401 the one route the wizard
/// has.
const PUBLIC_PATHS: [&str; 4] = [LOGIN_PATH, "/auth/logout", "/auth/me", SETUP_PATH];

/// The three auth settings, resolved once when the HTTP surface is built.
///
/// Startup-effective by design, like `service.read_only`: the trusted header is
/// parsed into a [`HeaderName`] here so a typo is a clear startup error rather
/// than a header that silently never matches, and so no request pays for the
/// parse.
#[derive(Clone, Debug)]
pub struct AuthCfg {
    /// The header a trusted proxy names the authenticated user in, from
    /// `auth.trusted_header`. `None` means the path is off.
    pub trusted_header: Option<HeaderName>,
    /// Whether a request carrying no identity is served anyway, from
    /// `auth.anonymous`.
    pub anonymous: bool,
    /// How many accounts trusted-header provisioning may mint in total, from
    /// `auth.max_users`. Only minting a *new* account is capped; an existing
    /// one always resolves, and the `crystalline users` CLI is never capped.
    pub max_users: usize,
}

impl Default for AuthCfg {
    fn default() -> AuthCfg {
        AuthCfg {
            trusted_header: None,
            anonymous: false,
            max_users: crystalline_core::config::DEFAULT_MAX_USERS,
        }
    }
}

impl AuthCfg {
    /// Read all three settings out of `config`, validating the header name.
    ///
    /// The settings layer only checks that the value is non-empty and has no
    /// whitespace (see `settings::set_trusted_header`), which still admits
    /// names HTTP does not allow. Rejecting those here means an operator who
    /// mistypes learns at startup instead of wondering why their proxy's header
    /// is ignored.
    pub fn resolve(config: &GlobalConfig) -> anyhow::Result<AuthCfg> {
        let trusted_header = match config.auth_trusted_header() {
            Some(raw) => Some(HeaderName::try_from(raw.to_ascii_lowercase()).with_context(
                || format!("auth.trusted_header is not a valid HTTP header name: '{raw}'"),
            )?),
            None => None,
        };
        Ok(AuthCfg {
            trusted_header,
            anonymous: config.auth_anonymous(),
            max_users: config.auth_max_users(),
        })
    }
}

/// Who the current request is. Resolved once by [`guard`] and handed to every
/// handler through the request extensions, so a route never repeats the store
/// lookups the guard already did.
#[derive(Clone, Debug, Default)]
pub struct Identity {
    /// The account behind the request, if any. Anonymous access has none.
    pub user: Option<User>,
    /// The CSRF token of the session this identity came from. `None` for the
    /// anonymous path, which has no session, and for a trusted-header identity
    /// that has not called `/auth/me` yet: that probe mints it one, and this
    /// adopts the token whenever the cookie names the same account.
    pub csrf: Option<String>,
    /// Whether this request is being served as the anonymous viewer.
    pub anonymous: bool,
}

/// Who a request is being served as, once a guard has allowed it through.
/// Anonymous access has no account behind it, so it carries no user row.
#[derive(Clone, Debug)]
pub enum Caller {
    /// A real account.
    Account(User),
    /// The anonymous viewer `auth.anonymous` allows.
    Anonymous,
}

impl Caller {
    /// What this caller may do. The anonymous viewer is exactly a viewer.
    pub fn role(&self) -> Role {
        match self {
            Caller::Account(user) => user.role,
            Caller::Anonymous => Role::Viewer,
        }
    }

    /// The account name, or `anonymous` when there is no account. Suitable for
    /// a log line or a write's provenance, never for an authorization decision.
    pub fn name(&self) -> &str {
        match self {
            Caller::Account(user) => &user.name,
            Caller::Anonymous => "anonymous",
        }
    }
}

/// Roles ordered least to most privileged, for the `at least` comparison the
/// guards make. A local mapping rather than an `Ord` on [`Role`]: the ordering
/// is an authorization rule of this layer, not a property of the stored value.
fn rank(role: Role) -> u8 {
    match role {
        Role::Viewer => 0,
        Role::Editor => 1,
        Role::Admin => 2,
    }
}

impl Identity {
    /// The caller, when the request may be served at viewer level or above.
    /// 401 when the request carries no identity at all.
    pub fn require_viewer(&self) -> Result<Caller, ApiError> {
        self.require(Role::Viewer)
    }

    /// The caller, when the request may mutate content. 403 for a viewer
    /// account, 401 for the anonymous viewer and for no identity at all:
    /// anonymous identities can NEVER write, whatever the deployment mode,
    /// and logging in is what would change that.
    pub fn require_editor(&self) -> Result<Caller, ApiError> {
        self.require(Role::Editor)
    }

    /// The caller, when the request may be served at admin level. 403 when an
    /// account is authenticated but not privileged enough, 401 when there is no
    /// account to judge: the anonymous viewer can log in to become an admin, so
    /// it is told to authenticate rather than that it is forbidden.
    pub fn require_admin(&self) -> Result<Caller, ApiError> {
        self.require(Role::Admin)
    }

    /// The caller, when the request may drive this instance's share surfaces.
    ///
    /// The one rule, in one place, for the routes that act on a team
    /// repository AND for the capability probe that tells a client whether to
    /// draw them at all. Two readers of one predicate rather than two copies
    /// of it: a client that drew a share card the routes then refused, or hid
    /// one they would have served, is worse than either answer on its own.
    ///
    /// The gate moves with `github.share_identity`, because what it protects
    /// moves with it: in `instance` mode every share goes out on the one
    /// machine credential and carries nobody's own name, so it stays admin
    /// only; in `personal` mode the share is signed by the acting account's
    /// own GitHub identity, which is the accountability the admin gate stood
    /// in for, so the role that may already write the knowledge may propose it
    /// too. A viewer never may, in either mode.
    pub fn may_share(&self, mode: ShareIdentityMode) -> Result<Caller, ApiError> {
        match mode {
            ShareIdentityMode::Instance => self.require_admin(),
            ShareIdentityMode::Personal => self.require_editor(),
        }
    }

    fn require(&self, min: Role) -> Result<Caller, ApiError> {
        if let Some(user) = &self.user {
            if rank(user.role) >= rank(min) {
                return Ok(Caller::Account(user.clone()));
            }
            return Err(ApiError::forbidden(format!(
                "this account is a {}, and {min} access is required",
                user.role
            )));
        }
        if self.anonymous && rank(min) == rank(Role::Viewer) {
            return Ok(Caller::Anonymous);
        }
        Err(ApiError::unauthorized(
            "this request carries no identity: log in first",
        ))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Identity {
    type Rejection = ApiError;

    /// Read what [`guard`] resolved. A missing identity is a wiring mistake
    /// (a route mounted outside the guarded router), never a caller error, so
    /// it fails as a 500 rather than quietly serving the request unidentified.
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Identity, ApiError> {
        parts
            .extensions
            .get::<Identity>()
            .cloned()
            .ok_or_else(|| ApiError::internal("this route was served without the auth middleware"))
    }
}

/// Resolve the identity, enforce CSRF and close every non-public path, then run
/// the route with the identity attached.
///
/// One middleware rather than three layers: the three steps share the store
/// lookups that resolving costs, and their order is the security property. It
/// runs for the whole `/api/v1` router including its fallback, so an unknown
/// path answers 401 rather than mapping out the API for an unauthenticated
/// caller.
pub async fn guard(
    State(state): State<RestState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let identity = resolve(&state, req.headers()).await?;
    check_csrf(&identity, &req)?;
    if !PUBLIC_PATHS.contains(&req.uri().path()) {
        identity.require_viewer()?;
    }
    req.extensions_mut().insert(identity);
    Ok(next.run(req).await)
}

/// The trusted header, then the session cookie, then anonymous, then nothing.
async fn resolve(state: &RestState, headers: &HeaderMap) -> Result<Identity, ApiError> {
    if let Some(name) = &state.auth_cfg.trusted_header
        && let Some(raw) = headers.get(name)
        && let Ok(value) = raw.to_str()
        && !value.trim().is_empty()
    {
        // The store folds the name, so a proxy that sends `Ada` today and `ada`
        // tomorrow keeps one account, and it hands disabled accounts back like
        // any other: refusing them is this layer's job.
        let user = state
            .auth
            .ensure_user(value, Role::Viewer, state.auth_cfg.max_users)
            .await
            .map_err(|e| {
                let msg = format!("{e:#}");
                if msg.contains("auth.max_users") || msg.contains("login name") {
                    // The header named an identity this instance will not
                    // provision: the caller cannot fix it, the operator can.
                    ApiError::forbidden(msg)
                } else {
                    ApiError::internal(msg)
                }
            })?;
        if user.disabled {
            return Err(ApiError::forbidden("this account is disabled"));
        }
        // The settlement gives trusted-header identities a real session too:
        // /auth/me mints it and this adopts its CSRF token. The cookie is
        // preferred when it names the same account the header does, so a
        // browser echoes the token of the session it actually holds. A cookie
        // for anyone else (the proxy re-mapped the identity) is ignored: the
        // header is the authority in this mode.
        let from_cookie = match CookieJar::from_headers(headers).get(SESSION_COOKIE) {
            Some(cookie) => match state.auth.session_user(cookie.value()).await? {
                Some((session_user, csrf)) if session_user.name == user.name => Some(csrf),
                _ => None,
            },
            None => None,
        };
        // With no usable cookie, fall back to the account's own live session.
        // In this mode the cookie carries nothing the header has not already
        // said, so binding the token to it would lock out exactly the callers
        // that have none: a client that keeps no cookie jar, a device whose
        // cookie went stale, and the second of two tabs opened at once, which
        // is handed a reused session precisely because it has no token of its
        // own to be given. What the token proves is unchanged either way - that
        // whoever sent this read an /auth/me answer for this identity, which no
        // other origin can do while no CORS layer exists.
        let csrf = match from_cookie {
            Some(csrf) => Some(csrf),
            None => state.auth.newest_session_csrf(&user.name).await?,
        };
        return Ok(Identity {
            user: Some(user),
            csrf,
            anonymous: false,
        });
    }
    if let Some(cookie) = CookieJar::from_headers(headers).get(SESSION_COOKIE)
        && let Some((user, csrf)) = state.auth.session_user(cookie.value()).await?
    {
        return Ok(Identity {
            user: Some(user),
            csrf: Some(csrf),
            anonymous: false,
        });
    }
    Ok(Identity {
        user: None,
        csrf: None,
        anonymous: state.auth_cfg.anonymous,
    })
}

/// Refuse a mutating request that does not echo its session's CSRF token.
///
/// One rule, for every identity mode: every unsafe request from an
/// account-bearing identity echoes its session's token. A cookie session has
/// one from login; a trusted-header identity is minted one by [`me`], which is
/// the probe every client opens on. Neither is waved through, so there is no
/// mode whose writes are protected by something other than this check. Login is
/// exempt because it is what mints the token, and the identities with no account
/// at all pass through here only because they can never reach a write:
/// [`Identity::require_editor`] and [`Identity::require_admin`] refuse the
/// anonymous viewer before any handler runs.
///
/// Historically the trusted-header path carried no token and leaned on the shape
/// a cross-site request is allowed to have instead: `PATCH` and `DELETE` are not
/// simple methods, so another origin cannot send them without a CORS preflight,
/// and the `POST` it can send is refused by [`ApiJson`], which demands
/// `application/json` while a cross-site form can only send
/// `application/x-www-form-urlencoded`, `text/plain` or `multipart/form-data`.
/// That argument is now a second line of defence rather than the only one.
///
/// **No CORS layer exists on this surface and one must not be added without
/// revisiting this check**: allowing a cross-origin preflight would remove the
/// only thing standing between another origin and a trusted-header admin's
/// account.
///
/// [`SETUP_PATH`] is exempt beside login and for the same reason: it runs
/// before the first account exists, so no session and therefore no token can
/// have been issued to echo. An account-less identity would pass the branch
/// below anyway; the exemption is spelled out because it documents why the
/// route is safe pre-session, and because it keeps the one identity that
/// arrives with an account and no token - a trusted-header caller - from being
/// told to fetch a token for an endpoint that is [`gone`] for it either way
/// (with a trusted header configured, that proxy's first request mints an
/// account, which closes setup).
fn check_csrf(identity: &Identity, req: &Request) -> Result<(), ApiError> {
    let path = req.uri().path();
    if req.method().is_safe() || path == LOGIN_PATH || path == SETUP_PATH {
        return Ok(());
    }
    let Some(expected) = identity.csrf.as_deref() else {
        if identity.user.is_some() {
            // An account with no token cannot prove the request came from a
            // same-origin client. A cookie session always has one; this is a
            // trusted-header identity that has not been minted one yet.
            return Err(ApiError::forbidden(format!(
                "this identity carries no CSRF token yet: call GET /auth/me \
                 to obtain one, then echo it in {CSRF_HEADER}"
            )));
        }
        // No account behind the request: there is nothing for another origin
        // to ride. The anonymous viewer never passes require_editor, and a
        // cookie the server has forgotten makes logout a no-op.
        return Ok(());
    };
    // An empty expected token is not a token: it would match an absent header
    // and wave every mutating request through. `create_session` always writes
    // one, so this is unreachable today - but a check that fails open when its
    // own input is missing is the wrong shape to leave lying around, and the
    // read that produces it (`session_user`'s `unwrap_or_default`) is one
    // column rename away from returning it for real.
    if expected.is_empty() {
        return Err(ApiError::forbidden(
            "this session carries no CSRF token: log in again",
        ));
    }
    let got = req
        .headers()
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if constant_time_eq(got.as_bytes(), expected.as_bytes()) {
        return Ok(());
    }
    Err(ApiError::forbidden(format!(
        "this request needs the session's {CSRF_HEADER} header"
    )))
}

/// Compare without an early exit, so the time taken does not narrow down how
/// much of the token an attacker has guessed. Lengths are allowed to leak: both
/// sides are fixed-width hex.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// What `POST /auth/login` takes.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct LoginBody {
    /// The account name, in any casing: the store folds it.
    #[schema(example = "ada")]
    name: String,
    /// The password, checked with argon2id.
    #[schema(example = "correct horse battery staple")]
    password: String,
}

/// What `POST /auth/login` answers with.
///
/// A type rather than an inline `json!`, so the OpenAPI document and the
/// response are one definition. Same for the two below it.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct LoginResponse {
    /// The account that was signed in.
    user: User,
    /// The session's CSRF token, which every later mutating request must echo
    /// in the `x-csrf-token` header (see `CSRF_HEADER`).
    ///
    /// The header is named in plain backticks rather than through an intra-doc
    /// link because utoipa copies this comment into the published document,
    /// where a Rust link target reads as noise. Same wherever else a schema
    /// field's comment names something in this crate.
    #[schema(example = "9f2c1d7e4b6a8035")]
    csrf: String,
}

/// What `POST /auth/logout` answers with: an acknowledgement and nothing else,
/// since a logout that found no session is a success too.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct LogoutResponse {
    /// Always true.
    ok: bool,
}

/// What `GET /auth/me` answers with: everything a client needs before it draws
/// anything.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct MeResponse {
    /// The account behind this request, or null when there is none.
    user: Option<User>,
    /// The CSRF token of the session this request arrived on, or null when it
    /// arrived on no session.
    ///
    /// Reissued here, not only by login, because the session cookie is
    /// `HttpOnly` and the token is not stored anywhere a reload survives: a
    /// browser that refreshes holds a live session whose token it can no longer
    /// produce, and would be unable to log out or to write until it logged in
    /// again. This probe is what a client opens on, so it is where the token
    /// belongs.
    ///
    /// Null only for the anonymous viewer, which has no account and can never
    /// write; a trusted-header identity is given a session here on the first
    /// call and handed that same session's token on every later one, so every
    /// identity that can mutate anything carries a token. See the `check_csrf`
    /// rule.
    ///
    /// Handing the token back on a `GET` is safe for the same reason handing it
    /// back from login is: no CORS layer exists on this surface, so another
    /// origin can send the request but cannot read the answer. **That is load
    /// bearing - a CORS layer must not be added without revisiting this.**
    #[schema(example = "9f2c1d7e4b6a8035")]
    csrf: Option<String>,
    /// Whether the request is being served as the anonymous viewer.
    anonymous: bool,
    /// Whether this instance refuses content mutations.
    read_only: bool,
    /// Whether no account exists yet, so the first-run setup path is open.
    ///
    /// Read fresh on every probe rather than cached: it is one indexed count
    /// over a table with a handful of rows, and a stale `true` would draw a
    /// wizard whose POST then answers 410 - correct, but baffling. That it
    /// tells an unauthenticated caller "this instance has no accounts yet" is
    /// deliberate: the login page needs to know before any identity exists, and
    /// the state itself is what makes `POST /auth/setup` answer at all.
    needs_setup: bool,
    /// Whether this session may drive the share surfaces: the sync status of a
    /// team domain, the share preview, the share, a withdrawal and a conflict.
    ///
    /// Computed here rather than derived by a client from the role, because
    /// the answer is not a property of the role alone: it is the role read
    /// against `github.share_identity`, which is instance configuration a
    /// browser cannot see. An admin may always; an editor may when this
    /// instance shares as the acting person rather than as the machine; a
    /// viewer and the anonymous reader never may.
    ///
    /// It is a rendering signal, never the gate. Every share route still
    /// refuses on its own, off this same predicate, so a stale or forged
    /// `true` costs a 403 rather than access. What it
    /// buys is a client that does not have to ask a domain's origin whether it
    /// is allowed to ask: an editor on an instance-mode install draws no share
    /// card and issues no request behind it.
    can_share: bool,
    /// The server version, so a mismatched UI can say so.
    #[schema(example = "0.12.0")]
    version: &'static str,
}

/// Exchange credentials for a session: sets [`SESSION_COOKIE`] and returns the
/// account plus the CSRF token every later mutating request must echo.
///
/// The body comes in through [`ApiJson`] rather than `axum::Json` so a
/// malformed one is refused in problem+json like every other failure here: this
/// is the first request a client ever sends, and the worst moment to hand it an
/// error shape it has no parser for.
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    operation_id = "login",
    // Spelled out rather than taken from the rustdoc above, which is written
    // for a Rust reader and carries intra-doc links that read as noise in a
    // published document. Same on the five other handlers whose rustdoc links.
    summary = "Exchange credentials for a session.",
    description = "Sets the `fluid_session` cookie and returns the account plus \
                   the CSRF token every later mutating request must echo in \
                   `x-csrf-token`. Any session the request arrived holding is \
                   revoked first, so a planted token cannot survive a login.",
    request_body = LoginBody,
    responses(
        (
            status = 200,
            description = "Signed in. The session cookie is set and the CSRF \
                           token is in the body.",
            body = LoginResponse,
            headers(
                ("set-cookie" = String, description = "The `fluid_session` \
                 session cookie, HttpOnly and SameSite=Lax."),
                ("cache-control" = String, description = "`no-store`: this \
                 answer carries a session cookie and a CSRF token, so no cache \
                 between the server and the browser may keep it."),
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
            description = "The name or password is wrong. One message for every \
                           way this can fail, so nothing is learned about which \
                           accounts exist.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
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
            description = "The body is JSON but not a login.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn login(
    State(state): State<RestState>,
    jar: CookieJar,
    headers: HeaderMap,
    ApiJson(body): ApiJson<LoginBody>,
) -> Result<(CookieJar, NoStore, axum::Json<LoginResponse>), ApiError> {
    let Some(user) =
        authenticate(&state.auth, &state.login_slots, &body.name, &body.password).await?
    else {
        // One message for every way this can fail, so the response says only
        // that the pair was wrong, never which half.
        return Err(ApiError::unauthorized("the name or password is wrong"));
    };
    sign_in(&state, jar, &headers, user).await
}

/// Issue a session for `user` and answer in the login shape: the cookie, the
/// `no-store` header and the account plus its CSRF token.
///
/// Shared by [`login`] and [`setup`] rather than written twice, which is what
/// makes "setup signs the caller in exactly as login does" a fact about the
/// code instead of a claim about it: the fixation rule, the cookie attributes,
/// the TTL and the response type are one definition with two callers.
async fn sign_in(
    state: &RestState,
    jar: CookieJar,
    headers: &HeaderMap,
    user: User,
) -> Result<(CookieJar, NoStore, axum::Json<LoginResponse>), ApiError> {
    // Whatever session the caller arrived holding is retired rather than left
    // live beside the new one. A session fixation attack works by planting a
    // token the victim then logs in under, so the token that was presented is
    // exactly the one that must not survive a successful login.
    if let Some(presented) = jar.get(SESSION_COOKIE) {
        state.auth.delete_session(presented.value()).await?;
    }
    let session = state
        .auth
        .create_session(&user.name, SESSION_TTL_SECS)
        .await?;
    let cookie = Cookie::build((SESSION_COOKIE, session.token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(cookie_needs_secure(headers))
        .max_age(time::Duration::seconds(SESSION_TTL_SECS))
        .build();
    Ok((
        jar.add(cookie),
        no_store(),
        axum::Json(LoginResponse {
            user,
            csrf: session.csrf,
        }),
    ))
}

/// Check a password at a cost that does not depend on which account it names.
///
/// Every login attempt runs exactly one argon2id verification, whatever it
/// finds. [`AuthStore::check_password`] runs one for the two outcomes that have
/// a hash to check ([`PasswordCheck::Verified`] and [`PasswordCheck::Mismatch`])
/// and none for [`PasswordCheck::NoHash`] - an unknown name, a disabled
/// account, an account provisioned without a password - so this pays the
/// missing one itself, against a hash nobody can match.
///
/// The balance is the whole point, and it is easy to get backwards: verifying
/// on top of a real check would make a wrong password cost two verifications
/// and an unknown name one, which is the same oracle with its sign flipped.
/// `one_argon2_verification_per_login_attempt` asserts the cost rather than the
/// shape of the code, so an inversion fails the test instead of reading fine.
///
/// Chosen over a login rate limiter, the other way to close this: a per-name
/// limiter hands an attacker a lockout lever against a known account, needs
/// eviction and a clock, and would still leak the difference within its own
/// window. This is stateless and closes the channel itself rather than
/// rationing access to it.
async fn authenticate(
    auth: &AuthStore,
    slots: &Semaphore,
    name: &str,
    password: &str,
) -> Result<Option<User>, ApiError> {
    with_login_slot(slots, async {
        match auth.check_password(name, password).await? {
            PasswordCheck::Verified(user) => Ok(Some(user)),
            // A verification already ran against the stored hash.
            PasswordCheck::Mismatch => Ok(None),
            // Nothing was hashed, so buy the same amount of time here.
            PasswordCheck::NoHash => {
                dummy_verify(password).await?;
                Ok(None)
            }
        }
    })
    .await?
}

/// Run `work` holding one of the [`LOGIN_SLOTS`] password-checking permits, so
/// concurrent argon2 work queues instead of each reserving its memory at once.
///
/// Every path on this surface that hashes or verifies a password goes through
/// here, not only login: an admin creating accounts or resetting passwords
/// (see `super::users_api`) spends the same 19 MiB per operation on the same
/// blocking pool, so a second, unbounded, source of it would defeat the cap
/// rather than sit beside it. [`RestState::with_login_slot`] is how a handler
/// outside this module reaches it.
pub(super) async fn with_login_slot<F: Future>(
    slots: &Semaphore,
    work: F,
) -> Result<F::Output, ApiError> {
    let _permit = slots.acquire().await.map_err(|_| {
        ApiError::internal("the login limiter is closed, so this instance is shutting down")
    })?;
    Ok(work.await)
}

/// What `POST /auth/setup` takes.
///
/// [`Debug`] is written by hand rather than derived, as on `users_api`'s
/// `CreateBody`: the derived one would print the plaintext password and the
/// one-time token, and this type is one `tracing::debug!` or one `unwrap` on a
/// rejection away from a log file.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[schema(description = "The first admin. `display` is the name as typed; no \
                        email is asked for, since that is a users-screen \
                        concern. `token` is the one-time setup token `serve` \
                        prints for a non-loopback bind, and is not needed when \
                        the request comes from the machine that serves this \
                        instance.")]
pub struct SetupBody {
    /// The login name of the first admin, in any casing: the store folds it,
    /// and the name as typed becomes the display name.
    #[schema(example = "ada")]
    name: String,
    /// The password for the new account. Never stored in the clear; the store
    /// hashes it with argon2id.
    #[schema(example = "correct horse battery staple")]
    password: String,
    /// The one-time setup token, when this instance printed one. In the body
    /// rather than a header on purpose: it is a one-shot secret rather than a
    /// session credential, and a body keeps it out of the header dumps an
    /// access log or a proxy trace collects.
    #[serde(default)]
    #[schema(example = "a1b2c3d4e5f60718293a4b5c6d7e8f90")]
    token: Option<String>,
}

impl std::fmt::Debug for SetupBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupBody")
            .field("name", &self.name)
            .field("password", &"<redacted>")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// The peer address the connection was accepted from, or `None` when the
/// router was served without connection info.
///
/// Read out of the request extensions rather than through axum's
/// [`axum::extract::ConnectInfo`] extractor, which rejects when the extension
/// is absent: a missing peer address is exactly the case that must resolve to
/// "not local" rather than to an error, so it is read as an `Option` and
/// [`is_local_setup_peer`] fails closed on it.
pub struct PeerAddr(Option<std::net::SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for PeerAddr {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<PeerAddr, Infallible> {
        Ok(PeerAddr(
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|info| info.0),
        ))
    }
}

/// Create the first admin and sign them in - the browser half of first-run
/// setup, valid only while this instance has no accounts at all.
///
/// Three rules carry it, each pinned by a named test in
/// `tests/rest_setup_api.rs`:
///
/// 1. **Once any account exists this endpoint is [`gone`], permanently.** The
///    count is checked first, before locality and before the token, so a remote
///    prober learns nothing it could not read off the public `needs_setup`
///    probe. The check is not what makes the slot safe, though: the store's
///    `add_first_admin` is one conditional statement, so a setup racing another
///    setup or racing `crystalline users add` in another process cannot both
///    win, and a lost race is mapped onto the same 410 as an outright refusal.
///    410 rather than 403 because the slot really did exist and is really gone,
///    and rather than 409 because the conflict never resolves and a 409 invites
///    a retry.
/// 2. **Locality is the socket peer address, never a header.** The peer comes
///    from the connection ([`PeerAddr`]); no `Host`, `X-Forwarded-For` or
///    `Forwarded` value can GRANT it, a missing peer is not local, and a
///    forwarded-shaped header may only take locality away: a loopback peer that
///    carries one is a reverse proxy on this host relaying a browser that is
///    somewhere else entirely (see [`is_local_setup_peer`]).
/// 3. **A non-local caller needs the one-time token**, generated once per
///    `serve` process, only for a non-loopback bind, printed once at startup
///    and compared with [`constant_time_eq`]. It is never echoed into a
///    response, a detail or a later log line. An instance that holds no token
///    says so instead of pointing at one that does not exist, and only the
///    refusals that a token could fix carry the `token_required` extension
///    member the wizard keys its token field on.
///
/// CSRF: exempt by path, exactly like [`login`] and for the same reason - the
/// first account is what every session descends from, so there is nothing to
/// echo yet. The trusted-header mode needs no special case: a configured proxy
/// mints an account on its first request, which closes this endpoint anyway.
///
/// **The [`ApiJson`] extractor's `application/json` requirement is the
/// cross-origin defense for this pre-session POST, not a formality.** No CORS
/// layer exists on this surface, so no preflight can ever approve a cross-site
/// JSON body, and a cross-site form can only send
/// `application/x-www-form-urlencoded`, `text/plain` or `multipart/form-data` -
/// all three refused here with a 415. That is the same argument
/// `MeResponse::csrf` records for handing a token back at all, and it breaks
/// the moment a CORS layer is added.
///
/// Read-only instances are the one deliberate carve-out (`refuse_read_only` is
/// not called here): accounts are not content, and a read-only mirror that
/// could never be given a first admin would have its UI locked shut forever.
/// Every content and account mutation stays refused there.
#[utoipa::path(
    post,
    path = "/api/v1/auth/setup",
    tag = "auth",
    operation_id = "setup",
    summary = "Create the first admin account and sign in.",
    description = "Valid only while this instance has no accounts at all, and \
                   only for a caller on the machine that serves it or one \
                   carrying the one-time setup token `serve` prints for a \
                   non-loopback bind. Succeeds exactly once: the account is \
                   created by a single conditional insert, so a concurrent \
                   caller - another browser, or `crystalline users add` in \
                   another process - cannot also win. Success sets the \
                   `fluid_session` cookie and answers in the login shape.",
    request_body = SetupBody,
    responses(
        (
            status = 200,
            description = "The first admin was created and signed in. The \
                           session cookie is set and the CSRF token is in the \
                           body, exactly as `POST /auth/login` answers.",
            body = LoginResponse,
            headers(
                ("set-cookie" = String, description = "The `fluid_session` \
                 session cookie, HttpOnly and SameSite=Lax."),
                ("cache-control" = String, description = "`no-store`: this \
                 answer carries a session cookie and a CSRF token, so no cache \
                 between the server and the browser may keep it."),
            ),
        ),
        (
            status = 400,
            description = "The body is not JSON.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The request did not come from the machine that \
                           serves this instance and carried no setup token, or \
                           the wrong one; one message covers both, so nothing \
                           says whether a token was close. When this instance \
                           holds a token, the problem document carries the \
                           extension member `token_required: true`, which is \
                           what a client keys a token field on - an instance \
                           that has no token to enter refuses without the \
                           member, so no dead-end input is ever offered.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 410,
            description = "An account already exists, so the first-run slot is \
                           permanently gone. Checked before locality and before \
                           the token, and answered for a lost race too. 410 \
                           rather than 403 because a credential cannot change \
                           the answer, and rather than 409 because the conflict \
                           never resolves.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 415,
            description = "The body is not `application/json`. Load bearing \
                           rather than pedantic: with no CORS layer on this \
                           surface, it is what stops another origin from \
                           driving this pre-session POST with a form.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The body is JSON but not a setup, the password is \
                           empty, or the name is not one this store can key on.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn setup(
    State(state): State<RestState>,
    jar: CookieJar,
    headers: HeaderMap,
    peer: PeerAddr,
    ApiJson(body): ApiJson<SetupBody>,
) -> Result<(CookieJar, NoStore, axum::Json<LoginResponse>), ApiError> {
    // First, and before anything that costs a hash or reveals whether a token
    // would have helped.
    if state.auth.user_count().await? > 0 {
        return Err(gone());
    }
    super::check_password(&body.password)?;
    authorize_setup(&state, peer, &headers, body.token.as_deref())?;
    // The login name as typed makes the better display name, matching
    // `users_api::create`: the store folds the login name and keeps this one.
    let display = body.name.trim().to_string();
    // Under the login limiter like every other path that hashes.
    let created = state
        .with_login_slot(
            state
                .auth
                .add_first_admin(&body.name, &display, &body.password),
        )
        .await?
        .map_err(setup_store_error)?;
    if !created {
        // Somebody else - another browser, or `crystalline users add` - claimed
        // the slot between the count above and the insert. The store decided
        // it, and the answer is the one every later caller gets.
        return Err(gone());
    }
    let user = super::users_api::read_back(&state, &body.name).await?;
    sign_in(&state, jar, &headers, user).await
}

/// The forever-after refusal: the setup slot existed and is permanently gone.
fn gone() -> ApiError {
    ApiError {
        status: axum::http::StatusCode::GONE,
        title: "gone",
        detail: "this instance already has an account, so first-run setup is \
                 closed: log in instead"
            .to_string(),
        token_required: None,
    }
}

/// Whether this request may create the first admin: it is local, or it carries
/// the token this process printed.
///
/// The two refusals are deliberately different documents. A caller of an
/// instance that HAS a token can act on the answer, so that one names the token
/// and carries the `token_required` member; a caller of an instance that never
/// generated one (every loopback bind) is told the truth instead, which is that
/// there is no token to find and setup has to run from the machine itself.
/// Neither ever contains the token, and a wrong token is answered exactly like
/// a missing one so the difference is not an oracle.
///
/// A blank configured token is treated as no token at all, the same rule
/// [`check_csrf`] applies to an empty expected CSRF token and for the same
/// reason: it would match the empty string a caller that sends nothing is
/// compared as, and hand first-run setup to the whole network. It is
/// unreachable as things stand - [`RestState::with_setup_token`] drops a blank
/// token on the way in, and the generator produces 32 hex characters - but a
/// check that fails open when its own input is missing is the wrong shape to
/// leave lying around, and the plan's own named follow-up (a `--setup-token`
/// flag for provisioning scripts) is one step away from making an operator's
/// empty string reachable.
fn authorize_setup(
    state: &RestState,
    peer: PeerAddr,
    headers: &HeaderMap,
    presented: Option<&str>,
) -> Result<(), ApiError> {
    if is_local_setup_peer(peer.0, headers) {
        return Ok(());
    }
    let Some(expected) = state.setup_token().filter(|token| !token.trim().is_empty()) else {
        return Err(ApiError::forbidden(
            "first-run setup is open to this instance's own machine only: run \
             it from there, or restart `crystalline serve` on a network \
             address and use the one-time setup token it prints",
        ));
    };
    if constant_time_eq(
        presented.unwrap_or_default().as_bytes(),
        expected.as_bytes(),
    ) {
        return Ok(());
    }
    Err(ApiError::forbidden(
        "this request did not come from the machine that serves this instance, \
         so first-run setup needs the one-time setup token `crystalline serve` \
         printed at startup",
    )
    .token_required())
}

/// Whether the connection this request arrived on is a browser on the machine
/// that serves this instance.
///
/// The peer address is the authority and the only thing that can say yes: a
/// loopback peer is a process on this host, and nothing a client sends can
/// forge one. A request with no peer address at all - a router served without
/// connection info - is not local, because failing closed is the only safe
/// reading of "we do not know who this is".
///
/// Headers are then allowed to say no, and only no. A loopback peer carrying
/// `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host` or `Forwarded` is
/// a reverse proxy on this host relaying a browser that may be anywhere, which
/// is the standard container and same-host proxy shape; treating it as local
/// would hand first-run setup to the internet. Using headers this way is not a
/// hole: an attacker gains nothing by adding one, since every one of them can
/// only narrow what the peer address already allowed.
///
/// An IPv4-mapped IPv6 peer (`::ffff:127.0.0.1`, what a dual-stack listener
/// reports for an IPv4 connection) is canonicalized first, so the same machine
/// reads as the same machine whichever stack it arrived on.
fn is_local_setup_peer(peer: Option<std::net::SocketAddr>, headers: &HeaderMap) -> bool {
    let Some(peer) = peer else {
        return false;
    };
    if !peer.ip().to_canonical().is_loopback() {
        return false;
    }
    !FORWARDING_HEADERS
        .iter()
        .any(|name| headers.contains_key(*name))
}

/// The headers that say a proxy relayed this request. Any one of them makes a
/// request non-local however loopback its peer address is.
const FORWARDING_HEADERS: [&str; 4] = [
    "x-forwarded-for",
    "x-forwarded-proto",
    "x-forwarded-host",
    "forwarded",
];

/// Classify a store failure from the first-admin insert. Only the name rules
/// are the caller's to fix; everything else is this server's problem.
fn setup_store_error(e: anyhow::Error) -> ApiError {
    let detail = format!("{e:#}");
    if detail.contains("a user name cannot be empty")
        || detail.contains("cannot contain whitespace")
    {
        return ApiError::unprocessable(detail);
    }
    ApiError::internal(detail)
}

/// Revoke the session and clear its cookie. Not an error without one: a browser
/// logging out twice, or with a cookie this server has already forgotten, is
/// ordinary. Guarded by the CSRF check like any other mutating request, so
/// another origin cannot log a user out.
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    operation_id = "logout",
    responses(
        (
            status = 200,
            description = "Signed out, whether or not there was a session to \
                           revoke.",
            body = LogoutResponse,
            headers(
                ("set-cookie" = String, description = "Clears the \
                 `fluid_session` cookie."),
                ("cache-control" = String, description = "`no-store`: this \
                 answer clears the session cookie, so no cache between the \
                 server and the browser may keep it."),
            ),
        ),
        (
            status = 403,
            description = "The identity did not echo its CSRF token, or carries \
                           none yet and must call `/auth/me` first, or the \
                           trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn logout(
    State(state): State<RestState>,
    jar: CookieJar,
) -> Result<(CookieJar, NoStore, axum::Json<LogoutResponse>), ApiError> {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        state.auth.delete_session(cookie.value()).await?;
    }
    // The removal has to carry the same path the cookie was set with, or the
    // browser keeps the original and only shadows it.
    let removal = Cookie::build(SESSION_COOKIE).path("/").build();
    Ok((
        jar.remove(removal),
        no_store(),
        axum::Json(LogoutResponse { ok: true }),
    ))
}

/// The capability probe a client calls before anything else: who it is, whether
/// it is being served anonymously, whether this instance refuses content
/// mutations, whether it has any accounts at all, whether this session may
/// drive the share surfaces, and which server version it is talking to, so a
/// mismatched UI can say so instead of failing later.
///
/// Answers without an identity on purpose. `user: null, anonymous: false` is
/// what tells a browser to show a login form; `anonymous: true` tells it to
/// browse instead; `needs_setup: true` tells it to draw the first-run wizard
/// over the login form instead (see [`setup`]).
///
/// It also issues and reissues the session's CSRF token, which is the only way
/// a reloaded browser gets it back and the only way a trusted-header identity
/// ever gets one: see `MeResponse::csrf`.
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "auth",
    operation_id = "get_me",
    description = "Who the caller is, whether it is being served anonymously, \
                   whether this instance refuses content mutations, whether it \
                   has no accounts yet and so still needs its first admin \
                   (`needs_setup`, which is what opens `POST /auth/setup`), \
                   whether this session may drive the share surfaces \
                   (`can_share`: an admin always, an editor when \
                   `github.share_identity` is `personal`), and \
                   which server version it is talking to. Also issues the CSRF token \
                   every later mutating request must echo in `x-csrf-token`: a \
                   cookie session has its token reissued here, and a \
                   trusted-header identity is minted a session on the first \
                   call, which is the only way that mode obtains a token.",
    responses(
        (
            status = 200,
            description = "Who the caller is and what this instance allows. \
                           Answered without an identity too, which is how a \
                           client learns it has to log in.",
            body = MeResponse,
            headers(
                ("set-cookie" = String, description = "The `fluid_session` \
                 session cookie, HttpOnly and SameSite=Lax. Set only when this \
                 call issues a session, which is the first call from a \
                 trusted-header identity whose account holds none; a later \
                 probe reuses that session and sets no cookie."),
                ("cache-control" = String, description = "`no-store`. Always \
                 set: this answer names the caller and carries their CSRF \
                 token, and a shared cache is allowed to store a GET 200 \
                 heuristically, which behind an SSO proxy would hand the next \
                 user the previous one's identity."),
            ),
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account. \
                           The guard resolves identity ahead of routing, so this \
                           answer reaches even the paths that are served without \
                           one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn me(
    State(state): State<RestState>,
    jar: CookieJar,
    headers: HeaderMap,
    identity: Identity,
) -> Result<(CookieJar, NoStore, axum::Json<MeResponse>), ApiError> {
    let mut jar = jar;
    let mut csrf = identity.csrf.clone();
    // A cookie that is not this identity's own session is retired here, the
    // same fixation rule login applies: a token planted on the victim must not
    // survive the call that hands them a working one. Judged on who the cookie
    // belongs to rather than on whether a token was resolved, because since the
    // trusted-header path reads its token by identity, an account holding any
    // live session arrives with `csrf` already set - and a foreign cookie
    // presented beside the header would otherwise be left live.
    if let Some(user) = &identity.user
        && let Some(presented) = jar.get(SESSION_COOKIE).map(|c| c.value().to_string())
    {
        let owner = state.auth.session_owner(&presented).await?;
        if owner.as_deref() != Some(user.name.as_str()) {
            state.auth.delete_session(&presented).await?;
        }
    }
    // A trusted-header identity arrives with an account and no session (a
    // cookie session always carries its token). Ensure one here: this probe is
    // what a client opens on, so it is where the token belongs - for the
    // trusted-header mode exactly as for a reloaded cookie session.
    if csrf.is_none()
        && let Some(user) = &identity.user
    {
        // Reuse before minting. A probe that issued a session every time would
        // add a row per call for a client that keeps no cookie, and would hand
        // two tabs opening at once two different tokens; `ensure_session` does
        // the check and the insert in one transaction, so the second probe of a
        // race sees the first's session instead of racing it. A reused session
        // sets no cookie because there is no unhashed token left to put in one
        // - `resolve` reads this mode's token by identity for that reason.
        let mint = state
            .auth
            .ensure_session(&user.name, SESSION_TTL_SECS)
            .await?;
        csrf = Some(mint.csrf().to_string());
        if let SessionMint::Created(session) = mint {
            let cookie = Cookie::build((SESSION_COOKIE, session.token))
                .path("/")
                .http_only(true)
                .same_site(SameSite::Lax)
                .secure(cookie_needs_secure(&headers))
                .max_age(time::Duration::seconds(SESSION_TTL_SECS))
                .build();
            jar = jar.add(cookie);
        }
    }
    // Read before the identity is taken apart below, and read LIVE rather than
    // captured at start: `share_identity` is a setting an admin can flip
    // through `configure`, and the next probe is what tells an editor's browser
    // the surfaces just opened to them.
    let can_share = identity
        .may_share(state.engine.share_identity_mode())
        .is_ok();
    Ok((
        jar,
        no_store(),
        axum::Json(MeResponse {
            user: identity.user,
            csrf,
            anonymous: identity.anonymous,
            read_only: state.engine.read_only(),
            needs_setup: state.auth.user_count().await? == 0,
            // The share routes' own predicate, asked rather than re-derived:
            // one rule decides what they serve and what a client draws.
            // Resolved off `identity` before it is moved into `user` above.
            can_share,
            version: crystalline_core::VERSION,
        }),
    ))
}

/// Whether the session cookie is marked `Secure`, which is to say: is there any
/// sign this request did not come straight from a browser on this machine?
///
/// The flag is dropped only when both signals agree that it is local. `Host`
/// rather than the peer address, because behind a reverse proxy the peer is the
/// proxy - usually loopback itself - while the browser is remote and on TLS,
/// and that is the one case where a missing `Secure` matters; `Host` is what
/// the browser asked for, so it stays right on both sides of a proxy. But
/// `Host` alone is not enough either: nginx's default when `proxy_set_header
/// Host` is left unconfigured is `Host $proxy_host`, literally the upstream's
/// `127.0.0.1:port`, so a plain `proxy_pass` in front of TLS would look local
/// and hand out a cookie that a downgrade could then read. A forwarded-protocol
/// header saying `https` therefore overrides the `Host` reading outright.
///
/// A request with no `Host` is treated as remote: HTTP/1.1 requires the header,
/// so its absence is not a local browser.
fn cookie_needs_secure(headers: &HeaderMap) -> bool {
    forwarded_https(headers) || !is_loopback_request(headers)
}

/// Whether a proxy in front of this instance says it terminated TLS, by either
/// the de-facto `X-Forwarded-Proto` or RFC 7239's `Forwarded: proto=`.
///
/// Any `proto=https` anywhere in the chain counts. The error that matters here
/// is dropping `Secure` from a cookie that travels over the internet, so an
/// ambiguous header resolves towards setting the flag.
fn forwarded_https(headers: &HeaderMap) -> bool {
    let x_forwarded = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        // A chain of proxies appends, so the first element is the one that
        // spoke to the client.
        .and_then(|v| v.split(',').next())
        .is_some_and(|proto| proto.trim().eq_ignore_ascii_case("https"));
    x_forwarded
        || headers
            .get(header::FORWARDED)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|raw| {
                raw.split([',', ';']).any(|part| {
                    part.split_once('=').is_some_and(|(key, value)| {
                        key.trim().eq_ignore_ascii_case("proto")
                            && value.trim().trim_matches('"').eq_ignore_ascii_case("https")
                    })
                })
            })
}

/// Whether the `Host` the client asked for names this machine.
fn is_loopback_request(headers: &HeaderMap) -> bool {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(host_is_loopback)
}

/// `localhost` or a loopback IP literal, with or without a port.
fn host_is_loopback(host: &str) -> bool {
    let host = host.trim();
    let bare = if let Some(rest) = host.strip_prefix('[') {
        // A bracketed IPv6 literal, `[::1]:8080`.
        rest.split(']').next().unwrap_or_default()
    } else if host.matches(':').count() > 1 {
        // An unbracketed IPv6 literal: no port can be attached to one.
        host
    } else {
        host.split(':').next().unwrap_or_default()
    };
    bare.eq_ignore_ascii_case("localhost")
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The `Cache-Control` header the auth endpoints answer with, as a type a
/// handler can name in its return position.
pub type NoStore = [(HeaderName, &'static str); 1];

/// `Cache-Control: no-store`, for every response that carries a CSRF token or a
/// `Set-Cookie`.
///
/// Not decoration. `GET /auth/me` is a 200 answer to a GET with no explicit
/// freshness information, which is exactly the shape a shared cache is allowed
/// to store and reuse on a heuristic; and the trusted-header mode puts a reverse
/// proxy in front of this surface by definition, which is the one place such a
/// cache is likely to be. A cached probe would hand the next user through that
/// proxy the previous one's identity, CSRF token and session cookie. Login and
/// logout carry the same material and are marked the same way: a POST response
/// is only cacheable with explicit freshness information, so it is defence in
/// depth there rather than a hole being closed, but a reader should not have to
/// work out which of the three was safe.
fn no_store() -> NoStore {
    [(header::CACHE_CONTROL, "no-store")]
}

/// The shared login limiter a [`RestState`] is built with.
pub(super) fn login_slots() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(LOGIN_SLOTS))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::auth_store;
    use super::*;

    async fn store() -> (tempfile::TempDir, AuthStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        (dir, store)
    }

    fn header_map(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(HeaderName::try_from(*name).unwrap(), value.parse().unwrap());
        }
        headers
    }

    /// The enumeration closure, asserted as the property that matters: a login
    /// attempt costs exactly one argon2 verification wherever it lands.
    ///
    /// Counting only the *dummy* verifications would pass just as happily on a
    /// version that runs the dummy on top of a real check, which costs an
    /// existing account two and an unknown one - the same oracle, inverted.
    /// Counting every verification is what pins the balance.
    ///
    /// The counting runs inside a task-local scope, so the verifications a
    /// sibling test is running in the same process at the same moment (plain
    /// `cargo test` runs them as threads) never land in this test's total.
    #[tokio::test]
    async fn one_argon2_verification_per_login_attempt() {
        auth_store::VERIFICATIONS
            .scope(std::cell::Cell::new(0), async {
                let (_dir, store) = store().await;
                store
                    .add_user("ada", "Ada", None, Role::Editor, "s3cret")
                    .await
                    .unwrap();
                // Provisioned by a trusted header, so it has no password at all.
                store
                    .ensure_user("bob", Role::Viewer, usize::MAX)
                    .await
                    .unwrap();
                store
                    .add_user("cyd", "Cyd", None, Role::Editor, "pw")
                    .await
                    .unwrap();
                store.set_disabled("cyd", true).await.unwrap();
                let slots = Semaphore::new(LOGIN_SLOTS);

                // An unknown name, a passwordless account, a disabled account, a
                // name that will not normalize, a wrong password, and the right
                // one.
                for (name, password, expected) in [
                    ("ghost", "s3cret", None),
                    ("bob", "", None),
                    ("cyd", "pw", None),
                    ("   ", "s3cret", None),
                    ("ada", "wrong", None),
                    ("AdA", "s3cret", Some("ada")),
                ] {
                    let before = auth_store::VERIFICATIONS.with(|count| count.get());
                    let got = authenticate(&store, &slots, name, password).await.unwrap();
                    assert_eq!(
                        got.map(|u| u.name).as_deref(),
                        expected,
                        "wrong outcome for {name:?}"
                    );
                    assert_eq!(
                        auth_store::VERIFICATIONS.with(|count| count.get()) - before,
                        1,
                        "logging in as {name:?} must cost exactly one verification, \
                         or how long it takes says which kind of miss it was"
                    );
                }
            })
            .await;
    }

    /// The memory cap: however many logins arrive at once, only [`LOGIN_SLOTS`]
    /// of them hold argon2's working memory at a time.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn login_slots_cap_concurrent_password_work() {
        let slots = Arc::new(Semaphore::new(LOGIN_SLOTS));
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for _ in 0..32 {
            let (slots, live, peak) = (slots.clone(), live.clone(), peak.clone());
            tasks.push(tokio::spawn(async move {
                with_login_slot(&slots, async {
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    live.fetch_sub(1, Ordering::SeqCst);
                })
                .await
                .unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(live.load(Ordering::SeqCst), 0, "every permit came back");
        let peak = peak.load(Ordering::SeqCst);
        assert!(
            peak <= LOGIN_SLOTS,
            "at most {LOGIN_SLOTS} verifications may run at once, saw {peak}"
        );
        assert!(peak > 1, "the limiter must not serialize every login");
    }

    #[test]
    fn a_trusted_header_name_is_validated_at_startup() {
        let mut config = GlobalConfig::default();
        assert!(AuthCfg::resolve(&config).unwrap().trusted_header.is_none());
        assert!(!AuthCfg::resolve(&config).unwrap().anonymous);
        assert_eq!(
            AuthCfg::resolve(&config).unwrap().max_users,
            crystalline_core::config::DEFAULT_MAX_USERS,
            "an absent auth.max_users resolves to the default cap"
        );

        config.auth = Some(crystalline_core::config::AuthConfig {
            trusted_header: Some("Remote-User".to_string()),
            anonymous: Some(true),
            max_users: Some(5),
        });
        let cfg = AuthCfg::resolve(&config).unwrap();
        assert_eq!(cfg.trusted_header.unwrap().as_str(), "remote-user");
        assert!(cfg.anonymous);
        assert_eq!(cfg.max_users, 5);

        config.auth = Some(crystalline_core::config::AuthConfig {
            trusted_header: Some("not a header".to_string()),
            anonymous: None,
            max_users: None,
        });
        let err = AuthCfg::resolve(&config).unwrap_err().to_string();
        assert!(
            err.contains("auth.trusted_header"),
            "the startup error must name the setting, got: {err}"
        );
    }

    #[test]
    fn only_loopback_hosts_leave_the_cookie_usable_over_plain_http() {
        for host in [
            "localhost",
            "localhost:8765",
            "127.0.0.1:8765",
            "[::1]:80",
            "::1",
            "127.0.0.2",
        ] {
            assert!(host_is_loopback(host), "{host} is loopback");
        }
        for host in [
            "example.com",
            "example.com:8765",
            "10.0.0.4:8765",
            "[2001:db8::1]:80",
            "",
        ] {
            assert!(!host_is_loopback(host), "{host} is not loopback");
        }
        assert!(is_loopback_request(&header_map(&[("host", "localhost:1")])));
        assert!(!is_loopback_request(&header_map(&[(
            "host",
            "example.com"
        )])));
        assert!(
            !is_loopback_request(&HeaderMap::new()),
            "a request with no Host is treated as remote"
        );
    }

    /// The `Secure` decision, including the case that makes `Host` alone
    /// insufficient: nginx's default for `proxy_pass` is `Host $proxy_host`,
    /// which rewrites the header to the upstream's own `127.0.0.1:port`, so a
    /// TLS deployment behind a stock config looks local. A forwarded-protocol
    /// header overrides that reading; only both signals together drop the flag.
    #[test]
    fn a_forwarded_https_hop_marks_the_cookie_secure_whatever_the_host_says() {
        let secure = |pairs: &[(&str, &str)]| cookie_needs_secure(&header_map(pairs));

        // Genuinely local: the one shape that may drop the flag.
        assert!(!secure(&[("host", "localhost:8765")]));
        assert!(!secure(&[("host", "127.0.0.1:8765")]));
        assert!(!secure(&[
            ("host", "127.0.0.1:8765"),
            ("x-forwarded-proto", "http")
        ]));
        assert!(!secure(&[
            ("host", "localhost"),
            ("forwarded", "for=192.0.2.1;proto=http")
        ]));

        // The stock-nginx shape: Host says loopback, the proxy says TLS.
        assert!(secure(&[
            ("host", "127.0.0.1:8765"),
            ("x-forwarded-proto", "https")
        ]));
        assert!(secure(&[
            ("host", "127.0.0.1:8765"),
            ("x-forwarded-proto", "HTTPS")
        ]));
        // A chain of proxies appends, so the client-facing hop comes first.
        assert!(secure(&[
            ("host", "localhost"),
            ("x-forwarded-proto", "https, http")
        ]));
        // RFC 7239 spelling, with and without the optional quotes.
        assert!(secure(&[
            ("host", "localhost"),
            ("forwarded", "for=192.0.2.1;proto=https;by=203.0.113.4")
        ]));
        assert!(secure(&[
            ("host", "localhost"),
            ("forwarded", "proto=\"https\"")
        ]));
        assert!(!secure(&[
            ("host", "localhost"),
            ("forwarded", "for=192.0.2.1")
        ]));

        // Not local at all: the flag is set with or without a proxy header.
        assert!(secure(&[("host", "example.com")]));
        assert!(secure(&[]), "no Host at all is treated as remote");
    }

    #[test]
    fn the_guards_separate_missing_identity_from_insufficient_role() {
        let account = |role| Identity {
            user: Some(User {
                name: "ada".to_string(),
                display: "Ada".to_string(),
                email: None,
                role,
                disabled: false,
                last_seen: None,
            }),
            csrf: None,
            anonymous: false,
        };

        let admin = account(Role::Admin);
        assert_eq!(admin.require_viewer().unwrap().name(), "ada");
        assert_eq!(admin.require_admin().unwrap().role(), Role::Admin);

        let viewer = account(Role::Viewer);
        assert!(viewer.require_viewer().is_ok());
        assert_eq!(
            viewer.require_admin().unwrap_err().status,
            axum::http::StatusCode::FORBIDDEN,
            "an authenticated account that is not privileged enough is forbidden"
        );

        let anonymous = Identity {
            anonymous: true,
            ..Identity::default()
        };
        assert!(matches!(
            anonymous.require_viewer().unwrap(),
            Caller::Anonymous
        ));
        assert_eq!(anonymous.require_viewer().unwrap().role(), Role::Viewer);
        assert_eq!(
            anonymous.require_admin().unwrap_err().status,
            axum::http::StatusCode::UNAUTHORIZED,
            "the anonymous viewer is told to log in, not that it is forbidden"
        );

        let nobody = Identity::default();
        assert_eq!(
            nobody.require_viewer().unwrap_err().status,
            axum::http::StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            nobody.require_admin().unwrap_err().status,
            axum::http::StatusCode::UNAUTHORIZED
        );
    }

    /// Invariant 2, at the unit the integration suite cannot reach: a loopback
    /// test socket can never present a genuinely remote peer, so the remote and
    /// the missing-peer legs live here.
    #[test]
    fn locality_is_the_peer_and_forwarding_headers_only_take_it_away() {
        let peer = |raw: &str| Some(raw.parse::<std::net::SocketAddr>().unwrap());
        let local = |p, pairs: &[(&str, &str)]| is_local_setup_peer(p, &header_map(pairs));

        // The peer address is the only thing that can say yes.
        assert!(local(peer("127.0.0.1:51234"), &[]));
        assert!(local(peer("[::1]:51234"), &[]));
        assert!(
            local(peer("[::ffff:127.0.0.1]:51234"), &[]),
            "a dual-stack listener reports an IPv4 connection this way"
        );
        assert!(!local(peer("192.168.1.5:51234"), &[]));
        assert!(!local(peer("[2001:db8::1]:51234"), &[]));
        assert!(
            !local(None, &[]),
            "no peer address at all is not local: this fails closed"
        );

        // No header can grant it.
        assert!(!local(
            peer("203.0.113.9:443"),
            &[("host", "localhost"), ("x-forwarded-for", "127.0.0.1")]
        ));

        // And every forwarding header takes it away from a loopback peer: that
        // is a proxy on this host relaying a browser that is somewhere else.
        for (name, value) in [
            ("x-forwarded-for", "203.0.113.9"),
            ("x-forwarded-proto", "https"),
            ("x-forwarded-host", "crystalline.example"),
            ("forwarded", "for=203.0.113.9"),
        ] {
            assert!(
                !local(peer("127.0.0.1:51234"), &[(name, value)]),
                "{name} means this request was relayed"
            );
        }
        // An unrelated header changes nothing.
        assert!(local(
            peer("127.0.0.1:51234"),
            &[("host", "localhost:7411")]
        ));
    }

    #[test]
    fn constant_time_eq_matches_ordinary_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"a"));
    }

    /// The settled CSRF rule, one for every identity mode: safe methods and
    /// login pass; an unsafe request from ANY identity with an account behind it
    /// must echo a valid token - a session that has one, and a trusted-header
    /// identity that has not yet been minted one (csrf: None) is refused
    /// outright, told to call /auth/me. Only the identities with no account
    /// (nobody, the anonymous viewer) pass through, because they cannot reach a
    /// write at all: require_editor refuses them before any handler runs.
    #[test]
    fn unsafe_methods_require_the_token_of_any_account_bearing_identity() {
        let request = |method: &str, path: &str, csrf: Option<&str>| {
            let mut builder = Request::builder().method(method).uri(path);
            if let Some(csrf) = csrf {
                builder = builder.header(CSRF_HEADER, csrf);
            }
            builder.body(axum::body::Body::empty()).unwrap()
        };
        let account = |csrf: Option<&str>| Identity {
            user: Some(User {
                name: "ada".to_string(),
                display: "Ada".to_string(),
                email: None,
                role: Role::Admin,
                disabled: false,
                last_seen: None,
            }),
            csrf: csrf.map(str::to_string),
            anonymous: false,
        };

        // A session with a token: the double-submit check as before.
        let session = account(Some("tok"));
        assert!(check_csrf(&session, &request("GET", "/domains", None)).is_ok());
        assert!(check_csrf(&session, &request("HEAD", "/domains", None)).is_ok());
        assert!(check_csrf(&session, &request("POST", LOGIN_PATH, None)).is_ok());
        assert!(check_csrf(&session, &request("POST", "/auth/logout", Some("tok"))).is_ok());
        for csrf in [None, Some(""), Some("wrong"), Some("tok ")] {
            assert!(check_csrf(&session, &request("POST", "/auth/logout", csrf)).is_err());
        }
        // Every unsafe method, not only the one the loop above spells out.
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            assert!(check_csrf(&session, &request(method, "/auth/logout", None)).is_err());
        }

        // The settlement: an account WITHOUT a token (a trusted-header identity
        // that has not called /auth/me yet) is refused on unsafe methods rather
        // than waved through on request-shape arguments.
        let tokenless_account = account(None);
        assert!(check_csrf(&tokenless_account, &request("GET", "/domains", None)).is_ok());
        let err = check_csrf(&tokenless_account, &request("POST", "/users", None)).unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
        assert!(
            err.detail.contains("/auth/me"),
            "told where the token comes from: {}",
            err.detail
        );

        // No account: nothing to ride. Logout with a forgotten cookie stays a
        // no-op success, and every data route 401s at the role guard anyway.
        let nobody = Identity::default();
        assert!(check_csrf(&nobody, &request("POST", "/auth/logout", None)).is_ok());
        let anonymous = Identity {
            anonymous: true,
            ..Identity::default()
        };
        assert!(check_csrf(&anonymous, &request("POST", "/auth/logout", None)).is_ok());

        // An empty stored token still fails closed.
        let empty = account(Some(""));
        assert!(check_csrf(&empty, &request("POST", "/auth/logout", Some(""))).is_err());
        // A safe method is still safe, and login is still exempt.
        assert!(check_csrf(&empty, &request("GET", "/domains", None)).is_ok());
        assert!(check_csrf(&empty, &request("POST", LOGIN_PATH, None)).is_ok());

        // Setup joins login's exemption by path, for every identity: it runs
        // before the first account exists, so no token can have been issued.
        // The trusted-header identity that arrives with an account and no token
        // is the case the exemption is spelled out for - it is told nothing
        // about CSRF here, and the handler answers it 410 anyway.
        for identity in [&session, &tokenless_account, &nobody, &anonymous, &empty] {
            assert!(
                check_csrf(identity, &request("POST", SETUP_PATH, None)).is_ok(),
                "setup is pre-session, like login"
            );
        }
        // And nothing else moved: the same identity still owes a token
        // everywhere else.
        assert!(check_csrf(&session, &request("POST", "/users", None)).is_err());
    }

    #[test]
    fn require_editor_separates_roles_the_same_way() {
        let account = |role| Identity {
            user: Some(User {
                name: "ada".to_string(),
                display: "Ada".to_string(),
                email: None,
                role,
                disabled: false,
                last_seen: None,
            }),
            csrf: None,
            anonymous: false,
        };
        assert!(account(Role::Editor).require_editor().is_ok());
        assert!(account(Role::Admin).require_editor().is_ok());
        assert_eq!(
            account(Role::Viewer).require_editor().unwrap_err().status,
            axum::http::StatusCode::FORBIDDEN
        );
        let anonymous = Identity {
            anonymous: true,
            ..Identity::default()
        };
        assert_eq!(
            anonymous.require_editor().unwrap_err().status,
            axum::http::StatusCode::UNAUTHORIZED,
            "the anonymous viewer is told to log in: anonymous identities never write"
        );
    }
}
