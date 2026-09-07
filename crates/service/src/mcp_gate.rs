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
//! No header, the wrong scheme, a token that is not an MCP token's shape, one
//! that was never issued, one that was revoked, one whose account was disabled:
//! all of them get the identical status, headers and body. Distinguishing them
//! would hand an attacker an oracle telling them which half of a guess landed,
//! and the honest cases (an expired token, a disabled colleague) are better
//! served by the teaching text, which names where a working token comes from.
//! `tests/mcp_auth.rs` asserts the refusals are byte-identical.
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

use crate::rest::{AuthStore, MCP_TOKEN_PREFIX, Role};

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

/// What an agent refused at the door is told, which is the whole of the
/// remedy: the header to send, the two places a token comes from, and where in
/// its own configuration the header belongs.
pub const MCP_AUTH_REQUIRED: &str = "This instance requires agents to \
authenticate: send 'Authorization: Bearer <your MCP token>'. A signed-in user \
issues one in Fluid under profile > Agent access, or an admin runs \
'crystalline users mcp-token <name>'. Add the header to this server's entry in \
your harness's MCP registration.";

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
            sessions,
        }
    }
}

/// The credential a request presents, if it presents one in a shape worth
/// asking the store about.
///
/// The scheme is matched case-insensitively (RFC 9110 makes it so, and clients
/// do send `bearer`), and the [`MCP_TOKEN_PREFIX`] check is a cheap local
/// refusal for anything that is plainly not one of ours - a session cookie
/// pasted into the wrong field, a GitHub token, an API key from another
/// service. It saves the database round trip; it never changes the answer,
/// because everything it rejects would have failed to resolve anyway.
fn presented_token(request: &Request) -> Option<String> {
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
    if credential.is_empty() || !credential.starts_with(MCP_TOKEN_PREFIX) {
        return None;
    }
    Some(credential.to_string())
}

/// The one refusal, built fresh per request so nothing about it can be
/// accidentally shared with a response that carries a body.
fn refusal() -> Response {
    (
        axum::http::StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
        axum::Json(serde_json::json!({ "error": MCP_AUTH_REQUIRED })),
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
        Box::pin(async move {
            let Some(token) = presented_token(&request) else {
                return Ok(refusal());
            };
            // One lookup, which is also what stamps `last_used` so a token list
            // in Fluid can show when an agent last connected.
            match auth.mcp_token_user(&token).await {
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
                Ok(None) => Ok(refusal()),
                Err(error) => {
                    // Never the token itself, at any level: the store holds
                    // only its hash, and this is the one place a live one is in
                    // hand.
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

    #[test]
    fn only_a_bearer_presentation_of_an_mcp_token_reaches_the_store() {
        let live = format!("{MCP_TOKEN_PREFIX}{}", "a".repeat(64));
        assert_eq!(
            presented_token(&request_with(Some(&format!("Bearer {live}")))),
            Some(live.clone()),
            "the ordinary presentation"
        );
        assert_eq!(
            presented_token(&request_with(Some(&format!("bearer {live}")))),
            Some(live.clone()),
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
            assert_eq!(
                presented_token(&request_with(rejected)),
                None,
                "must not reach the store: {rejected:?}"
            );
        }
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

    /// The teaching text is what an agent has to act on unaided, so it names
    /// the header, both ways to get a token, and where the header goes.
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
        }
        assert!(
            !MCP_AUTH_UNAVAILABLE.contains("Agent access"),
            "a store fault must not send an agent off to mint a token it already has"
        );
    }
}
