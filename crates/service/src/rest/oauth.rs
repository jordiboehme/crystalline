//! The resource-server half of OAuth for MCP clients: what this instance calls
//! itself, and the two documents a client discovers it through.
//!
//! An MCP server is an OAuth 2.1 resource server, and a hosted client finds its
//! way in by reading metadata rather than by being told: the `401` at the
//! transport carries a pointer to the protected-resource document, that
//! document names the authorization server, and the authorization-server
//! document names the three endpoints the flow runs against. All of it is one
//! fact spelled several ways - the origin this instance is reached at - which
//! is why the rule for deriving that origin lives here beside the documents
//! rather than in each of them.
//!
//! # Why the origin is the resource identifier
//!
//! RFC 8707 makes an access token an audience-bound credential: the client asks
//! for a token for a named resource, the server records which resource it
//! minted for, and every check compares the two. The name has to be something
//! both sides arrive at without agreeing on anything first, and for an MCP
//! server that is the address the client was pointed at - scheme, host, port,
//! no path. So the resource is derived from the request, exactly as the single
//! sign-on callback address is ([`super::auth::request_origin`]), with the same
//! override for the deployment whose `Host` is rewritten in front of this
//! process.
//!
//! The one tolerance is a trailing slash, because a person typing an address
//! into a client adds or omits one without meaning anything by it. That
//! tolerance is [`super::auth_store::normalize_resource`]'s, called here rather
//! than re-implemented: two spellings of one rule is how an audience check
//! develops a hole.
//!
//! # Why the documents exist unconditionally
//!
//! Both paths are declared whether or not `auth.oauth` is on, and answer `404`
//! while it is off, for the reason the single sign-on routes do: the answer
//! must not depend on configuration the caller cannot see. Mounting them only
//! when the setting is on would leave the paths to whatever is behind them -
//! the app shell for a browser's `Accept`, the MCP gate's own `401` for an API
//! client's - and a discovery probe would read either as something other than
//! "there is no OAuth here".
//!
//! # Registration, and why it is bounded
//!
//! A hosted client has never met this instance and holds no credential, so the
//! first thing it does is register itself (RFC 7591) and take an identifier
//! away. That makes `POST /oauth/register` the one unauthenticated write on
//! this surface: whoever can reach the port can make a row. Four bounds keep
//! that from being a way to fill somebody's disk - a body limit on the route
//! itself, well below the mount's, since an extractor has read the body before
//! any of the rest of this runs; a burst per window
//! ([`RegistrationLimiter`]); a ceiling on how many registrations are stored
//! at once ([`MAX_OAUTH_CLIENTS`]); and a prune, run before each new
//! registration is stored so an instance makes room for itself, that collects
//! a registration which never authorized within the hour and one that has gone
//! unused for thirty days.
//!
//! Every client here is a PUBLIC client: it runs on somebody else's machine,
//! so it can keep no secret, and the registration answer therefore carries
//! none. What it does carry is the redirect uris, and those are the whole
//! security story of this endpoint - whatever is stored is where a browser is
//! later sent with an authorization code in the query. [`redirect_uri_problem`]
//! is the rule that decides what may be stored and [`redirect_matches`] the
//! rule that decides what may be presented against it; they are two halves of
//! one decision and live side by side for that reason.
//!
//! # The authorization leg, and where the person comes in
//!
//! [`authorize`] is the front door: a client sends a browser to it, and it
//! checks everything a server can check without a person - a known
//! registration, a redirect uri that registration named, PKCE with `S256`, and
//! a `resource` naming this instance. Nothing is granted there. What it
//! produces is a [`PendingAuthorization`] under an unguessable id and a `302`
//! to the Fluid consent screen, and only a signed-in account pressing a button
//! turns that into an authorization code.
//!
//! Two rules carry the weight, and they are the two the checks are ordered
//! around:
//!
//! 1. **An error is only ever redirected to a registered redirect uri.** A bad
//!    `client_id` or a `redirect_uri` no registration named is answered here,
//!    to the person, as a problem detail. Redirecting it would make this
//!    endpoint an open redirect with an OAuth error stapled to it.
//! 2. **Everything a client can be told, it is told.** Once the redirect uri is
//!    trusted, RFC 6749 section 4.1.2.1 sends the refusal to the client with
//!    `state` and `iss`, because a browser stuck on an error page is a flow
//!    nobody can recover.
//!
//! Both stores here are in memory ([`AuthorizationStore`], [`CodeStore`]) and
//! that is the specification's own trade: a restart between the client's
//! redirect and the person's decision, or between the decision and the
//! exchange, costs a retry, where keeping either in the database would cost a
//! row per abandoned tab. The tokens a code turns into are in the database and
//! survive both.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use crystalline_core::config::GlobalConfig;
use openidconnect::url::{Host, Url};
use serde_json::{Value, json};

use super::auth::{Identity, NoStore, no_store, request_origin};
use super::auth_store::normalize_resource;
use super::{ApiError, ApiJson, ApiPath, ApiQuery, AuthStore, ProblemDetail, RestState};

/// RFC 9728 protected resource metadata, at the origin root. A client reads it
/// from the `resource_metadata` parameter of the gate's `401`, and falls back
/// to this path at the root when the challenge names none.
pub const PROTECTED_RESOURCE_PATH: &str = "/.well-known/oauth-protected-resource";

/// RFC 8414 authorization server metadata, at the origin root: the first
/// address a client tries for an issuer that carries no path, which this one
/// does not.
pub const AUTHORIZATION_SERVER_PATH: &str = "/.well-known/oauth-authorization-server";

/// Where a client sends a browser to start an authorization, relative to the
/// API mount. NOT the page a person sees: this route answers a redirect to
/// [`CONSENT_PAGE`], which is the Fluid screen. The two are deliberately
/// spelled apart.
pub const AUTHORIZE_PATH: &str = "/oauth/authorize";

/// The consent pair, relative to the API mount: `GET` reads what is being
/// asked for and `POST` answers it. Guarded like every other account-bearing
/// route - the whole point is that a person decides - so it is not in
/// [`super::auth::PUBLIC_PATHS`] the way [`AUTHORIZE_PATH`] is.
pub const AUTHORIZATIONS_PATH: &str = "/oauth/authorizations/{id}";

/// The Fluid screen a person consents on, at the application root rather than
/// under the API mount: a browser is navigated here, and what it loads is the
/// single-page app. `?request=<id>` names the pending authorization.
pub const CONSENT_PAGE: &str = "/authorize";

/// Where a code or a refresh token is exchanged, relative to the API mount.
pub const TOKEN_PATH: &str = "/oauth/token";

/// Where a client registers itself, relative to the API mount.
pub const REGISTER_PATH: &str = "/oauth/register";

/// The mount the JSON API is nested at, which the three endpoint paths above
/// are relative to.
///
/// Spelled here because the authorization-server document publishes absolute
/// urls: a client sent to `<origin>/oauth/authorize` would land on the MCP
/// transport, which is the fallback for everything the declared routes do not
/// claim. The two well-known paths are NOT under it - they are root documents
/// by specification - so they are never built through this.
const API_PREFIX: &str = "/api/v1";

/// The `resource_name` the protected-resource document carries: what a consent
/// screen in somebody else's client calls this server.
const RESOURCE_NAME: &str = "Crystalline";

/// How many registrations this process accepts per [`REGISTRATION_WINDOW`].
///
/// Generous for the case it exists for - a person setting up two harnesses on
/// three machines, each registering once - and nowhere near enough to fill the
/// table with. The bound is per process rather than per caller because there
/// is no caller identity to key one on: registration is what happens before
/// anybody has authenticated.
pub const REGISTRATION_BURST: usize = 30;

/// The window [`REGISTRATION_BURST`] is counted over.
pub const REGISTRATION_WINDOW: Duration = Duration::from_secs(10 * 60);

/// How many registrations this instance stores at once.
///
/// A ceiling rather than a quota: pruning runs first, so this is only reached
/// by an instance whose registrations are all in use, and the honest answer
/// there is that it cannot take another one now.
pub const MAX_OAUTH_CLIENTS: usize = 1000;

/// What a client that names itself nothing is called on the consent screen.
const DEFAULT_CLIENT_NAME: &str = "an MCP client";

/// How long a client name may be. It is shown to a person deciding whether to
/// trust the thing asking, so it has to fit on a line rather than be a wall of
/// attacker-chosen text.
const MAX_CLIENT_NAME_CHARS: usize = 100;

/// How many redirect uris one registration may name. Not in the specification
/// and deliberately here: the request body is capped at ten megabytes, so
/// without a bound on the list a thousand registrations could hold ten
/// gigabytes of somebody's disk.
const MAX_REDIRECT_URIS: usize = 10;

/// How long any single url in a registration may be, for the same reason, and
/// at the length every browser has settled on as the practical maximum.
const MAX_URI_LEN: usize = 2048;

/// The one authentication method this server registers a client with. Public
/// clients only: there is no secret to present.
const AUTH_METHOD_NONE: &str = "none";

/// How large a registration request body may be, in bytes.
///
/// Its own limit rather than the mount's ten megabytes, and layered on the
/// route rather than checked in the handler, because it is the only bound that
/// can apply before the body is read: axum resolves extractors first, so by the
/// time [`RegistrationLimiter::admit`] runs the body has been buffered and put
/// through serde already. The largest registration the rules here allow is ten
/// [`MAX_URI_LEN`] uris beside a [`MAX_CLIENT_NAME_CHARS`] name, so this is
/// room to spare for anything legitimate and a thousandth of what an anonymous
/// caller could otherwise make this process parse.
pub const MAX_REGISTER_BYTES: usize = 64 * 1024;

/// How long a started authorization waits for a person to decide.
///
/// The same ten minutes a single sign-on gets, and for the same reason: long
/// enough to read a consent screen, sign in at a provider and type a second
/// factor, short enough that an abandoned tab is forgotten while it is still
/// open.
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);

/// How many authorizations may be waiting for a decision at once.
///
/// [`authorize`] is public, so this map is something a stranger can add to.
/// Smaller than the sign-in map's ten thousand because a pending authorization
/// costs a client registration first, which is itself bounded three ways.
const MAX_PENDING_AUTHORIZATIONS: usize = 1000;

/// How old a pending authorization must be before a full map may drop it to
/// make room. See [`AuthorizationStore::insert`]; the rule and the number are
/// the single sign-on store's.
const MIN_EVICT_AGE: Duration = Duration::from_secs(30);

/// How long an authorization code may be exchanged for.
///
/// One minute: the code goes from this server to the client's redirect uri and
/// straight back to the token endpoint, which is two hops of a program rather
/// than anything a person waits through. OAuth 2.1 recommends a maximum of ten
/// minutes and every second past the exchange is a second an intercepted code
/// is worth something.
const CODE_TTL: Duration = Duration::from_secs(60);

/// The shortest and longest a PKCE `code_challenge` may be, from RFC 7636
/// section 4.2. An S256 challenge is always exactly 43 characters; the range
/// is the specification's and is checked rather than the exact width, so a
/// client padding within the rule is not refused for it.
const CHALLENGE_LEN: std::ops::RangeInclusive<usize> = 43..=128;

/// How long a `state` this server will carry back. Not in the specification:
/// `state` is opaque to this server and rides in a bounded map that a stranger
/// can add to, so it is bounded for the reason every other length here is.
const MAX_STATE_LEN: usize = 512;

/// How this instance names itself, per request.
///
/// Either the origin of a configured `auth.oidc.redirect_uri` - the one place
/// an operator behind a Host-rewriting proxy has already written the public
/// address - or the origin the request itself says it arrived at. Held by the
/// MCP gate and by the two well-known handlers, so the address a client is told
/// to use, the audience a token is minted for and the audience the gate checks
/// are one answer rather than three.
#[derive(Clone, Debug)]
pub struct OriginRule {
    /// The configured public origin, already parsed down to scheme, host and
    /// port. `None` means derive it from each request.
    override_origin: Option<String>,
}

impl OriginRule {
    /// Read the rule out of `config`.
    ///
    /// The override is read from the `auth.oidc` block directly rather than
    /// through [`super::OidcSettings`], because that resolves to `None` for a
    /// provider that is missing an issuer or a client, and the address in this
    /// key is still the operator's answer to "what is this instance called"
    /// whether or not single sign-on is switched on beside it.
    ///
    /// A value that is not an absolute `http` or `https` url leaves the rule
    /// deriving from the request. It is a fail-safe rather than tolerance: the
    /// settings layer already refuses anything but an absolute https (or
    /// loopback http) url ending at the callback path, so the only way another
    /// value arrives here is through the environment overlay, and refusing to
    /// serve at all would take the instance down over a key that has a
    /// perfectly good default behaviour. The warning is logged once, at
    /// startup, where an operator is still watching.
    ///
    /// **The scheme is what decides, not the presence of a host.**
    /// `Url::origin` answers a tuple origin for a short list of schemes
    /// (`http`, `https`, `ws`, `wss`, `ftp`, `blob`) and an opaque origin for
    /// every other one, and an opaque origin serializes to the literal
    /// `"null"`. A scheme that carries an authority (`foo://kb.example/...`,
    /// `ftp://kb.example/...`) therefore has a host and would still publish
    /// `"null"` as this instance's `resource` and `issuer`, in both documents
    /// and in the gate's challenge. Only the two schemes this surface is ever
    /// served over are accepted, so nothing an operator can mistype reaches a
    /// document.
    pub fn from_config(config: &GlobalConfig) -> OriginRule {
        let configured = config
            .auth_oidc()
            .and_then(|oidc| oidc.redirect_uri.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let override_origin =
            configured.and_then(|value| match openidconnect::url::Url::parse(value) {
                Ok(url) if matches!(url.scheme(), "http" | "https") && url.host().is_some() => {
                    Some(url.origin().ascii_serialization())
                }
                _ => {
                    tracing::warn!(
                        "auth.oidc.redirect_uri is not an absolute http or https url, so the \
                         OAuth resource identifier is derived from each request's Host instead"
                    );
                    None
                }
            });
        OriginRule { override_origin }
    }

    /// The origin a request arrived at, in the spelling everything else
    /// compares.
    ///
    /// [`normalize_resource`] is applied once, here, on both branches: the
    /// derived one never carries a trailing slash by construction, so the call
    /// is a no-op there, and one exit means the two branches cannot come to
    /// disagree about the spelling a token's audience is stored in.
    pub fn origin(&self, headers: &HeaderMap) -> Result<String, ApiError> {
        let raw = match &self.override_origin {
            Some(configured) => configured.clone(),
            None => request_origin(headers)?,
        };
        Ok(normalize_resource(&raw))
    }

    /// Whether two resource identifiers name the same resource: equal once at
    /// most one trailing slash has come off each.
    ///
    /// For comparing what a client *asked* for against what this instance calls
    /// itself. The gate does not use it - it hands the normalized origin to
    /// [`super::AuthStore::oauth_access_user`] and lets the audience condition
    /// in the statement decide, so no branch in Rust can forget the check.
    pub fn same_resource(a: &str, b: &str) -> bool {
        normalize_resource(a) == normalize_resource(b)
    }
}

/// Everything the OAuth surface needs that is resolved once, at startup.
///
/// `None` from [`OauthServer::new`] is an instance with `auth.oauth` off, which
/// is every instance until an operator turns it on; the rest of the surface
/// reads the option rather than the setting, so a running daemon serves the
/// tier it came up in like every other `auth.*` key.
pub struct OauthServer {
    /// What this instance calls itself. See [`OriginRule`].
    pub origin: OriginRule,
    /// What bounds the one unauthenticated write here. See
    /// [`RegistrationLimiter`].
    pub limiter: RegistrationLimiter,
    /// The authorizations waiting for somebody to decide them. See
    /// [`AuthorizationStore`].
    authorizations: Mutex<AuthorizationStore>,
    /// The codes a decision has issued and the token endpoint has not spent
    /// yet. See [`CodeStore`].
    codes: Mutex<CodeStore>,
}

impl OauthServer {
    /// The server, or `None` while `auth.oauth` is off.
    pub fn new(config: &GlobalConfig) -> Option<Arc<OauthServer>> {
        config.auth_oauth().then(|| {
            Arc::new(OauthServer {
                origin: OriginRule::from_config(config),
                limiter: RegistrationLimiter::default(),
                authorizations: Mutex::new(AuthorizationStore::default()),
                codes: Mutex::new(CodeStore::default()),
            })
        })
    }
}

/// A sliding window of the instants this process accepted a registration at,
/// which is what makes [`REGISTRATION_BURST`] a burst rather than a total.
///
/// Held behind a `Mutex` because the server is shared by every request and the
/// whole point is that they see one another's arrivals; the critical section is
/// a few pops and a push on a queue that never grows past the burst, so there
/// is nothing here to contend for. A poisoned lock is taken anyway
/// (`into_inner`): the only thing that could poison it is a panic inside those
/// few lines, and refusing every registration for the life of the process
/// afterwards would be a denial of service the panic did not manage on its own.
#[derive(Debug, Default)]
pub struct RegistrationLimiter {
    /// One instant per accepted registration still inside the window, oldest
    /// first.
    window: Mutex<VecDeque<Instant>>,
}

impl RegistrationLimiter {
    /// Take a slot for a registration arriving at `now`, or answer how many
    /// seconds until one frees up.
    ///
    /// `now` is a parameter rather than read here so the rule can be driven
    /// through a whole window in a test without a test that sleeps for ten
    /// minutes.
    ///
    /// The slot is taken before the body is *validated* and before anything is
    /// read from or written to the store, so an attempt that turns out to be
    /// malformed still costs a slot: a limiter a caller could walk past by
    /// sending rubbish would protect nothing.
    ///
    /// Not before the body is *read*, and the distinction is worth keeping
    /// straight. Axum resolves every extractor before a handler runs, so by
    /// the time this is called the request body has already been buffered and
    /// put through serde. What bounds that half is the route's own
    /// `DefaultBodyLimit` (64 KiB, far below the mount's ten megabytes),
    /// because it is the only thing that can: a limiter reached after
    /// extraction cannot decline to have parsed. Moving the count into a layer
    /// ahead of extraction would buy the stronger property and cost the two
    /// answers that come first - the `404` for an instance serving no OAuth,
    /// and any refusal a route-level extractor makes.
    pub fn admit(&self, now: Instant) -> Result<(), u64> {
        let mut window = self.window.lock().unwrap_or_else(|e| e.into_inner());
        while let Some(oldest) = window.front() {
            if now.duration_since(*oldest) >= REGISTRATION_WINDOW {
                window.pop_front();
            } else {
                break;
            }
        }
        if window.len() >= REGISTRATION_BURST {
            let oldest = *window.front().expect("the burst is never zero");
            let left = REGISTRATION_WINDOW.saturating_sub(now.duration_since(oldest));
            // Never zero: a `Retry-After: 0` invites an immediate retry into
            // the same refusal.
            return Err(left.as_secs().max(1));
        }
        window.push_back(now);
        Ok(())
    }
}

/// The url of the protected-resource document on `origin`, which is what a
/// `401` points a client at.
///
/// Deliberately not built through the same helper as the three endpoint urls:
/// this one is a root document and those three live under the API mount, and a
/// shared helper is how one of them would silently acquire the other's prefix.
pub fn resource_metadata_url(origin: &str) -> String {
    format!("{origin}{PROTECTED_RESOURCE_PATH}")
}

/// The absolute url of an endpoint that lives under the API mount.
fn api_url(origin: &str, path: &str) -> String {
    format!("{origin}{API_PREFIX}{path}")
}

/// The two root documents, mounted beside `/health` on the root router.
///
/// `None` is `auth.oauth` off: the paths still exist and answer `404`, so a
/// probe gets one answer whatever it accepts and whatever else the router
/// serves. See the module documentation.
pub fn well_known_routes(oauth: Option<OriginRule>) -> Router {
    Router::new()
        .route(PROTECTED_RESOURCE_PATH, get(protected_resource))
        .route(AUTHORIZATION_SERVER_PATH, get(authorization_server))
        .with_state(oauth)
}

/// `GET /.well-known/oauth-protected-resource`.
///
/// Never stored, like every other auth-shaped answer here: what these documents
/// say depends on the request's own `Host` unless an origin is configured, so a
/// cache in front of this instance holding one answer would hand a client the
/// wrong instance's endpoints and the wrong audience to ask a token for.
async fn protected_resource(
    State(oauth): State<Option<OriginRule>>,
    headers: HeaderMap,
) -> Result<(NoStore, Json<Value>), ApiError> {
    let origin = enabled(&oauth)?.origin(&headers)?;
    Ok((no_store(), Json(protected_resource_document(&origin))))
}

/// `GET /.well-known/oauth-authorization-server`. Never stored, for
/// [`protected_resource`]'s reason.
async fn authorization_server(
    State(oauth): State<Option<OriginRule>>,
    headers: HeaderMap,
) -> Result<(NoStore, Json<Value>), ApiError> {
    let origin = enabled(&oauth)?.origin(&headers)?;
    Ok((no_store(), Json(authorization_server_document(&origin))))
}

/// The rule, or the refusal for an instance that serves no OAuth.
///
/// A `404` rather than a `501`: the paths exist unconditionally so the answer
/// does not depend on configuration a caller cannot see, and "there is no
/// authorization server here" is the honest shape of it. The detail names the
/// two settings, because the operator reading it is the one who can change the
/// answer.
fn enabled(oauth: &Option<OriginRule>) -> Result<&OriginRule, ApiError> {
    oauth
        .as_ref()
        .ok_or_else(|| ApiError::not_found(NO_OAUTH_HERE))
}

/// What every OAuth path answers on an instance that serves none. One string
/// rather than one per route: the two well-known documents, the registration
/// endpoint and the rest of the flow are all the same absence, and an operator
/// reading any of them is the one person who can change the answer.
const NO_OAUTH_HERE: &str = "this instance does not serve OAuth for MCP clients - an administrator \
                             turns it on with auth.oauth, which needs auth.mcp beside it, and \
                             agents authenticate with a personal MCP token until then";

/// RFC 9728 protected resource metadata for `origin`.
///
/// Four members and no `scopes_supported`: a grant here is the whole account's
/// rights until it is revoked, and advertising scopes this server does not
/// narrow by would be a promise nothing keeps.
fn protected_resource_document(origin: &str) -> Value {
    json!({
        "resource": origin,
        // This server is its own authorization server, so the one entry is the
        // origin again. A client takes the first entry.
        "authorization_servers": [origin],
        "bearer_methods_supported": ["header"],
        "resource_name": RESOURCE_NAME,
    })
}

/// RFC 8414 authorization server metadata for `origin`.
///
/// `code_challenge_methods_supported` is not optional in practice: a client
/// that cannot see S256 advertised refuses to start the flow at all.
/// `token_endpoint_auth_methods_supported` is `["none"]` because every client
/// here is a public one - there is no secret to present - and
/// `authorization_response_iss_parameter_supported` says the authorization
/// response carries `iss`, which is how a client detects a mix-up attack
/// between two authorization servers it talks to.
fn authorization_server_document(origin: &str) -> Value {
    json!({
        "issuer": origin,
        "authorization_endpoint": api_url(origin, AUTHORIZE_PATH),
        "token_endpoint": api_url(origin, TOKEN_PATH),
        "registration_endpoint": api_url(origin, REGISTER_PATH),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "authorization_response_iss_parameter_supported": true,
    })
}

/// A failure at an OAuth endpoint, in the shape RFC 6749 section 5.2 and RFC
/// 7591 section 3.2.2 give it: a small stable `error` code and a human
/// description, as `application/json`.
///
/// **The one place this surface does not answer with a problem detail, and
/// deliberately.** The caller here is an OAuth client library that has never
/// signed in and never will - it branches on `error` and nothing else - so a
/// problem detail would be the single answer in the whole flow it cannot read.
/// Everything a *browser* reaches (the consent pair) keeps the mount's own
/// shape, and so does the `404` an instance with `auth.oauth` off gives this
/// path, because a client that got here was not sent by the metadata and there
/// is no OAuth code for "no such endpoint".
///
/// Two fields beyond the pair the specification names: `retry_after`, which the
/// `429` has to carry as a header rather than as prose, and `problem`, which is
/// how the `404` above renders in the mount's shape without a second error type
/// existing to hold it.
#[derive(Debug)]
pub struct OauthError {
    /// The HTTP status. RFC 7591 registration failures are `400`; the two
    /// bounds answer `429` and `503`.
    pub status: StatusCode,
    /// The registered error code a client branches on.
    pub error: &'static str,
    /// What is wrong, in words, for the person reading a client's log. Never
    /// echoes what the caller sent: the body is attacker-chosen and up to ten
    /// megabytes of it.
    pub description: String,
    /// Seconds until a refused caller may try again, sent as `Retry-After`.
    pub retry_after: Option<u64>,
    /// Render as this surface's problem detail instead. See the type's
    /// documentation.
    problem: bool,
}

impl OauthError {
    fn new(status: StatusCode, error: &'static str, description: impl Into<String>) -> OauthError {
        OauthError {
            status,
            error,
            description: description.into(),
            retry_after: None,
            problem: false,
        }
    }

    /// The body was not the JSON client metadata at all.
    pub fn invalid_request(description: impl Into<String>) -> OauthError {
        OauthError::new(StatusCode::BAD_REQUEST, "invalid_request", description)
    }

    /// One of the redirect uris may not be stored. See
    /// [`redirect_uri_problem`].
    pub fn invalid_redirect_uri(description: impl Into<String>) -> OauthError {
        OauthError::new(StatusCode::BAD_REQUEST, "invalid_redirect_uri", description)
    }

    /// The metadata describes a client this server does not register.
    pub fn invalid_client_metadata(description: impl Into<String>) -> OauthError {
        OauthError::new(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            description,
        )
    }

    /// The burst is spent; come back in `seconds`.
    pub fn come_back_in(seconds: u64) -> OauthError {
        OauthError {
            retry_after: Some(seconds),
            ..OauthError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "temporarily_unavailable",
                "this server is taking no more client registrations just now; the Retry-After \
                 header says when to try again",
            )
        }
    }

    /// The table is full of registrations that are none of them collectable
    /// yet.
    ///
    /// The description names what actually frees a slot, which is the
    /// registrations expiring on their own: there is no delete verb for a
    /// registration anywhere on this surface, so a line telling an operator to
    /// revoke something would send them looking for a control that does not
    /// exist.
    pub fn no_room() -> OauthError {
        OauthError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "this server is holding as many client registrations as it will just now; a \
             registration that never authorized is collected within the hour and an unused one \
             thirty days after its last use, so a client can register again shortly",
        )
    }

    /// The accounts database could not answer. Never carries its message: a
    /// storage error's text is for the operator's log, not for an
    /// unauthenticated caller.
    pub fn store_unavailable() -> OauthError {
        OauthError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            "this server could not reach its accounts database",
        )
    }

    /// There is no OAuth here. Rendered as a problem detail; see the type's
    /// documentation.
    pub fn no_oauth_here() -> OauthError {
        OauthError {
            problem: true,
            ..OauthError::new(StatusCode::NOT_FOUND, "invalid_request", NO_OAUTH_HERE)
        }
    }
}

impl IntoResponse for OauthError {
    fn into_response(self) -> Response {
        if self.problem {
            return ApiError::not_found(self.description).into_response();
        }
        let mut response = (
            self.status,
            no_store(),
            Json(OauthErrorBody {
                error: self.error,
                error_description: self.description,
            }),
        )
            .into_response();
        if let Some(seconds) = self.retry_after
            && let Ok(value) = seconds.to_string().parse()
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

/// The wire form of an [`OauthError`].
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "An OAuth error, sent as `application/json`. The only \
                        failures on this surface that are not RFC 9457 problem \
                        details: the client reading them speaks OAuth and \
                        branches on `error`.")]
pub struct OauthErrorBody {
    /// The registered error code.
    #[schema(example = "invalid_redirect_uri")]
    pub error: &'static str,
    /// What is wrong, in words.
    #[schema(
        example = "a redirect uri must be an https url, or an http url on a loopback address"
    )]
    pub error_description: String,
}

/// The client metadata of an RFC 7591 registration request.
///
/// Every member is optional at the type level, `redirect_uris` included, so a
/// body that leaves one out is answered by the rule that wanted it rather than
/// by a deserialization failure that could only be `invalid_request`.
///
/// Members this server does not implement are accepted and ignored, as the RFC
/// requires: `application_type`, `scope`, `contacts`, `logo_uri` and the rest
/// are read off the wire and dropped, so they are not named here either. The
/// ones this server *contradicts* are refused rather than ignored, because a
/// client that announced `client_secret_basic` and was quietly registered as a
/// public client would discover the disagreement one leg later, at the token
/// endpoint, where it is much harder to read.
#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
#[schema(
    description = "RFC 7591 client metadata. Members this server does not \
                        implement (`application_type`, `scope`, `contacts`, \
                        `logo_uri` and the rest) are accepted and ignored."
)]
pub struct RegisterBody {
    /// Where this client may be redirected back to. At least one, at most ten,
    /// each an https url or an http url on a loopback address, carrying no
    /// fragment and no user information, and sent in the form a url parser
    /// leaves it in (a lowercase scheme and host, no default port, no dot
    /// segments, and a path of at least `/`), because what is registered is
    /// matched exactly.
    #[serde(default)]
    #[schema(example = json!(["https://claude.ai/api/mcp/auth_callback"]))]
    pub redirect_uris: Vec<String>,
    /// What to call this client on the consent screen. Trimmed, at most 100
    /// characters, and defaulted when it is absent or blank.
    #[schema(example = "Claude")]
    pub client_name: Option<String>,
    /// The client's own home page, shown beside the name. Absolute https.
    #[schema(example = "https://claude.ai")]
    pub client_uri: Option<String>,
    /// How the client authenticates at the token endpoint. Absent or `none`:
    /// this server registers public clients only.
    #[schema(example = "none")]
    pub token_endpoint_auth_method: Option<String>,
    /// The grants this client will use, within `authorization_code` and
    /// `refresh_token`.
    pub grant_types: Option<Vec<String>>,
    /// The response types this client will ask for, within `code`.
    pub response_types: Option<Vec<String>>,
}

/// What a registration answers with: the identifier, and the metadata as
/// stored.
///
/// No `client_secret` and no `registration_access_token`. Both would be
/// credentials handed to a caller that proved nothing, and this server has no
/// use for either: every client is public, and a registration is managed by
/// being left to expire rather than by a management API. `Debug` is derived
/// because there is nothing here to redact - a client id authorizes nothing on
/// its own.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RegisteredClient {
    /// The identifier this client names itself with from now on: `coc_` plus
    /// 32 hex characters.
    #[schema(example = "coc_9f2c1d7e4b6a80351c8e0d2f4a6b8c1e")]
    pub client_id: String,
    /// When the registration was made, in seconds since the epoch.
    #[schema(example = 1_767_225_600_i64)]
    pub client_id_issued_at: i64,
    /// The name as stored, which is what a consent screen shows.
    #[schema(example = "Claude")]
    pub client_name: String,
    /// The home page as stored, absent when the registration named none.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "https://claude.ai")]
    pub client_uri: Option<String>,
    /// The redirect uris as stored, in the order they were sent.
    pub redirect_uris: Vec<String>,
    /// Always `none`: a public client presents no secret.
    #[schema(example = "none")]
    pub token_endpoint_auth_method: &'static str,
    /// Always `authorization_code` and `refresh_token`.
    pub grant_types: Vec<String>,
    /// Always `code`.
    pub response_types: Vec<String>,
}

impl From<super::OauthClient> for RegisteredClient {
    fn from(client: super::OauthClient) -> RegisteredClient {
        RegisteredClient {
            client_id_issued_at: issued_at(&client.created_at),
            client_id: client.client_id,
            client_name: client.client_name,
            client_uri: client.client_uri,
            redirect_uris: client.redirect_uris,
            token_endpoint_auth_method: AUTH_METHOD_NONE,
            grant_types: vec![
                "authorization_code".to_string(),
                "refresh_token".to_string(),
            ],
            response_types: vec!["code".to_string()],
        }
    }
}

/// The registration instant as seconds since the epoch, from the RFC 3339 the
/// store writes. An unreadable value reads as now, which is within a
/// millisecond of the truth for the row that was just inserted and is the only
/// way this is ever reached.
fn issued_at(created_at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(created_at)
        .map(|at| at.timestamp())
        .unwrap_or_else(|_| chrono::Utc::now().timestamp())
}

/// Why `uri` may not be stored as a redirect target, or `None` when it may.
///
/// This is the load-bearing check of the whole registration endpoint: whatever
/// passes here is where a browser is later sent with an authorization code in
/// the query, so it is the one thing an attacker registering a client actually
/// chooses. The rules, and what each is for:
///
/// - **Printable ASCII only, no whitespace anywhere.** The WHATWG url parser
///   strips tab, newline and carriage return from *anywhere* in its input, so
///   `https://knowledge.\nexample/cb` parses to a perfectly ordinary url while
///   the string kept on disk still holds the newline - and that string goes
///   into a `Location` header. Refusing them before the parse is what keeps
///   what was validated and what is stored the same bytes. Non-ASCII is
///   refused for the same reason: a stored uri is always a valid header value.
/// - **Absolute, and https - or http on a loopback address.** Plain http off
///   this machine puts the code on the wire in clear. Loopback http is RFC
///   8252's case: a native client has no https listener of its own and the
///   traffic never leaves the machine.
/// - **No fragment.** The fragment is not sent to the server, so a redirect
///   uri carrying one cannot be matched honestly, and a client with a fragment
///   in its registration is a client whose authorization response would be
///   read by whatever script is at that fragment.
/// - **No userinfo.** `https://user@knowledge.example/cb` is how a target is
///   made to read as one host and resolve as another.
/// - **At most [`MAX_URI_LEN`] characters.** See the constant.
/// - **Already normalized.** What is stored is the raw string (the first rule
///   is why), and [`redirect_matches`] compares it exactly, so a spelling a url
///   parser would canonicalize - an uppercase scheme or host, a default port, a
///   dot segment, an absent path - is a registration the client can never use:
///   it presents the canonical form one leg later and matches nothing. Refusing
///   it here says so at the moment it can still be fixed, rather than leaving a
///   "redirect_uri does not match" at the authorize endpoint for something the
///   client did not think it had changed.
pub fn redirect_uri_problem(uri: &str) -> Option<&'static str> {
    if uri.is_empty() {
        return Some("a redirect uri must not be empty");
    }
    if uri.len() > MAX_URI_LEN {
        return Some("a redirect uri must be at most 2048 characters");
    }
    if !uri.is_ascii() || uri.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some(
            "a redirect uri must be printable ASCII with no spaces, tabs or line breaks in it",
        );
    }
    let Ok(url) = Url::parse(uri) else {
        return Some("a redirect uri must be an absolute url");
    };
    if url.fragment().is_some() {
        return Some("a redirect uri must carry no fragment");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Some("a redirect uri must carry no user information before the host");
    }
    match url.scheme() {
        "https" if url.host().is_some() => {}
        "https" => return Some("a redirect uri must name a host"),
        "http" if is_loopback_host(&url) => {}
        _ => {
            return Some(
                "a redirect uri must be an https url, or an http url on a loopback address \
                 (localhost, 127.0.0.1 or [::1])",
            );
        }
    }
    // Last, because it is the only rule about the SPELLING rather than about
    // the address, and the one a client is most likely to trip on innocently.
    if url.as_str() != uri {
        return Some(
            "a redirect uri must be sent in the form a url parser leaves it in: a lowercase \
             scheme and host, no default port, no dot segments, and a path of at least '/'",
        );
    }
    None
}

/// Whether `presented` names the same redirect target as `registered`.
///
/// Exact string equality, with one exception RFC 8252 section 7.3 requires: a
/// native client binds an ephemeral port at run time, so the port it will
/// present is not knowable when it registers. Only the port is forgiven, and
/// only when *both* sides are loopback http - the host, path and query are
/// compared exactly, `localhost` is not `127.0.0.1`, and a presented value that
/// would not have been storable is not compared at all.
pub fn redirect_matches(registered: &str, presented: &str) -> bool {
    if registered == presented {
        return true;
    }
    if redirect_uri_problem(presented).is_some() {
        return false;
    }
    let (Ok(registered), Ok(presented)) = (Url::parse(registered), Url::parse(presented)) else {
        return false;
    };
    if registered.scheme() != "http" || presented.scheme() != "http" {
        return false;
    }
    if !is_loopback_host(&registered) || !is_loopback_host(&presented) {
        return false;
    }
    registered.host() == presented.host()
        && registered.path() == presented.path()
        && registered.query() == presented.query()
}

/// Whether `url`'s host is this machine, in the three spellings RFC 8252 names.
fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address == std::net::Ipv4Addr::LOCALHOST,
        Some(Host::Ipv6(address)) => address == std::net::Ipv6Addr::LOCALHOST,
        None => false,
    }
}

/// A registration that has passed every rule, in the spelling it will be
/// stored in.
#[derive(Debug)]
struct CheckedRegistration {
    name: String,
    client_uri: Option<String>,
    redirect_uris: Vec<String>,
}

/// Apply every metadata rule, before anything is read from or written to the
/// store.
///
/// Ordered so that the store's own invariant is never what answers: it refuses
/// an empty `redirect_uris` with a storage error, which would reach a client as
/// a `500` where RFC 7591 wants a `400 invalid_redirect_uri`.
fn check_metadata(body: RegisterBody) -> Result<CheckedRegistration, OauthError> {
    match body.token_endpoint_auth_method.as_deref() {
        None | Some(AUTH_METHOD_NONE) => {}
        Some(_) => {
            return Err(OauthError::invalid_client_metadata(
                "this server registers public clients only, so token_endpoint_auth_method is \
                 absent or 'none'; there is no client secret to present",
            ));
        }
    }
    // An empty list passes, deliberately: RFC 7591 makes the registration
    // ANSWER authoritative, so a client that named no grant types is registered
    // with the two this server runs and told so, which is the same place a
    // client that named them correctly ends up.
    if body
        .grant_types
        .iter()
        .flatten()
        .any(|grant| !matches!(grant.as_str(), "authorization_code" | "refresh_token"))
    {
        return Err(OauthError::invalid_client_metadata(
            "this server runs the authorization_code and refresh_token grants and no others",
        ));
    }
    if body
        .response_types
        .iter()
        .flatten()
        .any(|response| response != "code")
    {
        return Err(OauthError::invalid_client_metadata(
            "this server answers the code response type and no others",
        ));
    }

    // Trimmed, and stripped of everything that is not text, through the same
    // rule a single sign-on provider's display name goes through: this name is
    // shown to the person deciding whether to trust the client, and a
    // right-to-left override or a zero width character in it renders as a name
    // that is not the name that was stored. Escaping on the consent page does
    // not neutralize a direction change; taking it out does. Stripped rather
    // than refused, because a client with a stray invisible in its name has
    // done nothing wrong and the refusal would teach it nothing it could act
    // on - and what is left is the name it actually spells.
    let name = body
        .client_name
        .as_deref()
        .and_then(super::oidc::presentation_text);
    // Counted after the stripping, so the bound is on what is shown.
    if name
        .as_deref()
        .is_some_and(|name| name.chars().count() > MAX_CLIENT_NAME_CHARS)
    {
        return Err(OauthError::invalid_client_metadata(
            "a client name is at most 100 characters: it is shown to the person deciding \
             whether to trust this client",
        ));
    }
    let name = name.unwrap_or_else(|| DEFAULT_CLIENT_NAME.to_string());

    let client_uri = match body
        .client_uri
        .as_deref()
        .map(str::trim)
        .filter(|uri| !uri.is_empty())
    {
        None => None,
        Some(uri) => {
            let usable = uri.len() <= MAX_URI_LEN
                && uri.is_ascii()
                && !uri.chars().any(|c| c.is_whitespace() || c.is_control())
                && Url::parse(uri).is_ok_and(|url| url.scheme() == "https" && url.host().is_some());
            if !usable {
                return Err(OauthError::invalid_client_metadata(
                    "a client uri is an absolute https url of at most 2048 printable ASCII \
                     characters: it is shown beside the client's name",
                ));
            }
            Some(uri.to_string())
        }
    };

    if body.redirect_uris.is_empty() {
        return Err(OauthError::invalid_redirect_uri(
            "a registration names at least one redirect uri, or no authorization can ever \
             come back to it",
        ));
    }
    if body.redirect_uris.len() > MAX_REDIRECT_URIS {
        return Err(OauthError::invalid_redirect_uri(
            "a registration names at most 10 redirect uris",
        ));
    }
    if let Some(problem) = body
        .redirect_uris
        .iter()
        .find_map(|uri| redirect_uri_problem(uri))
    {
        return Err(OauthError::invalid_redirect_uri(problem));
    }

    Ok(CheckedRegistration {
        name,
        client_uri,
        redirect_uris: body.redirect_uris,
    })
}

/// `POST /oauth/register` - RFC 7591 dynamic registration for a public client.
///
/// Public by path, because it is what a client does before anybody has signed
/// in anywhere, and not CSRF-exempt: a browser that happens to hold a session
/// still echoes its token, so no other origin can make a registration on a
/// visitor's behalf. Served on a read-only instance for the reason the personal
/// token routes are - a registration is account state in the accounts database
/// rather than knowledge, and a read-only team server with `auth.oauth` on is
/// exactly where a client cannot connect at all without one.
#[utoipa::path(
    post,
    path = "/api/v1/oauth/register",
    tag = "oauth",
    operation_id = "register_oauth_client",
    summary = "Register an MCP client as a public OAuth client.",
    description = "RFC 7591 dynamic client registration, the endpoint the \
                   authorization server metadata advertises. Open to any \
                   caller, because a client registers before it can \
                   authenticate as anything. The answer carries a `client_id` \
                   and no secret: every client here is a public client, so \
                   `token_endpoint_auth_method` is always `none` and the \
                   proof of possession at the token endpoint is PKCE. Errors \
                   are OAuth JSON rather than problem details - see \
                   `OauthErrorBody`. Bounded four ways: a 64 KiB body, 30 \
                   registrations per 10 minutes per process, 1000 stored \
                   registrations, and a prune that collects a registration \
                   which never authorized within the hour and one that has \
                   gone 30 days since its last authorization.",
    request_body = RegisterBody,
    responses(
        (
            status = 201,
            description = "The registration, as stored.",
            body = RegisteredClient,
        ),
        (
            status = 400,
            description = "The body is not the JSON client metadata \
                           (`invalid_request`), a redirect uri may not be \
                           stored (`invalid_redirect_uri`), or the metadata \
                           describes a client this server does not register \
                           (`invalid_client_metadata`).",
            body = OauthErrorBody,
        ),
        (
            status = 403,
            description = "A cookie session did not echo its CSRF token.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "This instance does not serve OAuth: `auth.oauth` \
                           is off.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 429,
            description = "The burst is spent. `Retry-After` says when to come \
                           back.",
            body = OauthErrorBody,
        ),
        (
            status = 503,
            description = "This instance is holding as many registrations as \
                           it will, and none are old enough to collect.",
            body = OauthErrorBody,
        ),
    ),
)]
pub async fn register(
    State(state): State<RestState>,
    body: Result<ApiJson<RegisterBody>, ApiError>,
) -> Result<(StatusCode, NoStore, Json<RegisteredClient>), OauthError> {
    let outcome = registered(&state, body).await;
    // One WARN per refused registration, naming the category and nothing the
    // caller sent. The `404` is not a refused registration - it is an instance
    // that serves no OAuth at all - so it is not logged as one.
    if let Err(error) = &outcome
        && !error.problem
    {
        tracing::warn!(
            reason = error.error,
            status = error.status.as_u16(),
            "an mcp client registration was refused"
        );
    }
    outcome
}

/// [`register`]'s body, so the refusal can be logged in exactly one place.
async fn registered(
    state: &RestState,
    body: Result<ApiJson<RegisterBody>, ApiError>,
) -> Result<(StatusCode, NoStore, Json<RegisteredClient>), OauthError> {
    let oauth = state.oauth.as_ref().ok_or_else(OauthError::no_oauth_here)?;
    // Before the body is validated and before the store is touched, though
    // after axum has already read it: see `RegistrationLimiter::admit`.
    oauth
        .limiter
        .admit(Instant::now())
        .map_err(OauthError::come_back_in)?;
    // A wrong content type is axum's `415` and a broken payload its `400`, and
    // both arrive here as one `invalid_request`: RFC 7591 has no `415`, and the
    // client reading this cannot read a problem detail. The rest of the mount
    // keeps its own convention.
    let ApiJson(body) = body.map_err(|_| {
        OauthError::invalid_request(
            "a client registration is a JSON object sent as application/json",
        )
    })?;
    let checked = check_metadata(body)?;

    prune_registrations(&state.auth).await;
    let held = state
        .auth
        .count_oauth_clients()
        .await
        .map_err(|error| store_unavailable("counting the registrations", &error))?;
    if held >= MAX_OAUTH_CLIENTS {
        return Err(OauthError::no_room());
    }
    let client = state
        .auth
        .register_oauth_client(
            &checked.name,
            checked.client_uri.as_deref(),
            &checked.redirect_uris,
        )
        .await
        .map_err(|error| store_unavailable("storing the registration", &error))?;
    // Never the name or the uris: both are attacker-chosen text, and the id is
    // what every later line about this client is keyed on anyway.
    tracing::info!(
        client_id = %client.client_id,
        redirect_uris = client.redirect_uris.len(),
        "an mcp client registered"
    );
    Ok((
        StatusCode::CREATED,
        no_store(),
        Json(RegisteredClient::from(client)),
    ))
}

/// Log a storage failure for the operator and answer the caller with nothing
/// but the fact of it.
fn store_unavailable(doing: &str, error: &anyhow::Error) -> OauthError {
    tracing::error!("the accounts database failed while {doing}: {error:#}");
    OauthError::store_unavailable()
}

/// Collect what nobody is using: expired grants, then the registrations that
/// are left holding nothing.
///
/// **The order is required rather than incidental.** A registration is only
/// collectable once it holds no grant at all, so a registration whose one grant
/// expired months ago stays uncollectable until that dead grant is swept. Both
/// store methods say so in as many words.
///
/// Failures are logged and swallowed: this is housekeeping on the way to
/// somewhere else, and a registration that could not be collected is a row too
/// many rather than a reason to refuse the caller.
pub(super) async fn prune_registrations(auth: &AuthStore) {
    match auth.prune_oauth_grants().await {
        Ok(0) => {}
        Ok(grants) => tracing::info!(grants, "expired oauth grants collected"),
        Err(error) => tracing::warn!("expired oauth grants could not be collected: {error:#}"),
    }
    match auth.prune_oauth_clients().await {
        Ok(0) => {}
        Ok(clients) => tracing::info!(clients, "unused mcp client registrations collected"),
        Err(error) => {
            tracing::warn!("unused mcp client registrations could not be collected: {error:#}");
        }
    }
}

/// The same sweep at startup, off the critical path.
///
/// Spawned rather than awaited because the one place that can call it -
/// [`super::RestState::new`] - is synchronous, and detached rather than tracked
/// because nothing waits on the result: the next registration prunes again
/// anyway, so the worst case of losing this one is that a row lives until then.
/// Outside a runtime it does nothing at all, which is the case of a caller that
/// built a state without ever serving it.
pub(super) fn prune_at_start(auth: Arc<AuthStore>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move { prune_registrations(&auth).await });
}

// --- authorization, consent and codes ---------------------------------------

/// 32 bytes from the OS CSPRNG as lowercase hex: the shape of every
/// unguessable value this half of the surface mints - a pending
/// authorization's id and an authorization code.
///
/// Spelled here rather than reached for in [`super::auth_store`] because that
/// module's generator is private to it and the two have no reason to be one:
/// what it mints is written to a database, and what this mints never leaves
/// memory.
fn random_hex_32() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    crystalline_index::hex_lower(&bytes)
}

/// The sha256 of `value`, lowercase hex. What the code store keys on, so a
/// process dump of a running daemon yields nothing that can be exchanged.
fn sha256_hex(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    crystalline_index::hex_lower(&hasher.finalize())
}

/// An authorization request that passed every check a server can make on its
/// own, waiting for a person to allow or deny it.
///
/// It holds what the consent page shows and what the code will be bound to.
/// Nothing here is taken from the deciding request later: a person's browser
/// arrives at the consent page with an id and nothing else, so what is decided
/// about is what the client asked for, not what the browser says it asked for.
///
/// `Debug` is written rather than derived: the challenge and the state are the
/// client's protocol values and have no business in a log line.
struct PendingAuthorization {
    /// The registration this was started by.
    client_id: String,
    /// What to call that client on the consent page, as it was registered.
    client_name: String,
    /// The client's home page, when the registration named one. Copied here
    /// rather than looked up at consent time so that a re-registration between
    /// the two cannot change what the person is shown.
    client_uri: Option<String>,
    /// Where the browser goes with the answer: the uri the client PRESENTED,
    /// which for a loopback client is not the one it registered (RFC 8252
    /// section 7.3 forgives the port). The code binds to this one, because it
    /// is the one the token request will name.
    redirect_uri: String,
    /// The PKCE challenge the token endpoint will check a verifier against.
    code_challenge: String,
    /// The resource a token from this will be minted for: this instance's own
    /// identifier, normalized, never the spelling the client asked in.
    resource: String,
    /// The client's own value, carried back untouched, or `None` when it sent
    /// none.
    state: Option<String>,
    /// When the request arrived, for the TTL and the eviction order.
    started: Instant,
}

impl std::fmt::Debug for PendingAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingAuthorization")
            .field("client_id", &self.client_id)
            .field("client_name", &self.client_name)
            .field("redirect_uri", &self.redirect_uri)
            .field("code_challenge", &"[redacted]")
            .field("resource", &self.resource)
            .field("state", &self.state.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

/// The authorizations waiting for a decision, keyed by their unguessable id.
///
/// In memory and nowhere else, which is the trade the specification names: a
/// daemon restarted between the client's redirect and the person's decision
/// loses the request and the client starts over, and a second replica behind a
/// proxy does not see the first one's. Both cost a retry; putting a
/// short-lived consent record in the accounts database would cost a row per
/// abandoned tab forever.
#[derive(Default)]
struct AuthorizationStore {
    records: std::collections::HashMap<String, PendingAuthorization>,
    /// Every id inserted, oldest first. Records only ever go in with a fresh
    /// `started`, so this is chronological and both expiry and eviction walk
    /// it from the front. An entry whose record was taken is skipped when it
    /// is reached.
    order: VecDeque<(Instant, String)>,
}

/// The map is full of authorizations too young to evict. See
/// [`AuthorizationStore::insert`].
#[derive(Debug)]
struct AuthorizationsFull;

impl AuthorizationStore {
    /// How many authorizations are waiting.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.records.len()
    }

    /// Remember `record` under `id`, forgetting what has expired and, if the
    /// map is still full, the oldest records past [`MIN_EVICT_AGE`].
    ///
    /// The floor is the single sign-on store's and is the whole reason this is
    /// not a plain oldest-first eviction: without it, filling the map is a way
    /// to evict every real consent mid decision - the cap would be reached and
    /// the oldest record, somebody who pressed a button in their client a few
    /// seconds ago, would go. With it a full map of young records refuses the
    /// newcomer, which costs a flood its own next request and costs the person
    /// deciding nothing.
    fn insert(
        &mut self,
        id: String,
        record: PendingAuthorization,
    ) -> Result<(), AuthorizationsFull> {
        let now = Instant::now();
        self.purge_expired(now);
        while self.records.len() >= MAX_PENDING_AUTHORIZATIONS {
            let Some((started, key)) = self.order.front() else {
                break;
            };
            let already_taken = !self.records.contains_key(key);
            if !already_taken && now.duration_since(*started) < MIN_EVICT_AGE {
                return Err(AuthorizationsFull);
            }
            let (_, key) = self.order.pop_front().expect("checked just above");
            self.records.remove(&key);
        }
        self.order.push_back((record.started, id.clone()));
        self.records.insert(id, record);
        Ok(())
    }

    /// Read the record for `id` without spending it: the consent page is a
    /// page, and a person may open it twice before deciding once.
    fn get(&mut self, id: &str) -> Option<&PendingAuthorization> {
        self.purge_expired(Instant::now());
        self.records.get(id)
    }

    /// Take the record for `id`, if it exists and has not expired.
    ///
    /// Taking rather than reading is what makes a decision happen once: the
    /// record is gone before a code is minted, so a second press of Allow - or
    /// a page left open and pressed again tomorrow - finds nothing to decide.
    fn take(&mut self, id: &str) -> Option<PendingAuthorization> {
        let now = Instant::now();
        let taken = self.records.remove(id);
        self.purge_expired(now);
        taken.filter(|record| now.duration_since(record.started) < PENDING_TTL)
    }

    /// Forget every record older than [`PENDING_TTL`], from the front of the
    /// chronological order until the first one still live, dropping the spent
    /// entries met on the way. See the single sign-on store, whose reasoning
    /// this is.
    fn purge_expired(&mut self, now: Instant) {
        while let Some((started, key)) = self.order.front() {
            let spent = !self.records.contains_key(key);
            if !spent && now.duration_since(*started) < PENDING_TTL {
                break;
            }
            let (_, key) = self.order.pop_front().expect("checked just above");
            self.records.remove(&key);
        }
    }
}

/// What an authorization code stands for, kept under the code's own sha256.
///
/// Every field is a condition the token endpoint checks: the client that
/// presents the code must be the one it was issued to, at the redirect uri it
/// was issued for, holding the verifier behind the challenge, asking for the
/// same resource - and the grant that comes out is for the account named here,
/// which is the account that pressed Allow and no other.
///
/// `Debug` is written rather than derived, for [`PendingAuthorization`]'s
/// reason.
// Read by the token endpoint, which is the next task's: this half of the code
// store mints and the other half spends, and they land in two commits.
#[allow(dead_code)]
struct IssuedCode {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    resource: String,
    user: String,
    issued: Instant,
}

impl std::fmt::Debug for IssuedCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedCode")
            .field("client_id", &self.client_id)
            .field("redirect_uri", &self.redirect_uri)
            .field("code_challenge", &"[redacted]")
            .field("resource", &self.resource)
            .field("user", &self.user)
            .finish()
    }
}

/// The codes a decision has issued and the token endpoint has not spent yet,
/// keyed by sha256 of the code.
///
/// Hashed for the reason every credential here is hashed: what is held is not
/// what can be presented. In memory for [`AuthorizationStore`]'s reason, and
/// with a much shorter life - the code travels from this server to the
/// client's redirect uri and straight back to the token endpoint, which is two
/// hops of a program.
///
/// No eviction rule and no cap, deliberately. Every entry costs a signed-in
/// person pressing Allow and lives sixty seconds, and both halves purge what
/// has expired, so what bounds this is the rate a human can consent at.
#[derive(Default)]
struct CodeStore {
    codes: std::collections::HashMap<String, IssuedCode>,
}

impl CodeStore {
    /// Remember `code` under the hash of the code it was minted as.
    fn issue(&mut self, hash: String, code: IssuedCode) {
        self.purge_expired(Instant::now());
        self.codes.insert(hash, code);
    }

    /// Take the code named by `hash`, if it exists and is still live.
    ///
    /// Removed before the expiry is looked at, so a code past its window is
    /// spent by the attempt that presented it rather than left to be presented
    /// again.
    // The token endpoint is this method's only caller and lands with the next
    // task. See `IssuedCode`.
    #[allow(dead_code)]
    fn take(&mut self, hash: &str) -> Option<IssuedCode> {
        let now = Instant::now();
        let taken = self.codes.remove(hash);
        self.purge_expired(now);
        taken.filter(|code| now.duration_since(code.issued) < CODE_TTL)
    }

    /// Forget every code past [`CODE_TTL`]. A full scan, which is what a map
    /// holding a minute of one person's consents can afford.
    fn purge_expired(&mut self, now: Instant) {
        self.codes
            .retain(|_, code| now.duration_since(code.issued) < CODE_TTL);
    }
}

/// What a decision binds a code to, given the request it decides and the
/// account that decided it.
///
/// One function so the binding is one place: the client, the redirect uri, the
/// challenge and the resource all come from the record the client itself
/// started, and the account comes from the session that pressed the button.
/// Nothing about a code is ever taken from the request that mints it.
fn code_for(record: &PendingAuthorization, user: &str) -> IssuedCode {
    IssuedCode {
        client_id: record.client_id.clone(),
        redirect_uri: record.redirect_uri.clone(),
        code_challenge: record.code_challenge.clone(),
        resource: record.resource.clone(),
        user: user.to_string(),
        issued: Instant::now(),
    }
}

/// `uri` with `params` appended to whatever query it already has.
///
/// String work rather than a parse and a re-serialization, because the
/// registered uri has to reach the browser as the bytes it was registered as:
/// a url library normalizes on the way out (`https://knowledge.example`
/// becomes `https://knowledge.example/`), and a client comparing the redirect
/// it receives against the one it registered would see a different address.
/// What makes that safe is that a stored redirect uri is printable ASCII with
/// no whitespace and no fragment - see [`redirect_uri_problem`] - so appending
/// to it cannot produce anything but a valid header value.
///
/// Values are percent-encoded down to the unreserved set, which over-encodes a
/// little and is decoded transparently by every client.
fn redirect_with(uri: &str, params: &[(&str, &str)]) -> String {
    const UNRESERVED: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    let mut out = String::from(uri);
    let mut separator = if uri.contains('?') { '&' } else { '?' };
    for (name, value) in params {
        out.push(separator);
        out.push_str(name);
        out.push('=');
        out.extend(percent_encoding::utf8_percent_encode(value, UNRESERVED));
        separator = '&';
    }
    out
}

/// The query of `GET /oauth/authorize`.
///
/// Every member is optional at the type level so that a missing one is
/// answered by the rule that wanted it - which decides whether the answer is
/// rendered here or redirected to the client - rather than by a
/// deserialization failure that could only ever be rendered. `scope` is
/// accepted and ignored: a grant here is the whole account's rights until it
/// is revoked, so there is nothing to narrow.
///
/// `Debug` is written rather than derived: this struct is the request's
/// challenge and state.
#[derive(Default, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuthorizeQuery {
    /// Always `code`: this server answers no other response type.
    pub response_type: Option<String>,
    /// The registration this request is made under.
    pub client_id: Option<String>,
    /// Where to send the browser with the answer. Must be one the
    /// registration named, port-agnostically for a loopback client.
    pub redirect_uri: Option<String>,
    /// The PKCE challenge: 43 to 128 unreserved characters.
    pub code_challenge: Option<String>,
    /// Always `S256`.
    pub code_challenge_method: Option<String>,
    /// The client's own value, carried back on the answer.
    pub state: Option<String>,
    /// Accepted and ignored. See the struct's documentation.
    pub scope: Option<String>,
    /// Which resource a token is being asked for: absent, or this instance's
    /// own identifier (a trailing slash is tolerated).
    pub resource: Option<String>,
}

impl std::fmt::Debug for AuthorizeQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizeQuery")
            .field("response_type", &self.response_type)
            .field("client_id", &self.client_id)
            .field("redirect_uri", &self.redirect_uri)
            .field(
                "code_challenge",
                &self.code_challenge.as_ref().map(|_| "[redacted]"),
            )
            .field("code_challenge_method", &self.code_challenge_method)
            .field("state", &self.state.as_ref().map(|_| "[redacted]"))
            .field("scope", &self.scope)
            .field("resource", &self.resource)
            .finish()
    }
}

/// What the consent page shows: who is asking, where the answer goes, and who
/// is about to grant it.
///
/// Deliberately not the protocol: no code, no challenge, no state, no client
/// id. A person deciding whether to trust something is helped by the client's
/// name and the address the answer will be sent to, and by nothing else on
/// this list. `Debug` is derived because there is nothing here to redact,
/// which is the property rather than an accident.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct AuthorizationView {
    /// The client's name as it registered it, at most 100 characters of
    /// printable text.
    #[schema(example = "Claude")]
    pub client_name: String,
    /// The client's home page, when it registered one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "https://claude.ai")]
    pub client_uri: Option<String>,
    /// The host (and port, when it names one) the browser will be sent to.
    /// The one fact that says where an authorization actually goes.
    #[schema(example = "claude.ai")]
    pub redirect_host: String,
    /// Whether that address is this machine, so the page can say that the
    /// client is a program on this computer rather than a service elsewhere.
    pub loopback: bool,
    /// The account that will be granted. A client acts as the person who
    /// consented, so this is what is actually being handed over.
    #[schema(example = "ada")]
    pub account: String,
    /// Seconds until the request expires and has to be started again from the
    /// client.
    #[schema(example = 587)]
    pub expires_in: u64,
}

/// Allow or deny, and there is nothing else to choose: a grant here is the
/// whole account's rights until it is revoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// Issue a code for this account.
    Allow,
    /// Send the client away with `access_denied`.
    Deny,
}

/// What `POST /oauth/authorizations/{id}` takes.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct DecisionBody {
    /// `allow` or `deny`. Anything else is not a decision and is refused
    /// before the pending request is touched, so an unreadable answer leaves
    /// it there to be answered again.
    pub decision: Decision,
}

/// Where to send the browser once the decision is made.
///
/// A body rather than a redirect, for the reason `POST /auth/oidc/login`
/// answers one: only a script can send the session's CSRF token, and a script
/// cannot read where a redirect went, so a 302 here would hand the browser
/// somewhere the page could not learn. Fluid navigates the whole page to it.
///
/// `Debug` is written rather than derived: on an allow this location carries
/// the authorization code.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct DecisionResponse {
    /// The client's redirect uri with the answer on it: `code`, `state` and
    /// `iss` on an allow, `error=access_denied`, `state` and `iss` on a deny.
    #[schema(
        example = "https://claude.ai/api/mcp/auth_callback?code=...&state=...&iss=https://kb.example"
    )]
    pub location: String,
}

impl std::fmt::Debug for DecisionResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionResponse")
            .field("location", &"[redacted]")
            .finish()
    }
}

/// A `302` to `location`, never stored.
///
/// `302` rather than axum's `303`, for the reason `oidc::found` gives: it is
/// what the OAuth authorization request is specified and universally
/// implemented as.
fn found(location: String) -> Response {
    (
        StatusCode::FOUND,
        no_store(),
        [(header::LOCATION, location)],
    )
        .into_response()
}

/// An authorization refusal that goes back to the client, because the redirect
/// uri it named is one its registration named too.
///
/// RFC 6749 section 4.1.2.1: once the redirect uri is trusted, the error
/// belongs to the client rather than to the person, and a browser sitting on
/// an error page is a flow nothing can recover. `iss` (RFC 9207) says which
/// server answered, which is how a client talking to two authorization servers
/// detects a mix-up; `state` goes back exactly as it came, when it came at all.
fn redirect_error(
    redirect_uri: &str,
    error: &'static str,
    state: Option<&str>,
    origin: &str,
    client_id: &str,
) -> Response {
    // Never the state, the challenge or the redirect uri: the category and the
    // client are what an operator reading a log needs, and everything else on
    // this request is the caller's own text.
    tracing::warn!(
        reason = error,
        client_id = %client_id,
        "an authorization request was refused"
    );
    let mut params: Vec<(&str, &str)> = vec![("error", error)];
    if let Some(state) = state {
        params.push(("state", state));
    }
    params.push(("iss", origin));
    found(redirect_with(redirect_uri, &params))
}

/// A refusal the person at the browser reads, because there is no redirect uri
/// this server is willing to send anything to.
///
/// The one rule of this endpoint that cannot bend: an unknown client or a
/// redirect uri no registration named means an error sent there would be an
/// open redirect with an OAuth error attached to it. So it is rendered here,
/// in the mount's own problem-detail shape, and the browser stays.
fn rendered_refusal(reason: &str, detail: &str) -> ApiError {
    tracing::warn!(
        reason,
        "an authorization request was refused before any redirect"
    );
    ApiError::bad_request(detail)
}

/// `GET /oauth/authorize` - start an authorization the person will decide.
///
/// Public by path, like the registration endpoint and for the same reason: the
/// browser arriving here is following a link from a client and may well have no
/// session yet. Nothing is granted by this route - it checks what a server can
/// check without a person, remembers the request under an unguessable id and
/// hands the browser to the consent page, which is where an account appears.
///
/// The order of the checks is the security property. Everything that decides
/// **whether there is a redirect uri worth trusting** happens first and is
/// answered here; everything after it is the client's problem and is sent to
/// the client.
#[utoipa::path(
    get,
    path = "/api/v1/oauth/authorize",
    tag = "oauth",
    operation_id = "oauth_authorize",
    params(AuthorizeQuery),
    summary = "Start an authorization for an MCP client.",
    description = "The authorization endpoint the metadata advertises. \
                   `response_type=code` and PKCE `S256` are required - this \
                   server registers public clients only, so the challenge is \
                   what makes an intercepted code worthless. A good request \
                   answers 302 to the consent screen `/authorize?request=<id>`, \
                   where a signed-in person allows or denies it. An unknown \
                   `client_id`, or a `redirect_uri` the registration did not \
                   name, is answered here as a problem detail and is never \
                   redirected anywhere; every other refusal goes back to the \
                   client's redirect uri with `error`, `state` and `iss`.",
    responses(
        (
            status = 302,
            description = "Follow `location`: the consent screen for a good \
                           request, or the client's redirect uri carrying \
                           `error`, `state` and `iss`.",
            headers(
                ("location" = String, description = "The consent screen, or the client's redirect uri."),
                ("cache-control" = String, description = "`no-store`."),
            ),
        ),
        (
            status = 400,
            description = "No usable `client_id` or `redirect_uri`, so there \
                           is nowhere this server is willing to send an error.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "This instance does not serve OAuth: `auth.oauth` \
                           is off.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 500,
            description = "The accounts database could not be reached.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn authorize(
    State(state): State<RestState>,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<AuthorizeQuery>,
) -> Result<Response, ApiError> {
    let oauth = state
        .oauth
        .as_ref()
        .ok_or_else(|| ApiError::not_found(NO_OAUTH_HERE))?;
    // This instance's own identifier, normalized once, here: it is what the
    // `resource` is checked against, what the code is bound to and what `iss`
    // says, and deriving it three times is how those three come to disagree.
    let origin = oauth.origin.origin(&headers)?;

    // --- what decides whether there is anywhere to redirect to --------------
    let Some(client_id) = query
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(rendered_refusal(
            "no client id",
            "this authorization request names no client_id, so there is no registration to \
             check its redirect uri against - the client registers first, at \
             POST /api/v1/oauth/register",
        ));
    };
    let client = state
        .auth
        .oauth_client(client_id)
        .await
        .map_err(|error| {
            tracing::error!("the accounts database failed while reading a registration: {error:#}");
            ApiError::internal("this server could not reach its accounts database")
        })?
        .ok_or_else(|| {
            rendered_refusal(
                "unknown client",
                "no client is registered here under that client_id - it may have been collected \
                 after thirty days without use, in which case the client registers again",
            )
        })?;
    let Some(redirect_uri) = query
        .redirect_uri
        .as_deref()
        .filter(|presented| {
            client
                .redirect_uris
                .iter()
                .any(|registered| redirect_matches(registered, presented))
        })
        .map(str::to_string)
    else {
        return Err(rendered_refusal(
            "redirect uri not registered",
            "this authorization request names no redirect_uri, or one this client did not \
             register - an authorization is only ever sent to an address the registration \
             named, so nothing can be sent anywhere from here",
        ));
    };

    // --- everything the client can be told ---------------------------------
    // The state is bounded before it is echoed: it rides in a map a stranger
    // can add to, and one that is too long is a request this server will not
    // carry rather than a value to truncate.
    let state_value = match query.state.as_deref() {
        Some(state) if state.len() > MAX_STATE_LEN => {
            return Ok(redirect_error(
                &redirect_uri,
                "invalid_request",
                None,
                &origin,
                client_id,
            ));
        }
        other => other,
    };
    let refuse = |error: &'static str| {
        Ok(redirect_error(
            &redirect_uri,
            error,
            state_value,
            &origin,
            client_id,
        ))
    };

    if query.response_type.as_deref() != Some("code") {
        return refuse("unsupported_response_type");
    }
    // PKCE, and S256 only. `plain` would make the challenge worth exactly what
    // the code it protects is worth, and no challenge at all would make an
    // intercepted code enough on its own - which for a public client is the
    // whole credential.
    if query.code_challenge_method.as_deref() != Some("S256") {
        return refuse("invalid_request");
    }
    let Some(code_challenge) = query.code_challenge.as_deref().filter(|challenge| {
        CHALLENGE_LEN.contains(&challenge.len())
            && challenge
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
    }) else {
        return refuse("invalid_request");
    };
    // RFC 8707: a token is minted for a named resource, and this server mints
    // for itself alone. A client asking for another server's is asking the
    // wrong server.
    if let Some(asked) = query.resource.as_deref()
        && !OriginRule::same_resource(asked, &origin)
    {
        return refuse("invalid_target");
    }

    let record = PendingAuthorization {
        client_id: client.client_id.clone(),
        client_name: client.client_name.clone(),
        client_uri: client.client_uri.clone(),
        redirect_uri: redirect_uri.clone(),
        code_challenge: code_challenge.to_string(),
        // The instance's own spelling, never the one the client asked in: the
        // audience the gate checks comes out of `normalize_resource`, and a
        // trailing slash stored here would mint a token nothing accepts.
        resource: origin.clone(),
        state: state_value.map(str::to_string),
        started: Instant::now(),
    };
    let id = random_hex_32();
    let remembered = oauth
        .authorizations
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone(), record);
    if remembered.is_err() {
        return refuse("temporarily_unavailable");
    }

    // Only now, and deliberately: the thirty-day prune clock is "when did a
    // client last get an authorization under way", so a caller that never got
    // past PKCE must not be able to keep a registration alive with rubbish.
    if let Err(error) = state.auth.touch_oauth_client(client_id).await {
        // Not worth failing a good request for: the stamp decides when an
        // unused registration is collected, and one collected a little early
        // is a client that registers again.
        tracing::warn!("a registration's last use could not be stamped: {error:#}");
    }
    tracing::info!(
        client_id = %client_id,
        "an mcp client started an authorization"
    );
    Ok(found(format!("{CONSENT_PAGE}?request={id}")))
}

/// `GET /oauth/authorizations/{id}` - what is being asked for.
///
/// Any signed-in account, and that is the design rather than a gap: there is no
/// account at the moment a client starts a request, so there is nothing for the
/// request to belong to. What protects it is the id, which is 32 random bytes
/// and is only ever known by the browser the client sent here; the account that
/// reads it is the person at that browser, and the code binds to whoever
/// actually decides.
///
/// Served on a read-only instance, for the reason the personal token surface
/// is: a grant is account state in the accounts database rather than knowledge,
/// and a read-only team server with `auth.oauth` on is exactly where a client
/// cannot connect at all without one.
#[utoipa::path(
    get,
    path = "/api/v1/oauth/authorizations/{id}",
    tag = "oauth",
    operation_id = "oauth_authorization",
    params(("id" = String, Path, description = "The pending authorization, from the consent screen's `request` parameter.")),
    summary = "What an MCP client is asking this account to grant.",
    description = "The consent screen's own read: the client's name and home \
                   page, the host the answer will be sent to and whether it is \
                   this machine, the account that would be granted, and how \
                   long is left. Never the authorization code, the PKCE \
                   challenge or the client's `state` - none of them is \
                   anything a person decides with. Needs a signed-in account, \
                   any role.",
    responses(
        (status = 200, description = "The request, as the consent screen shows it.", body = AuthorizationView),
        (
            status = 401,
            description = "No signed-in account: there is nobody to grant anything.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such pending request: it expired, it was already \
                           decided, or this instance serves no OAuth.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn authorization(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(id): ApiPath<String>,
) -> Result<(NoStore, Json<AuthorizationView>), ApiError> {
    let account = identity.require_account().map_err(|_| {
        ApiError::unauthorized(
            "deciding what an MCP client may do needs a signed-in account - sign in, and the \
             consent screen will be here",
        )
    })?;
    let oauth = state
        .oauth
        .as_ref()
        .ok_or_else(|| ApiError::not_found(NO_OAUTH_HERE))?;
    let mut authorizations = oauth
        .authorizations
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let record = authorizations.get(&id).ok_or_else(gone_authorization)?;
    let view = view_of(record, &account.name);
    Ok((no_store(), Json(view)))
}

/// The consent screen's view of a pending request, for `account`.
fn view_of(record: &PendingAuthorization, account: &str) -> AuthorizationView {
    AuthorizationView {
        client_name: record.client_name.clone(),
        client_uri: record.client_uri.clone(),
        redirect_host: super::auth_store::redirect_host(&record.redirect_uri),
        loopback: Url::parse(&record.redirect_uri).is_ok_and(|url| is_loopback_host(&url)),
        account: account.to_string(),
        expires_in: PENDING_TTL
            .saturating_sub(record.started.elapsed())
            .as_secs(),
    }
}

/// One answer for a request that is not there, whichever way it went: expired,
/// already decided, or an id nobody ever issued.
///
/// Deliberately one answer. Telling them apart would tell a caller holding a
/// guessed id whether it ever named anything, and the person at the browser
/// does the same thing in every case - start again from the client.
fn gone_authorization() -> ApiError {
    ApiError::not_found(
        "there is no authorization request waiting under that id - it may have been decided \
         already, or waited more than ten minutes; start again from the client",
    )
}

/// `POST /oauth/authorizations/{id}` - allow or deny.
///
/// The record is TAKEN rather than read, before anything is minted, so a
/// decision happens once: a second press of Allow finds nothing, and so does a
/// tab left open overnight.
///
/// The answer is a location for the browser to navigate to, in both directions.
/// A person who says no is owed the same round trip as one who says yes - the
/// client is waiting at its redirect uri either way, and `access_denied` is
/// what tells it to stop waiting rather than time out.
#[utoipa::path(
    post,
    path = "/api/v1/oauth/authorizations/{id}",
    tag = "oauth",
    operation_id = "oauth_decide",
    params(("id" = String, Path, description = "The pending authorization being decided.")),
    summary = "Allow or deny what an MCP client is asking for.",
    description = "Takes the pending request - once, so a second answer finds \
                   nothing - and answers where to send the browser. On `allow` \
                   that is the client's redirect uri carrying a single-use \
                   authorization code, the client's `state` and `iss`; the \
                   code is bound to this account, this client, this redirect \
                   uri, the PKCE challenge and this resource, and lives sixty \
                   seconds. On `deny` it is the same uri carrying \
                   `error=access_denied`. A grant is the whole account's \
                   rights until it is revoked from the profile page. Needs a \
                   signed-in account, any role, and the session's CSRF token; \
                   served on a read-only instance, like the personal token \
                   surface.",
    request_body = DecisionBody,
    responses(
        (status = 200, description = "Navigate the whole page to `location`.", body = DecisionResponse),
        (
            status = 422,
            description = "The body is not `{\"decision\": \"allow\"}` or \
                           `{\"decision\": \"deny\"}`. The request is left \
                           undecided, because the body is read before it is \
                           taken.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No signed-in account: there is nobody to grant anything.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The session's CSRF token was missing or wrong.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such pending request: it expired, it was already \
                           decided, or this instance serves no OAuth.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn decide(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<DecisionBody>,
) -> Result<(NoStore, Json<DecisionResponse>), ApiError> {
    let account = identity.require_account().map_err(|_| {
        ApiError::unauthorized(
            "deciding what an MCP client may do needs a signed-in account - sign in, and the \
             consent screen will be here",
        )
    })?;
    let oauth = state
        .oauth
        .as_ref()
        .ok_or_else(|| ApiError::not_found(NO_OAUTH_HERE))?;
    let record = oauth
        .authorizations
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take(&id)
        .ok_or_else(gone_authorization)?;

    // `iss` and the code's resource are the record's, not this request's: the
    // consent page may well have been reached over a different `Host` than the
    // client's redirect was, and what a token is minted for was settled when
    // the client asked.
    let mut params: Vec<(&str, &str)> = Vec::new();
    let code = match body.decision {
        Decision::Allow => {
            let code = random_hex_32();
            oauth
                .codes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .issue(sha256_hex(&code), code_for(&record, &account.name));
            tracing::info!(
                account = %account.name,
                client_id = %record.client_id,
                client_name = %record.client_name,
                "an account granted an mcp client access"
            );
            Some(code)
        }
        Decision::Deny => {
            tracing::info!(
                account = %account.name,
                client_id = %record.client_id,
                "an account refused an mcp client"
            );
            None
        }
    };
    match &code {
        Some(code) => params.push(("code", code)),
        None => params.push(("error", "access_denied")),
    }
    if let Some(state) = record.state.as_deref() {
        params.push(("state", state));
    }
    params.push(("iss", &record.resource));
    Ok((
        no_store(),
        Json(DecisionResponse {
            location: redirect_with(&record.redirect_uri, &params),
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;
    use crystalline_core::config::{AuthConfig, OidcConfig};

    /// A pending authorization that started `age` ago.
    fn pending(age: Duration) -> PendingAuthorization {
        PendingAuthorization {
            client_id: "coc_1".to_string(),
            client_name: "Claude".to_string(),
            client_uri: Some("https://claude.ai".to_string()),
            redirect_uri: "https://claude.ai/api/mcp/auth_callback".to_string(),
            code_challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".to_string(),
            resource: "https://kb.example".to_string(),
            state: Some("st-1".to_string()),
            started: Instant::now() - age,
        }
    }

    /// A pending authorization is forgotten when it expires, and a full map
    /// makes room from the oldest end - but never at the expense of somebody
    /// who is deciding right now.
    ///
    /// The floor is what separates this from a plain oldest-first eviction and
    /// is the single sign-on store's rule: without it, filling the map is a way
    /// to push every real consent out from under the person reading it, because
    /// the oldest record in a busy map is somebody who pressed a button in
    /// their client a few seconds ago.
    #[test]
    fn a_pending_authorization_expires_and_the_cap_evicts_the_oldest_first() {
        // Expiry, from both halves: an insert purges, and so does a take, so an
        // idle instance does not hold spent requests until the next client
        // happens to arrive.
        let mut store = AuthorizationStore::default();
        store
            .insert(
                "old".to_string(),
                pending(PENDING_TTL + Duration::from_secs(1)),
            )
            .unwrap();
        assert!(
            store.take("old").is_none(),
            "a request nobody decided within ten minutes is gone"
        );
        store
            .insert(
                "old".to_string(),
                pending(PENDING_TTL + Duration::from_secs(1)),
            )
            .unwrap();
        assert!(store.get("old").is_none(), "and reading it finds nothing");
        assert_eq!(store.len(), 0, "the read purged it rather than leaving it");

        // Oldest first, once there is something old enough to take.
        let mut store = AuthorizationStore::default();
        store
            .insert(
                "stale".to_string(),
                pending(MIN_EVICT_AGE + Duration::from_secs(1)),
            )
            .unwrap();
        for i in 0..MAX_PENDING_AUTHORIZATIONS {
            store
                .insert(format!("fill-{i}"), pending(Duration::ZERO))
                .unwrap_or_else(|_| panic!("insert {i} should have evicted the stale record"));
        }
        assert!(store.take("stale").is_none(), "the oldest one made room");
        assert!(
            store.take("fill-0").is_some(),
            "and nothing younger was touched"
        );

        // A map full of records younger than the floor refuses the newcomer.
        let mut store = AuthorizationStore::default();
        store
            .insert("deciding".to_string(), pending(Duration::from_secs(5)))
            .unwrap();
        let mut refused = 0;
        for i in 0..(MAX_PENDING_AUTHORIZATIONS * 2) {
            if store
                .insert(format!("flood-{i}"), pending(Duration::ZERO))
                .is_err()
            {
                refused += 1;
            }
        }
        assert!(refused > 0, "past the cap a flood is refused, not served");
        assert!(store.len() <= MAX_PENDING_AUTHORIZATIONS);
        assert!(
            store.take("deciding").is_some(),
            "the person who pressed a button five seconds ago survived the flood"
        );
    }

    /// A code is spent by the first attempt that presents it, and only while it
    /// is fresh.
    ///
    /// Both halves matter to the token endpoint: single use is what stops an
    /// intercepted code being exchanged behind the client's back, and the
    /// sixty-second window is what makes an interception have to be immediate.
    /// An expired code is removed by the attempt that presented it rather than
    /// left to be presented again.
    #[test]
    fn a_code_is_taken_once_and_only_while_it_is_fresh() {
        let mut store = CodeStore::default();
        let issue = |store: &mut CodeStore, name: &str, age: Duration| {
            let mut code = code_for(&pending(Duration::ZERO), "ada");
            code.issued = Instant::now() - age;
            store.issue(sha256_hex(name), code);
        };

        issue(&mut store, "live", Duration::ZERO);
        assert!(store.take(&sha256_hex("live")).is_some(), "the first use");
        assert!(
            store.take(&sha256_hex("live")).is_none(),
            "and there is no second one"
        );

        issue(&mut store, "stale", CODE_TTL + Duration::from_secs(1));
        assert!(
            store.take(&sha256_hex("stale")).is_none(),
            "past its minute"
        );
        assert!(
            store.codes.is_empty(),
            "and taken out of the map by the try"
        );

        // A code nobody ever presents is collected by the next issue, so an
        // instance that consents and never exchanges does not grow a map.
        issue(&mut store, "abandoned", CODE_TTL + Duration::from_secs(1));
        issue(&mut store, "next", Duration::ZERO);
        assert_eq!(store.codes.len(), 1, "the abandoned one was swept");

        // The code itself is never a key: what is held is the digest, so a
        // dump of a running process yields nothing exchangeable.
        assert!(
            !store.codes.keys().any(|key| key == "next"),
            "a code is stored hashed"
        );
    }

    /// A code is bound to the request the client started and to the account
    /// that answered it, and to nothing the answering request said.
    ///
    /// This is the whole authorization decision in one function: the client,
    /// the redirect uri, the PKCE challenge and the resource all come out of
    /// the record - which was written when the client asked - and only the
    /// account comes from the session that pressed the button.
    #[test]
    fn a_decision_binds_a_code_to_the_request_and_the_account_that_answered_it() {
        let record = pending(Duration::ZERO);
        let code = code_for(&record, "ada");
        assert_eq!(code.client_id, record.client_id);
        assert_eq!(code.redirect_uri, record.redirect_uri);
        assert_eq!(code.code_challenge, record.code_challenge);
        assert_eq!(code.resource, record.resource);
        assert_eq!(code.user, "ada", "the account that decided, and no other");

        // Nothing that carries a code, a challenge or a state prints one.
        let rendered = format!(
            "{record:?} {code:?} {:?}",
            DecisionResponse {
                location: "https://claude.ai/cb?code=deadbeef".to_string(),
            }
        );
        for secret in [record.code_challenge.as_str(), "st-1", "deadbeef"] {
            assert!(
                !rendered.contains(secret),
                "{secret} leaked into {rendered}"
            );
        }
        let query = AuthorizeQuery {
            code_challenge: Some(record.code_challenge.clone()),
            state: Some("st-1".to_string()),
            ..AuthorizeQuery::default()
        };
        let rendered = format!("{query:?}");
        assert!(!rendered.contains("st-1"), "{rendered}");
        assert!(
            !rendered.contains(record.code_challenge.as_str()),
            "{rendered}"
        );
    }

    /// The answer is appended to whatever query the registered uri already
    /// carries, rather than replacing it or being parsed and rebuilt.
    ///
    /// A url library normalizes on the way out - `https://kb.example` comes
    /// back as `https://kb.example/` - and a client comparing the redirect it
    /// receives against the one it registered would see a different address.
    #[test]
    fn a_redirect_appends_to_whatever_query_it_already_has() {
        assert_eq!(
            redirect_with(
                "https://claude.ai/cb",
                &[("code", "abc"), ("iss", "https://kb.example")]
            ),
            "https://claude.ai/cb?code=abc&iss=https%3A%2F%2Fkb.example"
        );
        assert_eq!(
            redirect_with("https://claude.ai/cb?tenant=eu", &[("code", "abc")]),
            "https://claude.ai/cb?tenant=eu&code=abc"
        );
        // A path-less uri keeps its shape, which is the reason this is string
        // work and not a parse.
        assert_eq!(
            redirect_with("https://kb.example", &[("error", "access_denied")]),
            "https://kb.example?error=access_denied"
        );
        // A state is the client's own text and is encoded, never interpreted.
        assert_eq!(
            redirect_with("https://claude.ai/cb", &[("state", "a b&c=d#e")]),
            "https://claude.ai/cb?state=a%20b%26c%3Dd%23e"
        );
        assert_eq!(
            redirect_with("https://claude.ai/cb", &[]),
            "https://claude.ai/cb"
        );
    }

    /// The consent screen is shown what a person decides with, and none of the
    /// protocol.
    ///
    /// The loopback marker is the one thing on it that is not simply repeated
    /// from the registration: a client at `127.0.0.1` is a program on this
    /// computer, and one at `claude.ai` is a service somewhere else, and the
    /// person deciding is entitled to be told which.
    #[test]
    fn the_consent_view_shows_what_to_decide_with_and_no_protocol() {
        let view = view_of(&pending(Duration::from_secs(13)), "ada");
        assert_eq!(view.client_name, "Claude");
        assert_eq!(view.client_uri.as_deref(), Some("https://claude.ai"));
        assert_eq!(view.redirect_host, "claude.ai");
        assert!(!view.loopback);
        assert_eq!(view.account, "ada");
        // Truncating seconds, so thirteen seconds in leaves either 586 or 587
        // depending on where the sub-second remainder fell.
        let left = PENDING_TTL.as_secs() - 13;
        assert!(
            view.expires_in == left || view.expires_in == left - 1,
            "thirteen seconds of ten minutes have gone: {}",
            view.expires_in
        );

        let rendered = serde_json::to_string(&view).unwrap();
        for absent in ["code_challenge", "state", "coc_1", "E9Melhoa"] {
            assert!(
                !rendered.contains(absent),
                "{absent} is not a person's business: {rendered}"
            );
        }

        // The three loopback spellings, and a port that is part of the host a
        // person reads.
        for (uri, host) in [
            ("http://127.0.0.1:51902/callback", "127.0.0.1:51902"),
            ("http://localhost:8765/cb", "localhost:8765"),
            ("http://[::1]:9000/cb", "[::1]:9000"),
        ] {
            let mut record = pending(Duration::ZERO);
            record.redirect_uri = uri.to_string();
            let view = view_of(&record, "ada");
            assert!(view.loopback, "{uri} is this machine");
            assert_eq!(view.redirect_host, host);
        }

        // An expired record would report no time left rather than underflow.
        let view = view_of(&pending(PENDING_TTL + Duration::from_secs(30)), "ada");
        assert_eq!(view.expires_in, 0);
    }

    /// A config carrying `auth.oauth` on and whatever `redirect_uri` says.
    fn config_with(redirect_uri: Option<&str>) -> GlobalConfig {
        GlobalConfig {
            auth: Some(AuthConfig {
                mcp: Some(true),
                oauth: Some(true),
                oidc: redirect_uri.map(|uri| OidcConfig {
                    redirect_uri: Some(uri.to_string()),
                    ..OidcConfig::default()
                }),
                ..AuthConfig::default()
            }),
            ..GlobalConfig::default()
        }
    }

    fn headers_with(host: &str, forwarded: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, host.parse().unwrap());
        if let Some(proto) = forwarded {
            headers.insert("x-forwarded-proto", proto.parse().unwrap());
        }
        headers
    }

    /// The resource identifier is the callback address's own origin: the same
    /// `Host` and the same forwarded-scheme rule, with the path cut off. If the
    /// two ever diverged, an operator would have one public address for
    /// sign-in and another for tokens.
    #[test]
    fn an_origin_is_derived_like_the_callback_address() {
        let rule = OriginRule::from_config(&config_with(None));
        assert_eq!(
            rule.origin(&headers_with("127.0.0.1:7411", None)).unwrap(),
            "http://127.0.0.1:7411",
            "a loopback development server is plain http"
        );
        assert_eq!(
            rule.origin(&headers_with("knowledge.example", Some("https")))
                .unwrap(),
            "https://knowledge.example"
        );
        assert_eq!(
            rule.origin(&headers_with("knowledge.example", None))
                .unwrap(),
            "https://knowledge.example",
            "a non-loopback Host is https with or without a forwarded scheme"
        );
        assert!(
            rule.origin(&HeaderMap::new()).is_err(),
            "a request with no Host names no resource"
        );
        assert!(
            rule.origin(&headers_with("evil.test/x", None)).is_err(),
            "and a Host that could open a path is not interpolated into one"
        );
    }

    /// A configured callback address names the origin, path and all cut off,
    /// and it wins over whatever the request says.
    #[test]
    fn a_configured_callback_address_names_the_origin() {
        let rule = OriginRule::from_config(&config_with(Some(
            "https://knowledge.example/api/v1/auth/oidc/callback",
        )));
        assert_eq!(
            rule.origin(&headers_with("127.0.0.1:7411", None)).unwrap(),
            "https://knowledge.example",
            "the configured public address wins over the upstream's Host"
        );
        assert_eq!(
            rule.origin(&HeaderMap::new()).unwrap(),
            "https://knowledge.example",
            "and it needs no Host at all"
        );
        // A port is part of an origin; a default port is not spelled.
        assert_eq!(
            OriginRule::from_config(&config_with(Some(
                "https://knowledge.example:8443/api/v1/auth/oidc/callback"
            )))
            .origin(&HeaderMap::new())
            .unwrap(),
            "https://knowledge.example:8443"
        );
        // A value the settings layer would have refused, arriving through the
        // environment overlay: the rule falls back to deriving rather than
        // publishing something nobody can reach.
        let unusable = OriginRule::from_config(&config_with(Some("/api/v1/auth/oidc/callback")));
        assert_eq!(
            unusable
                .origin(&headers_with("127.0.0.1:7411", None))
                .unwrap(),
            "http://127.0.0.1:7411"
        );
    }

    /// **Only an http or https url names an origin here**, and every other
    /// scheme falls back to deriving from the request.
    ///
    /// The scheme is what decides, not the presence of a host. `Url::origin`
    /// answers a *tuple* origin for a handful of schemes and an opaque origin
    /// for everything else, and an opaque origin serializes to the literal
    /// "null" - which, published, would make `resource`, `issuer`, all three
    /// endpoint urls and the gate's challenge nonsense a client cannot use. A
    /// scheme with an authority (`foo://kb.example/...`, `ftp://`, `ws://`)
    /// gets past a host check and would produce exactly that, so the allowlist
    /// is the guard rather than the host.
    #[test]
    fn only_an_http_or_https_address_names_the_origin() {
        for unusable in [
            "foo://knowledge.example/api/v1/auth/oidc/callback",
            "ftp://knowledge.example/api/v1/auth/oidc/callback",
            "ws://knowledge.example/api/v1/auth/oidc/callback",
            "mailto:admin@knowledge.example",
            "data:text/plain,callback",
            "knowledge.example/api/v1/auth/oidc/callback",
        ] {
            let rule = OriginRule::from_config(&config_with(Some(unusable)));
            let origin = rule.origin(&headers_with("127.0.0.1:7411", None)).unwrap();
            assert_eq!(
                origin, "http://127.0.0.1:7411",
                "{unusable} must leave the rule deriving from the request"
            );
            assert!(
                !origin.contains("null"),
                "and an opaque origin must never be published: {unusable} gave {origin}"
            );
        }
        // The loopback development server the settings layer does allow.
        assert_eq!(
            OriginRule::from_config(&config_with(Some(
                "http://localhost:7411/api/v1/auth/oidc/callback"
            )))
            .origin(&HeaderMap::new())
            .unwrap(),
            "http://localhost:7411"
        );
    }

    /// One trailing slash is the same resource and nothing else is. A person
    /// typing the server address into a client adds or omits one without
    /// meaning anything by it; every other difference is a different origin.
    #[test]
    fn a_trailing_slash_names_the_same_resource() {
        assert!(OriginRule::same_resource(
            "https://knowledge.example",
            "https://knowledge.example/"
        ));
        assert!(OriginRule::same_resource(
            "https://knowledge.example/",
            " https://knowledge.example "
        ));
        assert!(
            !OriginRule::same_resource("https://knowledge.example", "https://knowledge.example//"),
            "one slash, not every slash"
        );
        assert!(
            !OriginRule::same_resource("https://knowledge.example", "https://KNOWLEDGE.example"),
            "nothing is lowercased: an audience check that folded spellings would be the hole \
             it exists to close"
        );
        assert!(!OriginRule::same_resource(
            "https://knowledge.example",
            "http://knowledge.example"
        ));
        assert!(!OriginRule::same_resource(
            "https://knowledge.example",
            "https://knowledge.example:8443"
        ));
    }

    /// The three endpoint urls carry the API mount and the metadata pointer
    /// does not. Getting that backwards sends a client either to the MCP
    /// transport or to a path the API guard answers 401 for, and in both cases
    /// the failure is far from here.
    #[test]
    fn the_documents_name_the_origin_and_only_the_endpoints_carry_the_api_mount() {
        let origin = "https://knowledge.example";
        assert_eq!(
            protected_resource_document(origin),
            json!({
                "resource": origin,
                "authorization_servers": [origin],
                "bearer_methods_supported": ["header"],
                "resource_name": "Crystalline",
            })
        );
        assert_eq!(
            authorization_server_document(origin),
            json!({
                "issuer": origin,
                "authorization_endpoint": "https://knowledge.example/api/v1/oauth/authorize",
                "token_endpoint": "https://knowledge.example/api/v1/oauth/token",
                "registration_endpoint": "https://knowledge.example/api/v1/oauth/register",
                "response_types_supported": ["code"],
                "grant_types_supported": ["authorization_code", "refresh_token"],
                "code_challenge_methods_supported": ["S256"],
                "token_endpoint_auth_methods_supported": ["none"],
                "authorization_response_iss_parameter_supported": true,
            })
        );
        assert_eq!(
            resource_metadata_url(origin),
            "https://knowledge.example/.well-known/oauth-protected-resource",
            "the pointer is a root document, with no API mount in it"
        );
    }

    /// **The rules a redirect uri is stored under, and the rule a presented
    /// one is matched by.**
    ///
    /// The single most load-bearing check on this surface: whatever passes
    /// here is where a browser is later sent with an authorization code in the
    /// query. The table is exhaustive on purpose - a rule that is only stated
    /// in prose is a rule that comes back off in a refactor.
    #[test]
    fn redirect_uri_rules_follow_rfc_8252() {
        // Storable.
        for uri in [
            "https://claude.ai/api/mcp/auth_callback",
            "https://knowledge.example:8443/cb?next=here",
            "https://knowledge.example/",
            "http://127.0.0.1:33418/callback",
            "http://127.0.0.1/callback",
            "http://localhost:8765/cb",
            "http://[::1]:9000/cb",
        ] {
            assert_eq!(redirect_uri_problem(uri), None, "{uri} should be storable");
        }
        // Not storable, and each for its own reason.
        for uri in [
            "",
            "/api/mcp/auth_callback",
            "knowledge.example/cb",
            "http://knowledge.example/cb",
            // The loopback rule is about the host, not about the word: this is
            // an ordinary internet name that happens to start with one.
            "http://localhost.evil.test/cb",
            "http://127.0.0.2/cb",
            "https://knowledge.example/cb#frag",
            "https://knowledge.example/cb#",
            "https://user@knowledge.example/cb",
            "https://user:pw@knowledge.example/cb",
            "app://callback",
            "javascript:alert(1)",
            "data:text/html,hi",
            "mailto:ada@knowledge.example",
            // The WHATWG parser strips these from anywhere in the input, so
            // the parsed url would look clean while the stored string still
            // carried them into a Location header.
            "https://knowledge.\nexample/cb",
            "https://knowledge.example/cb\r\nSet-Cookie: a=b",
            "https://knowledge.example/\tcb",
            " https://knowledge.example/cb",
            "https://knowledge.example/cb ",
            // Non-ASCII, which is not a header value.
            "https://knowledge.example/caf\u{e9}",
            // Spellings a url library would canonicalize between registering
            // and presenting, which would then match nothing: the scheme, the
            // host, a default port, a dot segment and an absent path.
            "HTTPS://claude.ai/api/mcp/auth_callback",
            "https://Claude.AI/api/mcp/auth_callback",
            "https://knowledge.example:443/cb",
            "http://127.0.0.1:80/cb",
            // Refused by the normalized-form rule rather than by the loopback
            // one: a url parser lowercases a host, so this registration could
            // never be presented back in the spelling it was stored in.
            "http://LOCALHOST:8765/cb",
            "https://knowledge.example/a/../cb",
            "https://knowledge.example",
        ] {
            assert!(
                redirect_uri_problem(uri).is_some(),
                "{uri:?} should not be storable"
            );
        }
        let long = format!("https://knowledge.example/{}", "x".repeat(MAX_URI_LEN));
        assert!(redirect_uri_problem(&long).is_some(), "too long to store");

        // Matching: exact, with the port forgiven on loopback and nothing else.
        let native = "http://127.0.0.1:33418/callback";
        assert!(redirect_matches(native, native));
        assert!(redirect_matches(native, "http://127.0.0.1:51902/callback"));
        assert!(redirect_matches(native, "http://127.0.0.1/callback"));
        assert!(redirect_matches("http://127.0.0.1/callback", native));
        assert!(redirect_matches("http://[::1]:1/cb", "http://[::1]:2/cb"));
        assert!(
            !redirect_matches(native, "http://localhost:33418/callback"),
            "localhost and 127.0.0.1 are different registrations"
        );
        assert!(!redirect_matches(native, "http://127.0.0.1:33418/other"));
        assert!(!redirect_matches(
            native,
            "http://127.0.0.1:33418/callback?a=b"
        ));
        assert!(
            !redirect_matches(native, "https://127.0.0.1:33418/callback"),
            "the scheme is never forgiven"
        );
        assert!(
            !redirect_matches(native, "http://user@127.0.0.1:33418/callback"),
            "a presented uri that could not have been stored is not compared"
        );

        let hosted = "https://claude.ai/api/mcp/auth_callback";
        assert!(redirect_matches(hosted, hosted));
        assert!(
            !redirect_matches(hosted, "https://claude.ai:8443/api/mcp/auth_callback"),
            "an https client's port is exact: the loopback rule is for loopback"
        );
        assert!(!redirect_matches(
            hosted,
            "https://claude.ai/api/mcp/auth_callback/"
        ));
        assert!(!redirect_matches(
            hosted,
            "https://claude.com/api/mcp/auth_callback"
        ));
    }

    /// **The window admits a burst and then says when to come back**, and it
    /// slides rather than resetting on the hour: ten minutes after the first
    /// registration, exactly one slot is free again.
    #[test]
    fn the_registration_window_admits_a_burst_and_then_slides() {
        let limiter = RegistrationLimiter::default();
        let start = Instant::now();
        for i in 0..REGISTRATION_BURST {
            assert!(
                limiter.admit(start + Duration::from_secs(i as u64)).is_ok(),
                "registration {i} is inside the burst"
            );
        }
        // The burst is spent. The wait is the rest of the OLDEST arrival's
        // window, which is what frees the next slot.
        let refused = limiter
            .admit(start + Duration::from_secs(60))
            .expect_err("the burst is spent");
        assert_eq!(refused, REGISTRATION_WINDOW.as_secs() - 60);
        // Never zero, however close the caller is to the boundary: a
        // `Retry-After: 0` invites an immediate retry into the same refusal.
        let edge = limiter
            .admit(start + REGISTRATION_WINDOW - Duration::from_millis(1))
            .expect_err("still inside the window");
        assert_eq!(edge, 1);
        // The first arrival has now left the window, so exactly one slot is
        // free and the one after it is refused again.
        assert!(limiter.admit(start + REGISTRATION_WINDOW).is_ok());
        assert!(limiter.admit(start + REGISTRATION_WINDOW).is_err());
    }

    /// **Client metadata is checked before anything is read from or written to
    /// the store**, and the two error codes are told apart the way RFC 7591
    /// tells them apart: a redirect uri that may not be stored is
    /// `invalid_redirect_uri`, and everything else about the client is
    /// `invalid_client_metadata`.
    #[test]
    fn client_metadata_is_checked_before_anything_is_stored() {
        let hosted = "https://claude.ai/api/mcp/auth_callback".to_string();
        let good = || RegisterBody {
            redirect_uris: vec![hosted.clone()],
            ..RegisterBody::default()
        };

        // A client that names nothing but where to come back to.
        let checked = check_metadata(good()).unwrap();
        assert_eq!(checked.name, "an MCP client");
        assert_eq!(checked.client_uri, None);
        assert_eq!(checked.redirect_uris, vec![hosted.clone()]);

        // The name is trimmed, and a name of nothing but space is no name.
        let named = check_metadata(RegisterBody {
            client_name: Some("  Claude  ".to_string()),
            client_uri: Some(" https://claude.ai ".to_string()),
            token_endpoint_auth_method: Some("none".to_string()),
            grant_types: Some(vec!["authorization_code".to_string()]),
            response_types: Some(vec!["code".to_string()]),
            ..good()
        })
        .unwrap();
        assert_eq!(named.name, "Claude");
        assert_eq!(named.client_uri.as_deref(), Some("https://claude.ai"));
        assert_eq!(
            check_metadata(RegisterBody {
                client_name: Some("   ".to_string()),
                ..good()
            })
            .unwrap()
            .name,
            "an MCP client"
        );

        let code_of = |body: RegisterBody| check_metadata(body).unwrap_err().error;
        assert_eq!(
            code_of(RegisterBody {
                token_endpoint_auth_method: Some("client_secret_basic".to_string()),
                ..good()
            }),
            "invalid_client_metadata",
            "there is no secret to present here"
        );
        assert_eq!(
            code_of(RegisterBody {
                grant_types: Some(vec!["client_credentials".to_string()]),
                ..good()
            }),
            "invalid_client_metadata"
        );
        assert_eq!(
            code_of(RegisterBody {
                response_types: Some(vec!["code".to_string(), "token".to_string()]),
                ..good()
            }),
            "invalid_client_metadata"
        );
        assert_eq!(
            code_of(RegisterBody {
                client_name: Some("x".repeat(MAX_CLIENT_NAME_CHARS + 1)),
                ..good()
            }),
            "invalid_client_metadata"
        );
        assert!(
            check_metadata(RegisterBody {
                client_name: Some("x".repeat(MAX_CLIENT_NAME_CHARS)),
                ..good()
            })
            .is_ok(),
            "and the boundary itself is fine"
        );

        // A name is shown to the person deciding whether to trust this client,
        // so what cannot be seen is taken out rather than refused: a client
        // with a stray zero width character still registers, and a client that
        // tried to reverse how its name renders registers under the name it
        // actually spells. Escaping on the consent page does not neutralize a
        // right-to-left override; removing it does.
        let name_of = |name: &str| {
            check_metadata(RegisterBody {
                client_name: Some(name.to_string()),
                ..good()
            })
            .unwrap()
            .name
        };
        assert_eq!(name_of("Claude\u{7}\n"), "Claude");
        assert_eq!(name_of("Ada\u{202e}ecalevoL"), "AdaecalevoL");
        assert_eq!(name_of("Cla\u{200b}ude"), "Claude");
        assert_eq!(
            name_of("\u{200b}\u{feff}\u{00ad}"),
            "an MCP client",
            "a name of nothing but invisibles is no name at all"
        );
        // Every letter survives, in every script: this takes out control and
        // formatting codepoints, never text.
        assert_eq!(
            name_of("\u{30af}\u{30ed}\u{30fc}\u{30c9}"),
            "\u{30af}\u{30ed}\u{30fc}\u{30c9}"
        );

        assert_eq!(
            code_of(RegisterBody {
                client_uri: Some("http://claude.ai".to_string()),
                ..good()
            }),
            "invalid_client_metadata"
        );
        assert_eq!(
            code_of(RegisterBody {
                client_uri: Some("not a url".to_string()),
                ..good()
            }),
            "invalid_client_metadata"
        );

        // The store refuses an empty list with a storage error, which would
        // reach a client as a 500. It never gets the chance.
        assert_eq!(
            code_of(RegisterBody::default()),
            "invalid_redirect_uri",
            "a registration with nowhere to come back to"
        );
        assert_eq!(
            code_of(RegisterBody {
                redirect_uris: vec![hosted.clone(); MAX_REDIRECT_URIS + 1],
                ..good()
            }),
            "invalid_redirect_uri"
        );
        assert_eq!(
            code_of(RegisterBody {
                redirect_uris: vec![hosted.clone(), "http://knowledge.example/cb".to_string()],
                ..good()
            }),
            "invalid_redirect_uri",
            "one bad uri refuses the registration rather than being dropped"
        );
    }

    /// **A registration answer is renderable and carries no secret.** The
    /// `Debug` is derived on purpose here (there is nothing to redact), so the
    /// thing to pin is that nothing secret ever gets into the struct.
    #[test]
    fn a_registration_answer_carries_no_secret() {
        let stored = super::super::OauthClient {
            client_id: "coc_9f2c1d7e4b6a80351c8e0d2f4a6b8c1e".to_string(),
            client_name: "Claude".to_string(),
            client_uri: None,
            redirect_uris: vec!["https://claude.ai/api/mcp/auth_callback".to_string()],
            created_at: "2026-09-07T12:00:00Z".to_string(),
            last_used: None,
        };
        let answer = RegisteredClient::from(stored);
        assert_eq!(answer.token_endpoint_auth_method, "none");
        assert_eq!(answer.client_id_issued_at, 1_788_782_400);
        let rendered = serde_json::to_value(&answer).unwrap();
        assert!(
            rendered.get("client_uri").is_none(),
            "an absent member is absent rather than null: {rendered}"
        );
        assert!(
            rendered.get("client_secret").is_none()
                && rendered.get("registration_access_token").is_none(),
            "{rendered}"
        );
        // An unreadable instant reads as now rather than as zero, so a client
        // never sees an issue date in 1970.
        assert!(issued_at("not a date") > 1_700_000_000);
    }

    /// **Every OAuth failure renders in the shape its reader can read**: the
    /// RFC's JSON for a client, this surface's problem detail for the one
    /// answer that is not an OAuth failure at all.
    #[test]
    fn an_oauth_failure_renders_in_the_shape_its_reader_reads() {
        let refused = OauthError::come_back_in(42).into_response();
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            refused.headers().get(header::RETRY_AFTER).unwrap(),
            "42",
            "a 429 says when to come back"
        );
        assert_eq!(
            refused.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            refused.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );

        let gone = OauthError::no_oauth_here().into_response();
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            gone.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json",
            "an instance that serves no OAuth is not answering an OAuth failure"
        );
    }

    /// **Neither document is ever cached.**
    ///
    /// Both are a function of the request's own `Host` unless an origin is
    /// configured, so one stored answer in front of an instance reached by two
    /// names would hand a client the other name's endpoints - and the resource
    /// identifier a token is minted for with them, which is the audience the
    /// gate then refuses.
    #[tokio::test]
    async fn neither_well_known_document_is_ever_stored() {
        let rule = Some(OriginRule::from_config(&config_with(None)));
        let headers = headers_with("127.0.0.1:7411", None);
        for answer in [
            protected_resource(State(rule.clone()), headers.clone())
                .await
                .unwrap()
                .into_response(),
            authorization_server(State(rule), headers)
                .await
                .unwrap()
                .into_response(),
        ] {
            assert_eq!(
                answer.headers().get(header::CACHE_CONTROL).unwrap(),
                "no-store",
                "a discovery document follows the request it answered"
            );
        }
    }

    /// With the setting off there is no rule to answer with, and the refusal
    /// names the two keys that change that.
    #[test]
    fn without_the_setting_there_is_no_server_and_the_documents_are_gone() {
        let mut config = config_with(None);
        config.auth.as_mut().unwrap().oauth = None;
        assert!(OauthServer::new(&config).is_none());
        assert!(OauthServer::new(&config_with(None)).is_some());

        let error = enabled(&None).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
        assert!(error.detail.contains("auth.oauth"), "{}", error.detail);
        assert!(error.detail.contains("auth.mcp"), "{}", error.detail);
    }
}
