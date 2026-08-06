//! The four ways a client finds its way around what an instance knows: search
//! across the domains, the vocabulary they are written in, the graph around one
//! engram, and what was recorded lately.
//!
//! Each one is the engine verb the matching MCP tool calls, behind a query
//! string that maps onto its parameter struct one for one, and each hands the
//! engine's own JSON back unchanged so the two surfaces answer with one payload
//! rather than two shapes that drift.
//!
//! A domain named in a query string here is a *filter*, never a resource: an
//! unmatched name narrows the answer to nothing and is answered 200, which is
//! what separates these routes from the ones that carry a domain in a path
//! segment and 404 when nobody registered it. Filtering to nothing and pointing
//! at nothing are different facts, and a client can tell them apart by which
//! shape of URL it asked with.

use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::Value;

use super::{ApiError, ApiQuery, RestState, csv};
use crate::params::{ContextParams, RecentParams, SearchParams, VocabularyParams};

/// The query string `GET /search` takes, mirroring [`SearchParams`]. The free
/// text is `q` rather than `query`, the spelling a browser client's address bar
/// and every other search API already use; the rest keep the engine's names.
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// The free-text query. Omit for a filter-only search.
    #[serde(default)]
    q: Option<String>,
    /// Restrict to these domains, comma separated. Defaults to every registered
    /// domain.
    #[serde(default)]
    domains: Option<String>,
    /// Filter by `type`.
    #[serde(rename = "type", default)]
    engram_type: Option<String>,
    /// Filter by `status`.
    #[serde(default)]
    status: Option<String>,
    /// Require all of these tags, comma separated.
    #[serde(default)]
    tags: Option<String>,
    /// Only engrams recorded on or after this ISO date.
    #[serde(default)]
    after: Option<String>,
    /// hybrid (default), text, semantic, title or permalink.
    #[serde(default)]
    search_type: Option<String>,
    /// Minimum cosine similarity for a semantic hit.
    #[serde(default)]
    min_similarity: Option<f32>,
    /// One-based page number. Defaults to 1.
    #[serde(default)]
    page: Option<usize>,
    /// Page size. Defaults to 10.
    #[serde(default)]
    limit: Option<usize>,
}

/// `GET /search` - search across the registered domains, or across the ones
/// `domains` names.
///
/// The answer is the engine's page envelope (`mode`, `total`, `page`, `limit`,
/// `count`, `hits`), so a client pages a search the way it pages a listing.
/// `mode` is the mode that actually ran rather than the one asked for: hybrid
/// and semantic fall back to text where there is nothing embedded to search, and
/// the response says which happened rather than quietly answering from a
/// different index than the caller expected.
///
/// A `search_type` the engine does not know is its own error, surfaced as a 422
/// problem detail carrying the engine's message, so the caller learns which
/// values do work instead of being handed a bare status.
///
/// `metadata_filters` is not exposed: it takes a JSON object of comparisons,
/// which has no honest query-string spelling, and no client needs it yet.
pub async fn search(
    State(state): State<RestState>,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    let value = state
        .engine
        .search_engrams(&SearchParams {
            query: query.q,
            domains: csv(query.domains.as_deref()),
            engram_type: query.engram_type,
            tags: csv(query.tags.as_deref()),
            status: query.status,
            metadata_filters: None,
            after: query.after,
            search_type: query.search_type,
            min_similarity: query.min_similarity,
            limit: query.limit,
            page: query.page,
        })
        .await?;
    Ok(Json(value))
}

/// The query string `GET /vocabulary` takes, mirroring [`VocabularyParams`].
#[derive(Debug, Deserialize)]
pub struct VocabularyQuery {
    /// Restrict to one domain. Omit for a vocabulary across every domain.
    #[serde(default)]
    domain: Option<String>,
}

/// `GET /vocabulary` - the words the domains are written in: the tags in use
/// with their counts, the observation categories and the relation types, plus
/// the near-duplicate clusters and tag aliases the engine reports when there are
/// any.
///
/// The domain is a filter here rather than a path segment, so an unknown name
/// answers an empty vocabulary rather than a 404: this route asks what is in
/// use, and the answer to that can legitimately be nothing.
pub async fn vocabulary(
    State(state): State<RestState>,
    ApiQuery(query): ApiQuery<VocabularyQuery>,
) -> Result<Json<Value>, ApiError> {
    let value = state
        .engine
        .vocabulary(&VocabularyParams {
            domain: query.domain,
        })
        .await?;
    Ok(Json(value))
}

/// The query string `GET /context` takes, mirroring [`ContextParams`]. `anchor`
/// has no default: a traversal with nothing to start from is not a request this
/// route can answer, so a missing one is rejected by the extractor as a 400
/// rather than guessed at.
#[derive(Debug, Deserialize)]
pub struct ContextQuery {
    /// A `crystalline://domain/permalink` anchor. A `/*` suffix globs a prefix.
    anchor: String,
    /// Traversal depth, 1 to 3. Defaults to 1.
    #[serde(default)]
    depth: Option<u8>,
    /// Restrict the returned neighborhood to these domains, comma separated.
    #[serde(default)]
    domains: Option<String>,
    /// Maximum related engrams beyond the anchors. Defaults to 10.
    #[serde(default)]
    max_related: Option<usize>,
}

/// `GET /context` - the graph around an anchor: the anchored engrams as seed
/// nodes, the neighborhood ranked out from them, and the edges that connect what
/// came back. This is what a client draws a relationship view from.
///
/// The three ways an anchor can be wrong stay distinguishable: absent is a 400
/// from the extractor, one that is not a `crystalline://` URL is the engine's
/// own 422, and one pointing at an engram nobody wrote is a 404.
///
/// `timeframe` is not exposed: the engine documents it as advisory in this
/// version, and a parameter that does not change the answer is not worth a
/// client learning.
pub async fn context(
    State(state): State<RestState>,
    ApiQuery(query): ApiQuery<ContextQuery>,
) -> Result<Json<Value>, ApiError> {
    let value = state
        .engine
        .build_context(&ContextParams {
            anchor: query.anchor,
            depth: query.depth,
            domains: csv(query.domains.as_deref()),
            timeframe: None,
            max_related: query.max_related,
        })
        .await?;
    Ok(Json(value))
}

/// The query string `GET /activity` takes, mirroring [`RecentParams`].
#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    /// Restrict to these domains, comma separated. Defaults to every registered
    /// domain.
    #[serde(default)]
    domains: Option<String>,
    /// A recency window such as `7d`, `24h` or `2w`. Defaults to `7d`.
    #[serde(default)]
    timeframe: Option<String>,
    /// Restrict to these `type` values, comma separated.
    #[serde(default)]
    types: Option<String>,
}

/// `GET /activity` - what was recorded lately, newest first, which is what a
/// client opens on.
///
/// The window defaults in the engine rather than here, so the API and the MCP
/// tool answer the same question when neither is told which window to use.
pub async fn activity(
    State(state): State<RestState>,
    ApiQuery(query): ApiQuery<ActivityQuery>,
) -> Result<Json<Value>, ApiError> {
    let value = state
        .engine
        .recent_activity(&RecentParams {
            domains: csv(query.domains.as_deref()),
            timeframe: query.timeframe,
            types: csv(query.types.as_deref()),
        })
        .await?;
    Ok(Json(value))
}
