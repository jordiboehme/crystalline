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
//! this surface: whoever can reach the port can make a row. Three bounds keep
//! that from being a way to fill somebody's disk - a burst per window
//! ([`RegistrationLimiter`]), a ceiling on how many registrations are stored
//! at once ([`MAX_OAUTH_CLIENTS`]), and a prune of every registration that has
//! sat unused for thirty days, run before each new one is stored so an
//! instance makes room for itself.
//!
//! Every client here is a PUBLIC client: it runs on somebody else's machine,
//! so it can keep no secret, and the registration answer therefore carries
//! none. What it does carry is the redirect uris, and those are the whole
//! security story of this endpoint - whatever is stored is where a browser is
//! later sent with an authorization code in the query. [`redirect_uri_problem`]
//! is the rule that decides what may be stored and [`redirect_matches`] the
//! rule that decides what may be presented against it; they are two halves of
//! one decision and live side by side for that reason.

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

use super::auth::{NoStore, no_store, request_origin};
use super::auth_store::normalize_resource;
use super::{ApiError, ApiJson, AuthStore, ProblemDetail, RestState};

/// RFC 9728 protected resource metadata, at the origin root. A client reads it
/// from the `resource_metadata` parameter of the gate's `401`, and falls back
/// to this path at the root when the challenge names none.
pub const PROTECTED_RESOURCE_PATH: &str = "/.well-known/oauth-protected-resource";

/// RFC 8414 authorization server metadata, at the origin root: the first
/// address a client tries for an issuer that carries no path, which this one
/// does not.
pub const AUTHORIZATION_SERVER_PATH: &str = "/.well-known/oauth-authorization-server";

/// Where a browser is sent to consent, relative to the API mount.
pub const AUTHORIZE_PATH: &str = "/oauth/authorize";

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
}

impl OauthServer {
    /// The server, or `None` while `auth.oauth` is off.
    pub fn new(config: &GlobalConfig) -> Option<Arc<OauthServer>> {
        config.auth_oauth().then(|| {
            Arc::new(OauthServer {
                origin: OriginRule::from_config(config),
                limiter: RegistrationLimiter::default(),
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
    /// The slot is taken before the body is read, so an attempt that turns out
    /// to be malformed still costs a slot: what is being protected is the
    /// server's willingness to look at unauthenticated requests at all, and a
    /// limiter a caller could walk past by sending rubbish would protect
    /// nothing.
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
async fn protected_resource(
    State(oauth): State<Option<OriginRule>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let origin = enabled(&oauth)?.origin(&headers)?;
    Ok(Json(protected_resource_document(&origin)))
}

/// `GET /.well-known/oauth-authorization-server`.
async fn authorization_server(
    State(oauth): State<Option<OriginRule>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let origin = enabled(&oauth)?.origin(&headers)?;
    Ok(Json(authorization_server_document(&origin)))
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

    /// The table is full of registrations that are all in use.
    pub fn no_room() -> OauthError {
        OauthError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "this server is holding as many client registrations as it will, and none of them \
             are old enough to collect; an administrator can revoke connections nobody uses",
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
    /// each an https url or an http url on a loopback address.
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
        "https" if url.host().is_some() => None,
        "https" => Some("a redirect uri must name a host"),
        "http" if is_loopback_host(&url) => None,
        _ => Some(
            "a redirect uri must be an https url, or an http url on a loopback address \
             (localhost, 127.0.0.1 or [::1])",
        ),
    }
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

    let name = body.client_name.as_deref().unwrap_or_default().trim();
    if name.chars().count() > MAX_CLIENT_NAME_CHARS {
        return Err(OauthError::invalid_client_metadata(
            "a client name is at most 100 characters: it is shown to the person deciding \
             whether to trust this client",
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(OauthError::invalid_client_metadata(
            "a client name carries no control characters",
        ));
    }
    let name = if name.is_empty() {
        DEFAULT_CLIENT_NAME.to_string()
    } else {
        name.to_string()
    };

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
                   `OauthErrorBody`. Bounded three ways: 30 registrations per \
                   10 minutes per process, 1000 stored registrations, and a \
                   prune of every registration that has gone 30 days without \
                   an authorization.",
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
    // Before the body is looked at: see `RegistrationLimiter::admit`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;
    use crystalline_core::config::{AuthConfig, OidcConfig};

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
            "https://knowledge.example",
            "http://127.0.0.1:33418/callback",
            "http://127.0.0.1/callback",
            "http://localhost:8765/cb",
            "http://LOCALHOST:8765/cb",
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
        assert_eq!(
            code_of(RegisterBody {
                client_name: Some("Claude\u{7}\n".to_string()),
                ..good()
            }),
            "invalid_client_metadata",
            "the name is shown to a person and written to a log"
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
