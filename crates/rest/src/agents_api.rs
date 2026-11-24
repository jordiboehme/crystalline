//! The agents of the loops, managed over HTTP: the same operations
//! `crystalline agents` performs, behind the same store.
//!
//! An agent is an identity of its own for the scheduled reflect and evolve
//! runs, never an account (see `crystalline_identity::auth_store`'s `agents`
//! module). These routes hold to the rules [`super::users_api`] writes down:
//! admin only in the handler, the CSRF check of the middleware, and a
//! read-only instance refuses every mutation (`crystalline agents` on the
//! server is the way there). A token is answered once, with
//! `Cache-Control: no-store`, and never written to a log. Each change logs
//! one INFO line naming the admin, the action and the agent.
//!
//! This surface takes no bearer at all: an agent token presented here is no
//! identity, and the answer is the one any request without a session gets.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;

use super::auth::{Identity, NoStore, no_store};
use super::auth_store::{
    AGENT_TOKEN_DEFAULT_TTL_SECS, Agent, AgentLoops, IssuedAgentToken, RefusalKind, StoreRefusal,
    normalize_account_name,
};
use super::{ApiError, ApiJson, ApiPath, ProblemDetail, RestState, refuse_read_only};

/// What `GET /agents` answers with.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct AgentsResponse {
    /// Every agent with its tokens, by name.
    #[schema(value_type = Vec<super::schemas::Agent>)]
    agents: Vec<Agent>,
    /// Whether `auth.mcp` is on in the effective configuration, the value a
    /// new agent is checked against. Agents work only with it on. The MCP
    /// endpoint reads the setting when the server starts, so a change made
    /// since needs a restart there.
    auth_mcp: bool,
}

/// A freshly issued agent token. `token` is shown here and nowhere else.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[schema(
    description = "A freshly issued agent token. `token` is shown here and \
                        nowhere else, ever: only its hash is stored. The agent \
                        sends it as `Authorization: Bearer <token>`."
)]
pub struct IssuedAgentTokenResponse {
    /// The row id, which a revoke names.
    #[schema(example = 4)]
    pub id: i64,
    /// The token itself, this once.
    #[schema(example = "cag_1f3c...")]
    pub token: String,
    /// Its label.
    #[schema(example = "issued 2026-11-22")]
    pub label: String,
    /// When it stops working, RFC 3339.
    #[schema(example = "2027-02-20T09:00:00Z")]
    pub expires_at: String,
}

/// Hand-written so the token never reaches a log line.
impl std::fmt::Debug for IssuedAgentTokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedAgentTokenResponse")
            .field("id", &self.id)
            .field("token", &"cag_[redacted]")
            .field("label", &self.label)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl From<IssuedAgentToken> for IssuedAgentTokenResponse {
    fn from(issued: IssuedAgentToken) -> IssuedAgentTokenResponse {
        IssuedAgentTokenResponse {
            id: issued.id,
            token: issued.token,
            label: issued.label,
            expires_at: issued.expires_at,
        }
    }
}

/// What `POST /agents` answers with: the agent as stored and its first token.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct CreatedAgentResponse {
    /// The agent as stored.
    #[schema(value_type = super::schemas::Agent)]
    agent: Agent,
    /// Its first token, this once.
    token: IssuedAgentTokenResponse,
}

/// The body of `POST /agents`.
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentBody {
    /// The unique name, folded like a login name. It may not be taken by an
    /// account or another agent.
    #[schema(example = "nightly")]
    name: String,
    /// The name shown to people. Defaults to the name.
    #[schema(example = "Nightly runner")]
    display: Option<String>,
    /// The loops it runs. Defaults to both.
    #[schema(value_type = Option<super::schemas::AgentLoops>)]
    loops: Option<AgentLoops>,
    /// The first token's life in days, 1 to 365. Defaults to 90.
    #[schema(example = 90)]
    expires_days: Option<u32>,
    /// The first token's label. Defaults to the day it was issued.
    label: Option<String>,
}

/// The body of `POST /agents/{name}/tokens`.
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueAgentTokenBody {
    /// The token's life in days, 1 to 365. Defaults to 90.
    #[schema(example = 90)]
    expires_days: Option<u32>,
    /// Its label. Defaults to the day it was issued.
    label: Option<String>,
}

/// A life in days as seconds, the default when none was given.
fn ttl_secs(days: Option<u32>) -> i64 {
    days.map_or(AGENT_TOKEN_DEFAULT_TTL_SECS, |d| i64::from(d) * 86_400)
}

/// A store failure as a status, classified by [`RefusalKind`].
fn store_error(e: anyhow::Error) -> ApiError {
    let words = || {
        e.chain()
            .find_map(|cause| cause.downcast_ref::<StoreRefusal>())
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("{e:#}"))
    };
    match StoreRefusal::kind_of(&e) {
        Some(RefusalKind::NameTaken | RefusalKind::AgentTokenLimit) => ApiError::conflict(words()),
        Some(RefusalKind::NoSuchAgent) => ApiError::not_found(words()),
        Some(
            RefusalKind::InvalidName
            | RefusalKind::InvalidDisplayName
            | RefusalKind::InvalidTokenExpiry,
        ) => ApiError::unprocessable(words()),
        _ => ApiError::internal(format!("{e:#}")),
    }
}

/// `GET /agents` - every agent with its tokens, by name. Never carries a
/// token: only hashes are stored.
#[utoipa::path(
    get,
    path = "/api/v1/agents",
    tag = "agents",
    operation_id = "list_agents",
    responses(
        (status = 200, description = "Every agent, by name.", body = AgentsResponse),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn list(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<AgentsResponse>, ApiError> {
    identity.require_admin()?;
    let agents = state.auth.agents().await.map_err(store_error)?;
    Ok(Json(AgentsResponse {
        agents,
        auth_mcp: state.engine.auth_mcp(),
    }))
}

/// `POST /agents` - create an agent and issue its first token, answering 201
/// with both. The token is shown this once.
#[utoipa::path(
    post,
    path = "/api/v1/agents",
    tag = "agents",
    operation_id = "create_agent",
    request_body = CreateAgentBody,
    responses(
        (status = 201, description = "The agent and its first token, this once.", body = CreatedAgentResponse),
        (
            status = 400,
            description = "The body is not JSON.",
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
            description = "The caller is not an admin, a cookie session did \
                           not echo its CSRF token, this instance is \
                           read-only, `auth.mcp` is off (an agent token \
                           would open nothing), or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "An account or another agent holds the name.",
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
            description = "The body is JSON but not an agent, the name or \
                           display name is not allowed, or the life is not \
                           1 to 365 days.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn create(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<CreateAgentBody>,
) -> Result<(StatusCode, NoStore, Json<CreatedAgentResponse>), ApiError> {
    let caller = identity.require_admin()?;
    refuse_read_only(&state)?;
    // An agent token opens nothing while `auth.mcp` is off, so no agent is
    // made then: the admin is told why, and how to turn it on.
    if !state.engine.auth_mcp() {
        return Err(ApiError::forbidden(crate::AGENTS_NEED_AUTH_MCP));
    }
    // An agent drafts under its name on a domain that reviews changes, so a
    // name still holding a removed person's drafts is refused, as for an account.
    crate::users_api::clear_name_for_new_account(&state, &body.name, true).await?;
    let (agent, issued) = state
        .auth
        .add_agent(
            &body.name,
            body.display.as_deref(),
            body.loops.unwrap_or_default(),
            caller.name(),
            body.label.as_deref().unwrap_or_default(),
            ttl_secs(body.expires_days),
        )
        .await
        .map_err(store_error)?;
    tracing::info!(
        admin = caller.name(),
        agent = %agent.name,
        loops = %agent.loops,
        token = issued.id,
        "agent created"
    );
    Ok((
        StatusCode::CREATED,
        no_store(),
        Json(CreatedAgentResponse {
            agent,
            token: issued.into(),
        }),
    ))
}

/// `DELETE /agents/{name}` - remove an agent, every token it holds and every
/// draft it left, answering 204.
#[utoipa::path(
    delete,
    path = "/api/v1/agents/{name}",
    tag = "agents",
    operation_id = "delete_agent",
    params(("name" = String, Path, description = "The agent, in any casing.")),
    responses(
        (status = 204, description = "The agent, its tokens, its drafts and the links on them are gone."),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, a cookie session did \
                           not echo its CSRF token, this instance is \
                           read-only, or the trusted-header identity names a \
                           disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such agent.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn remove(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(name): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    let caller = identity.require_admin()?;
    refuse_read_only(&state)?;
    state.auth.remove_agent(&name).await.map_err(store_error)?;
    let agent = normalize_account_name(&name).unwrap_or_default();
    // An agent's drafts go with it, unlike a person's: nobody comes back for
    // them, and its name would stay refused for a new account or agent. The
    // same end `DELETE /users/{name}/drafts` gives a removed account's drafts,
    // and the links it gave on them end too.
    state.auth.end_overlay_grants_of_owner(&agent).await?;
    let report = state
        .engine
        .discard_account_drafts(&agent, caller.name())
        .await?;
    if report.get("error").is_some()
        || report["domains"].as_array().is_some_and(|rows| {
            rows.iter().any(|r| {
                r.get("error").is_some() || r["left"].as_array().is_some_and(|l| !l.is_empty())
            })
        })
    {
        tracing::warn!(agent = %agent, %report, "not every draft of the removed agent could be ended");
    }
    tracing::info!(admin = caller.name(), agent = %agent, "agent removed");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /agents/{name}/tokens` - issue another token, answering 201 with it,
/// this once. Refused while the agent holds two live tokens.
#[utoipa::path(
    post,
    path = "/api/v1/agents/{name}/tokens",
    tag = "agents",
    operation_id = "issue_agent_token",
    params(("name" = String, Path, description = "The agent, in any casing.")),
    request_body = IssueAgentTokenBody,
    responses(
        (status = 201, description = "The token, this once.", body = IssuedAgentTokenResponse),
        (
            status = 400,
            description = "The body is not JSON.",
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
            description = "The caller is not an admin, a cookie session did \
                           not echo its CSRF token, this instance is \
                           read-only, or the trusted-header identity names a \
                           disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such agent.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "The agent already holds two live tokens.",
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
            description = "The life is not 1 to 365 days.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn issue_token(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(name): ApiPath<String>,
    ApiJson(body): ApiJson<IssueAgentTokenBody>,
) -> Result<(StatusCode, NoStore, Json<IssuedAgentTokenResponse>), ApiError> {
    let caller = identity.require_admin()?;
    refuse_read_only(&state)?;
    let issued = state
        .auth
        .issue_agent_token(
            &name,
            body.label.as_deref().unwrap_or_default(),
            ttl_secs(body.expires_days),
        )
        .await
        .map_err(store_error)?;
    tracing::info!(
        admin = caller.name(),
        agent = %normalize_account_name(&name).unwrap_or_default(),
        token = issued.id,
        "agent token issued"
    );
    Ok((StatusCode::CREATED, no_store(), Json(issued.into())))
}

/// `DELETE /agents/{name}/tokens/{id}` - revoke one token, answering 204. The
/// token stays listed as revoked.
#[utoipa::path(
    delete,
    path = "/api/v1/agents/{name}/tokens/{id}",
    tag = "agents",
    operation_id = "revoke_agent_token",
    params(
        ("name" = String, Path, description = "The agent, in any casing."),
        ("id" = i64, Path, description = "The token's id."),
    ),
    responses(
        (status = 204, description = "The token is revoked."),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, a cookie session did \
                           not echo its CSRF token, this instance is \
                           read-only, or the trusted-header identity names a \
                           disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No live token of that id for this agent.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn revoke_token(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((name, id)): ApiPath<(String, i64)>,
) -> Result<StatusCode, ApiError> {
    let caller = identity.require_admin()?;
    refuse_read_only(&state)?;
    if !state
        .auth
        .revoke_agent_token(&name, id)
        .await
        .map_err(store_error)?
    {
        return Err(ApiError::not_found(format!(
            "no unrevoked token {id} for agent '{}'",
            normalize_account_name(&name).unwrap_or_default()
        )));
    }
    tracing::info!(
        admin = caller.name(),
        agent = %normalize_account_name(&name).unwrap_or_default(),
        token = id,
        "agent token revoked"
    );
    Ok(StatusCode::NO_CONTENT)
}
