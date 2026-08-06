//! The neighborhood graph: the nodes and typed edges around one anchor, in the
//! flat shape a graph renderer draws from.
//!
//! This is the same traversal `/context` runs, answered differently rather than
//! answered again: `/context` ranks a neighborhood for reading, this one reports
//! it for drawing. Both hand the engine's own JSON back unchanged, so the two
//! agree about what is connected to what.

use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::Value;
use utoipa::IntoParams;

use super::{ApiError, ApiQuery, ProblemDetail, RestState};

/// The query string `GET /graph` takes. `anchor` has no default: a traversal
/// with nothing to start from is not a request this route can answer, so a
/// missing one is rejected by the extractor as a 400 rather than guessed at.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct GraphQuery {
    /// A `crystalline://domain/permalink` anchor. A `/*` suffix globs a prefix.
    #[param(example = "crystalline://eng/alpha")]
    anchor: String,
    /// Traversal depth, 1 or 2. Defaults to 1.
    #[serde(default)]
    #[param(example = 1)]
    depth: Option<u8>,
    /// The most nodes to return. Defaults to 100, capped by the engine.
    #[serde(default)]
    #[param(example = 100)]
    max_nodes: Option<usize>,
}

/// The hops walked when the caller does not say. One: an engram page opens on
/// its immediate neighborhood, and a second hop is a deliberate act.
const DEFAULT_DEPTH: u8 = 1;

/// The nodes returned when the caller does not say. Below the engine's ceiling
/// on purpose: it is what one view renders comfortably, and a client that wants
/// the ceiling asks for it.
const DEFAULT_MAX_NODES: usize = 100;

/// `GET /graph` - the nodes and typed edges around an anchor: every node with
/// the address, title, status and type a client labels it with, every edge with
/// its direction and `rel_type`, and `truncated` saying whether the node cap
/// cut anything.
///
/// Both bounds are the server's to decide: the engine clamps the depth to one or
/// two hops and the node count to its own ceiling, so a hand-written URL can ask
/// neither for nothing nor for a whole index in one payload. A clamped request
/// is answered rather than refused - the honest report of what was returned is
/// the payload itself.
///
/// Retired engrams are part of the answer, carrying their status. The graph is
/// the shape of what is written, and a client fades what is retired rather than
/// being served a graph with the retired nodes already cut out of it.
///
/// The three ways an anchor can be wrong stay distinguishable, as on `/context`:
/// absent is a 400 from the extractor, one that is not a `crystalline://` URL is
/// the engine's own 422, and one pointing at an engram nobody wrote is a 404.
#[utoipa::path(
    get,
    path = "/api/v1/graph",
    tag = "graph",
    operation_id = "get_graph",
    params(GraphQuery),
    responses(
        (
            status = 200,
            description = "The engine's own graph payload, unchanged: the flat \
                           node and edge lists a renderer draws from, with \
                           `truncated` saying whether the node cap cut anything. \
                           `id` is opaque and stable only within one response; \
                           the address is `crystalline://domain/permalink`.",
            body = Object,
            example = json!({
                "nodes": [
                    {
                        "id": 1,
                        "domain": "eng",
                        "permalink": "alpha",
                        "title": "Alpha",
                        "status": "stable",
                        "type": "engram"
                    },
                    {
                        "id": 2,
                        "domain": "eng",
                        "permalink": "notes/beta",
                        "title": "Beta",
                        "status": "deprecated",
                        "type": "engram"
                    }
                ],
                "edges": [{ "from": 1, "to": 2, "rel_type": "relates_to" }],
                "truncated": false
            }),
        ),
        (
            status = 400,
            description = "The query string will not parse, `anchor` included: it \
                           has no default.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "The anchor names no engram.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The anchor is not a `crystalline://` URL.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn graph(
    State(state): State<RestState>,
    ApiQuery(query): ApiQuery<GraphQuery>,
) -> Result<Json<Value>, ApiError> {
    let value = state
        .engine
        .graph_neighborhood(
            &query.anchor,
            query.depth.unwrap_or(DEFAULT_DEPTH),
            query.max_nodes.unwrap_or(DEFAULT_MAX_NODES),
        )
        .await?;
    Ok(Json(value))
}
