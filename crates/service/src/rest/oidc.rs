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
//! Three things a reader may go looking for a setting for, and will not find:
//!
//! - **No clock-skew leeway on `exp`.** `openidconnect` 4.0.1 checks the expiry
//!   against the current instant with no leeway knob and leaves `iat`
//!   unchecked; the only override is replacing its clock, which is a worse
//!   trade than asking an operator to run NTP.
//! - **One sign-in per browser at a time.** A second `/login` in the same
//!   browser overwrites the state cookie, so the first tab's callback answers
//!   the generic state mismatch. Correct and safe; the person starts again.
//! - **The provider's endpoints are trusted as far as the issuer is.** The
//!   token endpoint and the key set url come from the discovery document and
//!   are fetched wherever it says; a compromised provider could aim those at
//!   an internal address. Outbound redirects are refused and bodies are
//!   capped, which closes the cheap half; the rest is the trust the protocol
//!   places in the operator's choice of issuer.
//!
//! **What an operator sees.** The daemon's subscriber is capped at `info`, so
//! every refusal on these routes is a `warn!` line naming the reason category
//! and the issuer - never a state, a code, a nonce, a token, the secret or a
//! claim value - and a validated sign-in is one `info!` line naming the issuer
//! and the subject. The provider's own words, which can quote its response
//! body, go to `debug!` and are only seen in a build that raises the cap.
//!
//! **Presentation claims may come from userinfo.** A provider that keeps
//! `preferred_username`, `name` and `email` out of the ID token (Authelia
//! 4.38+ without a claims policy) has the missing ones fetched from its
//! userinfo endpoint with the access token, after the ID token validated and
//! only for the fields it lacked. Userinfo's `sub` must equal the token's or
//! the sign-in is refused; userinfo never supplies the issuer or the subject,
//! and never overrides a claim the token carried. An unreachable userinfo
//! costs the sign-in those fields, not the sign-in.
//!
//! **Who the claims are is decided in one place.** The callback ends at
//! [`resolve_oidc_identity`], which matches the `(issuer, subject)` pair to an
//! account and provisions one when there is no match. Keeping that behind a
//! single function is what keeps the rest of this file about the protocol, and
//! it is why "an address never reaches an account" is a property of one
//! function rather than a habit spread over a handler.

use std::collections::{HashMap, VecDeque};
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
    CoreProviderMetadata, CoreUserInfoClaims,
};
use openidconnect::{
    AccessToken, AuthorizationCode, ClaimsVerificationError, ClientId, ClientSecret, CsrfToken,
    IssuerUrl, JsonWebKeySet, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, SignatureVerificationError, SubjectIdentifier, UserInfoError, http,
};
use tokio::sync::{Mutex, RwLock};

use super::auth::Identity;
use super::auth_store::{DEFAULT_OIDC_ROLE, RefusalKind, Role, StoreRefusal, User};
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
/// this process reserve memory. A record is a few hundred bytes, so a full map
/// is single-digit megabytes; the number is generous next to any real
/// instance's concurrent logins and small next to what a flood would want.
const MAX_PENDING: usize = 10_000;

/// How old a record must be before a full map may drop it to make room.
///
/// Without this, filling the map is a way to evict every real sign-in mid
/// consent: the cap would be reached and the oldest record - somebody who
/// pressed the button a few seconds ago - would go. With it, a full map of
/// records younger than this refuses the newcomer instead, which costs a
/// flood its own next request and costs the person mid-consent nothing.
const MIN_EVICT_AGE: Duration = Duration::from_secs(30);

/// How long a failed discovery is remembered before the provider is asked
/// again.
///
/// Every discovery is an outbound request that can take the whole
/// [`PROVIDER_TIMEOUT`] to fail, started from a public route. Without a
/// negative cache a dark provider is probed once per sign-in, which makes N
/// strangers' GETs N ten-second requests aimed at it; with one, the sign-ins
/// inside the window are refused from memory with the same sentence.
const DISCOVERY_FAILURE_TTL: Duration = Duration::from_secs(5);

/// The most a provider's answer to any single outbound call may weigh.
///
/// A discovery document is a few kilobytes and a key set a few more; a
/// provider that sends a megabyte is not one whose answer this process should
/// buffer. The provider is operator-configured and so trusted, and the timeout
/// bounds the read in practice, but the bound is one comparison to make
/// explicit.
const MAX_PROVIDER_BODY: usize = 1024 * 1024;

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
    /// The callback address to send out verbatim, from
    /// `auth.oidc.redirect_uri`. `None` means derive it from each request,
    /// which is what an instance the browser reaches directly wants.
    redirect_uri: Option<RedirectUrl>,
}

impl OidcSettings {
    /// Read the `auth.oidc` block, or `None` when this instance has no
    /// provider configured.
    ///
    /// The Entra checks the settings layer already applies are repeated here
    /// on purpose. `CRYSTALLINE_AUTH_OIDC_ISSUER` reaches the config through
    /// the environment overlay, which does not go through `settings::apply`
    /// for every key on every path, and a template or a tenant-independent
    /// endpoint that got past the first guard would otherwise fail much later
    /// as an unreadable token. One function, called from both places.
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
        if let Some(help) = crate::settings::entra_issuer_problem(&issuer) {
            tracing::warn!("single sign-on is off: {help}");
            return None;
        }
        let issuer = match IssuerUrl::new(issuer) {
            Ok(issuer) => issuer,
            Err(err) => {
                tracing::warn!("single sign-on is off: auth.oidc.issuer is not a url ({err})");
                return None;
            }
        };
        if issuer_is_plaintext_off_loopback(&issuer) {
            tracing::warn!(
                "auth.oidc.issuer is served over plain http off loopback: the client secret and \
                 every ID token cross the network in clear text - use an https issuer"
            );
        }
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
        // to the default rather than refusing the whole block. That default is
        // named once, in `auth_store`, and the settings registry renders the
        // same constant as the key's unset value.
        let default_role = trimmed(&block.default_role)
            .and_then(|raw| raw.parse::<Role>().ok())
            .unwrap_or(DEFAULT_OIDC_ROLE);
        // A configured callback address is checked here as well as at the
        // settings layer, for the reason the Entra guard is: an environment
        // variable reaches the config without passing through `settings::apply`.
        // A provider whose callback address cannot work is one that must not be
        // offered at all - a sign-in that fails at the provider's own error page
        // teaches nobody anything, and this refusal names the key.
        let redirect_uri = match trimmed(&block.redirect_uri) {
            Some(raw) => {
                if let Some(problem) = crate::settings::oidc_redirect_uri_problem(&raw) {
                    tracing::warn!("single sign-on is off: {problem}");
                    return None;
                }
                match RedirectUrl::new(raw) {
                    Ok(url) => Some(url),
                    Err(err) => {
                        tracing::warn!(
                            "single sign-on is off: auth.oidc.redirect_uri is not a url ({err})"
                        );
                        return None;
                    }
                }
            }
            None => None,
        };
        Some(OidcSettings {
            issuer,
            client_id: ClientId::new(client_id),
            client_secret: ClientSecret::new(client_secret),
            name: trimmed(&block.name).unwrap_or_else(|| DEFAULT_PROVIDER_NAME.to_string()),
            scopes,
            default_role,
            redirect_uri,
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

    /// The address to send the provider back to, for a request that arrived
    /// with `headers`.
    ///
    /// The configured value wins verbatim when there is one: a deployment
    /// whose proxy rewrites the `Host`, or terminates a different public name
    /// in front of this process, has no way to derive the address it
    /// registered with the provider, and the token exchange has to repeat that
    /// address byte for byte. With the key unset the address is derived from
    /// the request, which is right wherever the browser reached this instance
    /// at the address the request says it did.
    pub fn redirect_uri(&self, headers: &HeaderMap) -> Result<RedirectUrl, ApiError> {
        if let Some(configured) = &self.redirect_uri {
            return Ok(configured.clone());
        }
        let derived = absolute_url(headers, CALLBACK_PATH).inspect_err(|_| {
            // The 400 an operator debugging a proxy most wants to see in the
            // log - and the moment to remember there is a key that fixes it.
            refused(
                "redirect uri could not be derived from the Host header: set \
                 auth.oidc.redirect_uri when a proxy rewrites it",
                &self.issuer,
            );
        })?;
        RedirectUrl::new(derived).map_err(|err| {
            tracing::debug!("the derived redirect uri is not a url: {err}");
            refused("derived redirect uri is not a url", &self.issuer);
            ApiError::bad_request(
                "the address this instance was reached at cannot be turned into a redirect uri",
            )
        })
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

/// Whether `issuer` would carry the client secret and every ID token in clear
/// text: an `http://` url whose host is not a loopback address.
///
/// Loopback is allowed without comment because a development setup and this
/// crate's own tests depend on it; anything else over plain http is a
/// configuration worth a warning, not a refusal.
fn issuer_is_plaintext_off_loopback(issuer: &IssuerUrl) -> bool {
    let url = issuer.url();
    if url.scheme() == "https" {
        return false;
    }
    match url.host() {
        Some(openidconnect::url::Host::Domain(domain)) => !domain.eq_ignore_ascii_case("localhost"),
        Some(openidconnect::url::Host::Ipv4(ip)) => !ip.is_loopback(),
        Some(openidconnect::url::Host::Ipv6(ip)) => !ip.is_loopback(),
        None => true,
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
    #[error("the identity provider's answer was larger than {MAX_PROVIDER_BODY} bytes")]
    TooLarge,
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
        let mut response = outbound.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > MAX_PROVIDER_BODY {
                return Err(OidcHttpError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let mut built = http::Response::builder().status(status);
        if let Some(existing) = built.headers_mut() {
            *existing = headers;
        }
        Ok(built.body(body)?)
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
    /// through `POST /auth/oidc/login`. `None` is an ordinary sign-in.
    /// Read by the account-linking task; carried here because the intent
    /// belongs to the request that started the flow, not to the one that
    /// finishes it.
    link_for: Option<String>,
    /// Where to send the browser once this sign-in completes, when the page
    /// that started it asked for somewhere in particular. `None` is the
    /// application root.
    ///
    /// Held here rather than in a cookie or in the url for the same reason
    /// everything else in this record is: a value the browser carries is a
    /// value the browser can be talked into replacing, and this one decides
    /// where a freshly signed-in person is sent. It has already been through
    /// [`safe_return_path`] before it gets here.
    return_to: Option<String>,
    /// When the record was made, for the TTL and for the eviction order.
    started: Instant,
}

/// The sign-ins in flight, keyed by state.
#[derive(Default)]
struct PendingStore {
    records: HashMap<String, Pending>,
    /// Every state inserted, oldest first. Records are only ever inserted
    /// with a fresh `started`, so this is chronological, and expiry and
    /// eviction both walk it from the front in O(1) per record. An entry
    /// whose record was already taken is skipped when it is reached.
    order: VecDeque<(Instant, String)>,
}

/// The map is full of sign-ins too young to evict. See [`MIN_EVICT_AGE`].
#[derive(Debug)]
struct PendingFull;

/// Why a state had no record to take: it names none, or it named one that
/// outlived [`PENDING_TTL`]. Both answer the same sentence; they warn
/// differently, because an operator reading "expired" checks the provider's
/// consent screen and one reading "unknown" checks for a replay.
#[derive(Debug, PartialEq, Eq)]
enum StateMiss {
    Unknown,
    Expired,
}

impl PendingStore {
    /// How many sign-ins are in flight.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.records.len()
    }

    /// How many entries the chronological order holds, live or spent.
    #[cfg(test)]
    fn order_len(&self) -> usize {
        self.order.len()
    }

    /// Remember `pending` under `state`, forgetting whatever has expired and,
    /// if the map is still full, the oldest records past [`MIN_EVICT_AGE`].
    /// Refuses when the map is full of records younger than that: a flood
    /// must not be able to push a real sign-in out from under someone.
    fn insert(&mut self, state: String, pending: Pending) -> Result<(), PendingFull> {
        let now = Instant::now();
        self.purge_expired(now);
        while self.records.len() >= MAX_PENDING {
            let Some((started, key)) = self.order.front() else {
                break;
            };
            let already_taken = !self.records.contains_key(key);
            if !already_taken && now.duration_since(*started) < MIN_EVICT_AGE {
                return Err(PendingFull);
            }
            let (_, key) = self.order.pop_front().expect("checked just above");
            self.records.remove(&key);
        }
        self.order.push_back((pending.started, state.clone()));
        self.records.insert(state, pending);
        Ok(())
    }

    /// Take the record for `state`, if it exists and has not expired.
    ///
    /// Taking rather than reading is the whole point: an authorization code
    /// may be presented once, so the record that authorizes presenting it is
    /// removed before the exchange runs and a replay finds nothing. Expired
    /// records are purged here too, so an idle instance does not hold them
    /// until the next login happens to insert.
    fn take(&mut self, state: &str) -> Result<Pending, StateMiss> {
        let now = Instant::now();
        // Looked up before the purge, so an expired record is reported as
        // expired rather than as never having existed.
        let taken = self.records.remove(state);
        self.purge_expired(now);
        match taken {
            Some(pending) if now.duration_since(pending.started) < PENDING_TTL => Ok(pending),
            Some(_) => Err(StateMiss::Expired),
            None => Err(StateMiss::Unknown),
        }
    }

    /// Forget every record older than [`PENDING_TTL`], from the front of the
    /// chronological order until the first one that is still live, and drop
    /// the spent entries met on the way.
    ///
    /// The spent ones matter for the bound: a sign-in started and taken at
    /// once (a login, then a callback whose exchange fails) never fills the
    /// map, so the eviction loop never runs, and without this the order would
    /// keep its entry for the full TTL at the cost of two cheap requests.
    /// Completions are roughly first in, first out, so popping spent entries
    /// from the front keeps the order close to the map.
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

// --- discovery ------------------------------------------------------------

/// Why discovery did not produce usable metadata, in the two shapes a caller
/// is told apart by. `Clone` because one result is handed to every sign-in
/// that shared the fetch.
#[derive(Clone, Debug)]
enum DiscoveryFailure {
    /// The document was fetched and its `issuer` is the Entra `{tenantid}`
    /// template: the operator configured a tenant-independent endpoint, and
    /// the provider was reached and said so.
    TenantIndependentIssuer,
    /// Everything else - unreachable, a non-2xx, malformed, a plain issuer
    /// mismatch.
    Unavailable,
}

impl DiscoveryFailure {
    fn classify(err: &openidconnect::DiscoveryError<OidcHttpError>) -> DiscoveryFailure {
        if let openidconnect::DiscoveryError::Validation(message) = err
            && message
                .to_ascii_lowercase()
                .contains(crate::settings::ENTRA_TEMPLATE_MARKER)
        {
            return DiscoveryFailure::TenantIndependentIssuer;
        }
        DiscoveryFailure::Unavailable
    }

    fn into_error(self, issuer: &IssuerUrl) -> ApiError {
        let reason = match self {
            DiscoveryFailure::TenantIndependentIssuer => {
                "discovery names a tenant-independent issuer"
            }
            DiscoveryFailure::Unavailable => "discovery failed",
        };
        refused(reason, issuer);
        match self {
            DiscoveryFailure::TenantIndependentIssuer => ApiError {
                status: StatusCode::BAD_GATEWAY,
                title: "Identity provider misconfigured",
                detail: format!(
                    "the identity provider answered discovery with a tenant-independent issuer, so \
                     auth.oidc.issuer is not a tenant's issuer: {}",
                    crate::settings::ENTRA_TEMPLATE_HELP
                ),
                token_required: None,
            },
            DiscoveryFailure::Unavailable => provider_unavailable("discovery"),
        }
    }
}

/// The value a shared discovery publishes: `None` while in flight, then the
/// result every waiter reads.
type Flight = Option<Result<CoreProviderMetadata, DiscoveryFailure>>;

/// The discovery bookkeeping beside the metadata cache: the one fetch in
/// flight, if any, and the last failure for the negative cache. Behind a
/// `std` mutex that is only ever held for a few instructions and never across
/// an await.
#[derive(Default)]
struct Discovery {
    in_flight: Option<tokio::sync::watch::Receiver<Flight>>,
    last_failure: Option<(Instant, DiscoveryFailure)>,
}

/// Clears the in-flight marker if the leading fetch is dropped before it
/// finishes - a browser that gave up mid-discovery takes its handler's future
/// with it - so the followers, whose `changed()` fails when the sender goes,
/// find the way clear to lead the next attempt instead of a marker nobody
/// will ever clear.
struct FlightGuard<'a> {
    discovery: &'a std::sync::Mutex<Discovery>,
    finished: bool,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let mut discovery = self
                .discovery
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            discovery.in_flight = None;
        }
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
    /// The fetch in flight and the last failure. See [`Discovery`].
    discovery: std::sync::Mutex<Discovery>,
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
            discovery: std::sync::Mutex::new(Discovery::default()),
            pending: Mutex::new(PendingStore::default()),
        })))
    }

    /// The settings this relying party was built from.
    pub fn settings(&self) -> &OidcSettings {
        &self.settings
    }

    /// The provider metadata, fetching and caching it on first use.
    ///
    /// The fetch is shared and unlocked. Shared: concurrent first sign-ins
    /// elect one leader, which fetches and publishes the result on a watch
    /// channel every follower awaits, so N sign-ins arriving at a cold cache
    /// are one outbound request rather than N. Unlocked: the leader holds no
    /// lock across the network call, so a provider that has gone dark cannot
    /// turn the followers into a queue of ten-second waits; they all wake on
    /// the one result. A failure is remembered for [`DISCOVERY_FAILURE_TTL`]
    /// and answered from memory inside that window, so a dark provider is
    /// probed once per window rather than once per stranger's GET.
    async fn metadata(&self) -> Result<CoreProviderMetadata, ApiError> {
        // A follower whose leader vanished mid-fetch takes the lead itself.
        // Bounded so a pathological run of vanishing leaders ends in an
        // answer rather than a loop.
        for _ in 0..4 {
            if let Some(cached) = self.metadata.read().await.clone() {
                return Ok(cached);
            }
            let role = {
                let mut discovery = self
                    .discovery
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(receiver) = &discovery.in_flight {
                    Err(receiver.clone())
                } else if let Some((at, failure)) = &discovery.last_failure
                    && at.elapsed() < DISCOVERY_FAILURE_TTL
                {
                    return Err(failure.clone().into_error(&self.settings.issuer));
                } else {
                    let (sender, receiver) = tokio::sync::watch::channel(None);
                    discovery.in_flight = Some(receiver);
                    Ok(sender)
                }
            };
            match role {
                Err(mut receiver) => loop {
                    if let Some(result) = receiver.borrow_and_update().clone() {
                        return result.map_err(|failure| failure.into_error(&self.settings.issuer));
                    }
                    if receiver.changed().await.is_err() {
                        // The leader is gone without publishing. Go round
                        // again; the guard has cleared the marker.
                        break;
                    }
                },
                Ok(sender) => {
                    let mut guard = FlightGuard {
                        discovery: &self.discovery,
                        finished: false,
                    };
                    let result = CoreProviderMetadata::discover_async(
                        self.settings.issuer.clone(),
                        &self.http,
                    )
                    .await
                    .map_err(|err| {
                        tracing::debug!("discovery against the identity provider failed: {err}");
                        DiscoveryFailure::classify(&err)
                    });
                    // The cache is written before the marker is cleared, so
                    // a sign-in arriving in between finds the document rather
                    // than an empty cache and a clear road to a second fetch.
                    if let Ok(fetched) = &result {
                        let mut slot = self.metadata.write().await;
                        if slot.is_none() {
                            *slot = Some(fetched.clone());
                        }
                    }
                    {
                        let mut discovery = self
                            .discovery
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        discovery.in_flight = None;
                        discovery.last_failure = result
                            .as_ref()
                            .err()
                            .map(|failure| (Instant::now(), failure.clone()));
                    }
                    guard.finished = true;
                    let _ = sender.send(Some(result.clone()));
                    return result.map_err(|failure| failure.into_error(&self.settings.issuer));
                }
            }
        }
        refused("discovery kept being abandoned", &self.settings.issuer);
        Err(provider_unavailable("discovery"))
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
            .map_err(|err| {
                provider_failure("fetching the signing keys", &self.settings.issuer, err)
            })?;
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
                return Err(refused_token(
                    token_refusal_reason(&err),
                    &self.settings.issuer,
                ));
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
                Err(refused_token(
                    token_refusal_reason(&err),
                    &self.settings.issuer,
                ))
            }
        }
    }

    /// Fill the presentation claims the ID token lacked from the provider's
    /// userinfo endpoint. See the module doc for the rules; in short, only the
    /// missing fields, never the identity, and a `sub` that is not the
    /// token's refuses the sign-in.
    ///
    /// An unreachable or unusable userinfo is a `warn!` and an `Ok(())`: the
    /// identity is the validated token's, which is already in hand, and the
    /// fields stay `None` for the resolver to cope with.
    async fn complete_from_userinfo(
        &self,
        metadata: CoreProviderMetadata,
        redirect_uri: RedirectUrl,
        access_token: AccessToken,
        claims: &mut OidcClaims,
    ) -> Result<(), ApiError> {
        let issuer = &self.settings.issuer;
        let provider = self.client(metadata, redirect_uri);
        let expected = SubjectIdentifier::new(claims.subject.clone());
        let request = match provider.user_info(access_token, Some(expected.clone())) {
            Ok(request) => request,
            Err(err) => {
                tracing::warn!(
                    "single sign-on at issuer {} carried a minimal id token and the provider \
                     publishes no userinfo endpoint, so the presentation claims stay absent ({err})",
                    issuer.as_str()
                );
                return Ok(());
            }
        };
        let info: CoreUserInfoClaims = match request.request_async(&self.http).await {
            Ok(info) => info,
            Err(UserInfoError::ClaimsVerification(err)) => {
                // The library already compared `sub` against `expected`; this
                // is the token substitution the check exists for.
                tracing::debug!("userinfo did not verify: {err}");
                return Err(refused_token("userinfo subject mismatch", issuer));
            }
            Err(err) => {
                tracing::debug!("userinfo against the identity provider failed: {err}");
                tracing::warn!(
                    "single sign-on at issuer {} could not complete its claims from userinfo, so \
                     the presentation claims stay absent (userinfo unavailable)",
                    issuer.as_str()
                );
                return Ok(());
            }
        };
        // Belt and braces beside the library's own comparison: the subject is
        // the one thing userinfo is never allowed to change.
        if info.subject() != &expected {
            return Err(refused_token("userinfo subject mismatch", issuer));
        }
        let filled = claims.fill_missing_from_userinfo(&info);
        if filled.is_empty() {
            tracing::debug!("userinfo carried none of the missing presentation claims");
        } else {
            tracing::debug!("filled {} from userinfo", filled.join(", "));
        }
        Ok(())
    }
}

/// The category a `warn!` line names for a token that did not validate: the
/// check that failed, never its inputs.
fn token_refusal_reason(err: &ClaimsVerificationError) -> &'static str {
    match err {
        ClaimsVerificationError::SignatureVerification(_) => "id token signature",
        ClaimsVerificationError::InvalidIssuer(_) => "id token issuer",
        ClaimsVerificationError::InvalidAudience(_) => "id token audience",
        ClaimsVerificationError::Expired(_) => "id token expired",
        ClaimsVerificationError::InvalidNonce(_) => "id token nonce",
        ClaimsVerificationError::InvalidSubject(_) => "id token subject",
        _ => "id token validation",
    }
}

/// The one `warn!` shape for every refusal on these routes: the reason
/// category and the issuer, nothing that came from the flow itself.
fn refused(reason: &str, issuer: &IssuerUrl) {
    tracing::warn!(
        "single sign-on refused ({reason}) for issuer {}",
        issuer.as_str()
    );
}

/// The one sentence a caller ever hears about a token that did not validate.
///
/// One message for every way validation can fail - a wrong issuer, a stale
/// nonce, an unknown key, an expired token - for the reason the password login
/// gives one message for a wrong name and a wrong password: which half failed
/// is exactly what a prober is asking. The reason category goes to the
/// operator's log at `warn`, the provider's own words at `debug`.
fn refused_token(reason: &str, issuer: &IssuerUrl) -> ApiError {
    refused(reason, issuer);
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
fn provider_failure(what: &str, issuer: &IssuerUrl, err: impl std::fmt::Display) -> ApiError {
    tracing::debug!("{what} against the identity provider failed: {err}");
    refused(&format!("{what} failed"), issuer);
    provider_unavailable(what)
}

/// The sentence for a provider that did not answer `what` usably, with
/// nothing to log. The negative cache answers with this too, so a refusal
/// from memory reads the same as the one it remembers.
fn provider_unavailable(what: &str) -> ApiError {
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
///
/// The origin half is [`super::auth::request_origin`], shared with the OAuth
/// resource identifier: this instance has one public address, and two rules for
/// deriving it would be two addresses the day they disagreed.
fn absolute_url(headers: &HeaderMap, path: &str) -> Result<String, ApiError> {
    Ok(format!(
        "{}/api/v1{path}",
        super::auth::request_origin(headers)?
    ))
}

/// Whether `host` is a bare host and optional port: a name of letters, digits,
/// dots and hyphens, or a dotted address, or a bracketed IPv6 address, and
/// then nothing but `:` and up to five digits. Deliberately conservative -
/// no underscores, no percent-encoding, nothing a url parser would read as
/// anything but a host.
///
/// `pub(super)` because [`super::auth::request_origin`] applies it: the whole
/// point of the check is that one rule decides what may be interpolated into
/// this instance's own address, and both callers build one.
pub(super) fn host_is_well_formed(host: &str) -> bool {
    let (name, port) = match host.strip_prefix('[') {
        Some(rest) => {
            let Some((address, tail)) = rest.split_once(']') else {
                return false;
            };
            let address_ok = !address.is_empty()
                && address
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
            if !address_ok {
                return false;
            }
            match tail {
                "" => (None, None),
                tail => match tail.strip_prefix(':') {
                    Some(port) => (None, Some(port)),
                    None => return false,
                },
            }
        }
        None => match host.rsplit_once(':') {
            Some((name, port)) => (Some(name), Some(port)),
            None => (Some(host), None),
        },
    };
    if let Some(name) = name
        && (name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
    {
        return false;
    }
    match port {
        None => true,
        Some(port) => {
            !port.is_empty() && port.len() <= 5 && port.chars().all(|c| c.is_ascii_digit())
        }
    }
}

// --- the routes ---------------------------------------------------------

/// The query of `GET /auth/oidc/login`.
#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LoginQuery {
    /// Recognized so that a request meaning to link is told where linking
    /// lives, rather than quietly started as an ordinary sign-in. Starting a
    /// link is `POST /auth/oidc/login`; see [`start_link`] for why it cannot
    /// be a GET.
    #[serde(default)]
    pub link: bool,
    /// Where to send the browser once the sign-in completes: a path on this
    /// instance, which the callback 302s to instead of `/`.
    ///
    /// The OAuth consent page is what this exists for. A client sends a
    /// browser to `/authorize?request=<id>`, Fluid finds nobody signed in and
    /// carries the intended location to the login page, and a provider sign-in
    /// has to come back to that exact request: the pending authorization is
    /// only reachable by its id, so landing anywhere else loses it.
    ///
    /// Anything [`safe_return_path`] does not accept is dropped rather than
    /// refused, and the sign-in lands on `/`: the person did sign in, and only
    /// the destination was unusable.
    pub return_to: Option<String>,
}

/// How long a `return_to` may be. Generous for a path with a query on it, and
/// far short of what would make the pending record a place to store somebody
/// else's data.
const MAX_RETURN_PATH: usize = 512;

/// `path` if it names somewhere on this instance, `None` if it names anything
/// else.
///
/// A `return_to` is a value a stranger puts in a link and sends to somebody, so
/// the only question worth asking is whether it can leave this origin. It
/// cannot if it is a path - and the rules are what stop a url from looking like
/// one:
///
/// - **Starts with `/`, and not with `//`.** `//evil.test/steal` is a
///   scheme-relative url, not a path: a browser sent there leaves this
///   instance entirely, which is the whole attack.
/// - **No backslash anywhere.** Several browsers read `\` as `/` in a url, so
///   `/\evil.test` reaches the same place `//evil.test` does while passing a
///   naive "starts with one slash" check.
/// - **Printable ASCII, no whitespace or control characters.** The value goes
///   into a `Location` header, so it has to be a valid header value; a CR or LF
///   in one is a response-splitting attempt, and this is the check that ends
///   it. Same rule, and the same reason, as [`super::oauth::redirect_uri_problem`].
/// - **At most [`MAX_RETURN_PATH`] characters.**
pub(crate) fn safe_return_path(path: &str) -> Option<String> {
    if !path.starts_with('/') || path.starts_with("//") {
        return None;
    }
    if path.len() > MAX_RETURN_PATH {
        return None;
    }
    if path.contains('\\') {
        return None;
    }
    if !path.is_ascii() || path.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    Some(path.to_string())
}

/// What `POST /auth/oidc/login` answers with: where to send the browser.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "Where to send the browser to link a provider \
                        identity to the caller's account. Navigate the whole \
                        page to it: what follows is a redirect to the \
                        provider and a redirect back, so a background fetch \
                        would land nowhere anybody can authenticate.")]
pub struct StartLinkResponse {
    /// The provider's authorization endpoint, with PKCE, state and nonce.
    #[schema(example = "https://idp.example/authorize?client_id=...")]
    pub location: String,
}

/// Everything one sign-on start produces: where to send the browser, and the
/// cookie that binds the journey to it.
///
/// One body for the two routes below, so the ordinary sign-in and the link
/// differ in exactly one thing - whether an account is recorded as the one
/// this journey is for - rather than in two copies of a protocol dance.
struct StartedSignOn {
    authorize_url: String,
    cookie: Cookie<'static>,
}

/// `GET /auth/oidc/login` - start a sign-in against the configured provider.
///
/// Answers 302 to the provider's authorization endpoint with PKCE (S256), a
/// server-generated state and a server-generated nonce, and sets the
/// short-lived state cookie that binds the sign-in to this browser.
///
/// `return_to` says where the callback should land the browser afterwards. It
/// is checked by [`safe_return_path`] here, before the record exists, and
/// dropped rather than refused when it names anywhere but this instance.
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
                   session yet. Linking a provider identity to an existing \
                   account is the POST on this same path, not a flag here: a \
                   GET carrying `link=true` is refused with a 400 that says \
                   so, because a GET could be sent by another origin. \
                   `return_to` names where the callback should land the \
                   browser once the sign-in completes - the OAuth consent \
                   page is what it exists for. It must be a path on this \
                   instance (starts with `/`, not `//`, no backslash, at most \
                   512 printable ASCII characters); anything else is dropped \
                   and the sign-in lands on `/`.",
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
            description = "`link=true`, which is the POST's job; or a request \
                           with no Host header, or one that is not a bare host \
                           and port, so no redirect uri can be derived.",
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
            description = "The provider's discovery document could not be fetched, \
                           or it names a tenant-independent issuer.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 503,
            description = "Too many sign-ins are in flight to start another. \
                           Wait a moment and try again.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn login(
    State(state): State<RestState>,
    jar: CookieJar,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<LoginQuery>,
) -> Result<Response, ApiError> {
    let client = state.oidc.as_ref().ok_or_else(sso_is_off)?;
    if query.link {
        // Recognized and refused rather than honored: see `start_link`. A GET
        // that started a link would be startable by any other origin, since
        // the session cookie is SameSite=Lax and rides a top-level navigation.
        refused("link intent on a GET", &client.settings.issuer);
        return Err(ApiError::bad_request(
            "a link is started with POST /auth/oidc/login, which needs the session's CSRF \
             token - this GET starts an ordinary sign-in and will not link anything",
        ));
    }
    let return_to = query.return_to.as_deref().and_then(safe_return_path);
    let started = start_sign_on(&state, &headers, None, return_to).await?;
    Ok((
        jar.add(started.cookie),
        super::auth::no_store(),
        found(&started.authorize_url),
    )
        .into_response())
}

/// `POST /auth/oidc/login` - start a sign-on that LINKS the provider identity
/// to the account this request is made by.
///
/// A POST, and that is the security property rather than a style choice. The
/// session cookie is `SameSite=Lax`, which rides a top-level cross-site
/// navigation, so a GET that started a link could be started by any page on
/// the internet: it would plant a state cookie in a signed-in browser and bind
/// a pending link to that account, and a browser then authenticated at the
/// provider as somebody else would finish it. An unsafe method cannot be sent
/// cross-site with the cookie at all, and the guard's one CSRF rule covers it
/// on top of that, in every identity mode - which matters most in the one
/// where `SameSite` protects nothing, because the identity rides a header a
/// proxy sets.
///
/// The answer is a body rather than a redirect. The only client that can send
/// the CSRF header is a script, and a script cannot read where a redirect went
/// (a followed cross-origin redirect is a CORS failure, a manual one is
/// opaque), so a 303 here would hand the browser somewhere it could not learn.
/// The caller navigates the whole page to `location` itself.
#[utoipa::path(
    post,
    path = "/api/v1/auth/oidc/login",
    tag = "auth",
    operation_id = "oidc_start_link",
    summary = "Start a single sign-on that links its identity to this account.",
    description = "Starts the same authorization-code flow the GET does, and \
                   records that it is for the calling account: the callback \
                   ties the provider identity to that account rather than \
                   signing in as whoever it turns out to be. Needs a \
                   signed-in account and the session's CSRF token, because \
                   starting a link is an unsafe act - a GET would be \
                   startable by another origin. Answers the authorization \
                   endpoint in `location`; navigate the whole page to it. \
                   Served on a read-only instance: an identity link is \
                   account state rather than knowledge.",
    responses(
        (
            status = 200,
            description = "Navigate to `location`.",
            body = StartLinkResponse,
            headers(
                ("set-cookie" = String, description = "The `fluid_oidc_state` \
                 cookie, HttpOnly and SameSite=Lax."),
            ),
        ),
        (
            status = 400,
            description = "The request carries no Host header, or one that is \
                           not a bare host and port, so no redirect uri can be \
                           derived.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No signed-in account to link to.",
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
            description = "No provider is configured on this instance.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 502,
            description = "The provider's discovery document could not be fetched, \
                           or it names a tenant-independent issuer.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 503,
            description = "Too many sign-ins are in flight to start another. \
                           Wait a moment and try again.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn start_link(
    State(state): State<RestState>,
    identity: Identity,
    jar: CookieJar,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // Before the provider is even consulted, and in this order: an instance
    // with no provider must still answer an unauthenticated caller the way
    // every other account-bearing route does.
    let account = identity.require_account().map_err(|_| {
        // Logged without the issuer, which is a field of a provider this
        // instance may not even have configured: the account check comes
        // first so that an unauthenticated caller is told to log in rather
        // than told which providers exist here.
        tracing::warn!("single sign-on refused (link start without a signed-in account)");
        ApiError::unauthorized(
            "linking a single sign-on identity needs a signed-in account - sign in first, then \
             link from your profile",
        )
    })?;
    // An instance with no provider is the 404 `start_sign_on` answers on its
    // own first line; the account check above it is what keeps that answer
    // from telling an unauthenticated caller anything.
    // No `return_to`: a link is started by a script that is already on the
    // page it wants to stay on, and answers a location in a body rather than
    // a redirect. The one caller that needs the landing carried is the GET.
    let started = start_sign_on(&state, &headers, Some(account.name), None).await?;
    Ok((
        jar.add(started.cookie),
        super::auth::no_store(),
        axum::Json(StartLinkResponse {
            location: started.authorize_url,
        }),
    )
        .into_response())
}

/// The authorization-code dance both routes above run: derive the redirect
/// uri, build the authorize url with PKCE, a state and a nonce, remember the
/// pending journey and hand back the cookie that binds it to this browser.
///
/// `link_for` is the only difference between an ordinary sign-in and a link,
/// and it is carried in the pending record rather than in the url, so nothing
/// a browser or a provider can rewrite decides which of the two this is.
/// `return_to` rides there for the same reason, and has already been through
/// [`safe_return_path`].
async fn start_sign_on(
    state: &RestState,
    headers: &HeaderMap,
    link_for: Option<String>,
    return_to: Option<String>,
) -> Result<StartedSignOn, ApiError> {
    let client = state.oidc.as_ref().ok_or_else(sso_is_off)?;
    let redirect_uri = client.settings.redirect_uri(headers)?;
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
    let remembered = client.pending.lock().await.insert(
        state_value.clone(),
        Pending {
            nonce,
            verifier,
            redirect_uri,
            link_for,
            return_to,
            started: Instant::now(),
        },
    );
    if remembered.is_err() {
        refused("too many sign-ins in flight", &client.settings.issuer);
        return Err(ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            title: "Too many sign-ins in flight",
            detail: "this instance has too many single sign-ons in flight to start another - \
                     wait a moment and try again"
                .to_string(),
            token_required: None,
        });
    }
    let cookie = Cookie::build((STATE_COOKIE, state_value))
        .path("/")
        .http_only(true)
        // Lax, never Strict: the provider sends the browser back with a
        // top-level cross-site navigation, and a Strict cookie is not sent on
        // one, so the callback would find nothing to match against.
        .same_site(SameSite::Lax)
        .secure(super::auth::cookie_needs_secure(headers))
        .max_age(time::Duration::seconds(PENDING_TTL.as_secs() as i64))
        .build();
    Ok(StartedSignOn {
        authorize_url: authorize_url.to_string(),
        cookie,
    })
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
                   account in. The account is the one linked to the token's \
                   `(issuer, sub)` pair, or a fresh one provisioned at \
                   `auth.oidc.default_role`; a matching address never reaches \
                   an existing account. A sign-on started by `POST \
                   /auth/oidc/login` instead ties the identity to the account \
                   that started it, which has to be the account finishing it \
                   too. The provider's own error text never reaches this \
                   response.",
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
            description = "The provider refused, the state did not match, the \
                           ID token did not validate, or a link was finished \
                           after its session was signed out. One message for \
                           every way the protocol can fail.",
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
            status = 403,
            description = "The identity names a disabled account, or a new \
                           account would pass `auth.max_users`.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The sign-in was started to link an identity and \
                           could not be: it was finished on another account's \
                           session, the identity belongs to another account \
                           (which is never named), or this account already \
                           holds one at that provider. Nothing was linked and \
                           no account was created.",
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
    identity: Identity,
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
    let (claims, return_to) = match outcome {
        Ok(finished) => finished,
        Err(err) => return Ok((jar, super::auth::no_store(), err).into_response()),
    };
    let user = resolve_oidc_identity(&state, claims, &identity).await;
    let user = match user {
        Ok(user) => user,
        Err(err) => return Ok((jar, super::auth::no_store(), err).into_response()),
    };
    let jar = super::auth::issue_session(&state, jar, &headers, &user).await?;
    // Where the page that started this asked to land, or the application root.
    // The value came out of the pending record rather than off this request,
    // so nothing the provider or the browser sent decides it.
    let landing = return_to.as_deref().unwrap_or("/");
    Ok((jar, super::auth::no_store(), found(landing)).into_response())
}

/// The protocol half of [`callback`]: everything from the query the provider
/// sent to a set of validated claims, plus where the page that started this
/// asked to land.
///
/// Split out so the mechanism can be exercised without an identity resolver
/// behind it, and so the seam below is one call rather than a tail of one long
/// handler. The landing rides out of here rather than being read off the
/// request because the record is the only place it was ever trusted.
async fn finish(
    state: &RestState,
    bound: Option<String>,
    query: CallbackQuery,
) -> Result<(OidcClaims, Option<String>), ApiError> {
    let client = state.oidc.as_ref().ok_or_else(sso_is_off)?;
    let issuer = &client.settings.issuer;
    if let Some(error) = query.error.as_deref() {
        // The provider's words go to the debug log; the browser gets ours. An
        // `error_description` is attacker-influenceable text on some providers
        // and is echoed into a page nobody would have reason to distrust.
        tracing::debug!(
            "the identity provider refused the sign-in: {error} ({})",
            query.error_description.as_deref().unwrap_or("no detail")
        );
        refused("the provider refused the sign-in", issuer);
        return Err(ApiError::unauthorized(
            "the identity provider did not complete this sign-in - start again from the sign-in \
             page",
        ));
    }
    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        refused("callback without a code and a state", issuer);
        return Err(ApiError::unauthorized(
            "this callback did not carry an authorization code and a state, so there is no \
             sign-in to finish",
        ));
    };
    // Both halves, and in this order: the cookie proves the browser is the one
    // that started a sign-in, the record proves the state is one this process
    // generated and has not already spent.
    let cookie_matches = bound.as_deref().is_some_and(|bound| {
        super::auth::constant_time_eq(bound.as_bytes(), returned_state.as_bytes())
    });
    if !cookie_matches {
        return Err(state_mismatch(
            "state does not match the browser's cookie",
            issuer,
        ));
    }
    let pending = match client.pending.lock().await.take(&returned_state) {
        Ok(pending) => pending,
        Err(StateMiss::Unknown) => {
            return Err(state_mismatch("state names no pending sign-in", issuer));
        }
        Err(StateMiss::Expired) => {
            return Err(state_mismatch("pending sign-in expired", issuer));
        }
    };
    let metadata = client.metadata().await?;
    let response = client
        .client(metadata.clone(), pending.redirect_uri.clone())
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|err| provider_failure("the token exchange", issuer, err))?
        .set_pkce_verifier(pending.verifier)
        .request_async(&client.http)
        .await
        .map_err(|err| provider_failure("the token exchange", issuer, err))?;
    let Some(id_token) = response.extra_fields().id_token() else {
        return Err(refused_token("no id token in the token response", issuer));
    };
    let verified = client
        .verify_id_token(
            metadata.clone(),
            pending.redirect_uri.clone(),
            id_token,
            &pending.nonce,
        )
        .await?;
    let return_to = pending.return_to;
    let mut claims = OidcClaims::from_id_token(&verified, pending.link_for);
    if claims.lacks_presentation_claims() {
        client
            .complete_from_userinfo(
                metadata,
                pending.redirect_uri,
                response.access_token().clone(),
                &mut claims,
            )
            .await?;
    }
    tracing::info!(
        "single sign-on validated for subject '{}' at issuer '{}'",
        claims.subject,
        claims.issuer
    );
    Ok((claims, return_to))
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
fn state_mismatch(reason: &str, issuer: &IssuerUrl) -> ApiError {
    refused(reason, issuer);
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
    /// `POST /auth/oidc/login`. `None` is an ordinary sign-in.
    pub link_for: Option<String>,
}

impl OidcClaims {
    /// Read the five claims this layer cares about out of a validated ID
    /// token.
    fn from_id_token(claims: &CoreIdTokenClaims, link_for: Option<String>) -> OidcClaims {
        let text = |value: &str| presentation_text(value);
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

    /// Whether any of the three presentation claims is absent, which is when
    /// userinfo is worth asking.
    fn lacks_presentation_claims(&self) -> bool {
        self.preferred_username.is_none() || self.display.is_none() || self.email.is_none()
    }

    /// Fill the presentation claims that are absent from `info`, and only
    /// those: a claim the ID token carried is never replaced, and the issuer
    /// and subject are not read at all. Returns the names of the fields
    /// filled, for the log.
    ///
    /// Userinfo is the second boundary a provider's strings enter through, so
    /// it reads them through the same [`presentation_text`] the ID token's
    /// claims do. A provider that keeps its presentation claims out of the
    /// token is not a provider whose strings are trusted any further.
    fn fill_missing_from_userinfo(&mut self, info: &CoreUserInfoClaims) -> Vec<&'static str> {
        let text = |value: &str| presentation_text(value);
        let mut filled = Vec::new();
        if self.preferred_username.is_none()
            && let Some(value) = info
                .preferred_username()
                .and_then(|value| text(value.as_str()))
        {
            self.preferred_username = Some(value);
            filled.push("preferred_username");
        }
        if self.display.is_none()
            && let Some(value) = info
                .name()
                .and_then(|localized| localized.get(None))
                .and_then(|value| text(value.as_str()))
        {
            self.display = Some(value);
            filled.push("name");
        }
        if self.email.is_none()
            && let Some(value) = info.email().and_then(|value| text(value.as_str()))
        {
            self.email = Some(value);
            filled.push("email");
        }
        filled
    }
}

/// One presentation claim as this layer will store it: trimmed, stripped of
/// the characters that are not text, and `None` when nothing is left.
///
/// Blank becomes absent so the resolver never has to tell `""` apart from a
/// claim the provider did not send. The stripping is the other half: a display
/// name and a derived login name both end up in the account list an operator
/// makes privilege decisions in, and a right-to-left override or a zero width
/// joiner in one of them renders as a name that is not the name that was
/// stored. A provider is trusted to assert who somebody is, not to write
/// direction changes into an admin's table. Every other character the provider
/// sends survives, including every script: this removes control and formatting
/// codepoints, never letters.
///
/// `pub(super)` because the OAuth registration endpoint holds text of exactly
/// the same kind: a client's own name, chosen by whoever registered it and
/// shown to the person deciding whether to trust it. One rule for both, rather
/// than two spellings of "what may be shown to a person" that could drift.
pub(super) fn presentation_text(value: &str) -> Option<String> {
    let cleaned: String = value
        .chars()
        .filter(|ch| {
            // Cc, plus the Cf ranges that reorder or hide what follows them.
            !ch.is_control()
                && !matches!(
                    ch,
                    '\u{00ad}'
                        | '\u{200b}'..='\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2060}'..='\u{206f}'
                        | '\u{feff}'
                )
        })
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The account name a provisioning falls back to when a provider sends
/// nothing usable to derive one from. Uniquified like any other name, so a
/// second such person becomes `sso-user-2`.
pub(super) const FALLBACK_ACCOUNT_NAME: &str = "sso-user";

/// How long a derived name may be before it is cut. Long enough for a full
/// `firstname.lastname`, short enough that a provider cannot make this
/// instance's user list unreadable. The uniquifying suffix is added after the
/// cut, so a name can end up a few characters longer.
const MAX_DERIVED_NAME: usize = 60;

/// The login name a first sign-in provisions, derived from what the provider
/// sent.
///
/// `preferred_username` first, the address's local part second, a generic name
/// last. Every candidate is sanitized and the first one that survives wins, so
/// a provider that sends `preferred_username: "!!!"` falls through to the
/// address rather than provisioning a name made of punctuation.
///
/// The result is a *hint*: the store folds it again and uniquifies it against
/// the names already taken. Nothing about this function is a lookup key - it
/// decides what a new account is called, never which account a sign-in lands
/// in.
fn derive_account_name(claims: &OidcClaims) -> String {
    let local_part = claims
        .email
        .as_deref()
        .and_then(|address| address.split('@').next());
    [claims.preferred_username.as_deref(), local_part]
        .into_iter()
        .flatten()
        .find_map(sanitize_account_name)
        .unwrap_or_else(|| FALLBACK_ACCOUNT_NAME.to_string())
}

/// Fold one candidate into something that can be a login name, or `None` when
/// nothing usable is left.
///
/// Lowercased (the store folds names anyway, so this only makes the derivation
/// visible), alphanumerics and `.`, `-`, `_` kept, everything else - spaces,
/// `@`, quotes, control characters - replaced by a single `-`. Runs collapse
/// and the ends are trimmed, so `"Ada Lovelace (Contoso)"` becomes
/// `ada-lovelace-contoso` rather than something with edges.
pub(super) fn sanitize_account_name(raw: &str) -> Option<String> {
    let mut out = String::new();
    for ch in raw.trim().to_lowercase().chars() {
        if ch.is_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
        if out.chars().count() >= MAX_DERIVED_NAME {
            break;
        }
    }
    let trimmed = out.trim_matches(|ch| ch == '-' || ch == '.' || ch == '_');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// What a sign-in into a disabled account is told, in the words the rest of
/// this surface uses for a disabled account.
fn account_is_disabled() -> ApiError {
    ApiError::forbidden("this account is disabled")
}

/// Turn validated claims into the account this sign-in is for.
///
/// The seam between the protocol and the accounts database. Three cases, and
/// the order they are in is the policy:
///
/// 1. A sign-in started to LINK an identity to an account (the POST on the
///    login path, which needs a session) ties the pair to the account that
///    started it
///    and signs that account in. It never provisions and never moves an
///    identity: see [`link_the_started_account`] for the three ways it refuses.
/// 2. A `(issuer, subject)` pair this instance has seen signs into the account
///    it is linked to. Its role, its login name and its disabled state are
///    untouched: the provider asserts who somebody is, never what they may do
///    here. Only the display name and the address refresh, because those are
///    the provider's to restate.
/// 3. Anything else is a person this instance has never seen, and gets a fresh
///    account at `auth.oidc.default_role` with a derived name and a stored
///    link.
///
/// What is deliberately absent is a fourth case. A matching address or a
/// matching username is NOT a match: an account is reached by an identity link
/// or not at all, so a provider that will hand anybody an `email` claim cannot
/// hand anybody somebody else's account.
///
/// `jar` is the callback request's own cookies, and case 1 is the only reader:
/// linking is an act by a signed-in person, so the session that finishes it has
/// to be there and has to be the one that started it.
async fn resolve_oidc_identity(
    state: &RestState,
    claims: OidcClaims,
    identity: &Identity,
) -> Result<User, ApiError> {
    if let Some(intent) = claims.link_for.clone() {
        return link_the_started_account(state, &claims, &intent, identity).await;
    }
    let settings = &state.oidc.as_ref().ok_or_else(sso_is_off)?.settings;
    if let Some(user) = state
        .auth
        .linked_user(&claims.issuer, &claims.subject)
        .await?
    {
        if user.disabled {
            return Err(account_is_disabled());
        }
        // Presentation only, and only what was actually sent: an ID token that
        // carries no `name` this time is not a person who lost their name.
        return Ok(state
            .auth
            .refresh_presentation(
                &user.name,
                claims.display.as_deref(),
                claims.email.as_deref(),
            )
            .await?);
    }
    let derived = derive_account_name(&claims);
    match state
        .auth
        .provision_linked_user(
            &claims.issuer,
            &claims.subject,
            &derived,
            claims.display.as_deref(),
            claims.email.as_deref(),
            settings.default_role(),
            state.auth_cfg.max_users,
        )
        .await
    {
        Ok(user) => Ok(user),
        Err(err) => {
            // Two first sign-ins for one subject can race: both miss the
            // lookup, one provisions and the other's link is refused. The
            // loser reads the winner's account rather than answering an error
            // for a sign-in that did work.
            if let Some(user) = state
                .auth
                .linked_user(&claims.issuer, &claims.subject)
                .await?
            {
                if user.disabled {
                    return Err(account_is_disabled());
                }
                return Ok(user);
            }
            let message = format!("{err:#}");
            if message.contains("auth.max_users") {
                // The caller cannot fix this and the operator can, so the
                // words that name the setting are the useful ones. Same
                // treatment as the trusted-header path's cap refusal.
                Err(ApiError::forbidden(message))
            } else {
                Err(ApiError::internal(message))
            }
        }
    }
}

/// Case 1 of [`resolve_oidc_identity`]: tie this identity to the account whose
/// session started the sign-in, and sign that account in.
///
/// The rule this whole design is built around is that linking is an explicit
/// act by the person who owns the account, so the account that started the
/// link has to be the account that finishes it. `link_for` is written into the
/// pending record by `POST /auth/oidc/login`, which refuses without a session;
/// this checks the other end of the same journey, against the identity the
/// callback actually arrives with:
///
/// * no live account on the callback - signed out, expired, or revoked while
///   the browser was away at the provider - is a 401. There is nobody to link
///   to, and signing in again is exactly what fixes it.
/// * a live session naming a DIFFERENT account is a 409. The one thing that
///   must never happen is an identity landing in whichever account happened to
///   be signed in when the browser came back.
/// * a pair already linked to some other account is a 409 that does not say
///   which. Naming it would turn any sign-on into a probe for who holds an
///   identity here. An admin can move it (`crystalline users unlink` then
///   `crystalline users link`), which is what the message points at.
///
/// A pair already linked to THIS account is not a refusal at all: somebody
/// pressed the button twice, and the honest answer is the sign-in they asked
/// for.
///
/// Who the caller is comes from [`Identity`] rather than from the session
/// cookie, which is the same question [`start_link`] asks at the other end of
/// the journey. That matters on an instance whose identities arrive in a
/// trusted header: reading the cookie here would let such a browser start a
/// link it could never finish, since it may hold no session cookie at all. The
/// guard has already resolved the identity for this route (the callback is
/// public by path, not unauthenticated by nature), so this costs no store
/// lookup either.
async fn link_the_started_account(
    state: &RestState,
    claims: &OidcClaims,
    intent: &str,
    identity: &Identity,
) -> Result<User, ApiError> {
    // A session the store has forgotten, one that expired, one revoked while
    // the browser was away at the provider, a disabled account and the
    // anonymous viewer all arrive here the same way: with no account behind
    // the request.
    let Ok(user) = identity.require_account() else {
        tracing::debug!("a link callback arrived on no live session");
        return Err(ApiError::unauthorized(
            "the session that started this link is no longer signed in - sign in again, then \
             start the link from your profile",
        ));
    };
    if user.name != intent {
        tracing::debug!("a link callback arrived on another account's session");
        return Err(ApiError::conflict(
            "this link was started from another account's session - start it again from the \
             profile of the account you want to link",
        ));
    }
    match state
        .auth
        .linked_user(&claims.issuer, &claims.subject)
        .await?
    {
        Some(holder) if holder.name == user.name => return Ok(user),
        Some(_) => return Err(identity_is_taken()),
        None => {}
    }
    match state
        .auth
        .link_identity(&claims.issuer, &claims.subject, &user.name, &user.name)
        .await
    {
        Ok(()) => Ok(user),
        // Both refusals are classified rather than read as prose, and neither
        // forwards the store's own words: one of them names the account that
        // holds the identity, which is precisely what this surface must not
        // say. The first is the race the read above cannot close - two link
        // flows for one pair, finishing at once.
        Err(err) => match StoreRefusal::kind_of(&err) {
            Some(RefusalKind::IdentityAlreadyLinked) => Err(identity_is_taken()),
            Some(RefusalKind::IssuerAlreadyHeld) => Err(ApiError::conflict(
                "this account already holds an identity at that provider - unlink it from your \
                 profile first, then link this one",
            )),
            _ => Err(ApiError::internal(format!("{err:#}"))),
        },
    }
}

/// What a caller is told when the identity they signed on with belongs to
/// somebody else's account. Deliberately the same sentence whoever asks, and
/// deliberately without a name in it.
fn identity_is_taken() -> ApiError {
    ApiError::conflict(
        "this SSO identity is already linked to another account - an admin can move it",
    )
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
                proxy_headers: None,
                anonymous: None,
                mcp: None,
                oauth: None,
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
            redirect_uri: None,
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

    /// The configured callback address is taken verbatim, which is the whole
    /// point of the key: a deployment whose proxy rewrites the Host, or serves
    /// this instance under a path a request cannot see, has no way to derive
    /// the address it registered with the provider.
    #[test]
    fn a_configured_redirect_uri_is_used_verbatim() {
        let configured = "https://kb.example.test/api/v1/auth/oidc/callback";
        let mut oidc = complete();
        oidc.redirect_uri = Some(configured.to_string());
        let resolved = OidcSettings::resolve(&config_with(oidc)).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "internal.svc:8787".parse().unwrap());
        assert_eq!(
            resolved.redirect_uri(&headers).unwrap().as_str(),
            configured,
            "the request's own Host is not consulted when the key is set"
        );
    }

    /// With the key unset the address is derived from the request, which is
    /// what every deployment that needs no override keeps doing.
    #[test]
    fn an_unset_redirect_uri_still_follows_the_request() {
        let resolved = OidcSettings::resolve(&config_with(complete())).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "knowledge.example".parse().unwrap());
        assert_eq!(
            resolved.redirect_uri(&headers).unwrap().as_str(),
            "https://knowledge.example/api/v1/auth/oidc/callback"
        );
    }

    /// The settings layer validates this key, and so does this one: an
    /// environment variable reaches the config without passing the first
    /// guard, and a callback address that cannot work is a provider that must
    /// not be offered at all rather than a sign-in that fails halfway.
    #[test]
    fn an_unusable_redirect_uri_leaves_sso_off() {
        for bad in [
            "http://kb.example.test/api/v1/auth/oidc/callback",
            "https://kb.example.test/somewhere-else",
            "not a url",
        ] {
            let mut oidc = complete();
            oidc.redirect_uri = Some(bad.to_string());
            assert!(
                OidcSettings::resolve(&config_with(oidc)).is_none(),
                "{bad} must not resolve into a working provider"
            );
        }
    }

    /// The path the settings layer validates a configured callback address
    /// against is the path this router serves it at. Two constants, one fact:
    /// moving the route without moving the guard would accept an address the
    /// browser never reaches.
    #[test]
    fn the_validated_callback_path_is_the_one_this_router_serves() {
        assert_eq!(
            crate::settings::OIDC_CALLBACK_PATH,
            format!("/api/v1{CALLBACK_PATH}")
        );
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

    /// A `return_to` is somewhere on this instance or it is nothing.
    ///
    /// The value arrives in a link, so the question is only ever whether a
    /// browser sent there leaves this origin. Each refused spelling below is a
    /// different way of writing a url that a "starts with a slash" check on
    /// its own would wave through.
    #[test]
    fn a_return_path_is_same_origin_or_nothing() {
        for accepted in [
            "/",
            "/authorize?request=9f2c1d7e4b6a80351c8e0d2f4a6b8c1e",
            "/domains/eng/engrams/alpha",
            "/search?q=a%20b&tag=x",
            "/a#section",
        ] {
            assert_eq!(
                safe_return_path(accepted).as_deref(),
                Some(accepted),
                "'{accepted}' is a path on this instance"
            );
        }

        for refused in [
            // Not a path at all.
            "",
            "authorize",
            "https://evil.test/steal",
            "http://evil.test",
            "javascript:alert(1)",
            // A url wearing a path's clothes: scheme-relative, and the
            // backslash spellings browsers read as slashes.
            "//evil.test/steal",
            "//evil.test",
            "/\\evil.test",
            "\\\\evil.test",
            "/authorize\\..\\..",
            // Not a header value: a newline here is response splitting, and a
            // non-ASCII byte is not one either.
            "/authorize\r\nSet-Cookie: a=b",
            "/authorize\nx",
            "/authorize\tx",
            "/authorize with a space",
            "/caf\u{e9}",
            "/\u{0}",
        ] {
            assert_eq!(
                safe_return_path(refused),
                None,
                "'{refused}' must not be returned to"
            );
        }

        // The length bound, on both sides of it.
        let longest = format!("/{}", "a".repeat(MAX_RETURN_PATH - 1));
        assert_eq!(
            safe_return_path(&longest).as_deref(),
            Some(longest.as_str())
        );
        assert_eq!(safe_return_path(&format!("{longest}a")), None);
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

    /// Userinfo fills exactly the presentation claims the ID token lacked. A
    /// claim the token carried stays, whatever userinfo says about it, and
    /// the issuer and subject are not read from it at all.
    #[test]
    fn userinfo_fills_only_what_the_id_token_lacked() {
        let info = CoreUserInfoClaims::from_json::<std::convert::Infallible>(
            br#"{"sub":"sub-1","preferred_username":"ada.lovelace","name":"Ada Lovelace","email":"ada@example.test","iss":"https://somewhere.else"}"#,
            None,
        )
        .expect("a userinfo document");
        let mut claims = OidcClaims {
            issuer: "https://idp.example/realm".to_string(),
            subject: "sub-1".to_string(),
            preferred_username: None,
            display: Some("Ada L".to_string()),
            email: None,
            link_for: None,
        };
        assert!(claims.lacks_presentation_claims());
        let filled = claims.fill_missing_from_userinfo(&info);
        assert_eq!(filled, vec!["preferred_username", "email"]);
        assert_eq!(claims.preferred_username.as_deref(), Some("ada.lovelace"));
        assert_eq!(
            claims.display.as_deref(),
            Some("Ada L"),
            "the token's name is not replaced by userinfo's"
        );
        assert_eq!(claims.email.as_deref(), Some("ada@example.test"));
        assert_eq!(claims.issuer, "https://idp.example/realm");
        assert_eq!(claims.subject, "sub-1");
        assert!(!claims.lacks_presentation_claims());

        // A userinfo with nothing useful fills nothing and says so.
        let sparse = CoreUserInfoClaims::from_json::<std::convert::Infallible>(
            br#"{"sub":"sub-2","preferred_username":"   "}"#,
            None,
        )
        .unwrap();
        let mut claims = OidcClaims {
            issuer: "i".to_string(),
            subject: "sub-2".to_string(),
            preferred_username: None,
            display: None,
            email: None,
            link_for: None,
        };
        assert!(claims.fill_missing_from_userinfo(&sparse).is_empty());
        assert!(claims.lacks_presentation_claims());
    }

    /// Why a take missed, without asking `Pending` - which holds the PKCE
    /// verifier and the nonce - to implement `Debug` for the sake of a test.
    fn miss(taken: Result<Pending, StateMiss>) -> StateMiss {
        match taken {
            Ok(_) => panic!("expected the take to miss"),
            Err(miss) => miss,
        }
    }

    /// The pending map is something an unauthenticated caller can add to, so
    /// its bounds are the ones worth pinning: a record is single use, the map
    /// has a ceiling, and the ceiling cannot be used to push a real sign-in
    /// out from under someone mid-consent.
    #[test]
    fn a_flood_of_sign_ins_cannot_evict_one_that_started_moments_ago() {
        let mut store = PendingStore::default();
        let record = |age: Duration| Pending {
            nonce: Nonce::new("n".to_string()),
            verifier: PkceCodeVerifier::new("v".repeat(43)),
            redirect_uri: RedirectUrl::new("https://example.test/cb".to_string()).unwrap(),
            link_for: None,
            return_to: None,
            started: Instant::now() - age,
        };
        // Single use.
        store
            .insert("first".to_string(), record(Duration::ZERO))
            .unwrap();
        assert!(store.take("first").is_ok());
        assert_eq!(
            miss(store.take("first")),
            StateMiss::Unknown,
            "a state may authorize exactly one code exchange"
        );

        // A real person started a sign-in a few seconds ago...
        store
            .insert("real".to_string(), record(Duration::from_secs(5)))
            .unwrap();
        // ...and then a stranger fills the map to the brim, all just now.
        let mut refused = 0;
        for i in 0..(MAX_PENDING * 2) {
            if store
                .insert(format!("flood-{i}"), record(Duration::ZERO))
                .is_err()
            {
                refused += 1;
            }
        }
        assert!(
            store.len() <= MAX_PENDING,
            "the map grew past its cap: {}",
            store.len()
        );
        assert!(
            refused > 0,
            "past the cap, a flood is refused rather than evicting"
        );
        assert!(
            store.take("real").is_ok(),
            "the sign-in that started five seconds ago survived the flood"
        );

        // A record older than the eviction age is fair game once the map is
        // full: it makes room instead of refusing.
        let mut store = PendingStore::default();
        store
            .insert(
                "stale".to_string(),
                record(MIN_EVICT_AGE + Duration::from_secs(1)),
            )
            .unwrap();
        for i in 0..MAX_PENDING {
            store
                .insert(format!("fill-{i}"), record(Duration::ZERO))
                .unwrap_or_else(|_| panic!("insert {i} should have evicted the stale record"));
        }
        assert_eq!(
            miss(store.take("stale")),
            StateMiss::Unknown,
            "the stale record was evicted"
        );
        assert!(store.len() <= MAX_PENDING);

        // An expired record is forgotten by a take as well as by an insert,
        // so an idle instance does not hold expired sign-ins until the next
        // login happens to purge them.
        let mut store = PendingStore::default();
        store
            .insert(
                "expired".to_string(),
                record(PENDING_TTL + Duration::from_secs(1)),
            )
            .unwrap();
        assert_eq!(miss(store.take("nobody")), StateMiss::Unknown);
        assert_eq!(store.len(), 0, "a take purges what has expired");
        // And one that is asked for by name after it expired says so.
        store
            .insert(
                "late".to_string(),
                record(PENDING_TTL + Duration::from_secs(1)),
            )
            .unwrap();
        assert_eq!(miss(store.take("late")), StateMiss::Expired);
    }

    /// The chronological order is bounded by the cap too, not only the map:
    /// a caller who starts a sign-in and spends it at once (a login, then a
    /// callback whose exchange fails) leaves the map empty and would otherwise
    /// leave an entry in the order for the full ten minutes, growing it by
    /// two cheap unauthenticated requests per entry.
    #[test]
    fn spending_sign_ins_does_not_grow_the_order_past_the_cap() {
        let mut store = PendingStore::default();
        let record = || Pending {
            nonce: Nonce::new("n".to_string()),
            verifier: PkceCodeVerifier::new("v".repeat(43)),
            redirect_uri: RedirectUrl::new("https://example.test/cb".to_string()).unwrap(),
            link_for: None,
            return_to: None,
            started: Instant::now(),
        };
        for i in 0..(MAX_PENDING * 2) {
            let state = format!("cycle-{i}");
            store.insert(state.clone(), record()).unwrap();
            assert!(store.take(&state).is_ok());
        }
        assert_eq!(store.len(), 0);
        assert!(
            store.order_len() <= MAX_PENDING,
            "spent entries piled up in the order: {}",
            store.order_len()
        );
    }

    /// `common`, `organizations` and `consumers` are refused where the
    /// template is: an environment variable reaches this guard without passing
    /// the settings layer's.
    #[test]
    fn the_tenant_independent_entra_endpoints_are_refused_here_too() {
        for endpoint in ["common", "organizations", "consumers"] {
            let mut oidc = complete();
            oidc.issuer = Some(format!("https://login.microsoftonline.com/{endpoint}/v2.0"));
            assert!(
                OidcSettings::resolve(&config_with(oidc)).is_none(),
                "{endpoint} must not resolve"
            );
        }
    }

    /// An `http://` issuer anywhere but loopback sends the client secret and
    /// the ID token in clear text. It is allowed, because the test suite and a
    /// development setup depend on it against loopback, and it is flagged.
    #[test]
    fn a_plaintext_issuer_off_loopback_is_flagged_and_loopback_is_not() {
        let flagged = |issuer: &str| {
            issuer_is_plaintext_off_loopback(&IssuerUrl::new(issuer.to_string()).unwrap())
        };
        assert!(flagged("http://idp.internal/realm"));
        assert!(flagged("http://10.0.0.7:8080/realm"));
        assert!(!flagged("https://idp.internal/realm"));
        assert!(!flagged("http://127.0.0.1:7411"));
        assert!(!flagged("http://localhost:8080/realm"));
        assert!(!flagged("http://[::1]:8080/realm"));
    }

    /// The `Host` header is untrusted input that ends up inside a url, so it
    /// is held to the shape of a host: a name or address plus an optional
    /// port, nothing that could open a path, a query or a userinfo component.
    #[test]
    fn the_host_header_must_be_a_bare_host_and_port() {
        for good in [
            "knowledge.example",
            "knowledge.example:8443",
            "127.0.0.1:7411",
            "localhost",
            "[::1]:7411",
            "[2001:db8::1]",
            "a-b.c-d.example",
        ] {
            assert!(host_is_well_formed(good), "{good} is a host");
        }
        for bad in [
            "",
            "evil.test/x",
            "a@b",
            "knowledge.example?x=1",
            "knowledge.example#frag",
            "knowledge.example:port",
            "knowledge.example:",
            "[::1",
            "[::1]x",
            "[zz]",
            "ünïcode.example",
            "knowledge.example:123456",
        ] {
            assert!(!host_is_well_formed(bad), "{bad:?} is not a host");
        }
        let mut forged = HeaderMap::new();
        forged.insert(header::HOST, "evil.test/x".parse().unwrap());
        assert!(absolute_url(&forged, CALLBACK_PATH).is_err());
    }

    /// Claims carrying just the two fields a derivation reads.
    fn naming_claims(preferred: Option<&str>, email: Option<&str>) -> OidcClaims {
        OidcClaims {
            issuer: "https://idp.example".to_string(),
            subject: "sub-1".to_string(),
            preferred_username: preferred.map(str::to_string),
            display: None,
            email: email.map(str::to_string),
            link_for: None,
        }
    }

    /// The derivation, candidate by candidate: `preferred_username` first, the
    /// address's local part second, a generic name last, and a candidate that
    /// sanitizes to nothing falls through to the next one instead of winning.
    #[test]
    fn an_account_name_is_derived_from_the_first_usable_claim() {
        let name = |preferred, email| derive_account_name(&naming_claims(preferred, email));
        assert_eq!(name(Some("Ada.Lovelace"), None), "ada.lovelace");
        assert_eq!(
            name(Some("Ada Lovelace (Contoso)"), None),
            "ada-lovelace-contoso",
            "spaces and punctuation collapse to single separators, with no edges"
        );
        assert_eq!(
            name(None, Some("Grace.Hopper@example.test")),
            "grace.hopper",
            "the address's local part, never the whole address"
        );
        assert_eq!(
            name(Some("!!!"), Some("grace@example.test")),
            "grace",
            "a candidate that sanitizes to nothing falls through"
        );
        assert_eq!(
            name(None, None),
            FALLBACK_ACCOUNT_NAME,
            "a provider that sends neither still gets a name"
        );
        assert_eq!(
            name(Some("   "), Some("@example.test")),
            FALLBACK_ACCOUNT_NAME,
            "and so does one that sends both, emptily"
        );
        assert_eq!(
            name(Some("ADA"), Some("someone.else@example.test")),
            "ada",
            "the username wins over the address, folded"
        );
    }

    /// A name cannot be made unreadable, or unbounded, by what a provider
    /// sends.
    #[test]
    fn a_derived_name_is_bounded_and_free_of_separators_at_the_edges() {
        let long = "a".repeat(200);
        let derived = derive_account_name(&naming_claims(Some(&long), None));
        assert_eq!(derived.chars().count(), MAX_DERIVED_NAME);
        for raw in [
            "--ada--",
            "..ada..",
            "__ada__",
            "  ada  ",
            "\u{0007}ada\u{0007}",
        ] {
            assert_eq!(
                derive_account_name(&naming_claims(Some(raw), None)),
                "ada",
                "{raw:?} should fold to a bare name"
            );
        }
        // Whatever comes out is a name the store will accept, which is the
        // property the provisioning path depends on.
        for raw in ["Ada Lovelace", "!!!", "  ", "a/b\\c", &long] {
            let derived = derive_account_name(&naming_claims(Some(raw), None));
            assert_eq!(
                super::super::auth_store::normalize_account_name(&derived).unwrap(),
                derived,
                "{raw:?} derived a name the store would have folded further"
            );
        }
    }

    /// A presentation claim keeps every letter and loses everything that is
    /// not text. The second half matters as much as the first: stripping is
    /// about direction changes and invisible joiners, never about scripts.
    #[test]
    fn presentation_claims_lose_only_the_characters_that_are_not_text() {
        assert_eq!(
            presentation_text("Ada\u{202e}ecalevoL").as_deref(),
            Some("AdaecalevoL"),
            "a right-to-left override never reaches a stored name"
        );
        assert_eq!(presentation_text("  Ada  ").as_deref(), Some("Ada"));
        assert_eq!(
            presentation_text("Ada\u{7}\u{200b}Lovelace").as_deref(),
            Some("AdaLovelace")
        );
        assert_eq!(
            presentation_text("\u{200b}\u{feff}\u{00ad}").as_deref(),
            None,
            "a claim of nothing but formatting is a claim of nothing"
        );
        assert_eq!(presentation_text("   ").as_deref(), None);
        for kept in ["Ada Lovelace", "Ада Лавлейс", "愛達", "josé", "\u{3a9}"] {
            assert_eq!(
                presentation_text(kept).as_deref(),
                Some(kept),
                "{kept:?} is text and must survive untouched"
            );
        }
    }

    /// The unset default is one constant, and it is the one the settings
    /// registry renders as this key's default.
    #[test]
    fn the_unset_default_role_is_the_shared_constant() {
        let mut oidc = complete();
        oidc.default_role = None;
        let settings = OidcSettings::resolve(&config_with(oidc)).unwrap();
        assert_eq!(settings.default_role(), DEFAULT_OIDC_ROLE);
        let mut unset = complete();
        unset.default_role = None;
        assert_eq!(
            crate::settings::snapshot(&config_with(unset), &crate::overlay::EnvOverlay::default())
                .iter()
                .find(|row| row.key == "auth.oidc.default_role")
                .map(|row| row.value.clone()),
            Some(DEFAULT_OIDC_ROLE.as_str().to_string()),
            "the registry renders the same default this resolves to"
        );
    }
}
