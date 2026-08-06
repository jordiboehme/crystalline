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

use std::future::Future;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, header};
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use crystalline_core::config::GlobalConfig;
use tokio::sync::Semaphore;

use super::auth_store::{AuthStore, PasswordCheck, Role, User, dummy_verify};
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

/// Paths served without an identity. The rest of the API is closed by default.
/// `/auth/me` is the capability probe a client calls before it knows whether it
/// is logged in, and `/auth/logout` must work with a cookie the server has
/// already forgotten, which is exactly the case a browser retries.
const PUBLIC_PATHS: [&str; 3] = [LOGIN_PATH, "/auth/logout", "/auth/me"];

/// The two auth settings, resolved once when the HTTP surface is built.
///
/// Startup-effective by design, like `service.read_only`: the trusted header is
/// parsed into a [`HeaderName`] here so a typo is a clear startup error rather
/// than a header that silently never matches, and so no request pays for the
/// parse.
#[derive(Clone, Debug, Default)]
pub struct AuthCfg {
    /// The header a trusted proxy names the authenticated user in, from
    /// `auth.trusted_header`. `None` means the path is off.
    pub trusted_header: Option<HeaderName>,
    /// Whether a request carrying no identity is served anyway, from
    /// `auth.anonymous`.
    pub anonymous: bool,
}

impl AuthCfg {
    /// Read both settings out of `config`, validating the header name.
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
    /// trusted-header and anonymous paths, which carry no session.
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

    /// The caller, when the request may be served at admin level. 403 when an
    /// account is authenticated but not privileged enough, 401 when there is no
    /// account to judge: the anonymous viewer can log in to become an admin, so
    /// it is told to authenticate rather than that it is forbidden.
    pub fn require_admin(&self) -> Result<Caller, ApiError> {
        self.require(Role::Admin)
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
        let user = state.auth.ensure_user(value, Role::Viewer).await?;
        if user.disabled {
            return Err(ApiError::forbidden("this account is disabled"));
        }
        return Ok(Identity {
            user: Some(user),
            csrf: None,
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
/// Only a session can be ridden by another origin, because only a cookie is
/// attached by the browser on its own: the trusted-header and anonymous paths
/// have no token to compare against, and for the trusted header the proxy that
/// injects the identity owns that boundary. Login is exempt because it is what
/// mints the token.
///
/// Passing the tokenless identities through is a deliberate invariant rather
/// than an accident, and the mutating routes lean on it:
///
/// - The anonymous viewer is a viewer and nothing else, so it never reaches a
///   route that changes anything - `require_admin` answers it 401 first.
/// - A trusted-header caller can be an admin, and its request carries a header
///   a browser attaches on its own for any origin. What keeps a cross-site
///   request off those routes is the shape it is allowed to have instead:
///   `PATCH` and `DELETE` are not simple methods, so another origin cannot send
///   them without a CORS preflight, and the `POST` it can send is refused by
///   [`ApiJson`], which demands `application/json` while a cross-site form can
///   only send `application/x-www-form-urlencoded`, `text/plain` or
///   `multipart/form-data`. **No CORS layer exists on this surface and one must
///   not be added without revisiting this check**: allowing a cross-origin
///   preflight would remove the only thing standing between another origin and
///   a trusted-header admin's account.
fn check_csrf(identity: &Identity, req: &Request) -> Result<(), ApiError> {
    if req.method().is_safe() || req.uri().path() == LOGIN_PATH {
        return Ok(());
    }
    let Some(expected) = identity.csrf.as_deref() else {
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
    /// Whether the request is being served as the anonymous viewer.
    anonymous: bool,
    /// Whether this instance refuses content mutations.
    read_only: bool,
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
            headers(("set-cookie" = String, description = "The `fluid_session` \
                     session cookie, HttpOnly and SameSite=Lax.")),
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
) -> Result<(CookieJar, axum::Json<LoginResponse>), ApiError> {
    let Some(user) =
        authenticate(&state.auth, &state.login_slots, &body.name, &body.password).await?
    else {
        // One message for every way this can fail, so the response says only
        // that the pair was wrong, never which half.
        return Err(ApiError::unauthorized("the name or password is wrong"));
    };
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
        .secure(cookie_needs_secure(&headers))
        .max_age(time::Duration::seconds(SESSION_TTL_SECS))
        .build();
    Ok((
        jar.add(cookie),
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
            headers(("set-cookie" = String, description = "Clears the \
                     `fluid_session` cookie.")),
        ),
        (
            status = 403,
            description = "A cookie session did not echo its CSRF token.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn logout(
    State(state): State<RestState>,
    jar: CookieJar,
) -> Result<(CookieJar, axum::Json<LogoutResponse>), ApiError> {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        state.auth.delete_session(cookie.value()).await?;
    }
    // The removal has to carry the same path the cookie was set with, or the
    // browser keeps the original and only shadows it.
    let removal = Cookie::build(SESSION_COOKIE).path("/").build();
    Ok((jar.remove(removal), axum::Json(LogoutResponse { ok: true })))
}

/// The capability probe a client calls before anything else: who it is, whether
/// it is being served anonymously, whether this instance refuses content
/// mutations, and which server version it is talking to, so a mismatched UI can
/// say so instead of failing later.
///
/// Answers without an identity on purpose. `user: null, anonymous: false` is
/// what tells a browser to show a login form; `anonymous: true` tells it to
/// browse instead.
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "auth",
    operation_id = "get_me",
    responses((
        status = 200,
        description = "Who the caller is and what this instance allows. Answered \
                       without an identity too, which is how a client learns it \
                       has to log in.",
        body = MeResponse,
    )),
)]
pub async fn me(State(state): State<RestState>, identity: Identity) -> axum::Json<MeResponse> {
    axum::Json(MeResponse {
        user: identity.user,
        anonymous: identity.anonymous,
        read_only: state.engine.read_only(),
        version: crystalline_core::VERSION,
    })
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
    #[tokio::test]
    async fn one_argon2_verification_per_login_attempt() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "s3cret")
            .await
            .unwrap();
        // Provisioned by a trusted header, so it has no password at all.
        store.ensure_user("bob", Role::Viewer).await.unwrap();
        store
            .add_user("cyd", "Cyd", None, Role::Editor, "pw")
            .await
            .unwrap();
        store.set_disabled("cyd", true).await.unwrap();
        let slots = Semaphore::new(LOGIN_SLOTS);

        // An unknown name, a passwordless account, a disabled account, a name
        // that will not normalize, a wrong password, and the right one.
        for (name, password, expected) in [
            ("ghost", "s3cret", None),
            ("bob", "", None),
            ("cyd", "pw", None),
            ("   ", "s3cret", None),
            ("ada", "wrong", None),
            ("AdA", "s3cret", Some("ada")),
        ] {
            let before = auth_store::VERIFICATIONS.load(Ordering::Relaxed);
            let got = authenticate(&store, &slots, name, password).await.unwrap();
            assert_eq!(
                got.map(|u| u.name).as_deref(),
                expected,
                "wrong outcome for {name:?}"
            );
            assert_eq!(
                auth_store::VERIFICATIONS.load(Ordering::Relaxed) - before,
                1,
                "logging in as {name:?} must cost exactly one verification, \
                 or how long it takes says which kind of miss it was"
            );
        }
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

        config.auth = Some(crystalline_core::config::AuthConfig {
            trusted_header: Some("Remote-User".to_string()),
            anonymous: Some(true),
        });
        let cfg = AuthCfg::resolve(&config).unwrap();
        assert_eq!(cfg.trusted_header.unwrap().as_str(), "remote-user");
        assert!(cfg.anonymous);

        config.auth = Some(crystalline_core::config::AuthConfig {
            trusted_header: Some("not a header".to_string()),
            anonymous: None,
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

    #[test]
    fn constant_time_eq_matches_ordinary_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"a"));
    }

    /// The CSRF rule, without a server: safe methods and login pass, a session
    /// must echo its token, and an identity that has no session has nothing to
    /// echo.
    #[test]
    fn csrf_is_required_of_sessions_on_unsafe_methods_only() {
        let session = Identity {
            csrf: Some("tok".to_string()),
            ..Identity::default()
        };
        let request = |method: &str, path: &str, csrf: Option<&str>| {
            let mut builder = Request::builder().method(method).uri(path);
            if let Some(csrf) = csrf {
                builder = builder.header(CSRF_HEADER, csrf);
            }
            builder.body(axum::body::Body::empty()).unwrap()
        };

        assert!(check_csrf(&session, &request("GET", "/domains", None)).is_ok());
        assert!(check_csrf(&session, &request("HEAD", "/domains", None)).is_ok());
        assert!(check_csrf(&session, &request("POST", LOGIN_PATH, None)).is_ok());
        assert!(check_csrf(&session, &request("POST", "/auth/logout", Some("tok"))).is_ok());

        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            for csrf in [None, Some(""), Some("wrong"), Some("tok ")] {
                let err = check_csrf(&session, &request(method, "/auth/logout", csrf))
                    .expect_err("{method} with {csrf:?} must be refused");
                assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
            }
        }

        // No session, no token to compare: the trusted-header and anonymous
        // paths are not ridable by another origin's cookie.
        let headerless = Identity::default();
        assert!(check_csrf(&headerless, &request("POST", "/auth/logout", None)).is_ok());

        // An empty stored token must not match an absent header. `None` (no
        // session) and `Some("")` (a session whose token went missing) are
        // opposite answers: the first has nothing to protect, the second has
        // something to protect and no way to check it.
        let tokenless_session = Identity {
            csrf: Some(String::new()),
            ..Identity::default()
        };
        for csrf in [None, Some(""), Some("anything")] {
            let err = check_csrf(&tokenless_session, &request("POST", "/auth/logout", csrf))
                .expect_err("an empty expected token must fail closed");
            assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
        }
        // A safe method is still safe, and login is still exempt.
        assert!(check_csrf(&tokenless_session, &request("GET", "/domains", None)).is_ok());
        assert!(check_csrf(&tokenless_session, &request("POST", LOGIN_PATH, None)).is_ok());
    }
}
