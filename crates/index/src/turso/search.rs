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
    EdgeKind, EngramId, FilterOp, GraphEdge, GraphNode, GraphSlice, HitKind, MetadataFilter, Page,
    SearchHit, SearchMode, SearchQuery,
};

use super::{cell_i64, cell_text, query_all, query_first, scalar_i64};

const SNIPPET_MARGIN: usize = 70;
const SNIPPET_LEAD: usize = 200;

/// Run a search and return one page of hits plus the total match count.
pub(super) async fn run_search(conn: &Connection, query: &SearchQuery) -> Result<Page<SearchHit>> {
    if matches!(query.mode, SearchMode::Semantic | SearchMode::Hybrid) {
        return Err(IndexError::Unsupported(
            "semantic and hybrid search arrive in M4".into(),
        ));
    }

    let limit = if query.limit == 0 { 10 } else { query.limit };
    let page = query.page.max(1);

    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    let mut n = 1usize;

    build_scalar_filters(query, &mut clauses, &mut params, &mut n);

    let terms: Vec<String> = query.text.as_deref().map(terms_of).unwrap_or_default();

    if !terms.is_empty() {
        let cols: &[&str] = match query.mode {
            SearchMode::Title => &["title"],
            SearchMode::Permalink => &["permalink"],
            _ => &["title", "description", "content"],
        };
        for term in &terms {
            let mut ors: Vec<String> = Vec::new();
            for col in cols {
                ors.push(format!("lower(e.{col}) LIKE ?{n} ESCAPE '\\'"));
                params.push(Value::Text(like_pattern(term)));
                n += 1;
            }
            clauses.push(format!("({})", ors.join(" OR ")));
        }
    }

    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };

    if terms.is_empty() {
        return filter_only(conn, &where_sql, params, limit, page).await;
    }

    // Text path: load candidate rows, score and page in Rust.
    let sql = format!(
        "SELECT e.id, d.name, e.permalink, e.title, e.engram_type, e.status, e.description, e.content \
         FROM engram e JOIN domain d ON d.id=e.domain_id {where_sql}"
    );
    let rows = query_all(conn, &sql, params).await?;

    let mut scored: Vec<(f64, Candidate)> = Vec::with_capacity(rows.len());
    for r in &rows {
        let c = Candidate::from_row(r);
        let score = c.score(&terms);
        scored.push((score, c));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.title.cmp(&b.1.title))
    });

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
        }
    }

    fn score(&self, terms: &[String]) -> f64 {
        let title = self.title.to_lowercase();
        let desc = self.description.clone().unwrap_or_default().to_lowercase();
        let content = self.content.to_lowercase();
        let mut score = 0.0;
        for term in terms {
            score += 3.0 * count_occ(&title, term) as f64;
            score += 2.0 * count_occ(&desc, term) as f64;
            score += count_occ(&content, term) as f64;
        }
        score
    }

    async fn into_hit(self, conn: &Connection, terms: &[String], score: f64) -> Result<SearchHit> {
        let in_title = terms
            .iter()
            .any(|t| count_occ(&self.title.to_lowercase(), t) > 0);
        let in_desc = self
            .description
            .as_ref()
            .map(|d| d.to_lowercase())
            .map(|d| terms.iter().any(|t| count_occ(&d, t) > 0))
            .unwrap_or(false);

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
            if terms
                .iter()
                .any(|t| count_occ(&self.content.to_lowercase(), t) > 0)
            {
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
