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
/// `Text`, `Title` and `Permalink` are the M3 modes. `Semantic` and `Hybrid`
/// are declared now so the enum and the [`SearchQuery`] shape do not change when
/// the embedding pipeline lands in M4; the store returns
/// [`crate::IndexError::Unsupported`] for them until then.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchMode {
    /// Match query terms against title, description and content.
    #[default]
    Text,
    /// Match against the title only.
    Title,
    /// Match against the permalink only.
    Permalink,
    /// Vector similarity search (M4).
    Semantic,
    /// Blended text and vector search (M4).
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
/// M5 MCP and CLI layers parse tool arguments through.
pub fn parse_metadata_filters(value: &serde_json::Value) -> Result<Vec<MetadataFilter>> {
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
    /// Number of engrams.
    pub engrams: i64,
    /// Number of observations.
    pub observations: i64,
    /// Number of relations.
    pub relations: i64,
    /// Number of relations still awaiting their target (forward refs).
    pub unresolved_relations: i64,
    /// Last successful sync, RFC 3339, or `None` if never synced.
    pub last_sync: Option<String>,
}

/// The backend-agnostic storage interface. All methods are async so a network
/// backend can implement the same trait.
#[async_trait]
pub trait Store: Send + Sync {
    /// Apply any pending schema migrations. Idempotent.
    async fn migrate(&self) -> Result<()>;

    /// Register or update a domain by name and root path, returning its id.
    async fn upsert_domain(&self, name: &str, path: &str) -> Result<DomainId>;

    /// The recorded file stamps for a domain, keyed by domain-relative path.
    async fn file_stamps(&self, domain: DomainId) -> Result<HashMap<String, FileStamp>>;

    /// Insert or update one engram, replacing its observations, relations, links
    /// and tags. Returns the engram id. A duplicate permalink (a different path
    /// already owning the permalink) is a [`crate::IndexError::Constraint`].
    async fn upsert_engram(&self, domain: DomainId, record: &EngramRecord) -> Result<EngramId>;

    /// Delete the engram at a domain-relative path and all its child rows.
    async fn delete_engram(&self, domain: DomainId, path: &str) -> Result<()>;

    /// Move an engram from one path to another without reparsing. The permalink
    /// follows only when it was path-derived.
    async fn rename_engram(&self, domain: DomainId, from: &str, to: &str) -> Result<()>;

    /// Resolve every pending forward reference in a domain in one batch,
    /// matching the target text against permalink then title within the target
    /// domain. Returns the number of relations newly resolved.
    async fn resolve_pending_relations(&self, domain: DomainId) -> Result<u64>;

    /// Run a search and return one page of hits plus the total match count.
    async fn search(&self, query: &SearchQuery) -> Result<Page<SearchHit>>;

    /// Return the neighborhood of a set of seed engrams up to `depth` hops
    /// (`1..=3`), following relations and links across domain boundaries.
    async fn neighbors(&self, ids: &[EngramId], depth: u8) -> Result<GraphSlice>;

    /// Return recent engrams matching a filter, newest first.
    async fn recent(&self, filter: &RecentFilter) -> Result<Vec<EngramSummary>>;

    /// Return chunks that need an embedding for the given model.
    async fn chunks_needing_embedding(&self, model: &str) -> Result<Vec<ChunkJob>>;

    /// Store a batch of embeddings against their chunks for the given model.
    async fn store_embeddings(&self, batch: &[EmbeddingRow], model: &str) -> Result<()>;

    /// Delete all indexed data, keeping the schema. The corruption-recovery and
    /// full-reindex path.
    async fn wipe(&self) -> Result<()>;

    /// Record that a domain finished syncing at the given RFC 3339 instant.
    async fn record_sync(&self, domain: DomainId, when: &str) -> Result<()>;

    /// Diagnostics about the open store.
    async fn store_info(&self) -> Result<StoreInfo>;

    /// Per-domain counts, in registration order.
    async fn domain_stats(&self) -> Result<Vec<DomainStats>>;

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
