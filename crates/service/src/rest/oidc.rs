//! Signing in against an OpenID Connect provider, on top of the local
//! accounts rather than instead of them.
//!
//! Three public routes, all GET and all listed in [`super::auth`]'s
//! `PUBLIC_PATHS` because none of them can carry a session yet:
//!
//! - [`login`] starts the authorization code flow: it generates the state,
//!   the nonce and the PKCE verifier, remembers them in a server-side pending
//!   record keyed by the state, binds that record to the browser with a
//!   short-lived cookie and redirects to the provider.
//! - [`callback`] is where the provider sends the browser back. It matches the
//!   state against both the cookie and the pending record, exchanges the code
//!   with the client secret and the PKCE verifier, validates the ID token
//!   (issuer, audience, expiry, signature, nonce) and hands the claims to
//!   [`resolve_oidc_identity`].
//! - [`providers`] says whether the sign-in button should be drawn at all.
//!
//! **The client secret never leaves this module.** It is read from the config
//! (or the environment, through the settings overlay), held in
//! `openidconnect`'s `ClientSecret` wrapper whose `Debug` prints a placeholder,
//! and every failure that comes back from the provider is mapped to a sentence
//! written here before it reaches a caller. The provider's own words go to
//! `tracing` at `debug` and nowhere else.
//!
//! **What this module does NOT do is decide who the claims are.** The callback
//! ends at [`resolve_oidc_identity`], which is the seam the JIT provisioning
//! task fills: matching an `(issuer, subject)` pair to an account, and creating
//! one when there is no match, is a decision about the accounts database rather
//! than about the protocol, and keeping it behind one function is what keeps
//! this file about the protocol.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use crystalline_core::config::GlobalConfig;
use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreIdTokenClaims, CoreIdTokenVerifier,
    CoreProviderMetadata,
};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, JsonWebKeySet, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, SignatureVerificationError, http,
};
use tokio::sync::{Mutex, RwLock};

use super::auth::Identity;
use super::auth_store::{Role, User};
use super::{ApiError, ApiQuery, ProblemDetail, RestState};

/// The route that starts a sign-in, relative to the `/api/v1` mount.
pub const LOGIN_PATH: &str = "/auth/oidc/login";

/// The route the provider sends the browser back to, relative to the `/api/v1`
/// mount. This is the string an operator registers with the provider, with the
/// instance's own scheme and host in front of it.
pub const CALLBACK_PATH: &str = "/auth/oidc/callback";

/// The route the login screen asks which ways in exist, relative to the
/// `/api/v1` mount.
pub const PROVIDERS_PATH: &str = "/auth/providers";

/// The cookie that binds a pending sign-in to the browser that started it.
///
/// Named beside [`super::auth::SESSION_COOKIE`] and set with the same
/// attributes but a lifetime measured in minutes: it exists only for the round
/// trip to the provider and back, and the callback deletes it whatever the
/// outcome.
pub const STATE_COOKIE: &str = "fluid_oidc_state";

/// How long a started sign-in may take to come back. Long enough for a person
/// to read a consent screen and type a second factor, short enough that an
/// abandoned attempt is forgotten while its tab is still open.
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);

/// How many sign-ins may be in flight at once.
///
/// [`login`] is public and unauthenticated, so the pending map is something a
/// stranger can add to. The cap is what stops that from being a way to make
/// this process reserve memory: past it, the oldest record is dropped, which
/// costs an abandoned tab a restarted sign-in and costs an attacker nothing
/// they wanted. Generous next to any real instance's concurrent logins.
const MAX_PENDING: usize = 256;

/// The scopes requested when `auth.oidc.scopes` is unset. `openid` is added by
/// the library itself and is deliberately not repeated here.
const DEFAULT_SCOPES: [&str; 2] = ["profile", "email"];

/// The button's label when `auth.oidc.name` is unset.
const DEFAULT_PROVIDER_NAME: &str = "single sign-on";

/// How long an outbound call to the provider may take. Discovery, the JWKS
/// fetch and the token exchange all run inside a browser's request, so a
/// provider that hangs must fail the sign-in rather than hold a connection.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(10);

// --- settings -----------------------------------------------------------

/// The `auth.oidc` block once it has been read, validated and found complete.
///
/// Resolved once, when the HTTP surface is built, exactly like
/// [`super::auth::AuthCfg`]: every `auth.oidc.*` key is startup-effective, so a
/// running daemon serves the provider it started with.
///
/// A block that is absent, incomplete or malformed resolves to `None` rather
/// than to a startup failure. The keys are set one at a time, so refusing to
/// come up on a half-configured block would strand an operator halfway through
/// configuring it with no daemon left to configure it through; instead SSO
/// stays off, `GET /auth/providers` says so, and a `warn` line at startup names
/// exactly what is missing.
#[derive(Clone)]
pub struct OidcSettings {
    /// The provider's issuer url, already parsed.
    issuer: IssuerUrl,
    /// The client id the provider issued for this instance.
    client_id: ClientId,
    /// The client secret. Wrapped rather than a `String` so it cannot be
    /// printed by accident: `ClientSecret`'s `Debug` renders a placeholder.
    client_secret: ClientSecret,
    /// The label on the sign-in button.
    name: String,
    /// The scopes to request beyond `openid`.
    scopes: Vec<Scope>,
    /// The role an account provisioned through this provider is created at.
    /// Read by [`resolve_oidc_identity`]'s implementation, not by the protocol.
    default_role: Role,
}

impl OidcSettings {
    /// Read the `auth.oidc` block, or `None` when this instance has no
    /// provider configured.
    ///
    /// The `{tenantid}` check the settings layer already applies is repeated
    /// here on purpose. `CRYSTALLINE_AUTH_OIDC_ISSUER` reaches the config
    /// through the environment overlay, which does not go through
    /// `settings::apply` for every key on every path, and an issuer template
    /// that got past the first guard would otherwise fail much later as an
    /// unreadable token. Same string, same sentence, one definition of both.
    pub fn resolve(config: &GlobalConfig) -> Option<OidcSettings> {
        let block = config.auth_oidc()?;
        let mut missing: Vec<&str> = Vec::new();
        let trimmed = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let issuer = trimmed(&block.issuer);
        let client_id = trimmed(&block.client_id);
        let client_secret = trimmed(&block.client_secret);
        if issuer.is_none() {
            missing.push("auth.oidc.issuer");
        }
        if client_id.is_none() {
            missing.push("auth.oidc.client_id");
        }
        if client_secret.is_none() {
            missing.push("auth.oidc.client_secret");
        }
        if !missing.is_empty() {
            tracing::warn!(
                "single sign-on is off: the auth.oidc block is missing {}",
                missing.join(", ")
            );
            return None;
        }
        let (issuer, client_id, client_secret) = (
            issuer.expect("checked above"),
            client_id.expect("checked above"),
            client_secret.expect("checked above"),
        );
        if issuer
            .to_ascii_lowercase()
            .contains(crate::settings::ENTRA_TEMPLATE_MARKER)
        {
            tracing::warn!(
                "single sign-on is off: {}",
                crate::settings::ENTRA_TEMPLATE_HELP
            );
            return None;
        }
        let issuer = match IssuerUrl::new(issuer) {
            Ok(issuer) => issuer,
            Err(err) => {
                tracing::warn!("single sign-on is off: auth.oidc.issuer is not a url ({err})");
                return None;
            }
        };
        let scopes = match trimmed(&block.scopes) {
            Some(raw) => raw
                .split_whitespace()
                // `openid` is added by the library on every authorization
                // request, so a configured copy of it would be sent twice.
                .filter(|scope| *scope != "openid")
                .map(|scope| Scope::new(scope.to_string()))
                .collect(),
            None => DEFAULT_SCOPES
                .iter()
                .map(|scope| Scope::new((*scope).to_string()))
                .collect(),
        };
        // Guaranteed parseable by the settings layer, which stores the
        // canonical spelling of a role it already parsed. An environment
        // variable can still carry anything, so an unreadable value falls back
        // to the least privileged role rather than refusing the whole block.
        let default_role = trimmed(&block.default_role)
            .and_then(|raw| raw.parse::<Role>().ok())
            .unwrap_or(Role::Viewer);
        Some(OidcSettings {
            issuer,
            client_id: ClientId::new(client_id),
            client_secret: ClientSecret::new(client_secret),
            name: trimmed(&block.name).unwrap_or_else(|| DEFAULT_PROVIDER_NAME.to_string()),
            scopes,
            default_role,
        })
    }

    /// The label the sign-in button carries.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The role an account provisioned through this provider is created at.
    pub fn default_role(&self) -> Role {
        self.default_role
    }
}

// The secret is a field of this struct, so the derived `Debug` would be only as
// safe as `ClientSecret`'s. Written by hand instead: nothing here prints a
// credential even if the wrapper ever changes.
impl std::fmt::Debug for OidcSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcSettings")
            .field("issuer", &self.issuer.as_str())
            .field("client_id", &self.client_id.as_str())
            .field("client_secret", &"(set)")
            .field("name", &self.name)
            .field("scopes", &self.scopes.len())
            .field("default_role", &self.default_role)
            .finish()
    }
}

// --- the http client ----------------------------------------------------

/// The one HTTP client the relying party makes its outbound calls with.
///
/// `openidconnect` ships an implementation over reqwest behind a default
/// feature, and this workspace deliberately does not enable it: that feature
/// reaches reqwest through `oauth2`'s `^0.12` requirement, and this workspace
/// is on 0.13, so switching it on would put a second reqwest and a second TLS
/// stack in the binary. The crate provides `AsyncHttpClient` for exactly this
/// case, and the request and response types on both sides are plain `http`
/// 1.x, so the adapter is a body swap.
struct OidcHttp {
    inner: reqwest::Client,
}

/// What an outbound call to the provider can fail with. Deliberately opaque in
/// its `Display`: it is rendered into a `tracing` line, never into a response.
#[derive(Debug, thiserror::Error)]
enum OidcHttpError {
    #[error("building the request to the identity provider failed: {0}")]
    Build(#[from] http::Error),
    #[error("the identity provider could not be reached: {0}")]
    Send(#[from] reqwest::Error),
}

impl OidcHttp {
    fn new() -> anyhow::Result<OidcHttp> {
        Ok(OidcHttp {
            inner: reqwest::Client::builder()
                .timeout(PROVIDER_TIMEOUT)
                // A redirect from a discovery, JWKS or token endpoint is not
                // something a relying party follows: it is the one lever that
                // turns a configured issuer into a request at an address the
                // operator never named.
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }

    async fn send(
        &self,
        request: openidconnect::HttpRequest,
    ) -> Result<openidconnect::HttpResponse, OidcHttpError> {
        let (parts, body) = request.into_parts();
        let mut outbound = self
            .inner
            .request(parts.method, parts.uri.to_string())
            .body(body);
        for (name, value) in parts.headers.iter() {
            outbound = outbound.header(name, value);
        }
        let response = outbound.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await?;
        let mut built = http::Response::builder().status(status);
        if let Some(existing) = built.headers_mut() {
            *existing = headers;
        }
        Ok(built.body(body.to_vec())?)
    }
}

impl<'c> openidconnect::AsyncHttpClient<'c> for OidcHttp {
    type Error = OidcHttpError;
    type Future =
        Pin<Box<dyn Future<Output = Result<openidconnect::HttpResponse, Self::Error>> + Send + 'c>>;

    fn call(&'c self, request: openidconnect::HttpRequest) -> Self::Future {
        Box::pin(self.send(request))
    }
}

// --- the pending record -------------------------------------------------

/// One sign-in in flight: everything the callback needs and the browser must
/// not be trusted to carry back.
///
/// The state, the nonce and the PKCE verifier live here rather than in the
/// cookie because a value the browser holds is a value the browser can be
/// talked into replacing. The cookie carries only the state, which is the
/// lookup key, so a record can only be consumed by the browser that started it
/// AND only against a state this process generated.
struct Pending {
    /// The nonce that must come back inside the ID token.
    nonce: Nonce,
    /// The PKCE verifier whose challenge went out with the authorization
    /// request.
    verifier: PkceCodeVerifier,
    /// The redirect uri that went out with the authorization request, stored
    /// rather than derived a second time: the provider compares the one in the
    /// token exchange byte for byte against the one it was given, and behind a
    /// proxy the second derivation can differ from the first.
    redirect_uri: RedirectUrl,
    /// The account this sign-in is linking an identity to, when it was started
    /// with `?link=1` from a signed-in session. `None` is an ordinary sign-in.
    /// Read by the account-linking task; carried here because the intent
    /// belongs to the request that started the flow, not to the one that
    /// finishes it.
    link_for: Option<String>,
    /// When the record was made, for the TTL and for the eviction order.
    started: Instant,
}

/// The sign-ins in flight, keyed by state.
#[derive(Default)]
struct PendingStore(HashMap<String, Pending>);

impl PendingStore {
    /// Remember `pending` under `state`, forgetting whatever has expired and,
    /// if the map is still full, the oldest record left.
    fn insert(&mut self, state: String, pending: Pending) {
        let now = Instant::now();
        self.0
            .retain(|_, p| now.duration_since(p.started) < PENDING_TTL);
        while self.0.len() >= MAX_PENDING {
            let Some(oldest) = self
                .0
                .iter()
                .min_by_key(|(_, p)| p.started)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.0.remove(&oldest);
        }
        self.0.insert(state, pending);
    }

    /// Take the record for `state`, if it exists and has not expired.
    ///
    /// Taking rather than reading is the whole point: an authorization code
    /// may be presented once, so the record that authorizes presenting it is
    /// removed before the exchange runs and a replay finds nothing.
    fn take(&mut self, state: &str) -> Option<Pending> {
        let pending = self.0.remove(state)?;
        (Instant::now().duration_since(pending.started) < PENDING_TTL).then_some(pending)
    }
}

// --- the client ---------------------------------------------------------

/// The relying party: the settings, the HTTP client, the cached provider
/// metadata and the sign-ins in flight.
///
/// Built once at daemon start when a provider is configured, and held by
/// [`RestState`]. Discovery is deliberately NOT done at start: an instance
/// whose provider is briefly unreachable must still come up, and the first
/// person to press the button pays the fetch instead. It is cached from then
/// on, refreshed only when a token fails to validate against the keys it holds.
pub struct OidcClient {
    settings: OidcSettings,
    http: OidcHttp,
    /// The discovery document plus its JSON Web Key Set, once fetched.
    metadata: RwLock<Option<CoreProviderMetadata>>,
    pending: Mutex<PendingStore>,
}

impl OidcClient {
    /// Build the relying party for `config`, or `None` when no provider is
    /// configured. Fails only when the HTTP client itself cannot be built.
    pub fn new(config: &GlobalConfig) -> anyhow::Result<Option<Arc<OidcClient>>> {
        let Some(settings) = OidcSettings::resolve(config) else {
            return Ok(None);
        };
        Ok(Some(Arc::new(OidcClient {
            settings,
            http: OidcHttp::new()?,
            metadata: RwLock::new(None),
            pending: Mutex::new(PendingStore::default()),
        })))
    }

    /// The settings this relying party was built from.
    pub fn settings(&self) -> &OidcSettings {
        &self.settings
    }

    /// The provider metadata, fetching and caching it on first use.
    async fn metadata(&self) -> Result<CoreProviderMetadata, ApiError> {
        if let Some(cached) = self.metadata.read().await.clone() {
            return Ok(cached);
        }
        let mut slot = self.metadata.write().await;
        // Another request may have won the race while this one waited.
        if let Some(cached) = slot.clone() {
            return Ok(cached);
        }
        let fetched =
            CoreProviderMetadata::discover_async(self.settings.issuer.clone(), &self.http)
                .await
                .map_err(|err| provider_failure("discovery", err))?;
        *slot = Some(fetched.clone());
        Ok(fetched)
    }

    /// Re-fetch the JSON Web Key Set and replace the cached copy's.
    ///
    /// The rotation path, and the only reason the metadata cache is ever
    /// written after the first fetch: a provider rolls its signing key, the
    /// token in hand is signed with one this process has never seen, and the
    /// verification fails on the signature alone. Called once per token, from
    /// [`OidcClient::verify_id_token`], which retries the same verification
    /// exactly once against the refreshed set.
    async fn refresh_keys(&self) -> Result<CoreProviderMetadata, ApiError> {
        let current = self.metadata().await?;
        let jwks = JsonWebKeySet::fetch_async(current.jwks_uri(), &self.http)
            .await
            .map_err(|err| provider_failure("fetching the signing keys", err))?;
        let refreshed = current.set_jwks(jwks);
        *self.metadata.write().await = Some(refreshed.clone());
        Ok(refreshed)
    }

    /// The `openidconnect` client for one request, pointed at `redirect_uri`.
    ///
    /// Rebuilt per request rather than cached with the metadata because the
    /// redirect uri is derived from the request's own `Host` and forwarded
    /// scheme: an instance reachable under two names must send each browser
    /// back to the one it came from.
    fn client(
        &self,
        metadata: CoreProviderMetadata,
        redirect_uri: RedirectUrl,
    ) -> CoreClient<
        openidconnect::EndpointSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointMaybeSet,
        openidconnect::EndpointMaybeSet,
    > {
        CoreClient::from_provider_metadata(
            metadata,
            self.settings.client_id.clone(),
            Some(self.settings.client_secret.clone()),
        )
        .set_redirect_uri(redirect_uri)
    }

    /// Validate an ID token, refreshing the signing keys once if the signature
    /// does not verify against the keys already held.
    ///
    /// Exactly one refetch per token, and only for a signature failure: a
    /// wrong issuer, a wrong audience, an expired token or a mismatched nonce
    /// are not things a new key would fix, and refetching on them would turn
    /// every rejected token into a request at the provider.
    async fn verify_id_token(
        &self,
        metadata: CoreProviderMetadata,
        redirect_uri: RedirectUrl,
        token: &openidconnect::core::CoreIdToken,
        nonce: &Nonce,
    ) -> Result<CoreIdTokenClaims, ApiError> {
        let provider = self.client(metadata, redirect_uri.clone());
        let verifier: CoreIdTokenVerifier = provider.id_token_verifier();
        let first = token.claims(&verifier, nonce);
        let signature_failed = matches!(
            first,
            Err(
                openidconnect::ClaimsVerificationError::SignatureVerification(
                    SignatureVerificationError::NoMatchingKey
                        | SignatureVerificationError::AmbiguousKeyId(_)
                        | SignatureVerificationError::CryptoError(_)
                        | SignatureVerificationError::InvalidKey(_)
                )
            )
        );
        match first {
            Ok(claims) => return Ok(claims.clone()),
            Err(err) if !signature_failed => {
                tracing::debug!("the id token did not validate: {err}");
                return Err(refused_token());
            }
            Err(err) => {
                tracing::debug!(
                    "the id token's signature did not validate, refreshing the signing keys once: {err}"
                );
            }
        }
        let refreshed = self.refresh_keys().await?;
        let provider = self.client(refreshed, redirect_uri);
        let verifier: CoreIdTokenVerifier = provider.id_token_verifier();
        match token.claims(&verifier, nonce) {
            Ok(claims) => Ok(claims.clone()),
            Err(err) => {
                tracing::debug!("the id token did not validate against the refreshed keys: {err}");
                Err(refused_token())
            }
        }
    }
}

/// The one sentence a caller ever hears about a token that did not validate.
///
/// One message for every way validation can fail - a wrong issuer, a stale
/// nonce, an unknown key, an expired token - for the reason the password login
/// gives one message for a wrong name and a wrong password: which half failed
/// is exactly what a prober is asking. The reason itself is in the `debug` log
/// beside the call.
fn refused_token() -> ApiError {
    ApiError::unauthorized(
        "the identity provider's answer could not be verified, so this sign-in was refused - \
         start again from the sign-in page",
    )
}

/// Map an outbound failure to a caller-safe sentence, keeping the provider's
/// own words for the log.
///
/// The `Display` of a `DiscoveryError` or a `RequestTokenError` can carry the
/// provider's raw response body, which for a token exchange is a response to a
/// request that carried this instance's client secret. None of it reaches the
/// browser.
fn provider_failure(what: &str, err: impl std::fmt::Display) -> ApiError {
    tracing::debug!("{what} against the identity provider failed: {err}");
    ApiError {
        status: StatusCode::BAD_GATEWAY,
        title: "Identity provider unavailable",
        detail: format!(
            "the identity provider could not be reached ({what}), so this sign-in could not \
             continue - try again, and ask an administrator to check auth.oidc.issuer if it keeps \
             happening"
        ),
        token_required: None,
    }
}

/// The refusal a route answers when no provider is configured.
///
/// A 404 rather than a 501: the three routes exist unconditionally so that the
/// answer does not depend on configuration a caller cannot see, and "there is
/// no provider here" is the honest shape of the refusal.
fn sso_is_off() -> ApiError {
    ApiError::not_found(
        "this instance has no single sign-on provider configured - sign in with a local account, \
         or ask an administrator to set auth.oidc.issuer, auth.oidc.client_id and \
         auth.oidc.client_secret",
    )
}

// --- the request's own address ------------------------------------------

/// The absolute url of `path` on this instance, as the browser reached it.
///
/// Built from the request's `Host` and whatever a proxy in front says about the
/// scheme, because a redirect uri is not a fact about the process: it is a
/// fact about how the browser got here, it has to be registered with the
/// provider in that exact spelling, and the token exchange has to repeat it
/// byte for byte. Derived once in [`login`] and stored in the pending record
/// rather than derived again in [`callback`].
fn absolute_url(headers: &HeaderMap, path: &str) -> Result<String, ApiError> {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(
                "this request carries no Host header, so the address to send the identity \
                 provider back to cannot be worked out",
            )
        })?;
    let scheme =
        if super::auth::forwarded_https(headers) || !super::auth::is_loopback_request(headers) {
            "https"
        } else {
            "http"
        };
    Ok(format!("{scheme}://{host}/api/v1{path}"))
}

// --- the routes ---------------------------------------------------------

/// The query of `GET /auth/oidc/login`.
#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LoginQuery {
    /// Link the provider identity to the caller's existing account instead of
    /// signing in as whoever it turns out to be. Needs a signed-in session.
    #[serde(default)]
    pub link: bool,
}

/// `GET /auth/oidc/login` - start a sign-in against the configured provider.
///
/// Answers 302 to the provider's authorization endpoint with PKCE (S256), a
/// server-generated state and a server-generated nonce, and sets the
/// short-lived state cookie that binds the sign-in to this browser.
#[utoipa::path(
    get,
    path = "/api/v1/auth/oidc/login",
    tag = "auth",
    operation_id = "oidc_login",
    params(LoginQuery),
    summary = "Start a single sign-on against the configured provider.",
    description = "Redirects to the provider's authorization endpoint with \
                   PKCE (S256), a server-generated state and a \
                   server-generated nonce, and sets the short-lived \
                   `fluid_oidc_state` cookie that binds the sign-in to this \
                   browser. Public, like the password login: there is no \
                   session yet. `link=true` links the provider identity to the \
                   caller's existing account and needs a signed-in session.",
    responses(
        (
            status = 302,
            description = "Follow `location` to the provider.",
            headers(
                ("location" = String, description = "The provider's authorization endpoint."),
                ("set-cookie" = String, description = "The `fluid_oidc_state` \
                 cookie, HttpOnly and SameSite=Lax."),
            ),
        ),
        (
            status = 400,
            description = "The request carries no Host header, so no redirect \
                           uri can be derived.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "`link=true` on a request with no signed-in session.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No provider is configured on this instance.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 502,
            description = "The provider's discovery document could not be fetched.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn login(
    State(state): State<RestState>,
    identity: Identity,
    jar: CookieJar,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<LoginQuery>,
) -> Result<Response, ApiError> {
    let client = state.oidc.as_ref().ok_or_else(sso_is_off)?;
    // Link intent is an authenticated act: it says "add this provider identity
    // to the account I am already signed in as", so there has to be one.
    let link_for = if query.link {
        Some(
            identity
                .require_account()
                .map_err(|_| {
                    ApiError::unauthorized(
                        "linking a single sign-on identity needs a signed-in account - sign in \
                         first, then link from your profile",
                    )
                })?
                .name,
        )
    } else {
        None
    };
    let redirect_uri = RedirectUrl::new(absolute_url(&headers, CALLBACK_PATH)?).map_err(|err| {
        tracing::debug!("the derived redirect uri is not a url: {err}");
        ApiError::bad_request(
            "the address this instance was reached at cannot be turned into a redirect uri",
        )
    })?;
    let metadata = client.metadata().await?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let provider = client.client(metadata, redirect_uri.clone());
    let mut request = provider
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_pkce_challenge(challenge);
    for scope in &client.settings.scopes {
        request = request.add_scope(scope.clone());
    }
    let (authorize_url, csrf, nonce) = request.url();
    let state_value = csrf.secret().clone();
    client.pending.lock().await.insert(
        state_value.clone(),
        Pending {
            nonce,
            verifier,
            redirect_uri,
            link_for,
            started: Instant::now(),
        },
    );
    let cookie = Cookie::build((STATE_COOKIE, state_value))
        .path("/")
        .http_only(true)
        // Lax, never Strict: the provider sends the browser back with a
        // top-level cross-site navigation, and a Strict cookie is not sent on
        // one, so the callback would find nothing to match against.
        .same_site(SameSite::Lax)
        .secure(super::auth::cookie_needs_secure(&headers))
        .max_age(time::Duration::seconds(PENDING_TTL.as_secs() as i64))
        .build();
    Ok((
        jar.add(cookie),
        super::auth::no_store(),
        found(authorize_url.as_str()),
    )
        .into_response())
}

/// The query of `GET /auth/oidc/callback`: what the provider sends back, in
/// either shape.
#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CallbackQuery {
    /// The authorization code, on success.
    pub code: Option<String>,
    /// The state this instance generated, echoed back.
    pub state: Option<String>,
    /// The provider's error code, when it refused.
    pub error: Option<String>,
    /// The provider's human-readable reason, when it refused.
    pub error_description: Option<String>,
}

/// `GET /auth/oidc/callback` - finish a sign-in the provider sent back.
///
/// Public by path for the reason the login route is: the browser arriving here
/// has no session yet, and the state is what this route authenticates against.
/// CSRF is not exempted so much as satisfied differently: the state is a
/// single-use value this process generated, stored server side and matched
/// against a cookie only this browser holds.
#[utoipa::path(
    get,
    path = "/api/v1/auth/oidc/callback",
    tag = "auth",
    operation_id = "oidc_callback",
    params(CallbackQuery),
    summary = "Finish a single sign-on the provider sent back.",
    description = "Matches the state against the `fluid_oidc_state` cookie and \
                   the server-side record, exchanges the code with the client \
                   secret and the PKCE verifier, validates the ID token \
                   (issuer, audience, expiry, signature, nonce) and signs the \
                   account in. The provider's own error text never reaches \
                   this response.",
    responses(
        (
            status = 302,
            description = "Signed in. The session cookie is set and `location` \
                           is the application root.",
            headers(
                ("location" = String, description = "`/`."),
                ("set-cookie" = String, description = "The `fluid_session` \
                 cookie, HttpOnly and SameSite=Lax."),
            ),
        ),
        (
            status = 401,
            description = "The provider refused, the state did not match, or \
                           the ID token did not validate. One message for \
                           every way this can fail.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No provider is configured on this instance.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 501,
            description = "The claims validated but this build cannot yet turn \
                           them into an account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 502,
            description = "The provider could not be reached for the token \
                           exchange.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn callback(
    State(state): State<RestState>,
    jar: CookieJar,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<CallbackQuery>,
) -> Result<Response, ApiError> {
    // Read before removing: `CookieJar::remove` takes the cookie out of this
    // jar's own view as well as sending the deletion, so the value has to be
    // in hand first.
    let bound = jar
        .get(STATE_COOKIE)
        .map(|cookie| cookie.value().to_string());
    // Whatever happens next, this browser's pending sign-in is over: the
    // deletion goes out even when the flow failed, so a stale state cannot be
    // presented twice.
    let jar = jar.remove(Cookie::build(STATE_COOKIE).path("/").build());
    let outcome = finish(&state, bound, query).await;
    let claims = match outcome {
        Ok(claims) => claims,
        Err(err) => return Ok((jar, err).into_response()),
    };
    let user = resolve_oidc_identity(&state, claims).await;
    let user = match user {
        Ok(user) => user,
        Err(err) => return Ok((jar, err).into_response()),
    };
    let jar = super::auth::issue_session(&state, jar, &headers, &user).await?;
    Ok((jar, super::auth::no_store(), found("/")).into_response())
}

/// The protocol half of [`callback`]: everything from the query the provider
/// sent to a set of validated claims.
///
/// Split out so the mechanism can be exercised without an identity resolver
/// behind it, and so the seam below is one call rather than a tail of one long
/// handler.
async fn finish(
    state: &RestState,
    bound: Option<String>,
    query: CallbackQuery,
) -> Result<OidcClaims, ApiError> {
    let client = state.oidc.as_ref().ok_or_else(sso_is_off)?;
    if let Some(error) = query.error.as_deref() {
        // The provider's words go to the log; the browser gets ours. An
        // `error_description` is attacker-influenceable text on some providers
        // and is echoed into a page nobody would have reason to distrust.
        tracing::debug!(
            "the identity provider refused the sign-in: {error} ({})",
            query.error_description.as_deref().unwrap_or("no detail")
        );
        return Err(ApiError::unauthorized(
            "the identity provider did not complete this sign-in - start again from the sign-in \
             page",
        ));
    }
    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        return Err(ApiError::unauthorized(
            "this callback did not carry an authorization code and a state, so there is no \
             sign-in to finish",
        ));
    };
    // Both halves, and in this order: the cookie proves the browser is the one
    // that started a sign-in, the record proves the state is one this process
    // generated and has not already spent.
    if bound.as_deref() != Some(returned_state.as_str()) {
        tracing::debug!("the callback's state does not match the state cookie");
        return Err(state_mismatch());
    }
    let Some(pending) = client.pending.lock().await.take(&returned_state) else {
        tracing::debug!("the callback's state names no pending sign-in");
        return Err(state_mismatch());
    };
    let metadata = client.metadata().await?;
    let response = client
        .client(metadata.clone(), pending.redirect_uri.clone())
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|err| provider_failure("the token exchange", err))?
        .set_pkce_verifier(pending.verifier)
        .request_async(&client.http)
        .await
        .map_err(|err| provider_failure("the token exchange", err))?;
    let Some(id_token) = response.extra_fields().id_token() else {
        tracing::debug!("the token response carried no id token");
        return Err(refused_token());
    };
    let claims = client
        .verify_id_token(metadata, pending.redirect_uri, id_token, &pending.nonce)
        .await?;
    Ok(OidcClaims::from_id_token(&claims, pending.link_for))
}

/// A 302 to `location`.
///
/// Spelled out rather than taken from `axum::response::Redirect`, whose `to`
/// is a 303: 302 is what the OAuth 2.0 authorization request and the
/// post-sign-in landing are specified and universally implemented as, and it
/// is what an operator debugging a provider's logs expects to see beside the
/// provider's own redirects.
fn found(location: &str) -> Response {
    (
        StatusCode::FOUND,
        [(header::LOCATION, location.to_string())],
    )
        .into_response()
}

/// One message for a state that did not match, whichever half missed.
fn state_mismatch() -> ApiError {
    ApiError::unauthorized(
        "this sign-in could not be matched to one this browser started - start again from the \
         sign-in page",
    )
}

/// What `GET /auth/providers` answers.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "Which ways into this instance exist, for the sign-in \
                        screen to draw. Public: it is read before anyone is \
                        signed in, and it carries no configuration beyond the \
                        button's label.")]
pub struct ProvidersResponse {
    /// Whether local name-and-password accounts are offered. Always true: they
    /// are the accounts single sign-on is layered on top of.
    #[schema(example = true)]
    pub local: bool,
    /// The single sign-on provider, if one is configured.
    pub oidc: OidcProviderView,
}

/// The single sign-on button's half of [`ProvidersResponse`].
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "The configured single sign-on provider as the sign-in \
                        screen needs it: whether to draw the button and what \
                        to write on it. Never the issuer, the client id or the \
                        secret.")]
pub struct OidcProviderView {
    /// Whether a provider is configured and usable.
    #[schema(example = true)]
    pub enabled: bool,
    /// The label for the button, when enabled.
    #[schema(example = "Contoso")]
    pub name: Option<String>,
}

/// `GET /auth/providers` - which ways into this instance exist.
#[utoipa::path(
    get,
    path = "/api/v1/auth/providers",
    tag = "auth",
    operation_id = "auth_providers",
    summary = "Which ways into this instance exist.",
    description = "Read by the sign-in screen before anyone is signed in, so \
                   it is public like the login route. Carries whether a single \
                   sign-on provider is configured and the label for its \
                   button, and nothing else about it: no issuer, no client id, \
                   no secret.",
    responses((status = 200, description = "The ways in.", body = ProvidersResponse)),
)]
pub async fn providers(State(state): State<RestState>) -> axum::Json<ProvidersResponse> {
    axum::Json(ProvidersResponse {
        local: true,
        oidc: OidcProviderView {
            enabled: state.oidc.is_some(),
            name: state
                .oidc
                .as_ref()
                .map(|client| client.settings.name.clone()),
        },
    })
}

// --- the identity seam --------------------------------------------------

/// The validated claims one sign-in produced, as the identity layer needs
/// them.
///
/// `(issuer, subject)` is the identity. Everything else is presentation: a
/// person's username, display name and address all change over a career, and
/// none of them may move an account. Deliberately carries no groups claim -
/// nothing in the settings surface asks for one, so nothing here pretends to
/// read one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcClaims {
    /// The provider that asserted this identity, as the ID token spells it.
    pub issuer: String,
    /// The provider's stable identifier for the person. The account key.
    pub subject: String,
    /// `preferred_username`, when the provider sends one. A naming hint for a
    /// new account, never a lookup key.
    pub preferred_username: Option<String>,
    /// `name`, the person's display name in the default locale.
    pub display: Option<String>,
    /// `email`. Presentation only: matching an address against an existing
    /// account is exactly the silent takeover this design refuses.
    pub email: Option<String>,
    /// The account this sign-in was started to link an identity to, from
    /// `?link=1`. `None` is an ordinary sign-in.
    pub link_for: Option<String>,
}

impl OidcClaims {
    /// Read the five claims this layer cares about out of a validated ID
    /// token.
    fn from_id_token(claims: &CoreIdTokenClaims, link_for: Option<String>) -> OidcClaims {
        let text = |value: &str| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        };
        OidcClaims {
            issuer: claims.issuer().as_str().to_string(),
            subject: claims.subject().as_str().to_string(),
            preferred_username: claims
                .preferred_username()
                .and_then(|value| text(value.as_str())),
            display: claims
                .name()
                // The default locale, which is the entry a provider that sends
                // one unlocalized name writes into.
                .and_then(|localized| localized.get(None))
                .and_then(|value| text(value.as_str())),
            email: claims.email().and_then(|value| text(value.as_str())),
            link_for,
        }
    }
}

/// Turn validated claims into the account this sign-in is for.
///
/// **The seam between the protocol and the accounts database, and it is not
/// filled yet.** Everything above this line is about OpenID Connect and is
/// finished; matching `(issuer, subject)` to an account, provisioning one at
/// `auth.oidc.default_role` when there is no match, and honouring
/// [`OidcClaims::link_for`] are decisions about accounts, and they land with
/// the provisioning task that owns the `identity_link` table.
///
/// Until then this refuses in the one shape a half-built feature honestly can:
/// the sign-in got all the way through validation, and the instance cannot
/// finish it. The signature is what the next task replaces the body of, so
/// nothing above has to move.
async fn resolve_oidc_identity(_state: &RestState, claims: OidcClaims) -> Result<User, ApiError> {
    tracing::debug!(
        "an oidc sign-in validated for subject '{}' at issuer '{}' with no identity resolver to \
         hand it to",
        claims.subject,
        claims.issuer
    );
    Err(ApiError {
        status: StatusCode::NOT_IMPLEMENTED,
        title: "Sign-in cannot be completed",
        detail: "this build validates a single sign-on but cannot yet turn it into an account"
            .to_string(),
        token_required: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crystalline_core::config::{AuthConfig, OidcConfig};

    /// A config carrying `oidc`, spelled the way a caller would set it.
    fn config_with(oidc: OidcConfig) -> GlobalConfig {
        GlobalConfig {
            auth: Some(AuthConfig {
                trusted_header: None,
                anonymous: None,
                mcp: None,
                max_users: None,
                oidc: Some(oidc),
            }),
            ..GlobalConfig::default()
        }
    }

    /// A complete block, the shape every test below varies one field of.
    fn complete() -> OidcConfig {
        OidcConfig {
            issuer: Some("https://idp.example/realm".to_string()),
            client_id: Some("crystalline".to_string()),
            client_secret: Some("s3cr3t".to_string()),
            name: None,
            scopes: None,
            default_role: None,
        }
    }

    /// The three keys the protocol cannot run without, one at a time: a block
    /// missing any of them leaves single sign-on off rather than failing the
    /// daemon's start.
    #[test]
    fn an_incomplete_block_leaves_sso_off_rather_than_failing_startup() {
        for blank in ["issuer", "client_id", "client_secret"] {
            let mut oidc = complete();
            match blank {
                "issuer" => oidc.issuer = None,
                "client_id" => oidc.client_id = None,
                _ => oidc.client_secret = None,
            }
            assert!(
                OidcSettings::resolve(&config_with(oidc)).is_none(),
                "a block without {blank} must not resolve"
            );
        }
        assert!(OidcSettings::resolve(&GlobalConfig::default()).is_none());
        assert!(OidcSettings::resolve(&config_with(complete())).is_some());
    }

    /// The settings layer refuses the tenant-independent Entra template, and
    /// so does this one: an environment variable reaches the config without
    /// passing the first guard.
    #[test]
    fn the_entra_issuer_template_is_refused_here_too() {
        let mut oidc = complete();
        oidc.issuer = Some("https://login.microsoftonline.com/{tenantid}/v2.0".to_string());
        assert!(OidcSettings::resolve(&config_with(oidc)).is_none());
        let mut cased = complete();
        cased.issuer = Some("https://login.microsoftonline.com/{tenantId}/v2.0".to_string());
        assert!(OidcSettings::resolve(&config_with(cased)).is_none());
        let mut real = complete();
        real.issuer = Some(
            "https://login.microsoftonline.com/00000000-1111-2222-3333-444444444444/v2.0"
                .to_string(),
        );
        assert!(OidcSettings::resolve(&config_with(real)).is_some());
    }

    /// An issuer that is not a url is the same class of mistake as a missing
    /// one, and gets the same answer.
    #[test]
    fn an_unparseable_issuer_leaves_sso_off() {
        let mut oidc = complete();
        oidc.issuer = Some("not a url".to_string());
        assert!(OidcSettings::resolve(&config_with(oidc)).is_none());
    }

    /// The defaults the settings layer promises: `viewer`, the generic label,
    /// and `profile email` beside the `openid` the library always sends.
    #[test]
    fn the_unset_defaults_are_viewer_the_generic_name_and_profile_email() {
        let resolved = OidcSettings::resolve(&config_with(complete())).unwrap();
        assert_eq!(resolved.default_role(), Role::Viewer);
        assert_eq!(resolved.name(), DEFAULT_PROVIDER_NAME);
        let scopes: Vec<&str> = resolved.scopes.iter().map(|s| s.as_str()).collect();
        assert_eq!(scopes, vec!["profile", "email"]);
    }

    /// A configured scope list is taken as written, minus `openid`, which the
    /// library adds to every authorization request and which would otherwise
    /// go out twice.
    #[test]
    fn configured_scopes_drop_a_redundant_openid() {
        let mut oidc = complete();
        oidc.scopes = Some("openid profile email groups".to_string());
        let resolved = OidcSettings::resolve(&config_with(oidc)).unwrap();
        let scopes: Vec<&str> = resolved.scopes.iter().map(|s| s.as_str()).collect();
        assert_eq!(scopes, vec!["profile", "email", "groups"]);
    }

    /// The role the settings layer stores is honoured; a value only an
    /// environment variable could carry falls back to the least privileged
    /// role rather than turning the whole block off.
    #[test]
    fn the_default_role_is_read_and_an_unreadable_one_falls_back_to_viewer() {
        let mut oidc = complete();
        oidc.default_role = Some("editor".to_string());
        assert_eq!(
            OidcSettings::resolve(&config_with(oidc))
                .unwrap()
                .default_role(),
            Role::Editor
        );
        let mut broken = complete();
        broken.default_role = Some("wizard".to_string());
        assert_eq!(
            OidcSettings::resolve(&config_with(broken))
                .unwrap()
                .default_role(),
            Role::Viewer
        );
    }

    /// The secret is a field of the settings struct, so the struct's own
    /// `Debug` is one of the ways it could escape into a log line.
    #[test]
    fn debugging_the_settings_never_prints_the_secret() {
        let resolved = OidcSettings::resolve(&config_with(complete())).unwrap();
        let rendered = format!("{resolved:?}");
        assert!(!rendered.contains("s3cr3t"), "{rendered}");
        assert!(rendered.contains("(set)"), "{rendered}");
    }

    /// The redirect uri is derived from the request, not from the process: a
    /// forwarded https proxy and a plain loopback development server must
    /// produce different absolute urls for the same route.
    #[test]
    fn the_redirect_uri_follows_the_host_and_the_forwarded_scheme() {
        let mut plain = HeaderMap::new();
        plain.insert(header::HOST, "127.0.0.1:7411".parse().unwrap());
        assert_eq!(
            absolute_url(&plain, CALLBACK_PATH).unwrap(),
            "http://127.0.0.1:7411/api/v1/auth/oidc/callback"
        );
        let mut proxied = HeaderMap::new();
        proxied.insert(header::HOST, "knowledge.example".parse().unwrap());
        proxied.insert("x-forwarded-proto", "https".parse().unwrap());
        assert_eq!(
            absolute_url(&proxied, CALLBACK_PATH).unwrap(),
            "https://knowledge.example/api/v1/auth/oidc/callback"
        );
        // A non-loopback Host with no forwarded scheme is still https: the
        // cookie rule and the redirect uri agree on what counts as public.
        let mut bare = HeaderMap::new();
        bare.insert(header::HOST, "knowledge.example".parse().unwrap());
        assert_eq!(
            absolute_url(&bare, CALLBACK_PATH).unwrap(),
            "https://knowledge.example/api/v1/auth/oidc/callback"
        );
        assert!(absolute_url(&HeaderMap::new(), CALLBACK_PATH).is_err());
    }

    /// The five fields the identity layer reads, mapped off a token the way a
    /// provider actually spells them.
    ///
    /// This is the shape the provisioning task consumes, so it is pinned here
    /// rather than left implied by the one integration test that can see it:
    /// `sub` is the key, the three presentation claims come through as
    /// written, and a blank one is absent rather than empty.
    #[test]
    fn the_claims_map_onto_what_the_identity_layer_reads() {
        let raw = serde_json::json!({
            "iss": "https://idp.example/realm",
            "sub": "sub-1",
            "aud": ["crystalline"],
            "exp": 4102444800u64,
            "iat": 1767225600u64,
            "preferred_username": "Ada.Lovelace",
            "name": "Ada Lovelace",
            "email": "ada@example.test",
        });
        let claims: CoreIdTokenClaims =
            serde_json::from_value(raw).expect("the fixture is a set of id token claims");
        let mapped = OidcClaims::from_id_token(&claims, None);
        assert_eq!(
            mapped,
            OidcClaims {
                issuer: "https://idp.example/realm".to_string(),
                subject: "sub-1".to_string(),
                preferred_username: Some("Ada.Lovelace".to_string()),
                display: Some("Ada Lovelace".to_string()),
                email: Some("ada@example.test".to_string()),
                link_for: None,
            }
        );

        // A provider that sends only the required claims, and one that sends a
        // blank string where a value would go: both leave the presentation
        // fields absent rather than empty, so the resolver never has to tell
        // "" apart from missing.
        let sparse = serde_json::json!({
            "iss": "https://idp.example/realm",
            "sub": "sub-2",
            "aud": ["crystalline"],
            "exp": 4102444800u64,
            "iat": 1767225600u64,
            "preferred_username": "   ",
        });
        let claims: CoreIdTokenClaims = serde_json::from_value(sparse).unwrap();
        let mapped = OidcClaims::from_id_token(&claims, Some("ada".to_string()));
        assert_eq!(mapped.subject, "sub-2");
        assert_eq!(mapped.preferred_username, None);
        assert_eq!(mapped.display, None);
        assert_eq!(mapped.email, None);
        assert_eq!(
            mapped.link_for,
            Some("ada".to_string()),
            "link intent belongs to the request that started the flow"
        );
    }

    /// The pending map is something an unauthenticated caller can add to, so
    /// its two bounds are the ones worth pinning: a record is single use, and
    /// the map has a ceiling.
    #[test]
    fn a_pending_record_is_single_use_and_the_map_is_capped() {
        let mut store = PendingStore::default();
        let record = |name: &str| Pending {
            nonce: Nonce::new(format!("nonce-{name}")),
            verifier: PkceCodeVerifier::new("v".repeat(43)),
            redirect_uri: RedirectUrl::new("https://example.test/cb".to_string()).unwrap(),
            link_for: None,
            started: Instant::now(),
        };
        store.insert("first".to_string(), record("first"));
        assert!(store.take("first").is_some());
        assert!(
            store.take("first").is_none(),
            "a state may authorize exactly one code exchange"
        );
        for i in 0..(MAX_PENDING * 2) {
            store.insert(format!("state-{i}"), record("bulk"));
        }
        assert!(
            store.0.len() <= MAX_PENDING,
            "the pending map grew past its cap: {}",
            store.0.len()
        );
    }
}
