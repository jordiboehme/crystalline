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

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
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
}

impl<S> McpGate<S> {
    /// Wrap `inner`. `auth` is `Some` only when `auth.mcp` is on; the daemon
    /// resolves that once, when the HTTP surface starts, like every other
    /// `auth.*` key.
    pub fn new(inner: S, auth: Option<Arc<AuthStore>>) -> McpGate<S> {
        McpGate { inner, auth }
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
        Box::pin(async move {
            let Some(token) = presented_token(&request) else {
                return Ok(refusal());
            };
            // One lookup, which is also what stamps `last_used` so a token list
            // in Fluid can show when an agent last connected.
            match auth.mcp_token_user(&token).await {
                Ok(Some(user)) => {
                    request.extensions_mut().insert(McpIdentity {
                        admin: matches!(user.role, Role::Admin),
                        name: user.name,
                    });
                    Ok(inner.call(request).await?.into_response())
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
        let mut gate = McpGate::new(CapturingService(seen.clone()), Some(store.clone()));

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
        let mut gate = McpGate::new(CapturingService(seen.clone()), None);
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
