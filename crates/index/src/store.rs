//! The backend-agnostic storage interface.
//!
//! [`Store`] is defined at the repository level, not the SQL level, so a second
//! backend (PostgreSQL with tsvector and pgvector) can implement the same trait
//! later without changing the sync engine, the search planner or any caller.
//! The Turso implementation lives in [`crate::turso`].
//!
//! Temporal semantics are open ended: an absent `valid_from` means always valid
//! and an absent `valid_to` means valid forever. The canonical current filter is
//! `status = 'current' AND (valid_from IS NULL OR valid_from <= :today) AND
//! (valid_to IS NULL OR valid_to > :today)`; it maps onto the promoted, indexed
//! `(status, valid_from, valid_to)` columns rather than the metadata JSON.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::NaiveDate;
pub use crystalline_core::config::DomainKind;
use crystalline_core::{Engram, slugify};
use serde::Serialize;

use crate::error::Result;

/// A domain's primary key in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct DomainId(pub i64);

/// An engram's primary key in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct EngramId(pub i64);

/// The recorded file identity used by the sync prefilter: modification time and
/// size are the cheap comparison, the SHA-256 is the authoritative one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    /// Modification time in whole seconds since the Unix epoch.
    pub mtime: i64,
    /// File size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256 of the file contents.
    pub sha256: String,
}

/// One observation bullet, ready to index.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationRecord {
    /// One-based source line.
    pub line: usize,
    /// The bracket category.
    pub category: String,
    /// The observation text without trailing tags or context.
    pub content: String,
    /// Trailing hashtags without the leading `#`.
    pub tags: Vec<String>,
    /// A trailing parenthesized context group, if present.
    pub context: Option<String>,
}

/// One relation bullet, ready to index. `to_id` is filled by
/// [`Store::resolve_pending_relations`] once the target exists.
#[derive(Debug, Clone, PartialEq)]
pub struct RelationRecord {
    /// One-based source line.
    pub line: usize,
    /// The relation type token or quoted phrase.
    pub rel_type: String,
    /// The link target text (a permalink or a title).
    pub to_target: String,
    /// An explicit cross-domain target domain, or `None` for same-domain.
    pub to_domain: Option<String>,
}

/// One prose wikilink, treated as a direct link edge.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRecord {
    /// One-based source line.
    pub line: usize,
    /// The link target text (a permalink or a title).
    pub to_target: String,
    /// An explicit cross-domain target domain, or `None` for same-domain.
    pub to_domain: Option<String>,
}

/// A fully prepared engram row plus its child rows and file stamp. Built from a
/// parsed [`Engram`] and the file metadata via [`EngramRecord::from_engram`].
#[derive(Debug, Clone, PartialEq)]
pub struct EngramRecord {
    /// Domain-relative file path, forward-slashed, including the `.md` suffix.
    pub path: String,
    /// Effective permalink: the frontmatter permalink, else `slugify(path)`.
    pub permalink: String,
    /// Title, empty when absent.
    pub title: String,
    /// `type` frontmatter, empty when absent.
    pub engram_type: String,
    /// `status` frontmatter, empty when absent.
    pub status: String,
    /// `recorded_at` as an ISO date string.
    pub recorded_at: Option<String>,
    /// `valid_from` as an ISO date string; absent = always valid.
    pub valid_from: Option<String>,
    /// `valid_to` as an ISO date string; absent = valid forever.
    pub valid_to: Option<String>,
    /// `timestamp` as an RFC 3339 string.
    pub timestamp: Option<String>,
    /// `description`, feeds search snippets.
    pub description: Option<String>,
    /// The raw body text, used for the candidate scan and snippet generation.
    pub content: String,
    /// A JSON object of every filterable non-promoted frontmatter key.
    pub metadata: serde_json::Value,
    /// Normalized tags.
    pub tags: Vec<String>,
    /// Observation bullets.
    pub observations: Vec<ObservationRecord>,
    /// Relation bullets.
    pub relations: Vec<RelationRecord>,
    /// Prose wikilinks.
    pub links: Vec<LinkRecord>,
    /// The file stamp for the sync prefilter.
    pub stamp: FileStamp,
}

fn date_str(d: Option<NaiveDate>) -> Option<String> {
    d.map(|d| d.format("%Y-%m-%d").to_string())
}

impl EngramRecord {
    /// Build a record from a parsed [`Engram`], its domain-relative path and its
    /// file stamp. The effective permalink is the frontmatter permalink, or the
    /// slug of the path minus `.md` when the frontmatter omits it.
    pub fn from_engram(engram: &Engram, path: &str, stamp: FileStamp) -> EngramRecord {
        let fm = &engram.frontmatter;
        let permalink = fm
            .permalink
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| slugify(path));

        // Filterable metadata: every preserved unknown key, plus the optional
        // non-promoted known fields, all as a JSON object. Promoted columns
        // (type, status, title, permalink, the temporal window, description)
        // and tags are stored in their own columns and join tables.
        let mut meta = serde_json::Map::new();
        for (k, v) in &fm.extra {
            if let Ok(jv) = serde_json::to_value(v) {
                meta.insert(k.clone(), jv);
            }
        }
        let mut add_str = |key: &str, value: Option<String>| {
            if let Some(v) = value {
                meta.insert(key.to_string(), serde_json::Value::String(v));
            }
        };
        add_str("source_date", date_str(fm.source_date));
        add_str("last_verified", date_str(fm.last_verified));
        add_str("review_after", date_str(fm.review_after));
        add_str("temporal_confidence", fm.temporal_confidence.clone());
        add_str("resource", fm.resource.clone());

        let observations = engram
            .observations
            .iter()
            .map(|o| ObservationRecord {
                line: o.line,
                category: o.category.clone(),
                content: o.content.clone(),
                tags: o.tags.clone(),
                context: o.context.clone(),
            })
            .collect();
        let relations = engram
            .relations
            .iter()
            .map(|r| RelationRecord {
                line: r.line,
                rel_type: r.rel_type.clone(),
                to_target: r.target.target.clone(),
                to_domain: r.target.domain.clone(),
            })
            .collect();
        let links = engram
            .links
            .iter()
            .map(|l| LinkRecord {
                line: l.line,
                to_target: l.target.target.clone(),
                to_domain: l.target.domain.clone(),
            })
            .collect();

        EngramRecord {
            path: path.to_string(),
            permalink,
            title: fm.title.clone(),
            engram_type: fm.engram_type.clone(),
            status: fm.status.clone().unwrap_or_default(),
            recorded_at: date_str(fm.recorded_at),
            valid_from: date_str(fm.valid_from),
            valid_to: date_str(fm.valid_to),
            timestamp: fm.timestamp.map(|t| t.to_rfc3339()),
            description: fm.description.clone(),
            content: engram.body.clone(),
            metadata: serde_json::Value::Object(meta),
            tags: fm.tags.clone(),
            observations,
            relations,
            links,
            stamp,
        }
    }
}

/// How a search query is matched.
///
/// `Text`, `Title` and `Permalink` are the lexical modes. `Semantic` and
/// `Hybrid` require [`SearchQuery::query_embedding`] and
/// [`SearchQuery::active_model`] to be set by the caller (who owns the embedding
/// provider). `Text` stays the enum default so lexical search never depends on a
/// model; a caller picks `Hybrid` as the interactive default only when the active
/// model has embeddings (see [`StoreInfo`] and the embedding coverage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchMode {
    /// Match query terms against title, description and content.
    #[default]
    Text,
    /// Match against the title only.
    Title,
    /// Match against the permalink only.
    Permalink,
    /// Vector similarity search over chunk embeddings.
    Semantic,
    /// Blended lexical and vector search.
    Hybrid,
}

/// A comparison operator for a metadata filter.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterOp {
    /// Equals.
    Eq(serde_json::Value),
    /// One of a set.
    In(Vec<serde_json::Value>),
    /// Greater than.
    Gt(serde_json::Value),
    /// Greater than or equal.
    Gte(serde_json::Value),
    /// Less than.
    Lt(serde_json::Value),
    /// Less than or equal.
    Lte(serde_json::Value),
    /// Inclusive range `[lo, hi]`.
    Between(serde_json::Value, serde_json::Value),
}

/// A single metadata filter: a frontmatter key and an operator.
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataFilter {
    /// The frontmatter key. Promoted keys map to columns; the rest map to
    /// `json_extract(metadata, '$.key')`.
    pub key: String,
    /// The comparison to apply.
    pub op: FilterOp,
}

/// Parse the wire form of `metadata_filters` into typed filters.
///
/// The wire form is a JSON object keyed by frontmatter key. Each value is either
/// a bare scalar (shorthand for `$eq`) or an operator object with exactly one of
/// `$eq`, `$in`, `$gt`, `$gte`, `$lt`, `$lte` or `$between`. `$in` takes an array
/// and `$between` takes a two-element `[lo, hi]` array. This is the boundary the
/// M5 MCP and CLI layers parse tool arguments through. Models routinely
/// double-encode nested tool arguments, so the whole object arriving as a JSON
/// string is also accepted and parsed first.
pub fn parse_metadata_filters(value: &serde_json::Value) -> Result<Vec<MetadataFilter>> {
    let decoded;
    let value = match value {
        serde_json::Value::String(raw) => {
            decoded = serde_json::from_str::<serde_json::Value>(raw).map_err(|_| {
                crate::IndexError::Invalid("metadata_filters must be an object".into())
            })?;
            &decoded
        }
        other => other,
    };
    let obj = value
        .as_object()
        .ok_or_else(|| crate::IndexError::Invalid("metadata_filters must be an object".into()))?;
    let mut out = Vec::with_capacity(obj.len());
    for (key, spec) in obj {
        let op = match spec {
            serde_json::Value::Object(ops) => {
                if ops.len() != 1 {
                    return Err(crate::IndexError::Invalid(format!(
                        "metadata filter for '{key}' must have exactly one operator"
                    )));
                }
                let (op_name, arg) = ops.iter().next().expect("one entry");
                parse_op(key, op_name, arg)?
            }
            // A bare scalar is shorthand for $eq.
            other => FilterOp::Eq(other.clone()),
        };
        out.push(MetadataFilter {
            key: key.clone(),
            op,
        });
    }
    Ok(out)
}

fn parse_op(key: &str, op: &str, arg: &serde_json::Value) -> Result<FilterOp> {
    let arr2 = |arg: &serde_json::Value| -> Result<(serde_json::Value, serde_json::Value)> {
        match arg.as_array() {
            Some(a) if a.len() == 2 => Ok((a[0].clone(), a[1].clone())),
            _ => Err(crate::IndexError::Invalid(format!(
                "{op} on '{key}' expects a two-element array"
            ))),
        }
    };
    Ok(match op {
        "$eq" => FilterOp::Eq(arg.clone()),
        "$gt" => FilterOp::Gt(arg.clone()),
        "$gte" => FilterOp::Gte(arg.clone()),
        "$lt" => FilterOp::Lt(arg.clone()),
        "$lte" => FilterOp::Lte(arg.clone()),
        "$in" => FilterOp::In(
            arg.as_array()
                .ok_or_else(|| {
                    crate::IndexError::Invalid(format!("$in on '{key}' expects an array"))
                })?
                .clone(),
        ),
        "$between" => {
            let (lo, hi) = arr2(arg)?;
            FilterOp::Between(lo, hi)
        }
        other => {
            return Err(crate::IndexError::Invalid(format!(
                "unknown operator '{other}' on '{key}'"
            )));
        }
    })
}

/// A search request. Filter-only searches (no `text`) are allowed.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// The free-text query, or `None` for a filter-only search.
    pub text: Option<String>,
    /// Restrict to these domain names; `None` searches all domains.
    pub domains: Option<Vec<String>>,
    /// Filter by `type`.
    pub engram_type: Option<String>,
    /// Filter by `status`.
    pub status: Option<String>,
    /// Require all of these tags.
    pub tags: Option<Vec<String>>,
    /// Arbitrary metadata filters.
    pub metadata_filters: Vec<MetadataFilter>,
    /// Only engrams with `recorded_at >= after`.
    pub after: Option<String>,
    /// Apply the canonical current filter as of `today` (or the real today when
    /// `today` is `None`). Overrides `status` with `status = 'current'`.
    pub current_only: bool,
    /// The date used by `current_only`, ISO `YYYY-MM-DD`.
    pub today: Option<String>,
    /// The match mode.
    pub mode: SearchMode,
    /// Minimum cosine similarity for a semantic hit, `None` uses the store
    /// default (`0.55`). Ignored by the text, title and permalink modes.
    pub min_similarity: Option<f32>,
    /// The salience-prior weight for hybrid ranking; `None` uses the store
    /// default (`DEFAULT_SALIENCE_WEIGHT`). The maximum lift a fully-salient
    /// engram receives on the normalized relevance score. A soft additive
    /// prior, never a filter: it can reorder within a relevance band but never
    /// excludes a result.
    pub salience_weight: Option<f64>,
    /// The query text already embedded by the active provider, required by the
    /// semantic and hybrid modes. The caller owns the provider and embeds the
    /// query (with the query instruction prefix) before calling the store, which
    /// keeps the backend free of any model dependency. Its length is the active
    /// dimensionality.
    pub query_embedding: Option<Vec<f32>>,
    /// The active provider's model id, paired with `query_embedding` for the
    /// staleness check. Required by the semantic and hybrid modes.
    pub active_model: Option<String>,
    /// Page size.
    pub limit: usize,
    /// One-based page number.
    pub page: usize,
}

impl SearchQuery {
    /// A text query over all domains with sane paging defaults.
    pub fn text(query: &str) -> SearchQuery {
        SearchQuery {
            text: Some(query.to_string()),
            limit: 10,
            page: 1,
            ..SearchQuery::default()
        }
    }
}

/// Whether a hit is a whole engram or a single observation within one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum HitKind {
    /// The engram itself matched.
    Engram,
    /// An observation matched; carries its source line.
    Observation {
        /// One-based source line of the observation.
        line: usize,
    },
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchHit {
    /// The domain the hit lives in.
    pub domain: String,
    /// The engram permalink.
    pub permalink: String,
    /// The engram title.
    pub title: String,
    /// A snippet cut around the match, or a lead-in for filter-only searches.
    pub snippet: String,
    /// The relevance score; higher is more relevant.
    pub score: f64,
    /// The engram `type`.
    pub engram_type: String,
    /// The engram `status`.
    pub status: String,
    /// The engram's tags, alphabetical and folded to lowercase; empty when the
    /// engram is untagged. Every hit passively teaches the querying agent the
    /// existing vocabulary. Observation-kind hits carry their engram's tags.
    pub tags: Vec<String>,
    /// Whether the hit is the engram or one of its observations.
    #[serde(flatten)]
    pub kind: HitKind,
}

/// A page of results plus the total match count for the same filters.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page<T> {
    /// The results for this page.
    pub items: Vec<T>,
    /// One-based page number.
    pub page: usize,
    /// Page size.
    pub limit: usize,
    /// Total matches across all pages.
    pub total: usize,
}

/// A node in a graph slice.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphNode {
    /// The engram id.
    pub id: EngramId,
    /// The domain the engram lives in.
    pub domain: String,
    /// The engram permalink.
    pub permalink: String,
    /// The engram title.
    pub title: String,
    /// The engram `type`.
    pub engram_type: String,
    /// The raw `salience` frontmatter value, `None` when absent. A non-numeric
    /// value reads as neutral too, though not byte-identically across backends:
    /// Postgres's `jsonb_typeof` guard yields `None`, Turso's `CAST` yields
    /// `Some(0.0)`. Both are zero lift for ranking, so callers must compare
    /// against neutral (`<= 0.0` or absent), never assert an exact `None`.
    pub salience: Option<f64>,
}

/// Whether an edge came from a relation bullet or a prose wikilink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// A `- rel_type [[Target]]` relation.
    Relation,
    /// A prose `[[Target]]` wikilink.
    Link,
}

/// A directed, typed edge between two engrams. May cross domains.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphEdge {
    /// The source engram id.
    pub from: EngramId,
    /// The target engram id.
    pub to: EngramId,
    /// The relation type; `links_to` for wikilinks.
    pub rel_type: String,
    /// Whether the edge is a relation or a link.
    pub kind: EdgeKind,
}

/// The neighborhood of a set of seed engrams: nodes plus typed edges.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GraphSlice {
    /// Every engram reachable within the requested depth, seeds included.
    pub nodes: Vec<GraphNode>,
    /// Every edge among those nodes, including cross-domain edges.
    pub edges: Vec<GraphEdge>,
}

/// The frontmatter salience scale: a salience of `SALIENCE_SCALE` maps to the
/// full prior; absent or non-positive salience adds nothing.
const SALIENCE_SCALE: f64 = 10.0;
/// The default salience-prior weight when a query does not override it: the
/// maximum lift a fully-salient engram gets on the normalized [0,1] relevance
/// score. Small on purpose so relevance dominates and the prior only reorders
/// within a relevance band, never filters.
pub const DEFAULT_SALIENCE_WEIGHT: f64 = 0.15;

/// The bounded, non-negative salience prior added to a normalized relevance
/// score. `salience` is the raw frontmatter value (`None` or non-positive means
/// no lift); `weight` is the maximum lift. The result is never negative and
/// never exceeds `weight`, so it can reorder within a relevance band but can
/// never exclude a result.
pub fn salience_prior(salience: Option<f64>, weight: f64) -> f64 {
    let s = salience.unwrap_or(0.0);
    if s <= 0.0 || weight <= 0.0 {
        return 0.0;
    }
    weight * (s / SALIENCE_SCALE).clamp(0.0, 1.0)
}

/// A filter for recent activity.
#[derive(Debug, Clone, Default)]
pub struct RecentFilter {
    /// Restrict to these domain names; `None` covers all domains.
    pub domains: Option<Vec<String>>,
    /// Only engrams with `recorded_at >= after`.
    pub after: Option<String>,
    /// Restrict to these `type` values.
    pub engram_types: Option<Vec<String>>,
    /// Maximum rows to return.
    pub limit: usize,
}

/// A compact engram summary for listings.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EngramSummary {
    /// The domain the engram lives in.
    pub domain: String,
    /// The engram permalink.
    pub permalink: String,
    /// The engram title.
    pub title: String,
    /// The engram `type`.
    pub engram_type: String,
    /// The engram `status`.
    pub status: String,
    /// `recorded_at` as an ISO date string.
    pub recorded_at: Option<String>,
    /// Tags.
    pub tags: Vec<String>,
}

/// A lightweight engram descriptor, enough to locate its file and address it.
///
/// Returned by the [`Store`] lookup methods that back identifier resolution,
/// browsing, validation and schema inference. It carries the ids so a caller can
/// go straight to a graph traversal or a single-file upsert without a second
/// query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EngramDescriptor {
    /// The engram id.
    pub id: EngramId,
    /// The owning domain id.
    pub domain_id: DomainId,
    /// The domain name.
    pub domain: String,
    /// The domain-relative file path, forward-slashed, with the `.md` suffix.
    pub path: String,
    /// The engram permalink.
    pub permalink: String,
    /// The engram title.
    pub title: String,
    /// The engram `type`.
    pub engram_type: String,
    /// The engram `status`.
    pub status: String,
}

/// A stored engram's addressing plus its full markdown content and checksum.
/// Returned by [`Store::all_engram_contents`] to materialize a whole domain for
/// `domain export`, and shaped so an export writes each engram back to disk
/// verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredEngram {
    /// The domain-relative file path, forward-slashed, with the `.md` suffix.
    pub path: String,
    /// The engram permalink.
    pub permalink: String,
    /// The full markdown content (frontmatter plus body) as stored.
    pub content: String,
    /// The lowercase hex SHA-256 of `content`, the CAS token.
    pub sha256: String,
}

/// One inbound reference to an engram: a relation or a prose link that resolves
/// to it. Used by the cross-domain move to rewrite linkers to the domain-prefixed
/// form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InboundRef {
    /// The linking engram's domain name.
    pub src_domain: String,
    /// The linking engram's domain id.
    pub src_domain_id: DomainId,
    /// The linking engram's domain-relative file path.
    pub src_path: String,
    /// The exact target text used in the link.
    pub to_target: String,
    /// Whether the reference came from a relation bullet or a prose link.
    pub kind: EdgeKind,
}

/// One outbound reference from an engram: a relation bullet or a prose wikilink,
/// with whether it currently resolves to a target in the index. Backs the
/// `read_engram` resolution flags so a reading agent learns which of its links
/// land and which dangle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutboundRef {
    /// One-based source line.
    pub line: usize,
    /// Whether the reference came from a relation bullet or a prose link.
    pub kind: EdgeKind,
    /// The relation type for a relation edge; `None` for a prose link.
    pub rel_type: Option<String>,
    /// The exact target text used in the reference.
    pub to_target: String,
    /// An explicit cross-domain target domain, or `None` for same-domain.
    pub to_domain: Option<String>,
    /// Whether the reference currently resolves to a target in the index.
    pub resolved: bool,
}

/// A chunk awaiting an embedding. Populated by M4; the M3 store returns none.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkJob {
    /// The chunk row id.
    pub chunk_id: i64,
    /// The owning engram id.
    pub engram_id: i64,
    /// The chunk sequence within the engram.
    pub seq: i64,
    /// The chunk text.
    pub text: String,
    /// The chunk fingerprint `sha256(model_id + text)`.
    pub text_hash: String,
}

/// A computed embedding to store back against a chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingRow {
    /// The chunk row id.
    pub chunk_id: i64,
    /// The embedding vector.
    pub embedding: Vec<f32>,
    /// The vector dimensionality.
    pub dims: usize,
}

/// A freshly computed chunk to store against an engram. Produced by the chunker
/// and handed to [`Store::replace_chunks`], which reconciles it against the
/// engram's existing chunk rows and carries over any matching embedding.
#[derive(Debug, Clone, PartialEq)]
pub struct NewChunk {
    /// The chunk sequence within the engram, zero-based.
    pub seq: i64,
    /// The chunk text.
    pub text: String,
    /// The chunk fingerprint `sha256(model_id + ":" + text)`.
    pub text_hash: String,
}

/// Embedded-chunk counts for one `(model, dims)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChunkModelCount {
    /// The embedding model that produced these chunks.
    pub model: String,
    /// The vector dimensionality.
    pub dims: usize,
    /// The number of embedded chunks with this model and dimensionality.
    pub count: usize,
}

/// Embedding coverage across the whole index: how many chunks exist and how many
/// are embedded, broken down by model. Callers derive per-model coverage,
/// staleness and the interactive default search mode from this.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct EmbeddingCoverage {
    /// Total chunk rows in the index.
    pub total_chunks: usize,
    /// Chunk rows that carry an embedding, across all models.
    pub embedded_chunks: usize,
    /// Embedded-chunk counts per `(model, dims)`, most chunks first.
    pub models: Vec<ChunkModelCount>,
}

impl EmbeddingCoverage {
    /// Embedded chunks produced by the given model.
    pub fn embedded_for(&self, model: &str) -> usize {
        self.models
            .iter()
            .filter(|m| m.model == model)
            .map(|m| m.count)
            .sum()
    }

    /// Whether the given model has at least one embedded chunk, so a caller may
    /// default interactive search to the hybrid mode.
    pub fn has_active_embeddings(&self, model: &str) -> bool {
        self.embedded_for(model) > 0
    }

    /// Chunks still awaiting embedding by the given model: the total minus what
    /// that model has already embedded.
    pub fn backlog_for(&self, model: &str) -> usize {
        self.total_chunks.saturating_sub(self.embedded_for(model))
    }
}

/// Which full-text path the store is using.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FtsMode {
    /// A native FTS5 virtual table is active.
    Native,
    /// The LIKE-candidate scan fallback is active.
    CandidateScan,
}

/// Diagnostics about the open store.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoreInfo {
    /// Which full-text path is active.
    pub fts_mode: FtsMode,
    /// The applied schema version.
    pub schema_version: i64,
    /// The database file path, or `None` for an in-memory store.
    pub db_path: Option<String>,
    /// The database file size in bytes, when known.
    pub db_size: Option<u64>,
}

/// Per-domain counts for `status` and `domain list`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DomainStats {
    /// The domain name.
    pub name: String,
    /// The domain root path as registered.
    pub path: String,
    /// The domain kind, `file` (default) or `virtual`.
    pub kind: DomainKind,
    /// Number of engrams.
    pub engrams: i64,
    /// Number of observations.
    pub observations: i64,
    /// Number of relations.
    pub relations: i64,
    /// Number of relations still awaiting their target (forward refs).
    pub unresolved_relations: i64,
    /// Number of prose wikilinks.
    pub links: i64,
    /// Number of prose wikilinks still awaiting their target (forward refs).
    pub unresolved_links: i64,
    /// Last successful sync, RFC 3339, or `None` if never synced.
    pub last_sync: Option<String>,
    /// The instance id currently hosting this file domain in a shared database,
    /// or `None` when unhosted (every domain in a single-instance deployment, and
    /// every virtual domain, which never take a host lock).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_instance_id: Option<String>,
    /// The host's human label, when hosted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_label: Option<String>,
    /// The host's last heartbeat, RFC 3339, when hosted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_heartbeat_at: Option<String>,
}

/// The instance currently holding a file domain's host lock in a shared
/// database. Returned by [`Store::domain_host`] and carried by
/// [`HostClaim::HeldByOther`] so a refused sync can name the host and its last
/// heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DomainHost {
    /// The holding instance's stable id.
    pub instance_id: String,
    /// The holding instance's human label.
    pub label: String,
    /// The host's last heartbeat, RFC 3339.
    pub heartbeat_at: String,
}

/// The outcome of a [`Store::claim_domain_host`]: this instance acquired (or
/// renewed, or took over) the host lock, or another live instance still holds
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum HostClaim {
    /// This instance holds the host lock now.
    Acquired,
    /// Another instance holds it and the claim did not qualify for takeover
    /// (not stale, no `take_over`).
    HeldByOther(DomainHost),
}

/// One tag in use, with how many engrams and how many observations carry it.
/// The two counts are separate scans so a tag applied on the frontmatter and one
/// applied on an observation are both visible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagCount {
    /// The tag name, without the leading `#`.
    pub name: String,
    /// Number of engrams tagged with it on their frontmatter.
    pub engrams: i64,
    /// Number of observations tagged with it.
    pub observations: i64,
}

/// One folded tag-alias mapping in effect: an `alias` that folds onto a
/// `canonical` at query time. Derived purely from MANIFEST declarations, so it
/// carries no usage count; the mapping, not a marker on [`TagCount`], is what
/// tells an agent two spellings are one tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagAlias {
    /// The aliased (old) spelling, folded to lowercase.
    pub alias: String,
    /// The canonical spelling it folds onto, folded to lowercase.
    pub canonical: String,
}

/// One named vocabulary term with its usage count: an observation category or a
/// relation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NamedCount {
    /// The term.
    pub name: String,
    /// How many times it is used.
    pub count: i64,
}

/// The vocabulary in use across a domain or the whole index: tags with their
/// engram and observation usage counts, observation categories with counts and
/// relation types with counts. Backs the `vocabulary` tool so an agent reuses an
/// existing term instead of coining a near-duplicate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Vocabulary {
    /// Tags, most-used first (engrams + observations), then by name.
    pub tags: Vec<TagCount>,
    /// Observation categories, most-used first, then by name.
    pub categories: Vec<NamedCount>,
    /// Relation types, most-used first, then by name.
    pub relation_types: Vec<NamedCount>,
    /// Tag aliases in effect, sorted by alias then canonical. Derived from the
    /// scanned domains' MANIFEST declarations, so an agent sees which spellings
    /// fold onto which and reuses the canonical.
    pub aliases: Vec<TagAlias>,
}

/// Merge the four decoded vocabulary aggregates into a sorted [`Vocabulary`].
/// Shared by both backends so the ordering is identical regardless of SQL's
/// grouping order: engram-tag and observation-tag counts merge by tag name, tags
/// sort by total usage (engrams + observations) descending then name, and
/// categories and relation types sort by count descending then name.
pub(crate) fn build_vocabulary(
    engram_tags: Vec<(String, i64)>,
    observation_tags: Vec<(String, i64)>,
    categories: Vec<(String, i64)>,
    relation_types: Vec<(String, i64)>,
    aliases: Vec<(String, String)>,
) -> Vocabulary {
    let mut merged: HashMap<String, (i64, i64)> = HashMap::new();
    for (name, count) in engram_tags {
        merged.entry(name).or_default().0 = count;
    }
    for (name, count) in observation_tags {
        merged.entry(name).or_default().1 = count;
    }
    let mut tags: Vec<TagCount> = merged
        .into_iter()
        .map(|(name, (engrams, observations))| TagCount {
            name,
            engrams,
            observations,
        })
        .collect();
    tags.sort_by(|a, b| {
        (b.engrams + b.observations)
            .cmp(&(a.engrams + a.observations))
            .then_with(|| a.name.cmp(&b.name))
    });

    let to_named = |rows: Vec<(String, i64)>| -> Vec<NamedCount> {
        let mut v: Vec<NamedCount> = rows
            .into_iter()
            .map(|(name, count)| NamedCount { name, count })
            .collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        v
    };

    // Aliases arrive deduped from a `SELECT DISTINCT`, but an all-domain sweep
    // can still surface the same pair from two domains; dedupe and sort here so
    // the order never depends on SQL grouping, matching the tag ordering above.
    let mut aliases: Vec<TagAlias> = aliases
        .into_iter()
        .map(|(alias, canonical)| TagAlias { alias, canonical })
        .collect();
    aliases.sort_by(|a, b| {
        a.alias
            .cmp(&b.alias)
            .then_with(|| a.canonical.cmp(&b.canonical))
    });
    aliases.dedup();

    Vocabulary {
        tags,
        categories: to_named(categories),
        relation_types: to_named(relation_types),
        aliases,
    }
}

/// The backend-agnostic storage interface. All methods are async so a network
/// backend can implement the same trait.
#[async_trait]
pub trait Store: Send + Sync {
    /// Apply any pending schema migrations. Idempotent.
    async fn migrate(&self) -> Result<()>;

    /// Register or update a domain by name, root path and kind, returning its
    /// id. A file domain passes `Some(path)`; a virtual domain passes `None`,
    /// since its engrams live in the database with no filesystem root.
    async fn upsert_domain(
        &self,
        name: &str,
        path: Option<&str>,
        kind: DomainKind,
    ) -> Result<DomainId>;

    /// The recorded file stamps for a domain, keyed by domain-relative path.
    async fn file_stamps(&self, domain: DomainId) -> Result<HashMap<String, FileStamp>>;

    /// Insert or update one engram, replacing its observations, relations, links
    /// and tags. Returns the engram id. A duplicate permalink (a different path
    /// already owning the permalink) is a [`crate::IndexError::Constraint`].
    async fn upsert_engram(&self, domain: DomainId, record: &EngramRecord) -> Result<EngramId>;

    /// Like [`Store::upsert_engram`], but a compare-and-swap on the stored
    /// content. When `expected_sha` is `Some` and a row already exists at
    /// `record.path` whose stored `sha256` differs, this returns
    /// [`crate::IndexError::StaleEdit`] instead of writing; otherwise it behaves
    /// exactly like `upsert_engram`. The compare and the write are one atomic
    /// unit (the engine wraps the call in a transaction). This is the guard that
    /// makes a stale virtual edit a conflict rather than a silent clobber.
    async fn upsert_engram_checked(
        &self,
        domain: DomainId,
        record: &EngramRecord,
        expected_sha: Option<&str>,
    ) -> Result<EngramId>;

    /// The full markdown content stored for the engram at a domain-relative
    /// path, or `None` when no such row exists. Serves the database read path
    /// (virtual domains and non-host reads) and reads current content for a
    /// virtual edit.
    async fn engram_content(&self, domain: DomainId, path: &str) -> Result<Option<String>>;

    /// Every engram in a domain with its path, permalink, content and checksum,
    /// ordered by path. Streams a whole domain for `domain export`.
    async fn all_engram_contents(&self, domain: DomainId) -> Result<Vec<StoredEngram>>;

    /// Delete every engram (and its child and chunk rows) in a single domain,
    /// keeping the domain row itself. This is the scoped clear the full reindex
    /// uses per file domain so virtual-domain rows, whose only source of truth is
    /// the database, are never destroyed. Contrast [`Store::wipe`], which clears
    /// everything.
    async fn clear_domain(&self, domain: DomainId) -> Result<()>;

    /// Delete the engram at a domain-relative path and all its child rows.
    async fn delete_engram(&self, domain: DomainId, path: &str) -> Result<()>;

    /// Move an engram from one path to another without reparsing. The permalink
    /// follows only when it was path-derived.
    async fn rename_engram(&self, domain: DomainId, from: &str, to: &str) -> Result<()>;

    /// Resolve every pending forward reference in a domain in one batch,
    /// matching the target text against permalink then title within the target
    /// domain. Returns the number of relations newly resolved.
    async fn resolve_pending_relations(&self, domain: DomainId) -> Result<u64>;

    /// Resolve every pending prose wikilink in a domain in one batch, the same
    /// permalink-then-title match as [`Store::resolve_pending_relations`] but
    /// over the `link` table. Wikilinks carry no relation type, so a resolved
    /// link becomes a `links_to` edge in graph traversal. Returns the number of
    /// links newly resolved.
    async fn resolve_pending_links(&self, domain: DomainId) -> Result<u64>;

    // --- lookups -------------------------------------------------------------
    // Repository-level addressing helpers used by the service layer to turn a
    // tool identifier or a `crystalline://` anchor into ids and a file path.
    // They are on the trait (not backend-specific) so the engine can hold a
    // `dyn Store` and a second backend reimplements them in its own dialect.

    /// Resolve a `(domain, permalink)` pair to an engram id. Backs graph anchors
    /// and `crystalline://` resolution.
    async fn lookup_id(&self, domain: &str, permalink: &str) -> Result<Option<EngramId>>;

    /// Resolve an identifier within one domain to a descriptor. Matches the
    /// permalink first, then the title case-insensitively.
    async fn find_engram(&self, domain: &str, key: &str) -> Result<Option<EngramDescriptor>>;

    /// Resolve an identifier across every domain, permalink first then title.
    /// Returns all matches so the caller can detect an ambiguous bare identifier.
    async fn find_engram_any(&self, key: &str) -> Result<Vec<EngramDescriptor>>;

    /// List engrams in a domain, optionally under a domain-relative path prefix
    /// and optionally of a given type, ordered by path. Backs browse, validate
    /// and schema inference.
    async fn list_engrams(
        &self,
        domain: &str,
        path_prefix: Option<&str>,
        engram_type: Option<&str>,
    ) -> Result<Vec<EngramDescriptor>>;

    /// Every engram carrying a tag, on its frontmatter or on one of its
    /// observations, optionally scoped to one domain. The bound tag is
    /// case-folded to match the case-folded `tag.name`. Ordered by domain name
    /// then path so a `tags rename` / `tags merge` rewrites files in a stable
    /// order. Backs the CLI tag-hygiene commands.
    async fn engrams_with_tag(
        &self,
        tag: &str,
        domain: Option<&str>,
    ) -> Result<Vec<EngramDescriptor>>;

    /// Replace a domain's derived tag-alias rows with the given folded
    /// `(alias, canonical)` pairs. Deletes every existing row for the domain,
    /// then inserts each pair, ignoring a duplicate alias. Called by
    /// [`crate::sync::refresh_tag_aliases`] on every sync, so an empty slice
    /// clears the domain's aliases. Both sides are already lowercased by the
    /// caller; the store never parses markdown.
    async fn replace_tag_aliases(&self, domain: DomainId, pairs: &[(String, String)])
    -> Result<()>;

    /// The distinct folded `(alias, canonical)` tag-alias pairs, optionally
    /// scoped to a set of domain names, ordered by alias then canonical. An
    /// all-domain sweep (no scope) unions every domain's map, so one alias can
    /// come back paired with more than one canonical. Backs query-time alias
    /// expansion and the vocabulary surface.
    async fn tag_aliases(&self, domains: Option<&[String]>) -> Result<Vec<(String, String)>>;

    /// Every relation or prose link that points at the given engram, with the
    /// linking engram's path and the exact target text. Used by the cross-domain
    /// move to rewrite inbound links.
    async fn inbound_refs(
        &self,
        engram_id: EngramId,
        domain_id: DomainId,
        permalink: &str,
        title: &str,
    ) -> Result<Vec<InboundRef>>;

    /// Every relation and prose link that points out of the given engram, each
    /// carrying whether it currently resolves to a target in the index. Ordered
    /// by source line. Backs the `read_engram` resolution flags.
    async fn outbound_refs(&self, engram_id: EngramId) -> Result<Vec<OutboundRef>>;

    /// Run a search and return one page of hits plus the total match count.
    async fn search(&self, query: &SearchQuery) -> Result<Page<SearchHit>>;

    /// Return the neighborhood of a set of seed engrams up to `depth` hops
    /// (`1..=3`), following relations and links across domain boundaries.
    async fn neighbors(&self, ids: &[EngramId], depth: u8) -> Result<GraphSlice>;

    /// Return recent engrams matching a filter, newest first.
    async fn recent(&self, filter: &RecentFilter) -> Result<Vec<EngramSummary>>;

    /// Replace an engram's chunk rows with a freshly computed set, carrying over
    /// the embedding of any chunk whose fingerprint is unchanged so an edit only
    /// re-embeds the paragraphs that actually changed. Called by the sync engine
    /// after each upsert.
    async fn replace_chunks(&self, engram_id: EngramId, chunks: &[NewChunk]) -> Result<()>;

    /// Return chunks that need an embedding for the given model: those with no
    /// embedding yet, plus those embedded by a different model (a model swap).
    /// `domains`, when `Some`, restricts the scan to those domains: the daemon
    /// scopes each instance's embed pass to the file domains it hosts plus all
    /// virtual domains, so a non-host does not wastefully re-embed a chunk
    /// another instance owns. `None` scans every domain (the single-instance and
    /// standalone default). Duplicate embedding across instances is always safe
    /// (the chunk fingerprint folds in the model id, so two instances converge
    /// on the same vector), just wasteful, and this filter avoids the waste.
    async fn chunks_needing_embedding(
        &self,
        model: &str,
        domains: Option<&[DomainId]>,
    ) -> Result<Vec<ChunkJob>>;

    /// Store a batch of embeddings against their chunks for the given model.
    async fn store_embeddings(&self, batch: &[EmbeddingRow], model: &str) -> Result<()>;

    /// Embedding coverage across the index: total chunks, embedded chunks and a
    /// per-model breakdown. Drives `status` reporting and the interactive
    /// default search mode.
    async fn embedding_coverage(&self) -> Result<EmbeddingCoverage>;

    /// Delete all indexed data, keeping the schema. The corruption-recovery and
    /// full-reindex path.
    async fn wipe(&self) -> Result<()>;

    /// Best-effort WAL checkpoint in TRUNCATE mode, shrinking a local WAL file
    /// back down after a sync or reindex, full or incremental, or a wipe. Any
    /// of these may precede a caller shipping the database file as a
    /// single-file snapshot (sidecars deleted), so the WAL must not be left
    /// holding an un-merged delta. The default is a no-op, which is correct
    /// for Postgres: it has no local WAL file to truncate. Turso overrides
    /// this with a real `PRAGMA wal_checkpoint(TRUNCATE)` (confirmed to work
    /// by a runtime probe; see the doc comment on `TursoStore::build`).
    async fn checkpoint_wal(&self) -> Result<()> {
        Ok(())
    }

    /// Record that a domain finished syncing at the given RFC 3339 instant.
    async fn record_sync(&self, domain: DomainId, when: &str) -> Result<()>;

    /// Diagnostics about the open store.
    async fn store_info(&self) -> Result<StoreInfo>;

    /// Per-domain counts, in registration order.
    async fn domain_stats(&self) -> Result<Vec<DomainStats>>;

    /// The vocabulary in use: tag, observation-category and relation-type usage
    /// counts, for one domain or (when `domain` is `None`) across every domain.
    /// An unknown domain name yields empty vectors rather than an error, so a
    /// caller can probe a domain that holds no engrams yet. The vectors are
    /// sorted by usage in Rust for cross-backend determinism.
    async fn vocabulary(&self, domain: Option<&str>) -> Result<Vocabulary>;

    // --- host locks ----------------------------------------------------------
    // The single-writer-per-file-domain rule for shared-database collaboration.
    // Only file domains take a host lock; a virtual domain's concurrency is
    // engram-level compare-and-swap, so the daemon never claims one. All four
    // are backend-agnostic: an atomic upsert plus a re-read on Turso, the same
    // with `RETURNING` on Postgres, over a `domain_lock` table.

    /// Claim the host lock for a file domain. Inserts when unheld; takes over
    /// when `take_over` is set or the current holder's `heartbeat_at` is older
    /// than `stale_before`; otherwise leaves the existing holder untouched. Times
    /// (`now`, `stale_before`) are RFC 3339 strings the caller computes from its
    /// stale threshold, so the comparison stays a plain lexical one, matching the
    /// temporal columns. Returns [`HostClaim::Acquired`] when this instance holds
    /// the lock afterward, else [`HostClaim::HeldByOther`] naming the live holder.
    async fn claim_domain_host(
        &self,
        domain: DomainId,
        instance_id: &str,
        label: &str,
        now: &str,
        stale_before: &str,
        take_over: bool,
    ) -> Result<HostClaim>;

    /// Refresh this instance's heartbeat on a lock it holds. Returns `true` when
    /// a row was updated (this instance still holds it), `false` when it does not
    /// (another instance took over), which tells the daemon to stop hosting.
    async fn renew_domain_host(
        &self,
        domain: DomainId,
        instance_id: &str,
        now: &str,
    ) -> Result<bool>;

    /// Release a lock this instance holds (graceful shutdown). A no-op when this
    /// instance is not the current holder, so a released takeover never deletes
    /// the new host's lock.
    async fn release_domain_host(&self, domain: DomainId, instance_id: &str) -> Result<()>;

    /// The current holder of a file domain's host lock, or `None` when unheld.
    async fn domain_host(&self, domain: DomainId) -> Result<Option<DomainHost>>;

    // --- batch control -------------------------------------------------------
    // These let the sync engine wrap its whole write phase in one transaction
    // for speed. A backend without explicit transactions may implement them as
    // no-ops.

    /// Begin a write transaction.
    async fn begin(&self) -> Result<()>;
    /// Commit the current write transaction.
    async fn commit(&self) -> Result<()>;
    /// Roll back the current write transaction.
    async fn rollback(&self) -> Result<()>;
}

#[cfg(test)]
mod salience_tests {
    use super::{DEFAULT_SALIENCE_WEIGHT, salience_prior};

    #[test]
    fn salience_prior_absent_is_neutral() {
        assert_eq!(salience_prior(None, 0.15), 0.0);
    }

    #[test]
    fn salience_prior_zero_or_negative_is_neutral() {
        assert_eq!(salience_prior(Some(0.0), 0.15), 0.0);
        assert_eq!(salience_prior(Some(-4.0), 0.15), 0.0);
    }

    #[test]
    fn salience_prior_max_is_weight() {
        assert!((salience_prior(Some(10.0), 0.15) - 0.15).abs() < 1e-9);
    }

    #[test]
    fn salience_prior_is_clamped_above_scale() {
        assert!((salience_prior(Some(50.0), 0.15) - 0.15).abs() < 1e-9);
    }

    #[test]
    fn salience_prior_is_monotonic_and_bounded() {
        let low = salience_prior(Some(3.0), 0.15);
        let high = salience_prior(Some(7.0), 0.15);
        assert!(low < high);
        assert!(high <= 0.15 + 1e-9);
    }

    #[test]
    fn salience_prior_zero_weight_disables() {
        assert_eq!(salience_prior(Some(9.0), 0.0), 0.0);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn default_weight_is_conservative() {
        assert!(DEFAULT_SALIENCE_WEIGHT > 0.0 && DEFAULT_SALIENCE_WEIGHT < 0.5);
    }
}
