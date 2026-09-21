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

use crate::alias::AliasMap;
use crate::error::{IndexError, Result};
use crate::store::{
    CURRENT_STATUS_CLASS, DEFAULT_RETIRED_WEIGHT, DEFAULT_SALIENCE_WEIGHT, EdgeKind,
    EmbeddingCoverage, EngramId, FilterOp, GraphEdge, GraphNode, GraphSlice, HitKind,
    MetadataFilter, Page, SearchHit, SearchMode, SearchQuery, is_current_status, link_frontier_sql,
    relation_frontier_sql, retired_factor, salience_prior,
};

use super::{
    cell_i64, cell_real, cell_text, like_escape, path_prefix_like, query_all, query_first,
    scalar_i64,
};

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
/// `candidate_cap` bounds the lexical prefilter; production passes
/// [`crate::store::LEXICAL_CANDIDATE_CAP`].
pub(super) async fn run_search(
    conn: &Connection,
    query: &SearchQuery,
    coverage: Option<&EmbeddingCoverage>,
    aliases: &AliasMap,
    candidate_cap: usize,
) -> Result<Page<SearchHit>> {
    match query.mode {
        SearchMode::Semantic => {
            run_semantic(conn, query, require_coverage(coverage)?, aliases).await
        }
        SearchMode::Hybrid => {
            run_hybrid(
                conn,
                query,
                require_coverage(coverage)?,
                aliases,
                candidate_cap,
            )
            .await
        }
        _ => run_lexical(conn, query, aliases, candidate_cap).await,
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

/// Attach each page hit's tags, drop the ids and wrap the page. The single seam
/// every search mode routes its final page through, so tagging is defined once
/// and never duplicated per mode.
async fn finish_page(
    conn: &Connection,
    mut hits: Vec<(i64, SearchHit)>,
    page: usize,
    limit: usize,
    total: usize,
) -> Result<Page<SearchHit>> {
    attach_tags(conn, &mut hits).await?;
    Ok(Page {
        items: hits.into_iter().map(|(_, hit)| hit).collect(),
        page,
        limit,
        total,
    })
}

/// Load the tags for the page's engrams in one batch query and set them on each
/// hit, alphabetical within a hit. Runs once per search over the final page's
/// engram ids only (never the candidate prefilter) and is skipped when the page
/// is empty. An untagged engram keeps its empty vec. An observation-kind hit is
/// keyed by its engram id, so it carries that engram's tags like any other.
async fn attach_tags(conn: &Connection, hits: &mut [(i64, SearchHit)]) -> Result<()> {
    if hits.is_empty() {
        return Ok(());
    }
    let ids = hits
        .iter()
        .map(|(id, _)| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT et.engram_id, t.name FROM engram_tag et JOIN tag t ON t.id=et.tag_id \
         WHERE et.engram_id IN ({ids}) ORDER BY et.engram_id, t.name"
    );
    let rows = query_all(conn, &sql, vec![]).await?;
    let mut by_id: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    for r in &rows {
        let id = cell_i64(r, 0).unwrap_or(0);
        by_id
            .entry(id)
            .or_default()
            .push(cell_text(r, 1).unwrap_or_default());
    }
    for (id, hit) in hits.iter_mut() {
        if let Some(tags) = by_id.get(id) {
            hit.tags = tags.clone();
        }
    }
    Ok(())
}

/// The actor screen every candidate leg opens its `WHERE` with: which rows of
/// the `engram` table this search is entitled to see.
///
/// `None` emits the base predicate verbatim, so a search that names no actor is
/// byte-for-byte the statement that was there before the actor dimension
/// existed. `Some(actor)` emits the shadowing form, which is two legs:
///
/// * that actor's own drafts, tombstones excluded - a draft deletion is a row
///   saying an engram is gone, never an engram to answer with, and it carries
///   the base row's own text, so a leg that forgot to exclude it would match
///   every query the deleted engram matched;
/// * plus every base row that actor holds no row of their own at, which is the
///   anti-join. It correlates on `(domain_id, path, actor)` alone - the columns
///   of `idx_engram_path_actor`, so each probe is an index seek and no body is
///   read - and it deliberately does NOT exclude tombstones. A tombstone is
///   exactly what takes its base row away; skipping them here would answer a
///   deletion with the deleted engram showing through from the base.
///
/// Path is the correlation key rather than permalink because a tombstone's
/// permalink is its own path, so a permalink anti-join would shadow nothing it
/// was written to shadow.
fn actor_screen(actor: Option<&str>, params: &mut Vec<Value>, n: &mut usize) -> String {
    actor_screen_on("e", actor, params, n)
}

/// [`actor_screen`] over a named alias of the `engram` table, for a statement
/// that screens more than one of them at once: the graph frontier joins the
/// table twice, once at each end of an edge, and each end is its own screen
/// with its own placeholder. `o` stays the anti-join's own alias in every copy,
/// which is legal because each `NOT EXISTS` opens a scope of its own.
pub(super) fn actor_screen_on(
    alias: &str,
    actor: Option<&str>,
    params: &mut Vec<Value>,
    n: &mut usize,
) -> String {
    let Some(actor) = actor else {
        return format!("{alias}.actor = ''");
    };
    let ph = *n;
    params.push(Value::Text(actor.to_string()));
    *n += 1;
    format!(
        "{alias}.tombstone = 0 AND ({alias}.actor = ?{ph} OR ({alias}.actor = '' AND NOT EXISTS (\
         SELECT 1 FROM engram o WHERE o.domain_id = {alias}.domain_id AND o.path = {alias}.path \
         AND o.actor = ?{ph})))"
    )
}

/// One actor's live drafts and nothing else, over a named alias.
///
/// The other half of the address map [`actor_screen_on`] builds: that one
/// answers "which row stands at this path for this reader", this one answers
/// "which of this reader's own pages answers to this name". A reference that
/// binds to nothing in the index can still reach one of them, which is how a
/// page only its author has written answers the domain's own dangling link for
/// that one author. Tombstones are excluded here too - a draft deletion is
/// never a page to arrive at.
pub(super) fn drafts_only_on(
    alias: &str,
    actor: &str,
    params: &mut Vec<Value>,
    n: &mut usize,
) -> String {
    let ph = *n;
    params.push(Value::Text(actor.to_string()));
    *n += 1;
    format!("{alias}.actor = ?{ph} AND {alias}.tombstone = 0")
}

/// The lexical modes: Text, Title and Permalink. A LIKE-candidate prefilter in
/// SQL, ranked in Rust by a weighted term-frequency score.
async fn run_lexical(
    conn: &Connection,
    query: &SearchQuery,
    aliases: &AliasMap,
    candidate_cap: usize,
) -> Result<Page<SearchHit>> {
    let limit = if query.limit == 0 { 10 } else { query.limit };
    let page = query.page.max(1);

    let terms: Vec<String> = query.text.as_deref().map(terms_of).unwrap_or_default();

    if terms.is_empty() {
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        let mut n = 1usize;
        build_scalar_filters(query, &mut clauses, &mut params, &mut n, aliases);
        // The reader-chosen filters, as a trailing conjunction rather than a
        // `WHERE` of their own: every statement below opens its own `WHERE` with
        // the actor screen, so a search answers out of the domain's files plus
        // the asking actor's own drafts and never out of anybody else's.
        let and_filters = if clauses.is_empty() {
            String::new()
        } else {
            format!("AND {}", clauses.join(" AND "))
        };
        let actor_screen = actor_screen(query.actor.as_deref(), &mut params, &mut n);
        return filter_only(conn, &actor_screen, &and_filters, params, limit, page).await;
    }

    let mut scored = scored_lexical(conn, query, &terms, aliases, candidate_cap).await?;
    let retired_weight = query.retired_weight.unwrap_or(DEFAULT_RETIRED_WEIGHT);
    for entry in &mut scored {
        entry.0 *= retired_factor(&entry.1.status, retired_weight);
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.title.cmp(&b.1.title))
    });
    let total = scored.len();
    let start = (page - 1) * limit;
    let mut items: Vec<(i64, SearchHit)> = Vec::new();
    for (score, cand) in scored.into_iter().skip(start).take(limit) {
        let id = cand.id;
        items.push((id, cand.into_hit(conn, &terms, score).await?));
    }
    finish_page(conn, items, page, limit, total).await
}

/// Load the lexical candidate rows and score them, sorted best first. Shared by
/// the lexical modes and the lexical half of hybrid search: the retired-status
/// fade is applied by each of those callers exactly once, over this function's
/// returned scores, never in here - applying it here would double-fade the
/// hybrid path and distort its `max_text` normalization.
///
/// `candidate_cap` bounds how many candidate rows the prefilter loads: the query
/// orders by engram id and takes at most that many, so a common term over a huge
/// domain can never pull the whole matched corpus into memory. See
/// [`crate::store::LEXICAL_CANDIDATE_CAP`] for what the cut means for ranking.
async fn scored_lexical(
    conn: &Connection,
    query: &SearchQuery,
    terms: &[String],
    aliases: &AliasMap,
    candidate_cap: usize,
) -> Result<Vec<(f64, Candidate)>> {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    let mut n = 1usize;
    build_scalar_filters(query, &mut clauses, &mut params, &mut n, aliases);

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

    // The reader-chosen filters, as a trailing conjunction rather than a
    // `WHERE` of their own: every statement below opens its own `WHERE` with
    // the actor screen, so a search answers out of the domain's files plus the
    // asking actor's own drafts and never out of anybody else's.
    let and_filters = if clauses.is_empty() {
        String::new()
    } else {
        format!("AND {}", clauses.join(" AND "))
    };
    let actor_screen = actor_screen(query.actor.as_deref(), &mut params, &mut n);
    let sql = lexical_candidate_sql(&actor_screen, &and_filters, candidate_cap);
    let rows = query_all(conn, &sql, params).await?;

    // Consume the raw rows by value so each row's buffers are freed as its
    // candidate is built: peak memory is one copy of the matched bytes in the
    // candidates plus one lowered copy, never a third live copy in the rows.
    // The lowered copy stays: snippets are cut from the original body and
    // Unicode lowercasing can shift byte offsets, so scoring against a lowered
    // copy is the only offset-safe way to keep snippets correct.
    let mut scored: Vec<(f64, Candidate)> = Vec::with_capacity(rows.len());
    for r in rows {
        let mut c = Candidate::from_row(&r);
        drop(r);
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
    actor_screen: &str,
    and_filters: &str,
    params: Vec<Value>,
    limit: usize,
    page: usize,
) -> Result<Page<SearchHit>> {
    let total = scalar_i64(
        conn,
        &format!(
            "SELECT count(*) FROM engram e JOIN domain d ON d.id=e.domain_id \
             WHERE {actor_screen} {and_filters}"
        ),
        params.clone(),
    )
    .await?
    .max(0) as usize;

    let offset = (page - 1) * limit;
    // This one does open a sorter, but with a `LIMIT` and no `GROUP BY` turso
    // applies its bounded-sorter optimization and holds only `limit + offset`
    // records, so the wide projection costs one page of bodies rather than the
    // whole match set. Adding a `GROUP BY` here would remove that bound.
    //
    // The undated engram sorts last, said out loud rather than inherited from
    // the dialect: SQLite already puts a NULL last under `DESC` and Postgres
    // puts it first, so the twin statement has to spell `NULLS LAST` and this
    // one spells the same rule with the key SQLite would apply anyway. A
    // listing that opens on the newest engrams must not lead with the one
    // nobody dated, and the sort decides which rows land on a page at all.
    let sql = format!(
        "SELECT {CANDIDATE_COLUMNS} FROM engram e JOIN domain d ON d.id=e.domain_id \
         WHERE {actor_screen} {and_filters} \
         ORDER BY e.recorded_at IS NULL, e.recorded_at DESC, e.permalink ASC LIMIT {limit} OFFSET {offset}"
    );
    let rows = query_all(conn, &sql, params).await?;
    let items: Vec<(i64, SearchHit)> = rows
        .iter()
        .map(|r| {
            let c = Candidate::from_row(r);
            let id = c.id;
            let snippet = if let Some(d) = c.description.as_ref().filter(|d| !d.is_empty()) {
                lead(d)
            } else {
                lead(&c.content)
            };
            (
                id,
                SearchHit {
                    domain: c.domain,
                    permalink: c.permalink,
                    title: c.title,
                    snippet,
                    score: 0.0,
                    engram_type: c.engram_type,
                    status: c.status,
                    tags: Vec::new(),
                    kind: HitKind::Engram,
                },
            )
        })
        .collect();
    finish_page(conn, items, page, limit, total).await
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
    aliases: &AliasMap,
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
    let retired_weight = query.retired_weight.unwrap_or(DEFAULT_RETIRED_WEIGHT);

    check_staleness(coverage, active, dims)?;

    let mut hits = semantic_candidates(conn, query, qvec, active, dims, aliases).await?;
    // The min_similarity gate applies to the raw cosine similarity; the
    // retired-status fade lands after it, so fading can never drop a hit that
    // cleared the gate - a retired hit's reported score is the faded
    // similarity, which may itself land below min_similarity.
    hits.retain(|(sim, _)| *sim >= min_sim);
    for entry in &mut hits {
        entry.0 *= retired_factor(&entry.1.status, retired_weight);
    }
    hits.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.title.cmp(&b.1.title))
    });

    let total = hits.len();
    let start = (page - 1) * limit;
    let items: Vec<(i64, SearchHit)> = hits
        .into_iter()
        .skip(start)
        .take(limit)
        .map(|(sim, cand)| {
            let id = cand.id;
            let snippet = cand.lead_snippet();
            (id, cand.into_engram_hit(snippet, sim))
        })
        .collect();
    finish_page(conn, items, page, limit, total).await
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
    aliases: &AliasMap,
    candidate_cap: usize,
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
    let salience_weight = query.salience_weight.unwrap_or(DEFAULT_SALIENCE_WEIGHT);
    let retired_weight = query.retired_weight.unwrap_or(DEFAULT_RETIRED_WEIGHT);

    check_staleness(coverage, active, dims)?;

    let terms: Vec<String> = query.text.as_deref().map(terms_of).unwrap_or_default();
    let text_scored = if terms.is_empty() {
        Vec::new()
    } else {
        scored_lexical(conn, query, &terms, aliases, candidate_cap).await?
    };
    let mut sem = semantic_candidates(conn, query, qvec, active, dims, aliases).await?;
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
            let relevance = match (m.text, m.sem) {
                (Some(t), Some(s)) => HYBRID_TEXT_WEIGHT * t + HYBRID_SEMANTIC_WEIGHT * s,
                (Some(t), None) => t * SINGLE_SOURCE_PENALTY,
                (None, Some(s)) => s * SINGLE_SOURCE_PENALTY,
                (None, None) => 0.0,
            };
            let score = (relevance + salience_prior(m.cand.salience, salience_weight))
                * retired_factor(&m.cand.status, retired_weight);
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
    let items: Vec<(i64, SearchHit)> = ranked
        .into_iter()
        .skip(start)
        .take(limit)
        .map(|(score, m)| {
            let id = m.cand.id;
            let snippet = if !terms.is_empty() && m.text.is_some() {
                m.cand.text_snippet(&terms)
            } else {
                m.cand.lead_snippet()
            };
            (id, m.cand.into_engram_hit(snippet, score))
        })
        .collect();
    finish_page(conn, items, page, limit, total).await
}

/// The projection every candidate row carries, in the column order
/// [`Candidate::from_row`] reads. Shared by the lexical scan, the filter-only
/// listing and the semantic hydrate so all three decode identically.
const CANDIDATE_COLUMNS: &str = "e.id, d.name, e.permalink, e.title, e.engram_type, e.status, \
     e.description, e.content, CAST(json_extract(e.metadata, '$.salience') AS REAL)";

/// Phase 1 of the semantic scan: the narrow top-k. Groups the matching chunk
/// rows by their parent engram, keeps each engram's closest chunk and orders by
/// that distance, projecting nothing but the id and the distance.
///
/// The narrowness is the whole point. Turso feeds one record per chunk row into
/// the `GROUP BY` sorter, so any wide column in this projection is written to
/// the sorter's spill file once per chunk of its own engram: quadratic in engram
/// size, measured at tens of GB on a real corpus (see
/// `research/2026-07-28-turso-sorter-spill.md`). Two 8-byte columns per record
/// keep the sorter in its 2MB buffer instead.
///
/// Ties. `dist` alone leaves engrams at an equal distance in sorter-defined
/// order, which decides arbitrarily which of them survives the `LIMIT` cut. The
/// `c.engram_id ASC` tiebreak makes that cut deterministic (the lower id wins)
/// and costs nothing: it is the grouping key, already in the sorter record.
/// The lexical candidate prefilter: every row matching the reader's terms and
/// filters, capped, ranked afterwards in Rust.
///
/// `ORDER BY e.id` is the cheapest order this wide projection can be given:
/// unscoped, or scoped by path alone, it is satisfied from the table's own
/// rowid order and no sorter opens at all. Scoped to a domain it does sort -
/// `d.name IN (...)` drives the join from `domain` and reaches `engram`
/// through `idx_engram_domain`, whose order is not rowid order - and what
/// holds that sorter down is the `LIMIT` in this same statement, which lets
/// turso keep `candidate_cap` records rather than the match set. Both
/// properties are pinned by `EXPLAIN QUERY PLAN` beside this builder
/// ([`the_lexical_candidate_scan_stays_index_ordered_under_a_folder_filter`])
/// and by a source scan in `tests/turso_only.rs`. Keep the bound and keep the
/// order: any other ordering here, or a `GROUP BY`, would spill every matched
/// body to disk.
///
/// Built here rather than inline so the plan guard explains the statement this
/// store issues rather than a copy of it: the copy it used to explain had lost
/// the actor screen and the `WHERE {actor_screen} {and_filters}` restructuring
/// the wave gave the shipped one.
#[doc(hidden)]
pub fn lexical_candidate_sql(
    actor_screen: &str,
    and_filters: &str,
    candidate_cap: usize,
) -> String {
    format!(
        "SELECT {CANDIDATE_COLUMNS} FROM engram e JOIN domain d ON d.id=e.domain_id \
         WHERE {actor_screen} {and_filters} ORDER BY e.id LIMIT {candidate_cap}"
    )
}

#[doc(hidden)]
pub fn semantic_phase1_sql(actor_screen: &str, and_filters: &str) -> String {
    format!(
        "SELECT c.engram_id, min(vector_distance_cos(c.embedding, ?1)) AS dist \
         FROM chunk c JOIN engram e ON e.id=c.engram_id JOIN domain d ON d.id=e.domain_id \
         WHERE {actor_screen} {and_filters} \
         GROUP BY c.engram_id ORDER BY dist ASC, c.engram_id ASC LIMIT {SEMANTIC_TOPK}"
    )
}

/// Phase 2 of the semantic scan: hydrate the at most [`SEMANTIC_TOPK`] winners
/// by primary key. No `ORDER BY` and no `GROUP BY`, so the wide columns never
/// reach a sorter; the phase-1 order is reapplied in Rust. The id list is
/// interpolated because every id is an `i64` read out of this same database.
#[doc(hidden)]
pub fn semantic_hydrate_sql(actor_screen: &str, ids: &str) -> String {
    format!(
        "SELECT {CANDIDATE_COLUMNS} FROM engram e JOIN domain d ON d.id=e.domain_id \
         WHERE {actor_screen} AND e.id IN ({ids})"
    )
}

/// The nearest engrams to the query vector, as `(similarity, candidate)` pairs.
/// One row per engram (the closest of its chunks), filtered by every scalar
/// filter on the query, ordered by distance and capped at the top-k.
///
/// Two queries, not one: a narrow top-k ([`semantic_phase1_sql`]) and a
/// hydrate-by-id ([`semantic_hydrate_sql`]). The ranking is unchanged - the same
/// minimum distance per engram, the same [`SEMANTIC_TOPK`] cut, the same
/// distance order - but the engram bodies are read once per hit instead of once
/// per chunk fed to a sorter.
async fn semantic_candidates(
    conn: &Connection,
    query: &SearchQuery,
    qvec: &[f32],
    active: &str,
    dims: usize,
    aliases: &AliasMap,
) -> Result<Vec<(f64, Candidate)>> {
    // ?1 is the query vector; scalar filters and the model and dims predicates
    // take the placeholders after it.
    let mut params: Vec<Value> = vec![Value::Blob(pack_vector(qvec))];
    let mut clauses: Vec<String> = Vec::new();
    let mut n = 2usize;
    build_scalar_filters(query, &mut clauses, &mut params, &mut n, aliases);
    let model_ph = n;
    params.push(Value::Text(active.to_string()));
    n += 1;
    let dims_ph = n;
    params.push(Value::Integer(dims as i64));
    // The counter goes on past the last placeholder this block wrote, because
    // the actor screen below takes the next one.
    n += 1;
    clauses.push(format!(
        "c.embedding IS NOT NULL AND c.model = ?{model_ph} AND c.dims = ?{dims_ph}"
    ));

    let and_filters = format!("AND {}", clauses.join(" AND "));
    let phase1_screen = actor_screen(query.actor.as_deref(), &mut params, &mut n);
    let rows = query_all(
        conn,
        &semantic_phase1_sql(&phase1_screen, &and_filters),
        params,
    )
    .await?;
    let winners: Vec<(i64, f64)> = rows
        .iter()
        .map(|r| (cell_i64(r, 0).unwrap_or(0), cell_real(r, 1).unwrap_or(1.0)))
        .collect();
    drop(rows);
    if winners.is_empty() {
        return Ok(Vec::new());
    }

    let ids = winners
        .iter()
        .map(|(id, _)| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    // The hydrate carries the screen too, rather than trusting phase 1 to have
    // applied it: the two statements are edited apart from each other, and a
    // wide projection is the one place a leak would carry the whole document.
    let mut hydrate_params: Vec<Value> = Vec::new();
    let mut hn = 1usize;
    let hydrate_screen = actor_screen(query.actor.as_deref(), &mut hydrate_params, &mut hn);
    let rows = query_all(
        conn,
        &semantic_hydrate_sql(&hydrate_screen, &ids),
        hydrate_params,
    )
    .await?;
    // Consume the rows by value so each engram body is moved into its candidate
    // rather than copied beside it, the same discipline `scored_lexical` uses.
    let mut by_id: std::collections::HashMap<i64, Candidate> =
        std::collections::HashMap::with_capacity(rows.len());
    for r in rows {
        let c = Candidate::from_row(&r);
        drop(r);
        by_id.insert(c.id, c);
    }

    // Reapply the phase-1 order in Rust: distance ascending, then engram id.
    // A winner missing from the hydrate is impossible under the foreign key and
    // is skipped rather than faked into a hit.
    let mut out = Vec::with_capacity(winners.len());
    for (id, dist) in winners {
        if let Some(c) = by_id.remove(&id) {
            out.push((1.0 - dist, c));
        }
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
    aliases: &AliasMap,
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

    // A folder filter, matched as a literal prefix: the caller hands the folder
    // with its trailing slash, so `notes/` selects `notes/deep/y.md` and never
    // the sibling `notes-misc/z.md`, and `like_escape` keeps a folder named
    // `50%` or `a_b` a name rather than a pattern. It folds case on both sides,
    // in SQL - see `path_prefix_like` for why that shape and not the other one
    // in this crate.
    if let Some(prefix) = query.path_prefix.as_deref().filter(|p| !p.is_empty()) {
        clauses.push(path_prefix_like(*n, false));
        params.push(Value::Text(format!("{}%", like_escape(prefix))));
        *n += 1;
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
            "e.status IN ('stable', 'current') AND (e.valid_from IS NULL OR e.valid_from <= ?{n}) AND (e.valid_to IS NULL OR e.valid_to > ?{})",
            *n + 1
        ));
        params.push(Value::Text(today.clone()));
        params.push(Value::Text(today));
        *n += 2;
    } else if let Some(s) = &query.status {
        // `stable` and `current` are one equivalence class in both directions,
        // so a pre-flip domain and a foreign OKF bundle both answer either
        // spelling. Every other status stays an exact match.
        if is_current_status(s) {
            let ph: Vec<String> = CURRENT_STATUS_CLASS
                .iter()
                .map(|member| {
                    params.push(Value::Text((*member).to_string()));
                    let p = format!("?{n}");
                    *n += 1;
                    p
                })
                .collect();
            clauses.push(format!("e.status IN ({})", ph.join(",")));
        } else {
            clauses.push(format!("e.status = ?{n}"));
            params.push(Value::Text(s.clone()));
            *n += 1;
        }
    }

    if let Some(after) = &query.after {
        clauses.push(format!("e.recorded_at >= ?{n}"));
        params.push(Value::Text(after.clone()));
        *n += 1;
    }

    if let Some(tags) = &query.tags {
        for tag in tags {
            // One EXISTS per requested tag (require-ALL preserved), each matching
            // the tag's whole alias equivalence class via an IN list (OR within
            // the class). The class is sorted, so the binding order is stable.
            clauses.push(tag_class_exists(&tag.to_lowercase(), aliases, params, n));
        }
    }

    for f in &query.metadata_filters {
        if let Some(clause) = metadata_clause(f, params, n, aliases) {
            clauses.push(clause);
        }
    }
}

/// An `EXISTS` matching an engram that carries any member of a folded tag's
/// alias equivalence class. Tag identity is case-folded in the index, so the
/// class members (already folded) match `tag.name` directly. When the map is
/// empty the class is the tag alone, so this is a one-placeholder `IN`.
fn tag_class_exists(
    folded: &str,
    aliases: &AliasMap,
    params: &mut Vec<Value>,
    n: &mut usize,
) -> String {
    let class = aliases.class_of(folded);
    let ph: Vec<String> = class
        .iter()
        .map(|member| {
            params.push(Value::Text(member.clone()));
            let p = format!("?{n}");
            *n += 1;
            p
        })
        .collect();
    format!(
        "EXISTS (SELECT 1 FROM engram_tag et JOIN tag t ON t.id=et.tag_id WHERE et.engram_id=e.id AND t.name IN ({}))",
        ph.join(",")
    )
}

/// Map one metadata filter to a SQL predicate, appending its bound values.
/// Promoted keys map to columns; everything else to `json_extract`.
fn metadata_clause(
    f: &MetadataFilter,
    params: &mut Vec<Value>,
    n: &mut usize,
    aliases: &AliasMap,
) -> Option<String> {
    let key = f.key.as_str();

    if key == "tags" {
        // A `tags` metadata filter folds the same as the dedicated `tags` field:
        // each value expands to its alias equivalence class.
        let exists = |val: &serde_json::Value, params: &mut Vec<Value>, n: &mut usize| {
            tag_class_exists(&fold_tag_value(val), aliases, params, n)
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

/// Fold a `tags` filter value to the lowercase string used for alias expansion
/// and matched against the case-folded `tag.name`. A tags filter is a string in
/// practice; a non-string value is stringified so it still binds to the TEXT
/// column.
fn fold_tag_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.to_lowercase(),
        other => other.to_string().to_lowercase(),
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
    /// The raw `salience` frontmatter value if present, read from the metadata
    /// JSON column. `None` when absent or non-numeric. Feeds the ranking prior.
    salience: Option<f64>,
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
            salience: cell_real(r, 8),
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
                tags: Vec::new(),
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
            tags: Vec::new(),
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
            tags: Vec::new(),
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

/// The frontier arm for references the index bound to nothing, which exists
/// only when a reader is named.
///
/// A base row's reference resolves against the base and stays unbound when
/// nothing there answers it - and one reader's own draft may answer it all the
/// same. That reading is theirs alone and is never written into `to_id`, so it
/// is made here, at the far end of the edge, against that actor's live drafts
/// and nothing else. With no actor the arm is not emitted at all, so a
/// traversal that names nobody is the statement that was always there.
/// Eight arguments, like `push_edge` below and for the same reason: the two
/// reference tables are one shape written twice, and a struct to carry the
/// spelling differences would be a type nothing else ever holds.
#[allow(clippy::too_many_arguments)]
fn pending_arm(
    select: &str,
    table: &str,
    alias: &str,
    actor: Option<&str>,
    list: &str,
    src_screen: &str,
    params: &mut Vec<Value>,
    n: &mut usize,
) -> String {
    let Some(actor) = actor else {
        return String::new();
    };
    let drafts = drafts_only_on("e", actor, params, n);
    let reach = crate::store::reference_match(
        alias,
        crate::store::ReferenceCandidates::DraftsOnly { screen: &drafts },
    );
    format!(
        " UNION ALL \
         SELECT {select} FROM {table} {alias} \
         JOIN engram src ON src.id={alias}.engram_id AND {src_screen} \
         JOIN engram dst ON dst.id = {reach} \
         WHERE {alias}.to_id IS NULL \
           AND ({alias}.engram_id IN ({list}) OR dst.id IN ({list}))"
    )
}

/// The node hydrate that closes [`neighbors`]: the addressing of every engram
/// the traversal met, screened for this reader.
///
/// Not shared with postgres, unlike the two frontier statements the traversal
/// issues: the salience column is read out of turso's `json_extract` and out of
/// postgres's `jsonb` operators, so the two texts genuinely differ.
#[doc(hidden)]
pub fn node_hydrate_sql(node_screen: &str, list: &str) -> String {
    format!(
        "SELECT e.id, d.name, e.permalink, e.title, e.engram_type, \
         CAST(json_extract(e.metadata, '$.salience') AS REAL), e.status, e.actor \
         FROM engram e JOIN domain d ON d.id=e.domain_id \
         WHERE {node_screen} AND e.id IN ({list}) ORDER BY e.id"
    )
}

/// Traverse the neighborhood of the seed engrams up to `depth` hops, in
/// `actor`'s view of the index: `None` walks the base rows alone and
/// `Some(a)` walks that actor's shadowed view, their own drafts standing in
/// for the rows they are drafts of and a path they have deleted reachable
/// from nothing.
///
/// **Every edge arrives where the reader's own address map says.** The stored
/// `to_id` names a row; the hop through `tgt` reads that row's address and the
/// hop through `dst` reads what this reader holds there. So a team edge into a
/// page they are drafting arrives at their draft, one into a page they have
/// deleted arrives nowhere and is not drawn, and with no actor named `dst` is
/// the row `to_id` named all along.
///
/// **Which edges are looked for is still keyed on the stored `to_id`.** An
/// inbound edge is found when the row it was bound to is in the frontier, not
/// when the reader's draft at that address is - the same sentence as
/// `inbound_refs`: who points at an address is a fact about the address the
/// team shares. Deliberate, and the reason a reader seeded on their own draft
/// meets what it points at rather than what points at the page beneath it.
pub(super) async fn neighbors(
    conn: &Connection,
    ids: &[EngramId],
    depth: u8,
    actor: Option<&str>,
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

        let mut rel_params: Vec<Value> = Vec::new();
        let mut rel_n = 1usize;
        let src_screen = actor_screen_on("src", actor, &mut rel_params, &mut rel_n);
        let dst_screen = actor_screen_on("dst", actor, &mut rel_params, &mut rel_n);
        let rel_pending = pending_arm(
            "r.engram_id, dst.id, r.rel_type",
            "relation",
            "r",
            actor,
            &list,
            &src_screen,
            &mut rel_params,
            &mut rel_n,
        );
        // The statement, both screens and why they are there: see
        // `crate::store::relation_frontier_sql`, which both backends issue.
        let rel_rows = query_all(
            conn,
            &relation_frontier_sql(&list, &src_screen, &dst_screen, &rel_pending),
            rel_params,
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

        let mut link_params: Vec<Value> = Vec::new();
        let mut link_n = 1usize;
        let src_screen = actor_screen_on("src", actor, &mut link_params, &mut link_n);
        let dst_screen = actor_screen_on("dst", actor, &mut link_params, &mut link_n);
        let link_pending = pending_arm(
            "l.engram_id, dst.id",
            "link",
            "l",
            actor,
            &list,
            &src_screen,
            &mut link_params,
            &mut link_n,
        );
        let link_rows = query_all(
            conn,
            &link_frontier_sql(&list, &src_screen, &dst_screen, &link_pending),
            link_params,
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
        let mut node_params: Vec<Value> = Vec::new();
        let mut node_n = 1usize;
        let node_screen = actor_screen_on("e", actor, &mut node_params, &mut node_n);
        let rows = query_all(conn, &node_hydrate_sql(&node_screen, &list), node_params).await?;
        for r in &rows {
            nodes.push(GraphNode {
                id: EngramId(cell_i64(r, 0).unwrap_or(0)),
                domain: cell_text(r, 1).unwrap_or_default(),
                permalink: cell_text(r, 2).unwrap_or_default(),
                title: cell_text(r, 3).unwrap_or_default(),
                engram_type: cell_text(r, 4).unwrap_or_default(),
                salience: cell_real(r, 5),
                status: cell_text(r, 6).unwrap_or_default(),
                actor: cell_text(r, 7).unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The column list a statement projects: everything between `SELECT` and the
    /// first top-level ` FROM `. The queries under test have no subquery in
    /// their projection, so the first ` FROM ` is the right one.
    fn projection_of(sql: &str) -> &str {
        let rest = sql.strip_prefix("SELECT ").expect("a SELECT statement");
        let end = rest.find(" FROM ").expect("a FROM clause");
        &rest[..end]
    }

    /// A representative trailing conjunction, shaped like the one
    /// `build_scalar_filters` produces for a domain-scoped semantic query: the
    /// statement opens its own `WHERE` with the actor screen and these ride
    /// behind it.
    const WHERE_SQL: &str = "AND d.name IN (?2) AND c.embedding IS NOT NULL \
                             AND c.model = ?3 AND c.dims = ?4";

    /// The regression guard for the 2026-07-28 spill: phase 1 runs the grouping
    /// and the ordering, so every column it projects is written into turso's
    /// sorter once per chunk row. An engram body in there is quadratic in engram
    /// size (tens of GB were measured in the field). Only the grouping key and
    /// the aggregate may appear.
    #[test]
    fn the_semantic_phase_one_projection_carries_no_engram_columns() {
        let sql = semantic_phase1_sql("e.actor = ''", WHERE_SQL);
        let projection = projection_of(&sql);
        assert_eq!(
            projection, "c.engram_id, min(vector_distance_cos(c.embedding, ?1)) AS dist",
            "phase 1 projects the grouping key and the distance, nothing else"
        );
        for wide in ["e.content", "e.description", "e.title", "e.metadata"] {
            assert!(
                !projection.contains(wide),
                "phase 1 must not feed {wide} into the sorter, projection was: {projection}"
            );
        }
        assert!(
            sql.contains("GROUP BY c.engram_id ORDER BY dist ASC, c.engram_id ASC"),
            "the LIMIT cut stays deterministic on a distance tie: {sql}"
        );
        assert!(sql.ends_with(&format!("LIMIT {SEMANTIC_TOPK}")));
    }

    /// The wide columns live in phase 2, which is a primary-key lookup: no
    /// grouping and no ordering, so nothing it projects can reach a sorter.
    #[test]
    fn the_semantic_hydrate_never_sorts() {
        let sql = semantic_hydrate_sql("e.actor = ''", "1,2,3");
        assert!(
            !sql.contains("ORDER BY"),
            "no ordering in the hydrate: {sql}"
        );
        assert!(
            !sql.contains("GROUP BY"),
            "no grouping in the hydrate: {sql}"
        );
        assert!(
            projection_of(&sql).contains("e.content"),
            "the hydrate is where the bodies are read: {sql}"
        );
    }

    /// The projection every caller of `Candidate::from_row` shares, pinned so a
    /// column added to one path can never silently shift another path's decode.
    #[test]
    fn every_candidate_query_shares_one_column_order() {
        assert_eq!(
            CANDIDATE_COLUMNS,
            "e.id, d.name, e.permalink, e.title, e.engram_type, e.status, \
     e.description, e.content, CAST(json_extract(e.metadata, '$.salience') AS REAL)"
        );
    }

    /// The lexical candidate scan keeps its index order under a folder filter.
    ///
    /// This is the one query in the tree that carries full bodies (`e.content`,
    /// `e.description`) past a plan decision: it loads up to
    /// `LEXICAL_CANDIDATE_CAP` rows and ranks them in Rust. `ORDER BY e.id` is
    /// served from the table's own rowid order, so nothing sorts those bodies,
    /// and the folder prefix the listing pushes into the same `WHERE` is bound
    /// here on purpose - it is the newest predicate on this query, and a
    /// predicate is exactly what can talk a planner out of an index-ordered
    /// scan. It does not: the plan stays a rowid-ordered scan of `engram`.
    ///
    /// What this test also records, because it is measured rather than assumed:
    /// a **domain-scoped** candidate query does open a sorter. `d.name IN (...)`
    /// drives the join from `domain` and reaches `engram` through
    /// `idx_engram_domain`, whose order is not rowid order, so turso sorts. That
    /// is older than the folder filter and it is bounded by the candidate cap in
    /// the same statement, which is the property
    /// `the_candidate_projection_is_never_unbounded` pins. Recorded here so the
    /// next reader of the "never reaches a sorter" comment beside the query
    /// knows which shape it was written about.
    ///
    /// **The statement is the shipped one**, built by `lexical_candidate_sql`
    /// out of the actor screen and the filters `build_scalar_filters` produces,
    /// with only the term group written out here. Its predecessor lived in
    /// `tests/turso_only.rs` and copied the statement by hand, which meant it
    /// went on explaining a query without the actor screen long after the
    /// shipped one had grown one.
    #[tokio::test]
    async fn the_lexical_candidate_scan_stays_index_ordered_under_a_folder_filter() {
        let store = crate::TursoStore::open_in_memory().await.unwrap();

        let query = SearchQuery {
            path_prefix: Some("notes/".to_string()),
            ..SearchQuery::default()
        };
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        let mut n = 1usize;
        build_scalar_filters(
            &query,
            &mut clauses,
            &mut params,
            &mut n,
            &crate::alias::AliasMap::default(),
        );
        // One term group over the three lexical columns, exactly as
        // `scored_lexical` composes it for a full-text mode.
        let mut ors: Vec<String> = Vec::new();
        for col in ["title", "description", "content"] {
            ors.push(format!("lower(e.{col}) LIKE ?{n} ESCAPE '\\'"));
            n += 1;
        }
        clauses.push(format!("({})", ors.join(" OR ")));
        let and_filters = format!("AND {}", clauses.join(" AND "));
        let actor_screen = actor_screen(None, &mut params, &mut n);

        let plan = store
            .explain_query_plan(&lexical_candidate_sql(&actor_screen, &and_filters, 5000))
            .await
            .unwrap()
            .join(" | ");
        assert!(
            !plan.to_uppercase().contains("SORTER") && !plan.contains("TEMP B-TREE"),
            "a folder filter must not cost the candidate scan its rowid order, \
             plan was: {plan}"
        );
    }

    /// The plan-level half of the guard, in the same discipline as the
    /// `idx_engram_current` plan test: turso opens sorters for phase 1 (that is
    /// what a `GROUP BY` plus an `ORDER BY` costs) and opens none at all for the
    /// hydrate, so the bodies are read by rowid and never serialized.
    #[tokio::test]
    async fn the_semantic_split_plans_a_sorter_free_hydrate() {
        let store = crate::TursoStore::open_in_memory().await.unwrap();

        let and_filters = "AND c.embedding IS NOT NULL AND c.model = ?2 AND c.dims = ?3";
        let phase1 = store
            .explain_query_plan(&semantic_phase1_sql("e.actor = ''", and_filters))
            .await
            .unwrap()
            .join(" | ");
        assert!(
            phase1.contains("chunk") || phase1.contains("c "),
            "phase 1 drives off the chunk table, plan was: {phase1}"
        );
        // Whatever sorters this plan opens are fed two 8-byte columns, which the
        // projection test above pins. On an empty database turso answers the
        // grouping from `idx_chunk_engram` and opens only the ORDER BY sorter;
        // that is a bonus, not a guarantee, so it is not asserted here - the
        // narrowness is the invariant, not the sorter count.

        let hydrate = store
            .explain_query_plan(&semantic_hydrate_sql("e.actor = ''", "1,2,3"))
            .await
            .unwrap()
            .join(" | ");
        assert!(
            !hydrate.to_uppercase().contains("SORTER"),
            "the hydrate must open no sorter, plan was: {hydrate}"
        );
        assert!(
            hydrate.contains("INTEGER PRIMARY KEY") || hydrate.contains("USING INDEX"),
            "the hydrate is a keyed lookup, plan was: {hydrate}"
        );
    }
}
