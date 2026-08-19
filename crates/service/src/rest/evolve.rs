//! The consolidation queue: what the knowledge needs next, ranked, in the shape
//! a maintenance page renders.
//!
//! The same sweep the `evolve` MCP tool runs, answered read-only. The tool
//! records that a sweep happened, because an agent that asks for the queue is
//! about to work it; this route calls the detection half instead
//! ([`crate::engine::Engine::evolve_detect`]), so a person leaving the page open
//! never tells this machine that its backlog was attended to. Nothing here
//! writes: not an engram, not the maintenance state a Stop hook nudges from.
//!
//! The engine's own JSON is handed back unchanged, like every other read on this
//! surface, so the page and the tool describe one queue rather than two.

use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::Value;
use utoipa::IntoParams;

use super::{ApiError, ApiQuery, ProblemDetail, RestState, csv};
use crate::params::EvolveParams;

/// The query string `GET /evolve` takes, mirroring [`EvolveParams`] minus its
/// `today`.
///
/// That omission is the one deliberate difference from the tool: pinning the
/// date makes an agent's run reproducible across a session, and a page always
/// asks about now. A caller who needs a fixed date has the tool.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EvolveQuery {
    /// Restrict the sweep to these domains, comma separated. Defaults to every
    /// registered domain.
    #[serde(default)]
    #[param(example = "eng,ops")]
    domains: Option<String>,
    /// Restrict to these detector families, comma separated: `temporal`,
    /// `structure` or `redundancy`. Defaults to all three.
    #[serde(default)]
    #[param(example = "temporal,structure")]
    families: Option<String>,
    /// Restrict to these rule ids, comma separated, for example `V006,V201`.
    #[serde(default)]
    #[param(example = "V006")]
    rules: Option<String>,
    /// Drop findings scoring under this priority, 0 to 100.
    #[serde(default)]
    #[param(example = 50)]
    min_priority: Option<u8>,
    /// Page size. Defaults to 10, clamped by the engine to 100.
    #[serde(default)]
    #[param(example = 25)]
    limit: Option<usize>,
    /// One-based page number. Defaults to 1.
    #[serde(default)]
    #[param(example = 1)]
    page: Option<usize>,
    /// Include the findings acknowledgments suppressed, each marked
    /// `acknowledged` with the note that silenced it. Off by default: the queue
    /// is what still needs deciding.
    #[serde(default)]
    #[param(example = true)]
    include_acknowledged: Option<bool>,
}

/// `GET /evolve` - the consolidation queue: the ranked list of what the
/// registered domains need next, with the evidence behind each finding and the
/// instruction that says how to work it.
///
/// A read and only a read. Detection runs over the scope and nothing at all is
/// written, the maintenance state included: viewing the queue never counts as a
/// consolidation run, so opening this page does not stop a Stop hook from asking
/// for the sweep that is actually owed. The `evolve` MCP tool is the surface that
/// records a run, because an agent calling it is about to do the work.
///
/// Each row carries its rank across the whole result (`n`), its priority, the
/// rule that fired, the engram it fired on, and a `class`: `mechanical` for work
/// that completes intent the knowledge already records, `judgment` for work that
/// changes what it claims. A client renders the two differently - a judgment
/// finding is a question for a person, never a change to apply.
///
/// The per-rule instruction rides `actions` rather than a column, so a page of
/// findings from one rule carries it once; only the rules on this page appear.
/// `families` counts the whole filtered result rather than the page, which is
/// what section headings are drawn from, and `truncations` names any per-domain
/// cap that fired so a short queue is never mistaken for a finished one.
///
/// `acknowledged` counts what acknowledgments kept out of the queue, whole and
/// per family, so a short queue is never mistaken for a healthy one; a row whose
/// acknowledgment was given for evidence that has since changed comes back
/// carrying `ack_stale` and the old `ack_note`. Pass `include_acknowledged` to
/// see the suppressed rows themselves, each marked `acknowledged`.
///
/// `today` is not exposed. The temporal rules are evaluated as of now, which is
/// the only question a page asks; the tool takes a pinned date for a run that
/// has to be reproducible.
#[utoipa::path(
    get,
    path = "/api/v1/evolve",
    tag = "maintenance",
    operation_id = "get_evolve_queue",
    params(EvolveQuery),
    responses(
        (
            status = 200,
            description = "The engine's own evolve payload, unchanged: the swept \
                           scope, the ranked queue page, the per-family counts \
                           over the whole result, the per-rule instructions for \
                           the rules on this page, the shared guidance and any \
                           truncation that fired.",
            body = Object,
            example = json!({
                "scope": {
                    "domains": ["eng"],
                    "families": [],
                    "rules": [],
                    "min_priority": null,
                    "today": "2026-08-17"
                },
                "engrams_scanned": 42,
                "unparsed": 0,
                "total": 2,
                "page": 1,
                "limit": 10,
                "count": 2,
                "families": [{ "family": "temporal", "findings": 2 }],
                "acknowledged": {
                    "total": 1,
                    "by_family": { "temporal": 0, "structure": 1, "redundancy": 0 }
                },
                "queue": [{
                    "n": 1,
                    "priority": 58,
                    "rule": "V006",
                    "class": "judgment",
                    "domain": "eng",
                    "permalink": "human-capture",
                    "title": "Incident capture",
                    "line": null,
                    "finding": "captured by a person and never reviewed",
                    "evidence": "generated.by human:jordi; recorded 2026-08-16; no verified entry",
                    "fix": "review, then record a verified entry (edit_engram set_frontmatter verified)"
                }],
                "actions": [{
                    "rule": "V006",
                    "instruction": "A person captured this directly and nobody has reviewed it since. ..."
                }],
                "guidance": "This queue changes nothing by itself. ...",
                "truncations": []
            }),
        ),
        (
            status = 400,
            description = "The query string will not parse.",
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
            description = "`domains` names a domain nobody registered.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "`families` or `rules` names something the catalog does \
                           not have.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn queue(
    State(state): State<RestState>,
    ApiQuery(query): ApiQuery<EvolveQuery>,
) -> Result<Json<Value>, ApiError> {
    let value = state
        .engine
        .evolve_detect(&EvolveParams {
            domains: csv(query.domains.as_deref()),
            families: csv(query.families.as_deref()),
            rules: csv(query.rules.as_deref()),
            min_priority: query.min_priority,
            limit: query.limit,
            page: query.page,
            include_acknowledged: query.include_acknowledged.unwrap_or(false),
            // Never from the caller: see the type and the operation doc above.
            today: None,
        })
        .await?;
    Ok(Json(value))
}
