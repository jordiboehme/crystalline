//! The door an HTTP agent authenticates at.
//!
//! With `auth.mcp` on, every request bound for the streamable-HTTP transport
//! presents a personal MCP token (`Authorization: Bearer cmt_...`) or is
//! refused before the transport sees a byte of it. With the setting off the
//! gate is a pass-through and the endpoint serves the legacy open tier exactly
//! as it did before this existed - which is what makes the default install
//! byte-identical to its old self.
//!
//! # Why the refusal is an HTTP status rather than a JSON-RPC error
//!
//! A JSON-RPC error is something a peer reads *inside* a session, and answering
//! one would mean opening the session first: the transport would create session
//! state, run the handshake and only then say no. The gate is the door, not the
//! room. A `401` with `WWW-Authenticate: Bearer` is also what every HTTP client
//! and proxy in the path already understands, so a harness that can attach
//! credentials is told to, in the one language it is guaranteed to speak.
//!
//! # Why there is exactly one refusal
//!
//! No header, the wrong scheme, a token that is not one of ours in shape, one
//! that was never issued, one that was revoked, one whose account was disabled,
//! and - where `auth.oauth` is on - an OAuth access token that expired or was
//! minted for another deployment of this server: all of them get the identical
//! status, headers and body. Distinguishing them would hand an attacker an
//! oracle telling them which half of a guess landed, and the honest cases (an
//! expired token, a disabled colleague) are better served by the teaching text,
//! which names where a working credential comes from. `tests/mcp_auth.rs`
//! asserts the refusals are byte-identical.
//!
//! The refusal is a function of this instance's configuration and of the
//! request's own `Host`, and of nothing else. With `auth.oauth` on it carries
//! the OAuth teaching text and points at the protected-resource document on the
//! origin the request arrived at, which is how a hosted client discovers there
//! is an authorization server to talk to; with it off it is the plain `Bearer`
//! challenge it always was. What never varies is the part a prober controls:
//! for one `Host`, every rejected credential gets the same bytes.
//!
//! # Why an OAuth token is checked against this request's origin
//!
//! An access token is audience bound (RFC 8707): it was minted for one resource
//! identifier, and this server's identifier is the origin its transport is
//! served from. So the gate derives that origin per request and hands it to the
//! store, which makes the audience a condition of the lookup statement rather
//! than a branch a future edit could forget. A token for another deployment is
//! a live credential of a real account, and it opens nothing here.
//!
//! What that delivers is exactly what RFC 8707 asks of a resource server, and
//! it is worth saying which half it is. The guarantee holds against a client
//! presenting a token at the wrong server: a token naming another resource is
//! refused rather than accepted on the strength of being valid somewhere. It is
//! not a guarantee against the presenter, because with no override configured
//! the identifier is the `Host` that presenter sent, so whoever holds a token
//! can satisfy the check by naming the audience the token already carries.
//! (This instance answers to more than one identifier for that reason:
//! `http://127.0.0.1` and `http://127.0.0.1:7411` are different resources.)
//! `auth.oidc.redirect_uri` is what pins the identifier to one address
//! independent of the header, and a public deployment behind a proxy wants it
//! set.
//!
//! # Why a session is bound to the identity that opened it
//!
//! A transport session is a bag of protocol state keyed by an `Mcp-Session-Id`
//! the server minted, and rmcp routes by that id alone. Authenticating each
//! request on its own is therefore not enough: two accounts that both hold
//! valid tokens could share one session, and every later task that scopes an
//! operation by the identity this gate injects would be reading one caller's
//! name against another caller's session state. So the gate records the
//! identity a session was created under and checks it on every request that
//! carries one. A mismatch is a `403` naming a session-identity mismatch and
//! nothing else: the request is never re-routed, never silently accepted, and
//! the answer never says whose session it is. Requests carrying no session id
//! are stateless and untouched by this.

use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use axum::extract::Request;
use axum::response::{IntoResponse, Response};

use crate::rest::{AuthStore, MCP_TOKEN_PREFIX, OAUTH_ACCESS_PREFIX, OriginRule, Role};

/// The account a request authenticated as, inserted into the request
/// extensions the transport hands on. rmcp copies the remaining
/// [`http::request::Parts`] into every tool call's `ctx.extensions`, so a tool
/// reads its caller as
/// `ctx.extensions.get::<http::request::Parts>()?.extensions.get::<McpIdentity>()`.
///
/// `admin` is the account's role at the moment the token resolved, carried
/// along rather than looked up again: an authorization decision inside a tool
/// call would otherwise cost a second database round trip per call to learn
/// something this resolution already read.
#[derive(Clone, Debug)]
pub struct McpIdentity {
    /// The account name, already folded by the auth store.
    pub name: String,
    /// Whether that account is an admin.
    pub admin: bool,
}

/// The teaching text both refusals share, as a macro rather than a constant
/// because `concat!` takes literals: the OAuth refusal below is this text plus
/// one sentence, and a second copy of it is a sentence that drifts.
macro_rules! mcp_auth_required {
    () => {
        "This instance requires agents to authenticate: send 'Authorization: \
Bearer <your MCP token>'. A signed-in user issues one in Fluid under profile > \
Agent access, or an admin runs 'crystalline users mcp-token <name>'. Add the \
header to this server's entry in your harness's MCP registration."
    };
}

/// What an agent refused at the door is told, which is the whole of the
/// remedy: the header to send, the two places a token comes from, and where in
/// its own configuration the header belongs.
pub const MCP_AUTH_REQUIRED: &str = mcp_auth_required!();

/// The same, on an instance that also serves OAuth: a harness that speaks it
/// needs no token pasted anywhere, and the challenge beside this body says
/// where the metadata is. Named as the alternative rather than described, so an
/// agent reading only this knows there are two doors and which one it can open
/// unaided.
pub const MCP_AUTH_REQUIRED_OAUTH: &str = concat!(
    mcp_auth_required!(),
    " This instance also serves OAuth for MCP clients: a harness that speaks \
it connects by signing in through this server instead, with no token pasted \
anywhere - see the 'WWW-Authenticate' header for where its metadata lives."
);

/// What a caller is told when the gate could not reach the account store at
/// all. Deliberately not [`MCP_AUTH_REQUIRED`]: nothing is wrong with the
/// caller's credential, and telling an agent to go and issue a fresh token
/// because a database was briefly unreadable would send it to fix the wrong
/// thing.
const MCP_AUTH_UNAVAILABLE: &str = "The account store could not be read, so \
this request could not be authenticated. This is a server-side fault rather \
than a problem with your token; retry, and tell the operator if it persists.";

/// What a caller is told when its token is good but the session it named was
/// opened by somebody else. It says that much and no more: naming the owner
/// would turn a session id into a way of enumerating who is connected.
pub const MCP_SESSION_IDENTITY_MISMATCH: &str = "This session belongs to a \
different identity. Open your own session rather than reusing one another \
account started: drop the 'Mcp-Session-Id' header and handshake again with \
your own MCP token.";

/// The header rmcp mints a session under and routes every later request of that
/// session by.
const MCP_SESSION_ID: &str = "mcp-session-id";

/// Which identity opened which session, for the life of this process.
///
/// The decision itself is kept here rather than in rmcp's session manager, so
/// the whole identity rule is one place: the manager's job is protocol state,
/// and a binding it held would be a second rule to keep in step with this one.
/// What the manager does own is the *end* of a session, and it ends one on three
/// paths - a client `DELETE`, the 300 second idle keep-alive, and a worker
/// error - all of which funnel through `SessionManager::close_session` (rmcp
/// 3.2.0 `tower.rs:1331` for the latter two, `:2073` for the DELETE). So the map
/// is handed to the session-manager wrapper as well and released from there,
/// which is what actually keeps it in step with rmcp's own sessions rather than
/// growing one permanent entry per connection that ever went idle.
///
/// Shared by every clone of the gate and by that wrapper, so it is constructed
/// once, before either, and both are handed the same handle.
#[derive(Default)]
pub struct SessionOwners(Mutex<HashMap<String, String>>);

impl SessionOwners {
    /// The account that opened `session`, if this process minted it. `None`
    /// means no claim is on record - an id from a previous process, or one
    /// nothing ever issued - and the transport answers those itself.
    pub(crate) fn owner(&self, session: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session)
            .cloned()
    }

    /// Record the identity a freshly minted session belongs to. Idempotent: a
    /// response that echoes an existing id can only have passed the mismatch
    /// check, so it rewrites the same name.
    pub(crate) fn claim(&self, session: String, name: String) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session, name);
    }

    /// Forget an ended session. Called from the session-manager wrapper on
    /// every path rmcp ends one, and again from the gate's own `DELETE` branch,
    /// which is a fast path rather than a second rule: removing an entry twice
    /// is removing it once.
    pub fn release(&self, session: &str) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session);
    }
}

/// The MCP transport with the door in front of it. `auth: None` is the gate
/// off, in which case `call` is the inner service's own `call` with one clone
/// in between.
///
/// Cloned per mount site and again per request, so both fields are cheap to
/// clone by construction: the inner transport is an `Arc` bundle of its own and
/// the store is behind an `Arc` here.
#[derive(Clone)]
pub struct McpGate<S> {
    inner: S,
    auth: Option<Arc<AuthStore>>,
    /// How this instance names itself, when `auth.oauth` is on: the audience an
    /// OAuth access token has to have been minted for, and the origin the
    /// refusal's metadata pointer is built on. `None` is OAuth off, and then
    /// nothing here so much as recognizes an access token's prefix.
    oauth: Option<OriginRule>,
    /// Shared across every clone, because the mount sites clone the gate and a
    /// session opened through one path is reused through another.
    sessions: Arc<SessionOwners>,
}

impl<S> McpGate<S> {
    /// Wrap `inner`. `auth` is `Some` only when `auth.mcp` is on; the daemon
    /// resolves that once, when the HTTP surface starts, like every other
    /// `auth.*` key. `sessions` is the same handle the session-manager wrapper
    /// holds, which is why it is passed in rather than made here: the manager
    /// has to exist before the transport does, and the transport before this.
    pub fn new(inner: S, auth: Option<Arc<AuthStore>>, sessions: Arc<SessionOwners>) -> McpGate<S> {
        McpGate {
            inner,
            auth,
            oauth: None,
            sessions,
        }
    }

    /// Also accept OAuth access tokens, minted for the origin `origin` derives.
    ///
    /// Inert when the gate is off: with no store there is nothing to resolve a
    /// token through, so an origin rule here would only be a challenge pointing
    /// at documents on an instance that authenticates nobody. `AuthCfg::resolve`
    /// already refuses `auth.oauth` without `auth.mcp`, so this is the second
    /// of two locks on the same door rather than the only one.
    pub fn with_oauth(mut self, origin: OriginRule) -> McpGate<S> {
        self.oauth = self.auth.is_some().then_some(origin);
        self
    }
}

/// The credential a request presents and which door it belongs to. The prefix
/// decides, before any hashing: an OAuth access token is never looked up as a
/// personal MCP token and a personal MCP token is never looked up as an OAuth
/// one, so neither can be resolved without its own rule - the audience check in
/// particular, which only the OAuth arm carries.
enum Presented {
    /// A personal MCP token, [`MCP_TOKEN_PREFIX`].
    Mcp(String),
    /// An OAuth access token, [`OAUTH_ACCESS_PREFIX`].
    Oauth(String),
}

/// What the `401` says, which is a function of this instance's configuration
/// and - for the pointer alone - of whether the request's own `Host` could be
/// read at all.
enum Challenge {
    /// `auth.oauth` off: the plain `Bearer` challenge and the token-only text.
    TokenOnly,
    /// `auth.oauth` on: the OAuth teaching text, and the metadata pointer when
    /// there was an origin to build one on. A malformed `Host` drops the
    /// pointer and nothing else - the body stays what this instance's
    /// configuration says it is, so no rejected credential can be told from
    /// another by it.
    Oauth(Option<String>),
}

/// The credential a request presents, if it presents one in a shape worth
/// asking the store about.
///
/// The scheme is matched case-insensitively (RFC 9110 makes it so, and clients
/// do send `bearer`), and the prefix check is a cheap local refusal for
/// anything that is plainly not one of ours - a session cookie pasted into the
/// wrong field, a GitHub token, an API key from another service. It saves the
/// database round trip; it never changes the answer, because everything it
/// rejects would have failed to resolve anyway.
///
/// `oauth` is whether this instance serves OAuth at all. With it off an access
/// token is not a credential here, whoever minted it: the setting is read when
/// the HTTP surface starts, so turning OAuth off stops every token it ever
/// issued from opening anything, without anybody having to sweep a table.
fn presented_token(request: &Request, oauth: bool) -> Option<Presented> {
    let raw = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, credential) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let credential = credential.trim();
    if credential.starts_with(MCP_TOKEN_PREFIX) {
        return Some(Presented::Mcp(credential.to_string()));
    }
    if oauth && credential.starts_with(OAUTH_ACCESS_PREFIX) {
        return Some(Presented::Oauth(credential.to_string()));
    }
    None
}

/// The one refusal, built fresh per request so nothing about it can be
/// accidentally shared with a response that carries a body.
///
/// The `resource_metadata` parameter is what a hosted MCP client reads on a
/// `401` before it does anything else: it names the protected-resource document
/// on this origin, which names the authorization server, which is how a client
/// gets from "refused" to "registered and consented" with no human pasting
/// anything. RFC 9728 spells the parameter as a quoted url, and the origin it
/// is built on has already been through the same well-formedness check the
/// address of this instance always is, so there is nothing left in it that
/// could close the quotes.
fn refusal(challenge: Challenge) -> Response {
    let (value, body) = match challenge {
        Challenge::TokenOnly => ("Bearer".to_string(), MCP_AUTH_REQUIRED),
        Challenge::Oauth(None) => ("Bearer".to_string(), MCP_AUTH_REQUIRED_OAUTH),
        Challenge::Oauth(Some(origin)) => (
            format!(
                "Bearer resource_metadata=\"{}\"",
                crate::rest::resource_metadata_url(&origin)
            ),
            MCP_AUTH_REQUIRED_OAUTH,
        ),
    };
    (
        axum::http::StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, value)],
        axum::Json(serde_json::json!({ "error": body })),
    )
        .into_response()
}

/// The session a request names, if it names one.
fn session_of(request: &Request) -> Option<String> {
    request
        .headers()
        .get(MCP_SESSION_ID)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// The session a response minted, which rmcp sets only when it created one.
fn minted_session(response: &Response) -> Option<String> {
    response
        .headers()
        .get(MCP_SESSION_ID)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// A good token pointed at somebody else's session. `403` rather than `401`,
/// because nothing is wrong with the credential and re-authenticating would not
/// help, and with no `WWW-Authenticate`, so no client retries with a fresh
/// token it does not need.
fn session_mismatch() -> Response {
    (
        axum::http::StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({ "error": MCP_SESSION_IDENTITY_MISMATCH })),
    )
        .into_response()
}

/// The store was unreachable. A `503` rather than a `401`, so a retrying client
/// backs off instead of hunting for a better credential, and with no
/// `WWW-Authenticate`, so nothing reads it as a challenge.
fn store_unavailable() -> Response {
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({ "error": MCP_AUTH_UNAVAILABLE })),
    )
        .into_response()
}

impl<S> tower_service::Service<Request> for McpGate<S>
where
    S: tower_service::Service<Request, Error = Infallible> + Clone + Send + 'static,
    S::Response: IntoResponse,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: Request) -> Self::Future {
        // The readiness dance every tower wrapper owes its inner service: the
        // `self.inner` that `poll_ready` was called on is the one that must
        // serve this request, so it is moved out and a fresh clone left behind
        // for the next one. rmcp's transport is always ready and clones
        // internally anyway, but a wrapper that only works against a
        // particular inner service is a trap for the next one mounted here.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let Some(auth) = self.auth.clone() else {
            // The gate is off: the legacy open tier, untouched.
            return Box::pin(async move { Ok(inner.call(request).await?.into_response()) });
        };
        let sessions = self.sessions.clone();
        let oauth = self.oauth.clone();
        Box::pin(async move {
            // The origin this request arrived at, derived once and used twice:
            // as the audience an OAuth token must have been minted for, and as
            // the address the refusal's metadata pointer is built on. Nothing
            // to derive with OAuth off, and a `Host` the rule will not name -
            // a malformed one, or one `service.allowed_hosts` does not cover -
            // leaves it `None`, which refuses every OAuth token and drops the
            // pointer, leaving the bare `Bearer` challenge. That is the right
            // way round: a request the transport itself would answer `403` is
            // not one to hand an address to authorize against.
            let origin = oauth
                .as_ref()
                .and_then(|rule| rule.origin(request.headers()).ok());
            let challenge = || match &oauth {
                Some(_) => Challenge::Oauth(origin.clone()),
                None => Challenge::TokenOnly,
            };
            let Some(presented) = presented_token(&request, oauth.is_some()) else {
                return Ok(refusal(challenge()));
            };
            // One lookup, whichever door the credential came through, and it is
            // also what stamps `last_used` so a token list or a connected-client
            // list in Fluid can show when an agent last connected.
            let resolved = match (&presented, &origin) {
                (Presented::Mcp(token), _) => auth.mcp_token_user(token).await,
                (Presented::Oauth(token), Some(origin)) => {
                    auth.oauth_access_user(token, origin).await
                }
                // No origin is no audience, and an audience-bound credential
                // with nothing to check the audience against resolves to
                // nobody. The store is never asked, so nothing about this can
                // be read as the token being unknown either.
                (Presented::Oauth(_), None) => Ok(None),
            };
            match resolved {
                Ok(Some(user)) => {
                    let named = session_of(&request);
                    // A revoked or rotated token never reaches here at all: it
                    // stops resolving, and the `Ok(None)` arm below refuses it
                    // with the ordinary 401 whether or not it names a session.
                    if let Some(session) = &named
                        && let Some(owner) = sessions.owner(session)
                        && owner != user.name
                    {
                        return Ok(session_mismatch());
                    }
                    let terminating = request.method() == axum::http::Method::DELETE;
                    let name = user.name.clone();
                    request.extensions_mut().insert(McpIdentity {
                        admin: matches!(user.role, Role::Admin),
                        name: user.name,
                    });
                    let response = inner.call(request).await?.into_response();
                    if let Some(session) = minted_session(&response) {
                        sessions.claim(session, name);
                    }
                    // rmcp has already called `close_session` by the time it
                    // answers a DELETE (`tower.rs:2073`), so the wrapper has
                    // released this claim already; doing it again here costs a
                    // map lookup and means the release does not depend on which
                    // side of that ordering a future rmcp lands on.
                    if terminating
                        && response.status().is_success()
                        && let Some(session) = &named
                    {
                        sessions.release(session);
                    }
                    Ok(response)
                }
                Ok(None) => Ok(refusal(challenge())),
                Err(error) => {
                    // Never the token itself, at any level: the store holds
                    // only its hash, and this is the one place a live one is in
                    // hand. Both doors fail this way, so a store that cannot be
                    // read never reads as a bad credential whichever kind was
                    // presented.
                    tracing::error!(
                        error = %format!("{error:#}"),
                        "MCP gate could not read the account store"
                    );
                    Ok(store_unavailable())
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request carrying `authorization`, or none at all.
    fn request_with(authorization: Option<&str>) -> Request {
        let mut builder = axum::http::Request::builder().uri("/");
        if let Some(value) = authorization {
            builder = builder.header(axum::http::header::AUTHORIZATION, value);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    /// The credential a presentation carries, as a plain string, or `None`
    /// when nothing about it was worth a lookup. The variant is asserted
    /// separately where it matters.
    fn credential(presented: &Option<Presented>) -> Option<&str> {
        match presented {
            Some(Presented::Mcp(token) | Presented::Oauth(token)) => Some(token),
            None => None,
        }
    }

    #[test]
    fn only_a_bearer_presentation_of_an_mcp_token_reaches_the_store() {
        let live = format!("{MCP_TOKEN_PREFIX}{}", "a".repeat(64));
        assert_eq!(
            credential(&presented_token(
                &request_with(Some(&format!("Bearer {live}"))),
                false
            )),
            Some(live.as_str()),
            "the ordinary presentation"
        );
        assert_eq!(
            credential(&presented_token(
                &request_with(Some(&format!("bearer {live}"))),
                false
            )),
            Some(live.as_str()),
            "schemes are case-insensitive on the wire"
        );
        for rejected in [
            None,
            Some(""),
            Some("Bearer "),
            Some("Bearer not-an-mcp-token"),
            Some(&format!("Basic {live}") as &str),
            Some(&live as &str),
        ] {
            assert!(
                presented_token(&request_with(rejected), false).is_none(),
                "must not reach the store: {rejected:?}"
            );
        }
    }

    /// **The prefix decides which door a credential belongs to, and OAuth's
    /// only exists while `auth.oauth` is on.**
    ///
    /// This is the audience hole in miniature. An access token resolved as a
    /// personal MCP token would skip the check that it was minted for this
    /// origin, and a personal token resolved as an access token would be
    /// checked against an audience it never carried; neither lookup can be
    /// reached from the other's prefix, and that is what this pins.
    #[test]
    fn each_prefix_reaches_only_its_own_door_and_oauth_only_while_it_is_on() {
        let mcp = format!("{MCP_TOKEN_PREFIX}{}", "a".repeat(64));
        let access = format!("{OAUTH_ACCESS_PREFIX}{}", "b".repeat(64));

        assert!(
            matches!(
                presented_token(&request_with(Some(&format!("Bearer {mcp}"))), true),
                Some(Presented::Mcp(_))
            ),
            "an MCP token is never looked up as an OAuth one, whatever the setting says"
        );
        assert!(
            matches!(
                presented_token(&request_with(Some(&format!("Bearer {access}"))), true),
                Some(Presented::Oauth(token)) if token == access
            ),
            "and an access token is never looked up as an MCP one"
        );
        assert!(
            presented_token(&request_with(Some(&format!("Bearer {access}"))), false).is_none(),
            "with OAuth off an access token is not a credential here at all"
        );
        // A refresh token is not an access token: it is spent at the token
        // endpoint and never presented at this door.
        assert!(
            presented_token(
                &request_with(Some(&format!("Bearer cor_{}", "c".repeat(64)))),
                true
            )
            .is_none()
        );
    }

    /// **The refusal is a function of the configuration and of nothing a
    /// prober can vary but the `Host`.**
    ///
    /// With OAuth on the body is the same text whether or not an origin could
    /// be derived, so a malformed `Host` drops the pointer and nothing else;
    /// with OAuth off nothing points at metadata this instance does not serve.
    #[test]
    fn the_challenge_follows_the_setting_and_the_body_never_varies_with_the_credential() {
        let plain = refusal(Challenge::TokenOnly);
        assert_eq!(plain.status(), axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(
            plain.headers()[axum::http::header::WWW_AUTHENTICATE],
            "Bearer"
        );

        let pointed = refusal(Challenge::Oauth(Some(
            "https://knowledge.example".to_string(),
        )));
        assert_eq!(
            pointed.headers()[axum::http::header::WWW_AUTHENTICATE],
            "Bearer resource_metadata=\"https://knowledge.example/.well-known/oauth-protected-resource\""
        );
        assert_eq!(
            refusal(Challenge::Oauth(None)).headers()[axum::http::header::WWW_AUTHENTICATE],
            "Bearer",
            "a Host that names no origin drops the pointer rather than inventing one"
        );

        assert!(
            MCP_AUTH_REQUIRED_OAUTH.starts_with(MCP_AUTH_REQUIRED),
            "the OAuth refusal teaches the token remedy too: {MCP_AUTH_REQUIRED_OAUTH}"
        );
        assert!(
            MCP_AUTH_REQUIRED_OAUTH.contains("OAuth"),
            "and names the door a harness can open unaided: {MCP_AUTH_REQUIRED_OAUTH}"
        );
        assert!(
            !MCP_AUTH_REQUIRED.contains("OAuth"),
            "while the plain one offers no door that is closed: {MCP_AUTH_REQUIRED}"
        );
    }

    /// An inner service that records the request it was handed and answers
    /// `200`, so a test can see both whether a request reached the transport at
    /// all and what the gate put in its extensions on the way.
    #[derive(Clone)]
    struct CapturingService(Arc<std::sync::Mutex<Option<axum::http::Extensions>>>);

    impl tower_service::Service<Request> for CapturingService {
        type Response = Response;
        type Error = Infallible;
        type Future = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, request: Request) -> Self::Future {
            *self.0.lock().unwrap() = Some(request.extensions().clone());
            Box::pin(async { Ok(axum::http::StatusCode::OK.into_response()) })
        }
    }

    /// A store holding one admin and one editor, each with a live token.
    async fn store_with_two_agents() -> (tempfile::TempDir, Arc<AuthStore>, String, String) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            AuthStore::open(&tmp.path().join("web-auth.db"))
                .await
                .unwrap(),
        );
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw12345678")
            .await
            .unwrap();
        store
            .add_user("eve", "Eve", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let admin = store.issue_mcp_token("ada", "agent").await.unwrap().token;
        let editor = store.issue_mcp_token("eve", "agent").await.unwrap().token;
        (tmp, store, admin, editor)
    }

    /// The gate's half of the contract Task 5 reads: an authenticated request
    /// reaches the transport carrying its account, a refused one does not reach
    /// the transport at all, and the role travels with the name so an
    /// authorization decision inside a tool call costs no second lookup.
    ///
    /// rmcp's half - copying the remaining `http::request::Parts` into every
    /// tool call's `ctx.extensions` - is verified by source reading at the four
    /// injection sites in `transport/streamable_http_server/tower.rs` (3.2.0:
    /// `:1219` stateless negotiated, `:1775` session POST, `:1855` session
    /// creation, `:1974` stateless POST).
    #[tokio::test]
    async fn an_authenticated_request_carries_its_account_into_the_transport() {
        let (_tmp, store, admin, editor) = store_with_two_agents().await;
        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut gate = McpGate::new(
            CapturingService(seen.clone()),
            Some(store.clone()),
            Arc::new(SessionOwners::default()),
        );

        let response =
            tower_service::Service::call(&mut gate, request_with(Some(&format!("Bearer {admin}"))))
                .await
                .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let identity = seen
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|extensions| extensions.get::<McpIdentity>().cloned())
            .expect("the transport must see the resolved account");
        assert_eq!(identity.name, "ada");
        assert!(identity.admin, "the role travels with the name");

        let response = tower_service::Service::call(
            &mut gate,
            request_with(Some(&format!("Bearer {editor}"))),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let identity = seen
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|extensions| extensions.get::<McpIdentity>().cloned())
            .unwrap();
        assert_eq!(identity.name, "eve");
        assert!(!identity.admin, "an editor is not an admin");

        // A refused request never reaches the transport at all, which is the
        // difference between a door and a check inside the room.
        *seen.lock().unwrap() = None;
        let refused = tower_service::Service::call(&mut gate, request_with(None))
            .await
            .unwrap();
        assert_eq!(refused.status(), axum::http::StatusCode::UNAUTHORIZED);
        assert!(
            seen.lock().unwrap().is_none(),
            "a refused request must never reach the transport"
        );
    }

    /// With the gate off every request passes and none of them arrives claiming
    /// an identity, so the legacy open tier cannot be handed a forged one.
    #[tokio::test]
    async fn with_the_gate_off_requests_pass_and_carry_no_identity() {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut gate = McpGate::new(
            CapturingService(seen.clone()),
            None,
            Arc::new(SessionOwners::default()),
        );
        let response = tower_service::Service::call(&mut gate, request_with(None))
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert!(
            seen.lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .get::<McpIdentity>()
                .is_none(),
            "the open tier must not manufacture an identity"
        );
    }

    /// **An origin rule on a gate with no store opens nothing.**
    ///
    /// `AuthCfg::resolve` already refuses `auth.oauth` without `auth.mcp`, so
    /// this combination should be unreachable; the gate fails closed anyway,
    /// because an OAuth rule on an open tier would be a challenge pointing at
    /// documents on an instance that authenticates nobody.
    #[test]
    fn with_the_gate_off_an_origin_rule_opens_no_door() {
        let gate = McpGate::new(
            CapturingService(Arc::new(std::sync::Mutex::new(None))),
            None,
            Arc::new(SessionOwners::default()),
        )
        .with_oauth(OriginRule::from_config(
            &crystalline_core::config::GlobalConfig::default(),
            &[],
        ));
        assert!(gate.oauth.is_none());
    }

    /// The teaching text is what an agent has to act on unaided, so it names
    /// the header, both ways to get a token, and where the header goes. The
    /// OAuth variant carries the same remedy plus the extra door, so it is
    /// checked against the identical fragments rather than trusted to inherit
    /// them from the macro.
    #[test]
    fn the_refusal_teaches_the_whole_remedy() {
        for fragment in [
            "Authorization: Bearer",
            "Agent access",
            "crystalline users mcp-token",
            "MCP registration",
        ] {
            assert!(
                MCP_AUTH_REQUIRED.contains(fragment),
                "the refusal must name '{fragment}': {MCP_AUTH_REQUIRED}"
            );
            assert!(
                MCP_AUTH_REQUIRED_OAUTH.contains(fragment),
                "the OAuth refusal must name '{fragment}' too: {MCP_AUTH_REQUIRED_OAUTH}"
            );
        }
        assert!(
            !MCP_AUTH_UNAVAILABLE.contains("Agent access"),
            "a store fault must not send an agent off to mint a token it already has"
        );
    }
}
