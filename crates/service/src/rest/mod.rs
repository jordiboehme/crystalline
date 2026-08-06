//! The JSON API `serve --http` nests at `/api/v1`, the surface the Fluid UI
//! talks to. Handlers pass the engine's own JSON values through unchanged, so
//! the MCP tools and this API stay one source of truth, and every failure is
//! an [`ApiError`] rendered as RFC 9457 problem detail.

mod auth;
mod auth_store;
mod domains;
mod error;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use tokio::sync::Semaphore;

pub use auth::{
    AuthCfg, CSRF_HEADER, Caller, Identity, LOGIN_SLOTS, SESSION_COOKIE, SESSION_TTL_SECS,
};
pub use auth_store::*;
pub use error::{ApiError, ApiJson, ApiPath, ApiQuery};

use crate::engine::Engine;

/// What every REST handler is given: the one shared engine the daemon owns,
/// the one auth store this process holds open, and the auth settings resolved
/// at startup. Cheap to clone; axum clones it per request.
#[derive(Clone)]
pub struct RestState {
    /// The engine backing every request, shared with the MCP router.
    pub engine: Arc<Engine>,
    /// The one users-and-sessions store this process holds open. Shared rather
    /// than opened per request: it serializes its own database access, so a
    /// second store would only add handles on one small file.
    pub auth: Arc<AuthStore>,
    /// The auth settings as of startup. See [`AuthCfg`].
    pub auth_cfg: AuthCfg,
    /// Caps how many password verifications run at once. See
    /// [`LOGIN_SLOTS`].
    login_slots: Arc<Semaphore>,
}

impl RestState {
    /// Assemble the state, resolving and validating the auth settings out of
    /// the engine's config. Fails when `auth.trusted_header` is not a usable
    /// HTTP header name: the HTTP surface then refuses to come up, naming the
    /// setting, rather than serving with a header that silently never matches.
    pub fn new(engine: Arc<Engine>, auth: Arc<AuthStore>) -> anyhow::Result<RestState> {
        let auth_cfg = AuthCfg::resolve(&engine.config())?;
        Ok(RestState {
            engine,
            auth,
            auth_cfg,
            login_slots: auth::login_slots(),
        })
    }
}

/// Build the REST router. Mounted with `nest("/api/v1", ...)`, so the paths
/// here are relative to that prefix and the fallback below only ever answers
/// for unknown paths under it.
///
/// [`auth::guard`] is layered over the whole thing, fallback included, so
/// identity resolution, the CSRF check and the closed-by-default rule apply to
/// every path under the mount - including the ones later tasks add, which are
/// guarded the moment they are registered. Every route therefore belongs
/// *above* the `.layer` call: axum only wraps what was declared before it, so a
/// route added below would serve unguarded.
pub fn router(state: RestState) -> Router {
    Router::new()
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/domains", get(domains::list))
        .route("/domains/{domain}/tree", get(domains::tree))
        .route("/domains/{domain}/manifest", get(domains::manifest))
        .fallback(unknown_path)
        // Applies to every method router registered above it, so it stays
        // below the routes and above the guard.
        .method_not_allowed_fallback(wrong_method)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::guard,
        ))
        .with_state(state)
}

/// Answer an unknown `/api/v1` path in problem+json rather than letting it
/// fall through to the MCP transport, which would reply in its own shape.
async fn unknown_path() -> ApiError {
    ApiError::not_found("unknown API path")
}

/// Answer a known path asked for with a method it does not serve, in
/// problem+json rather than axum's empty 405.
async fn wrong_method() -> ApiError {
    ApiError::method_not_allowed()
}
