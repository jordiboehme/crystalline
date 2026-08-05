//! The JSON API `serve --http` nests at `/api/v1`, the surface the Fluid UI
//! talks to. Handlers pass the engine's own JSON values through unchanged, so
//! the MCP tools and this API stay one source of truth, and every failure is
//! an [`ApiError`] rendered as RFC 9457 problem detail.

mod error;

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use serde_json::Value;

pub use error::ApiError;

use crate::engine::Engine;

/// What every REST handler is given: the one shared engine the daemon owns.
/// Cheap to clone; axum clones it per request.
#[derive(Clone)]
pub struct RestState {
    /// The engine backing every request, shared with the MCP router.
    pub engine: Arc<Engine>,
}

/// Build the REST router. Mounted with `nest("/api/v1", ...)`, so the paths
/// here are relative to that prefix and the fallback below only ever answers
/// for unknown paths under it.
pub fn router(state: RestState) -> Router {
    Router::new()
        .route("/auth/me", get(me))
        .fallback(unknown_path)
        .with_state(state)
}

/// The capability probe a client calls before anything else: who it is (no
/// one yet - authentication arrives with the session endpoints), whether this
/// instance refuses content mutations, and which server version it is
/// talking to, so a mismatched UI can say so instead of failing later.
async fn me(State(state): State<RestState>) -> axum::Json<Value> {
    axum::Json(serde_json::json!({
        "user": null,
        "anonymous": true,
        "read_only": state.engine.read_only(),
        "version": crystalline_core::VERSION,
    }))
}

/// Answer an unknown `/api/v1` path in problem+json rather than letting it
/// fall through to the MCP transport, which would reply in its own shape.
async fn unknown_path() -> ApiError {
    ApiError::not_found("unknown API path")
}
