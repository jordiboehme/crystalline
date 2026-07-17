//! The search planner and graph traversal for the Turso backend.
//!
//! Native FTS5 is unavailable in Turso 0.6.1, so text search is a LIKE-candidate
//! prefilter ranked in Rust by a weighted term-frequency score (title x3,
//! description x2, content x1). Filters (type, status, tags, the canonical
//! temporal window and arbitrary metadata) are pushed into SQL; the temporal and
//! promoted keys hit indexed columns while everything else routes through
//! `json_extract(metadata, '$.key')`. Snippets are cut around the first match.

use std::collections::HashSet;

use turso::{Connection, Row, Value};

use crate::error::{IndexError, Result};
use crate::store::{
    EdgeKind, EmbeddingCoverage, EngramId, FilterOp, GraphEdge, GraphNode, GraphSlice, HitKind,
    MetadataFilter, Page, SearchHit, SearchMode, SearchQuery,
};

use super::{cell_i64, cell_real, cell_text, query_all, query_first, scalar_i64};

const SNIPPET_MARGIN: usize = 70;
const SNIPPET_LEAD: usize = 200;

/// The default minimum cosine similarity for a semantic hit.
pub(super) const DEFAULT_MIN_SIMILARITY: f32 = 0.55;
/// How many nearest chunks the vector scan considers before the cutoff and paging.
const SEMANTIC_TOPK: usize = 100;
/// Hybrid blend weights: the semantic signal leads, the lexical signal supports.
const HYBRID_TEXT_WEIGHT: f64 = 0.4;
const HYBRID_SEMANTIC_WEIGHT: f64 = 0.6;
/// A hit found by only one of the two signals keeps its normalized score scaled
/// by this factor, so an equally strong hit corroborated by both signals ranks
/// above it (a both-signal hit can reach 1.0, a single-signal hit at most 0.85).
const SINGLE_SOURCE_PENALTY: f64 = 0.85;

/// Run a search and return one page of hits plus the total match count. The
/// `coverage` snapshot is supplied by the store for the semantic and hybrid modes
/// (which gate on embedding staleness) and is `None` for the lexical modes.
pub(super) async fn run_search(
    conn: &Connection,
    query: &SearchQuery,
    coverage: Option<&EmbeddingCoverage>,
) -> Result<Page<SearchHit>> {
    match query.mode {
        SearchMode::Semantic => run_semantic(conn, query, require_coverage(coverage)?).await,
        SearchMode::Hybrid => run_hybrid(conn, query, require_coverage(coverage)?).await,
        _ => run_lexical(conn, query).await,
    }
}

/// The staleness gate for semantic and hybrid search needs the coverage snapshot,
/// which the store computes for exactly those two modes. A missing snapshot here
/// is an internal wiring break, never a user input error.
fn require_coverage(coverage: Option<&EmbeddingCoverage>) -> Result<&EmbeddingCoverage> {
    coverage.ok_or_else(|| {
        IndexError::Db("semantic search dispatched without an embedding coverage snapshot".into())
    })
}

/// The lexical modes: Text, Title and Permalink. A LIKE-candidate prefilter in
/// SQL, ranked in Rust by a weighted term-frequency score.
async fn run_lexical(conn: &Connection, query: &SearchQuery) -> Result<Page<SearchHit>> {
    let limit = if query.limit == 0 { 10 } else { query.limit };
    let page = query.page.max(1);

    let terms: Vec<String> = query.text.as_deref().map(terms_of).unwrap_or_default();

    if terms.is_empty() {
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        let mut n = 1usize;
        build_scalar_filters(query, &mut clauses, &mut params, &mut n);
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        return filter_only(conn, &where_sql, params, limit, page).await;
    }

    let scored = scored_lexical(conn, query, &terms).await?;
    let total = scored.len();
    let start = (page - 1) * limit;
    let mut items = Vec::new();
    for (score, cand) in scored.into_iter().skip(start).take(limit) {
        items.push(cand.into_hit(conn, &terms, score).await?);
    }
    Ok(Page {
        items,
        page,
        limit,
        total,
    })
}

/// Load the lexical candidate rows and score them, sorted best first. Shared by
/// the lexical modes and the lexical half of hybrid search.
async fn scored_lexical(
    conn: &Connection,
    query: &SearchQuery,
    terms: &[String],
) -> Result<Vec<(f64, Candidate)>> {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    let mut n = 1usize;
    build_scalar_filters(query, &mut clauses, &mut params, &mut n);

    let cols: &[&str] = match query.mode {
        SearchMode::Title => &["title"],
        SearchMode::Permalink => &["permalink"],
        _ => &["title", "description", "content"],
    };
    for term in terms {
        let mut ors: Vec<String> = Vec::new();
        for col in cols {
            ors.push(format!("lower(e.{col}) LIKE ?{n} ESCAPE '\\'"));
            params.push(Value::Text(like_pattern(term)));
            n += 1;
        }
        clauses.push(format!("({})", ors.join(" OR ")));
    }

    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    let sql = format!(
        "SELECT e.id, d.name, e.permalink, e.title, e.engram_type, e.status, e.description, e.content \
         FROM engram e JOIN domain d ON d.id=e.domain_id {where_sql}"
    );
    let rows = query_all(conn, &sql, params).await?;

    let mut scored: Vec<(f64, Candidate)> = Vec::with_capacity(rows.len());
    for r in &rows {
        let mut c = Candidate::from_row(r);
        c.lower();
        let score = c.score(terms);
        scored.push((score, c));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.title.cmp(&b.1.title))
    });
    Ok(scored)
}

async fn filter_only(
    conn: &Connection,
    where_sql: &str,
    params: Vec<Value>,
    limit: usize,
    page: usize,
) -> Result<Page<SearchHit>> {
    let total = scalar_i64(
        conn,
        &format!("SELECT count(*) FROM engram e JOIN domain d ON d.id=e.domain_id {where_sql}"),
        params.clone(),
    )
    .await?
    .max(0) as usize;

    let offset = (page - 1) * limit;
    let sql = format!(
        "SELECT e.id, d.name, e.permalink, e.title, e.engram_type, e.status, e.description, e.content \
         FROM engram e JOIN domain d ON d.id=e.domain_id {where_sql} \
         ORDER BY e.recorded_at DESC, e.permalink ASC LIMIT {limit} OFFSET {offset}"
    );
    let rows = query_all(conn, &sql, params).await?;
    let items = rows
        .iter()
        .map(|r| {
            let c = Candidate::from_row(r);
            let snippet = if let Some(d) = c.description.as_ref().filter(|d| !d.is_empty()) {
                lead(d)
            } else {
                lead(&c.content)
            };
            SearchHit {
                domain: c.domain,
                permalink: c.permalink,
                title: c.title,
                snippet,
                score: 0.0,
                engram_type: c.engram_type,
                status: c.status,
                kind: HitKind::Engram,
            }
        })
        .collect();
    Ok(Page {
        items,
        page,
        limit,
        total,
    })
}

// --- semantic and hybrid search ----------------------------------------------

/// Pack a query vector to the raw little-endian f32 blob that turso's vector
/// functions read. `vector_distance_cos` infers the dimensionality from the blob
/// length, so the same call works for any provider width.
fn pack_vector(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for f in v {
        b.extend_from_slice(&f.to_le_bytes());
    }
    b
}

/// Semantic search over chunk embeddings. Requires the caller to have embedded
/// the query and set the active model on the [`SearchQuery`].
async fn run_semantic(
    conn: &Connection,
    query: &SearchQuery,
    coverage: &EmbeddingCoverage,
) -> Result<Page<SearchHit>> {
    let qvec = query
        .query_embedding
        .as_deref()
        .ok_or_else(|| IndexError::Invalid("semantic search requires a query embedding".into()))?;
    let active = query
        .active_model
        .as_deref()
        .ok_or_else(|| IndexError::Invalid("semantic search requires the active model".into()))?;
    let dims = qvec.len();
    let limit = if query.limit == 0 { 10 } else { query.limit };
    let page = query.page.max(1);
    let min_sim = query.min_similarity.unwrap_or(DEFAULT_MIN_SIMILARITY) as f64;

    check_staleness(coverage, active, dims)?;

    let mut hits = semantic_candidates(conn, query, qvec, active, dims).await?;
    hits.retain(|(sim, _)| *sim >= min_sim);
    hits.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.title.cmp(&b.1.title))
    });

    let total = hits.len();
    let start = (page - 1) * limit;
    let items: Vec<SearchHit> = hits
        .into_iter()
        .skip(start)
        .take(limit)
        .map(|(sim, cand)| {
            let snippet = cand.lead_snippet();
            cand.into_engram_hit(snippet, sim)
        })
        .collect();
    Ok(Page {
        items,
        page,
        limit,
        total,
    })
}

/// Hybrid search: a lexical candidate scan and a semantic top-k, each normalized
/// to `[0, 1]`, then blended.
///
/// Normalization. The lexical term-frequency score is unbounded, so it is scaled
/// by the top lexical score in this query's candidate set, mapping the best
/// lexical hit to `1.0`. The semantic score is cosine similarity, already in
/// `[-1, 1]` for unit vectors and clamped to `[0, 1]` (only hits at or above
/// `min_similarity` survive, so retained values sit in `[min_similarity, 1]`).
///
/// Blend. An engram found by both signals scores
/// `0.4 * lexical + 0.6 * semantic`. An engram found by only one keeps that
/// signal's normalized score scaled by a `0.85` penalty, so a both-signal hit
/// (up to `1.0`) can outrank an equally strong single-signal hit (up to `0.85`).
/// Hits are deduplicated per engram, keeping the best score, and every filter is
/// pushed into the SQL of both halves rather than dropped afterwards.
async fn run_hybrid(
    conn: &Connection,
    query: &SearchQuery,
    coverage: &EmbeddingCoverage,
) -> Result<Page<SearchHit>> {
    let qvec = query
        .query_embedding
        .as_deref()
        .ok_or_else(|| IndexError::Invalid("hybrid search requires a query embedding".into()))?;
    let active = query
        .active_model
        .as_deref()
        .ok_or_else(|| IndexError::Invalid("hybrid search requires the active model".into()))?;
    let dims = qvec.len();
    let limit = if query.limit == 0 { 10 } else { query.limit };
    let page = query.page.max(1);
    let min_sim = query.min_similarity.unwrap_or(DEFAULT_MIN_SIMILARITY) as f64;

    check_staleness(coverage, active, dims)?;

    let terms: Vec<String> = query.text.as_deref().map(terms_of).unwrap_or_default();
    let text_scored = if terms.is_empty() {
        Vec::new()
    } else {
        scored_lexical(conn, query, &terms).await?
    };
    let mut sem = semantic_candidates(conn, query, qvec, active, dims).await?;
    sem.retain(|(sim, _)| *sim >= min_sim);

    let max_text = text_scored.iter().map(|(s, _)| *s).fold(0.0_f64, f64::max);

    struct Merged {
        cand: Candidate,
        text: Option<f64>,
        sem: Option<f64>,
    }
    let mut merged: std::collections::HashMap<(String, String), Merged> =
        std::collections::HashMap::new();

    for (score, cand) in text_scored {
        let norm = if max_text > 0.0 {
            score / max_text
        } else {
            0.0
        };
        let key = (cand.domain.clone(), cand.permalink.clone());
        merged
            .entry(key)
            .and_modify(|m| m.text = Some(m.text.map_or(norm, |t| t.max(norm))))
            .or_insert(Merged {
                cand,
                text: Some(norm),
                sem: None,
            });
    }
    for (sim, cand) in sem {
        let norm = sim.clamp(0.0, 1.0);
        let key = (cand.domain.clone(), cand.permalink.clone());
        merged
            .entry(key)
            .and_modify(|m| m.sem = Some(m.sem.map_or(norm, |s| s.max(norm))))
            .or_insert(Merged {
                cand,
                text: None,
                sem: Some(norm),
            });
    }

    let mut ranked: Vec<(f64, Merged)> = merged
        .into_values()
        .map(|m| {
            let score = match (m.text, m.sem) {
                (Some(t), Some(s)) => HYBRID_TEXT_WEIGHT * t + HYBRID_SEMANTIC_WEIGHT * s,
                (Some(t), None) => t * SINGLE_SOURCE_PENALTY,
                (None, Some(s)) => s * SINGLE_SOURCE_PENALTY,
                (None, None) => 0.0,
            };
            (score, m)
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cand.title.cmp(&b.1.cand.title))
    });

    let total = ranked.len();
    let start = (page - 1) * limit;
    let items: Vec<SearchHit> = ranked
        .into_iter()
        .skip(start)
        .take(limit)
        .map(|(score, m)| {
            let snippet = if !terms.is_empty() && m.text.is_some() {
                m.cand.text_snippet(&terms)
            } else {
                m.cand.lead_snippet()
            };
            m.cand.into_engram_hit(snippet, score)
        })
        .collect();
    Ok(Page {
        items,
        page,
        limit,
        total,
    })
}

/// The nearest engrams to the query vector, as `(similarity, candidate)` pairs.
/// One row per engram (the closest of its chunks), filtered by every scalar
/// filter on the query, ordered by distance and capped at the top-k.
async fn semantic_candidates(
    conn: &Connection,
    query: &SearchQuery,
    qvec: &[f32],
    active: &str,
    dims: usize,
) -> Result<Vec<(f64, Candidate)>> {
    // ?1 is the query vector; scalar filters and the model and dims predicates
    // take the placeholders after it.
    let mut params: Vec<Value> = vec![Value::Blob(pack_vector(qvec))];
    let mut clauses: Vec<String> = Vec::new();
    let mut n = 2usize;
    build_scalar_filters(query, &mut clauses, &mut params, &mut n);
    let model_ph = n;
    params.push(Value::Text(active.to_string()));
    n += 1;
    let dims_ph = n;
    params.push(Value::Integer(dims as i64));
    clauses.push(format!(
        "c.embedding IS NOT NULL AND c.model = ?{model_ph} AND c.dims = ?{dims_ph}"
    ));

    let where_sql = format!("WHERE {}", clauses.join(" AND "));
    let sql = format!(
        "SELECT e.id, d.name, e.permalink, e.title, e.engram_type, e.status, e.description, e.content, \
         min(vector_distance_cos(c.embedding, ?1)) AS dist \
         FROM chunk c JOIN engram e ON e.id=c.engram_id JOIN domain d ON d.id=e.domain_id \
         {where_sql} GROUP BY e.id ORDER BY dist ASC LIMIT {SEMANTIC_TOPK}"
    );
    let rows = query_all(conn, &sql, params).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let dist = cell_real(r, 8).unwrap_or(1.0);
        out.push((1.0 - dist, Candidate::from_row(r)));
    }
    Ok(out)
}

/// Refuse semantic search when the stored embeddings cannot be compared against
/// the active provider's vector space. A different dimensionality is always
/// unsafe (cosine across widths is meaningless), and a same-width model swap
/// with nothing yet re-embedded means every stored vector is in the wrong space.
/// Either case surfaces as [`IndexError::StaleEmbeddings`] so callers report
/// "reindex in progress"; text search never calls this. When nothing is embedded
/// yet, this is not an error: the semantic scan simply returns no hits.
///
/// This consumes the shared coverage snapshot (the same one `effective_mode`
/// reads) rather than issuing its own aggregate scan, so a semantic or hybrid
/// search costs one cached snapshot, not a second `GROUP BY`. The snapshot's
/// `models` omits the empty-model group, but `store_embeddings` always writes a
/// non-empty model id and nothing else ever writes an embedding, so no embedded
/// chunk can lack a model: the omitted group is unreachable, and `embedded_chunks`
/// plus `total_chunks` still account for every chunk. The produced
/// [`IndexError::StaleEmbeddings`] fields are therefore byte-identical to the old
/// direct scan for every reachable database state.
fn check_staleness(coverage: &EmbeddingCoverage, active_model: &str, dims: usize) -> Result<()> {
    let mut active_embedded = 0usize;
    let mut foreign_dims = false;
    let mut other: Option<(&str, usize)> = None;
    for m in &coverage.models {
        if m.model == active_model && m.dims == dims {
            active_embedded += m.count;
            continue;
        }
        if m.dims != dims {
            foreign_dims = true;
        }
        if other.as_ref().map(|(_, oc)| m.count > *oc).unwrap_or(true) {
            other = Some((m.model.as_str(), m.count));
        }
    }

    let stale = coverage.embedded_chunks > 0 && (foreign_dims || active_embedded == 0);
    if stale {
        let stored_model = other
            .map(|(m, _)| m.to_string())
            .unwrap_or_else(|| active_model.to_string());
        return Err(IndexError::StaleEmbeddings {
            stored_model,
            active_model: active_model.to_string(),
            embedded: active_embedded,
            total: coverage.total_chunks,
        });
    }
    Ok(())
}

fn build_scalar_filters(
    query: &SearchQuery,
    clauses: &mut Vec<String>,
    params: &mut Vec<Value>,
    n: &mut usize,
) {
    if let Some(domains) = &query.domains
        && !domains.is_empty()
    {
        let ph: Vec<String> = domains
            .iter()
            .map(|d| {
                params.push(Value::Text(d.clone()));
                let p = format!("?{n}");
                *n += 1;
                p
            })
            .collect();
        clauses.push(format!("d.name IN ({})", ph.join(",")));
    }

    if let Some(t) = &query.engram_type {
        clauses.push(format!("e.engram_type = ?{n}"));
        params.push(Value::Text(t.clone()));
        *n += 1;
    }

    if query.current_only {
        let today = query.today.clone().unwrap_or_else(|| {
            chrono::Utc::now()
                .date_naive()
                .format("%Y-%m-%d")
                .to_string()
        });
        clauses.push(format!(
            "e.status = 'current' AND (e.valid_from IS NULL OR e.valid_from <= ?{n}) AND (e.valid_to IS NULL OR e.valid_to > ?{})",
            *n + 1
        ));
        params.push(Value::Text(today.clone()));
        params.push(Value::Text(today));
        *n += 2;
    } else if let Some(s) = &query.status {
        clauses.push(format!("e.status = ?{n}"));
        params.push(Value::Text(s.clone()));
        *n += 1;
    }

    if let Some(after) = &query.after {
        clauses.push(format!("e.recorded_at >= ?{n}"));
        params.push(Value::Text(after.clone()));
        *n += 1;
    }

    if let Some(tags) = &query.tags {
        for tag in tags {
            clauses.push(format!(
                "EXISTS (SELECT 1 FROM engram_tag et JOIN tag t ON t.id=et.tag_id WHERE et.engram_id=e.id AND t.name=?{n})"
            ));
            params.push(Value::Text(tag.clone()));
            *n += 1;
        }
    }

    for f in &query.metadata_filters {
        if let Some(clause) = metadata_clause(f, params, n) {
            clauses.push(clause);
        }
    }
}

/// Map one metadata filter to a SQL predicate, appending its bound values.
/// Promoted keys map to columns; everything else to `json_extract`.
fn metadata_clause(f: &MetadataFilter, params: &mut Vec<Value>, n: &mut usize) -> Option<String> {
    let key = f.key.as_str();

    if key == "tags" {
        let exists = |val: &serde_json::Value, params: &mut Vec<Value>, n: &mut usize| {
            let p = format!(
                "EXISTS (SELECT 1 FROM engram_tag et JOIN tag t ON t.id=et.tag_id WHERE et.engram_id=e.id AND t.name=?{n})"
            );
            params.push(json_to_value(val));
            *n += 1;
            p
        };
        return match &f.op {
            FilterOp::Eq(v) => Some(exists(v, params, n)),
            FilterOp::In(vs) => {
                let parts: Vec<String> = vs.iter().map(|v| exists(v, params, n)).collect();
                Some(format!("({})", parts.join(" OR ")))
            }
            _ => None,
        };
    }

    let col = match key {
        "status" => Some("e.status".to_string()),
        "type" | "engram_type" => Some("e.engram_type".to_string()),
        "recorded_at" => Some("e.recorded_at".to_string()),
        "valid_from" => Some("e.valid_from".to_string()),
        "valid_to" => Some("e.valid_to".to_string()),
        "timestamp" => Some("e.timestamp".to_string()),
        "title" => Some("e.title".to_string()),
        "permalink" => Some("e.permalink".to_string()),
        _ => {
            if !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
            {
                return None;
            }
            Some(format!("json_extract(e.metadata, '$.{key}')"))
        }
    }?;

    Some(op_clause(&col, &f.op, params, n))
}

fn op_clause(col: &str, op: &FilterOp, params: &mut Vec<Value>, n: &mut usize) -> String {
    let bind = |v: &serde_json::Value, params: &mut Vec<Value>, n: &mut usize| {
        let p = format!("?{n}");
        params.push(json_to_value(v));
        *n += 1;
        p
    };
    match op {
        FilterOp::Eq(v) => format!("{col} = {}", bind(v, params, n)),
        FilterOp::Gt(v) => format!("{col} > {}", bind(v, params, n)),
        FilterOp::Gte(v) => format!("{col} >= {}", bind(v, params, n)),
        FilterOp::Lt(v) => format!("{col} < {}", bind(v, params, n)),
        FilterOp::Lte(v) => format!("{col} <= {}", bind(v, params, n)),
        FilterOp::Between(lo, hi) => {
            let a = bind(lo, params, n);
            let b = bind(hi, params, n);
            format!("{col} BETWEEN {a} AND {b}")
        }
        FilterOp::In(vs) => {
            if vs.is_empty() {
                return "0".to_string();
            }
            let ph: Vec<String> = vs.iter().map(|v| bind(v, params, n)).collect();
            format!("{col} IN ({})", ph.join(","))
        }
    }
}

fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Bool(b) => Value::Integer(i64::from(*b)),
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Number(num) => {
            if let Some(i) = num.as_i64() {
                Value::Integer(i)
            } else {
                Value::Real(num.as_f64().unwrap_or(0.0))
            }
        }
        other => Value::Text(other.to_string()),
    }
}

/// A loaded candidate engram row.
struct Candidate {
    id: i64,
    domain: String,
    permalink: String,
    title: String,
    engram_type: String,
    status: String,
    description: Option<String>,
    content: String,
    // Lowercased title, description and content, computed once by `lower` on the
    // lexical scoring path and reused by `score`, `into_hit` and `text_snippet`
    // so those methods never re-lowercase per term. Left empty on the semantic
    // and filter-only paths, which never term-score, so those paths pay nothing;
    // the methods that read these run only on lexically constructed candidates.
    title_lower: String,
    desc_lower: String,
    content_lower: String,
}

impl Candidate {
    fn from_row(r: &Row) -> Candidate {
        Candidate {
            id: cell_i64(r, 0).unwrap_or(0),
            domain: cell_text(r, 1).unwrap_or_default(),
            permalink: cell_text(r, 2).unwrap_or_default(),
            title: cell_text(r, 3).unwrap_or_default(),
            engram_type: cell_text(r, 4).unwrap_or_default(),
            status: cell_text(r, 5).unwrap_or_default(),
            description: cell_text(r, 6),
            content: cell_text(r, 7).unwrap_or_default(),
            title_lower: String::new(),
            desc_lower: String::new(),
            content_lower: String::new(),
        }
    }

    /// Lowercase title, description and content once for the lexical path.
    /// `to_lowercase` (full Unicode case folding) is kept rather than an
    /// ASCII-only comparison so a non-ASCII term still matches its own case.
    fn lower(&mut self) {
        self.title_lower = self.title.to_lowercase();
        self.desc_lower = self
            .description
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        self.content_lower = self.content.to_lowercase();
    }

    fn score(&self, terms: &[String]) -> f64 {
        let mut score = 0.0;
        for term in terms {
            score += 3.0 * count_occ(&self.title_lower, term) as f64;
            score += 2.0 * count_occ(&self.desc_lower, term) as f64;
            score += count_occ(&self.content_lower, term) as f64;
        }
        score
    }

    async fn into_hit(self, conn: &Connection, terms: &[String], score: f64) -> Result<SearchHit> {
        let in_title = terms.iter().any(|t| count_occ(&self.title_lower, t) > 0);
        let in_desc =
            self.description.is_some() && terms.iter().any(|t| count_occ(&self.desc_lower, t) > 0);

        // When the match is not in the title or description, prefer an
        // observation-level hit so the caller gets the source line.
        if !in_title
            && !in_desc
            && let Some(row) = matching_observation(conn, self.id, terms).await?
        {
            let line = cell_i64(&row, 0).unwrap_or(0) as usize;
            let content = cell_text(&row, 1).unwrap_or_default();
            return Ok(SearchHit {
                snippet: make_snippet(&content, terms),
                kind: HitKind::Observation { line },
                domain: self.domain,
                permalink: self.permalink,
                title: self.title,
                score,
                engram_type: self.engram_type,
                status: self.status,
            });
        }

        let snippet_src = if in_title || !in_desc {
            // Prefer a content window; fall back to description or title.
            if terms.iter().any(|t| count_occ(&self.content_lower, t) > 0) {
                self.content.clone()
            } else if let Some(d) = self.description.clone().filter(|d| !d.is_empty()) {
                d
            } else {
                self.title.clone()
            }
        } else {
            self.description.clone().unwrap_or_default()
        };

        Ok(SearchHit {
            snippet: make_snippet(&snippet_src, terms),
            kind: HitKind::Engram,
            domain: self.domain,
            permalink: self.permalink,
            title: self.title,
            score,
            engram_type: self.engram_type,
            status: self.status,
        })
    }

    /// A lead-in snippet for a hit with no term to window around (the semantic
    /// case): the description if present, else the body.
    fn lead_snippet(&self) -> String {
        match self.description.as_ref().filter(|d| !d.is_empty()) {
            Some(d) => lead(d),
            None => lead(&self.content),
        }
    }

    /// A snippet windowed around the first matching term, for the lexical half
    /// of a hybrid hit.
    fn text_snippet(&self, terms: &[String]) -> String {
        let src = if terms.iter().any(|t| count_occ(&self.content_lower, t) > 0) {
            &self.content
        } else if let Some(d) = self.description.as_ref().filter(|d| !d.is_empty()) {
            d
        } else {
            &self.title
        };
        make_snippet(src, terms)
    }

    /// Build an engram-level hit with a precomputed snippet and score. Used by
    /// the semantic and hybrid paths, which rank whole engrams rather than
    /// individual observations.
    fn into_engram_hit(self, snippet: String, score: f64) -> SearchHit {
        SearchHit {
            domain: self.domain,
            permalink: self.permalink,
            title: self.title,
            snippet,
            score,
            engram_type: self.engram_type,
            status: self.status,
            kind: HitKind::Engram,
        }
    }
}

async fn matching_observation(
    conn: &Connection,
    engram_id: i64,
    terms: &[String],
) -> Result<Option<Row>> {
    // The first observation (lowest line) that contains any term.
    let mut clause = Vec::new();
    let mut params = vec![Value::Integer(engram_id)];
    for (n, term) in (2..).zip(terms.iter()) {
        clause.push(format!("lower(content) LIKE ?{n} ESCAPE '\\'"));
        params.push(Value::Text(like_pattern(term)));
    }
    let sql = format!(
        "SELECT line, content FROM observation WHERE engram_id=?1 AND ({}) ORDER BY line ASC LIMIT 1",
        clause.join(" OR ")
    );
    query_first(conn, &sql, params).await
}

/// Traverse the neighborhood of the seed engrams up to `depth` hops.
pub(super) async fn neighbors(
    conn: &Connection,
    ids: &[EngramId],
    depth: u8,
) -> Result<GraphSlice> {
    let depth = depth.clamp(1, 3);
    let mut visited: HashSet<i64> = ids.iter().map(|e| e.0).collect();
    let mut frontier: Vec<i64> = visited.iter().copied().collect();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut edge_seen: HashSet<(i64, i64, String, u8)> = HashSet::new();

    for _ in 0..depth {
        if frontier.is_empty() {
            break;
        }
        let list = frontier
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut next: Vec<i64> = Vec::new();

        let rel_rows = query_all(
            conn,
            &format!(
                "SELECT engram_id, to_id, rel_type FROM relation \
                 WHERE to_id IS NOT NULL AND (engram_id IN ({list}) OR to_id IN ({list}))"
            ),
            vec![],
        )
        .await?;
        for r in &rel_rows {
            let from = cell_i64(r, 0).unwrap_or(0);
            let to = cell_i64(r, 1).unwrap_or(0);
            let rel_type = cell_text(r, 2).unwrap_or_default();
            push_edge(
                &mut edges,
                &mut edge_seen,
                &mut visited,
                &mut next,
                from,
                to,
                rel_type,
                EdgeKind::Relation,
            );
        }

        let link_rows = query_all(
            conn,
            &format!(
                "SELECT engram_id, to_id FROM link \
                 WHERE to_id IS NOT NULL AND (engram_id IN ({list}) OR to_id IN ({list}))"
            ),
            vec![],
        )
        .await?;
        for r in &link_rows {
            let from = cell_i64(r, 0).unwrap_or(0);
            let to = cell_i64(r, 1).unwrap_or(0);
            push_edge(
                &mut edges,
                &mut edge_seen,
                &mut visited,
                &mut next,
                from,
                to,
                "links_to".to_string(),
                EdgeKind::Link,
            );
        }

        frontier = next;
    }

    let mut nodes = Vec::new();
    if !visited.is_empty() {
        let list = visited
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let rows = query_all(
            conn,
            &format!(
                "SELECT e.id, d.name, e.permalink, e.title, e.engram_type \
                 FROM engram e JOIN domain d ON d.id=e.domain_id WHERE e.id IN ({list}) ORDER BY e.id"
            ),
            vec![],
        )
        .await?;
        for r in &rows {
            nodes.push(GraphNode {
                id: EngramId(cell_i64(r, 0).unwrap_or(0)),
                domain: cell_text(r, 1).unwrap_or_default(),
                permalink: cell_text(r, 2).unwrap_or_default(),
                title: cell_text(r, 3).unwrap_or_default(),
                engram_type: cell_text(r, 4).unwrap_or_default(),
            });
        }
    }

    Ok(GraphSlice { nodes, edges })
}

#[allow(clippy::too_many_arguments)]
fn push_edge(
    edges: &mut Vec<GraphEdge>,
    edge_seen: &mut HashSet<(i64, i64, String, u8)>,
    visited: &mut HashSet<i64>,
    next: &mut Vec<i64>,
    from: i64,
    to: i64,
    rel_type: String,
    kind: EdgeKind,
) {
    let kind_tag = match kind {
        EdgeKind::Relation => 0u8,
        EdgeKind::Link => 1u8,
    };
    let key = (from, to, rel_type.clone(), kind_tag);
    if edge_seen.insert(key) {
        edges.push(GraphEdge {
            from: EngramId(from),
            to: EngramId(to),
            rel_type,
            kind,
        });
    }
    for endpoint in [from, to] {
        if visited.insert(endpoint) {
            next.push(endpoint);
        }
    }
}

// --- text utilities ----------------------------------------------------------

fn terms_of(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty())
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

fn like_pattern(term: &str) -> String {
    let mut s = String::with_capacity(term.len() + 2);
    s.push('%');
    for c in term.chars() {
        if c == '%' || c == '_' || c == '\\' {
            s.push('\\');
        }
        s.extend(c.to_lowercase());
    }
    s.push('%');
    s
}

fn count_occ(haystack_lower: &str, needle_lower: &str) -> usize {
    if needle_lower.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut start = 0;
    while let Some(pos) = haystack_lower[start..].find(needle_lower) {
        count += 1;
        start += pos + needle_lower.len();
    }
    count
}

fn lead(text: &str) -> String {
    let collapsed = collapse_ws(text);
    let chars: Vec<char> = collapsed.chars().collect();
    if chars.len() <= SNIPPET_LEAD {
        chars.into_iter().collect()
    } else {
        let head: String = chars.into_iter().take(SNIPPET_LEAD).collect();
        format!("{head}...")
    }
}

fn make_snippet(text: &str, terms: &[String]) -> String {
    let chars: Vec<char> = text.chars().collect();
    let pos = terms.iter().filter_map(|t| find_ci(&chars, t)).min();
    let Some(pos) = pos else {
        return lead(text);
    };
    let start = pos.saturating_sub(SNIPPET_MARGIN);
    let end = (pos + SNIPPET_MARGIN * 2).min(chars.len());
    let window: String = chars[start..end].iter().collect();
    let window = collapse_ws(&window);
    let mut out = String::new();
    if start > 0 {
        out.push_str("...");
    }
    out.push_str(window.trim());
    if end < chars.len() {
        out.push_str("...");
    }
    out
}

fn find_ci(haystack: &[char], needle_lower: &str) -> Option<usize> {
    let needle: Vec<char> = needle_lower.chars().collect();
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    for i in 0..=haystack.len() - needle.len() {
        if (0..needle.len()).all(|j| eq_ci(haystack[i + j], needle[j])) {
            return Some(i);
        }
    }
    None
}

fn eq_ci(a: char, b: char) -> bool {
    a == b || a.to_ascii_lowercase() == b
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
