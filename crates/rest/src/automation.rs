//! What the reflect and evolve loops did on a domain, for the pages that
//! show it. A read, with the session cookie: the tokens a runner works with
//! stay off this API, but a person looking at Fluid needs to see when each
//! loop last ran, what it left for them and whether its webhook works.
//!
//! The engine answers ([`crate::engine::Engine::automation_overview`]); this
//! only gives its entries a typed shape. Nothing is computed here. A run
//! carries counts only, never the items it left, which are keyed by page
//! paths.

use axum::Json;
use axum::extract::State;
use crystalline_core::automation::Loop;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use super::auth::Identity;
use super::{ApiError, ApiPath, ApiQuery, ProblemDetail, RestState};

/// One recorded run of a loop.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LoopRun {
    /// When the run started, RFC 3339.
    #[schema(example = "2026-10-21T09:00:00+00:00")]
    pub started_at: String,
    /// When the run ended, RFC 3339.
    #[schema(example = "2026-10-21T09:04:00+00:00")]
    pub ended_at: String,
    /// The runner's label.
    #[schema(example = "n8n")]
    pub runner: String,
    /// The account the run worked as; only for a caller who may run the loop.
    /// `null` for a run by an agent of the loops, which `agent` names.
    #[schema(example = "robot")]
    pub account: Option<String>,
    /// The unique name of the agent of the loops the run worked as; only for
    /// a caller who may run the loop, and `null` for a person's run or the
    /// machine's own.
    #[serde(default)]
    #[schema(example = "nightly")]
    pub agent: Option<String>,
    /// The name `agent` is shown under, read when the answer is made; `null`
    /// without an agent, or once the agent is gone.
    #[serde(default)]
    #[schema(example = "Nightly curator")]
    pub agent_display: Option<String>,
    /// `done`, `stopped_at_cap`, `failed`, `nothing_due` or `expired`.
    #[schema(example = "done")]
    pub outcome: String,
    /// Items done.
    #[schema(example = 4)]
    pub done: u64,
    /// Items the run left for a person.
    #[schema(example = 3)]
    pub left: u64,
    /// Judgment findings waiting when the run ended (evolve).
    #[schema(example = 2)]
    pub judgment_waiting: u64,
    /// A short summary the runner wrote; only for a caller who may run the
    /// loop.
    pub summary: Option<String>,
    /// The pull request the run shared its changes in, when it shared; only
    /// for a caller who may run the loop.
    pub pr_url: Option<String>,
}

/// What a loop's trigger webhook last did on a domain.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LoopDelivery {
    /// Whether this instance has a webhook URL and a secret to sign with.
    pub configured: bool,
    /// When the receiver last took a webhook, RFC 3339.
    pub last_sent: Option<String>,
    /// Why the last delivery failed; cleared by the next one that works.
    pub last_error: Option<String>,
}

/// A runner holding the loop's lease right now.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LoopRunning {
    /// The runner's label.
    pub runner: String,
    /// When the run took the lease, RFC 3339.
    pub taken_at: String,
}

/// One loop on one domain.
#[derive(Debug, Serialize, ToSchema)]
pub struct LoopState {
    /// The last recorded run; `null` when the loop never ran here.
    pub last_run: Option<LoopRun>,
    /// The runner working now; `null` when none is.
    pub running: Option<LoopRunning>,
    /// Items a finished run left for a person that have not changed since.
    /// `null` unless the caller may run the loop and asked for counts.
    #[schema(example = 3)]
    pub left_for_person: Option<u64>,
    /// Judgment findings waiting for a person (evolve only). `null` unless
    /// the caller may run the loop and asked for counts.
    #[schema(example = 2)]
    pub judgment_waiting: Option<u64>,
    /// The trigger webhook; `null` unless the MANIFEST turns the loop's
    /// trigger on and the caller manages the domain or administers the
    /// instance.
    pub delivery: Option<LoopDelivery>,
}

/// What `GET /domains/{domain}/automation` answers. A loop the request did
/// not ask for is `null`.
#[derive(Debug, Serialize, ToSchema)]
pub struct AutomationResponse {
    /// The reflect loop.
    pub reflect: Option<LoopState>,
    /// The evolve loop.
    pub evolve: Option<LoopState>,
}

/// The query string of `GET /domains/{domain}/automation`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AutomationQuery {
    /// Only this loop: `reflect` or `evolve`. Defaults to both.
    #[serde(default, rename = "loop")]
    #[param(example = "reflect")]
    loop_: Option<String>,
    /// Whether to work out what waits for a person, which sweeps the
    /// domain. Defaults to true; a page that shows only the last run or the
    /// delivery sends false.
    #[serde(default)]
    #[param(example = false)]
    counts: Option<bool>,
}

fn parse<T: serde::de::DeserializeOwned>(entry: &Value, key: &str) -> Result<Option<T>, ApiError> {
    match entry.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|e| {
            ApiError::internal(format!("the automation state's {key} is unreadable: {e}"))
        }),
    }
}

fn state(entry: &Value) -> Result<Option<LoopState>, ApiError> {
    if entry.is_null() {
        return Ok(None);
    }
    Ok(Some(LoopState {
        last_run: parse(entry, "last_run")?,
        running: parse(entry, "running")?,
        left_for_person: parse(entry, "left_for_person")?,
        judgment_waiting: entry.pointer("/counts/judgment").and_then(Value::as_u64),
        delivery: parse(entry, "delivery")?,
    }))
}

/// `GET /domains/{domain}/automation`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/automation",
    tag = "domains",
    operation_id = "read_automation",
    summary = "When the reflect and evolve loops last ran on this domain.",
    description = "Reading needs the right to read the domain, and each \
                   caller sees its own share. Every reader gets the last run \
                   (when, which runner, the outcome and the counts) and the \
                   runner working now. A caller who may run the loop also \
                   gets the run's summary, pull request and account (or, \
                   for a run by an agent of the loops, the agent's name \
                   and display name), and, \
                   with `counts` on, how many items wait for a person. Only a \
                   domain manager or an instance admin gets `delivery`, the \
                   state of the loop's webhook trigger where the MANIFEST \
                   turns it on. Never the items a run left.",
    params(
        ("domain" = String, Path, description = "The domain."),
        AutomationQuery,
    ),
    responses(
        (status = 200, description = "The loops asked for on this domain.", body = AutomationResponse),
        (
            status = 400,
            description = "`loop` is neither reflect nor evolve.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn read(
    State(state_): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiQuery(query): ApiQuery<AutomationQuery>,
) -> Result<Json<AutomationResponse>, ApiError> {
    let only = match query.loop_.as_deref() {
        None => None,
        Some("reflect") => Some(Loop::Reflect),
        Some("evolve") => Some(Loop::Evolve),
        Some(other) => {
            return Err(ApiError::bad_request(format!(
                "loop must be reflect or evolve, not '{other}'"
            )));
        }
    };
    let overview = state_
        .engine
        .automation_overview(
            &domain,
            &identity.scope(),
            only,
            query.counts.unwrap_or(true),
        )
        .await?;
    let mut reflect = state(&overview["reflect"])?;
    let mut evolve = state(&overview["evolve"])?;
    for loop_state in [reflect.as_mut(), evolve.as_mut()].into_iter().flatten() {
        if let Some(run) = loop_state.last_run.as_mut() {
            show_agent(&state_, run).await;
        }
    }
    Ok(Json(AutomationResponse { reflect, evolve }))
}

/// Fill in the display name of the agent a run worked as. A store that
/// cannot be read leaves it out; the unique name stays.
async fn show_agent(state: &RestState, run: &mut LoopRun) {
    let Some(name) = run.agent.as_deref() else {
        return;
    };
    run.agent_display = match state.auth.agent_display(name).await {
        Ok(display) => display,
        Err(e) => {
            tracing::warn!("the display name of agent {name} could not be read: {e:#}");
            None
        }
    };
}
