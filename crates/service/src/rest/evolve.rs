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
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use utoipa::IntoParams;

use super::auth::Identity;
use super::{ApiError, ApiJson, ApiPath, ApiQuery, ProblemDetail, RestState, csv, refuse_read_only};
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

/// What the two acknowledgment endpoints take: which engram, which rule and,
/// on the way in, why.
///
/// The permalink rides in the body rather than the path for the same reason
/// `RetireBody`'s does: a permalink is a path of its own, so an action segment
/// after a wildcard would be eaten by the wildcard.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[schema(description = "Acknowledge one finding on one engram: the engram by \
                        permalink, the rule id that fired and an optional note \
                        saying why it is intentional. The scope an \
                        acknowledgment holds for is never sent - the server \
                        computes it by running detection.")]
pub struct AckBody {
    /// The engram the finding fired on.
    #[schema(example = "notes/beta")]
    permalink: String,
    /// The rule id to acknowledge, for example `V101`.
    #[schema(example = "V101")]
    rule: String,
    /// Why the finding is intentional. Ignored on `DELETE`.
    #[serde(default)]
    #[schema(example = "lineage citation, keep")]
    note: Option<String>,
}

/// `POST /domains/{domain}/evolve/ack` - rule a finding intentional so future
/// sweeps count it instead of raising it.
///
/// The evidence the acknowledgment is given for is the server's to determine:
/// it runs detection over the engram's domain, takes the firing finding's scope
/// and stores it with the entry. That is what makes an acknowledgment hold
/// while its evidence holds and come back marked stale when the evidence
/// changes, without a human or an agent ever handling a fingerprint.
///
/// The entry lands in the engram's own frontmatter through the same edit path
/// the MCP `set_frontmatter` verb uses, so it travels with team sharing,
/// survives a resync and can be removed by hand.
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/evolve/ack",
    tag = "maintenance",
    operation_id = "acknowledge_finding",
    summary = "Acknowledge one evolve finding on one engram.",
    description = "Records `evolve_ack` on the engram: the rule, the evidence \
                   the server computed it fired on, the note, the acknowledging \
                   user and the instant. A matching acknowledgment keeps the \
                   finding out of the queue and counted in `acknowledged`; when \
                   the evidence changes the finding returns marked `ack_stale`.",
    params(("domain" = String, Path, description = "The engram's domain.")),
    request_body = AckBody,
    responses(
        (
            status = 200,
            description = "The stored entry.",
            body = Object,
            example = json!({
                "rule": "V101",
                "scope": "eng/old-runbook",
                "note": "lineage citation, keep",
                "by": "human:jordi",
                "at": "2026-08-20T09:00:00+00:00"
            }),
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one: an identity with \
                           no account behind it never writes.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an editor, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain or engram.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 415,
            description = "The body is not `application/json`.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The rule id is not one the sweep catalog holds.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn acknowledge(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<AckBody>,
) -> Result<Json<Value>, ApiError> {
    let caller = identity.require_editor()?;
    refuse_read_only(&state)?;
    let entry = state
        .engine
        .acknowledge_finding_as(
            &domain,
            &body.permalink,
            &body.rule,
            body.note.as_deref(),
            Some(&format!("human:{}", caller.name())),
        )
        .await?;
    Ok(Json(entry))
}

/// `DELETE /domains/{domain}/evolve/ack` - withdraw an acknowledgment, so the
/// finding rejoins the queue on the next sweep.
///
/// The half deliberately missing from the MCP surface: an agent may silence a
/// finding a person ruled intentional, and un-silencing it is the person's
/// call, made here or by editing the file.
#[utoipa::path(
    delete,
    path = "/api/v1/domains/{domain}/evolve/ack",
    tag = "maintenance",
    operation_id = "unacknowledge_finding",
    summary = "Withdraw an acknowledgment.",
    description = "Removes the engram's `evolve_ack` entry for that rule, \
                   leaving its other entries alone. 404 when the engram carries \
                   none for the rule, rather than reporting a removal that did \
                   not happen.",
    params(("domain" = String, Path, description = "The engram's domain.")),
    request_body = AckBody,
    responses(
        (status = 204, description = "The acknowledgment is gone."),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "Not an editor, no CSRF token echoed, read-only \
                           instance, or a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain or engram, or no acknowledgment for \
                           that rule on it.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 415,
            description = "The body is not `application/json`.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The rule id is not one the sweep catalog holds.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn unacknowledge(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<AckBody>,
) -> Result<StatusCode, ApiError> {
    let caller = identity.require_editor()?;
    refuse_read_only(&state)?;
    let removed = state
        .engine
        .unacknowledge_finding_as(
            &domain,
            &body.permalink,
            &body.rule,
            Some(&format!("human:{}", caller.name())),
        )
        .await?;
    if !removed {
        return Err(ApiError::not_found(format!(
            "no acknowledgment of {} on '{}' in domain '{}'",
            body.rule.trim().to_ascii_uppercase(),
            body.permalink,
            domain
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}
