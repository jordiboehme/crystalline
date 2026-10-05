//! `POST /api/v1/ctl`: the control protocol for a local Crystalline that
//! `crystalline connect` signed in to this server.
//!
//! The body is the envelope the socket speaks (`{ "v": 1, "cmd": ... }`) and
//! so is the answer (`{ "v", "ok", "data" | "error" }`). Everything else about
//! it is different from the socket, on purpose:
//!
//! - **It always needs a token**, a personal MCP token or, where `auth.oauth`
//!   is on, an OAuth access token minted for this origin, whatever
//!   `auth.mcp` says. There is no anonymous ctl: the open tier is a promise
//!   about MCP, and nothing here is MCP. The check is
//!   [`crate::mcp_gate::resolve_bearer`], the one the MCP gate runs.
//! - **It runs as the token's account** ([`Scope::User`]), never as the
//!   machine owner. The control socket's handler ([`crate::control`]) is
//!   `Scope::Unrestricted` and `ShareActor::Owner` in every arm, which is
//!   right for a process on this machine and wrong for anybody else, so this
//!   route has a dispatcher of its own and never calls that one.
//! - **It serves an allow-list.** [`REMOTE_COMMANDS`] is the whole of it; any
//!   other command, including every command added to the socket later, is
//!   refused with [`NOT_REMOTE`] until somebody adds it here on purpose.
//!
//! It lives in the service crate beside the MCP mount rather than in
//! `crates/rest`, because a remote `tool` call runs through the MCP server's
//! own gate path (`McpServer::remote_tool`), and the rest crate never reaches
//! the MCP router.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::control::{envelope_err, envelope_ok};
use crate::engine::Engine;
use crate::mcp_gate::{Bearer, McpIdentity, resolve_bearer};
use crate::params::ListDomainsParams;
use crate::rest::{AuthStore, OriginRule};
use crate::scope::Scope;
use crate::web_url::WebBase;

/// Where the route is mounted; the client in `crystalline_remote` posts to
/// the same constant.
pub use crystalline_remote::CTL_PATH;

/// The commands a connected Crystalline may send. Pinned exactly by
/// `tests/remote/ctl.rs::the_remote_allow_lists_are_pinned`.
pub const REMOTE_COMMANDS: &[&str] = &["status", "tool"];

/// The engine verbs a remote `tool` call may run: exactly the ones the control
/// socket's `tool` command dispatches. Pinned exactly by
/// `tests/remote/ctl.rs::the_remote_allow_lists_are_pinned`. Each has its arm
/// in `McpServer::remote_tool`, which runs the same verb core the MCP handler
/// of that name runs.
pub const REMOTE_TOOLS: &[&str] = &[
    "write_engram",
    "read_engram",
    "edit_engram",
    "move_engram",
    "split_engram",
    "delete_engram",
    "search_engrams",
    "build_context",
    "recent_activity",
    "list_domains",
    "browse_domain",
    "validate_engrams",
    "infer_schema",
    "vocabulary",
    crate::EVOLVE_TOOL_NAME,
];

/// The client half a remote write records when the connected machine named
/// no agent: a CLI verb or a hook.
pub const REMOTE_CLIENT: &str = "crystalline-cli";

/// The end of every refusal of a command off the allow-list.
pub const NOT_REMOTE: &str = "is not available over a remote connection";

/// What the route needs, resolved once when the HTTP surface starts.
#[derive(Clone)]
pub struct CtlState {
    /// The shared engine.
    pub engine: Arc<Engine>,
    /// The accounts store. Always present: the route needs a token even where
    /// the MCP gate is off.
    pub auth: Arc<AuthStore>,
    /// This instance's origin rule when `auth.oauth` is on, which is what lets
    /// an OAuth access token through.
    pub oauth: Option<OriginRule>,
    /// The same rule, always: a `Host` the transport would refuse is refused
    /// here too.
    pub origin_rule: OriginRule,
}

/// The route. The JSON API's body ceiling ([`crate::rest::MAX_BODY_BYTES`])
/// is applied by [`handle`] itself, after the bearer check, rather than by a
/// body-limit layer in front of an extractor that would read the body first.
pub fn route(state: CtlState) -> axum::Router {
    axum::Router::new()
        .route(CTL_PATH, axum::routing::post(handle))
        .with_state(state)
}

/// The body is read only once the origin and the bearer passed, so a caller
/// with no token cannot make the server buffer a request body at all.
async fn handle(State(state): State<CtlState>, headers: HeaderMap, body: Body) -> Response {
    if let Err(refused) = state.origin_rule.origin(&headers) {
        return refused.into_response();
    }
    let identity = match resolve_bearer(&state.auth, state.oauth.as_ref(), &headers).await {
        Bearer::Account(identity) => identity,
        Bearer::Refused(response) | Bearer::Unavailable(response) => return response,
    };
    let body = match axum::body::to_bytes(body, crate::rest::MAX_BODY_BYTES).await {
        Ok(body) => body,
        Err(e) => return axum::Json(envelope_err(format!("invalid body: {e}"))).into_response(),
    };
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(e) => return axum::Json(envelope_err(format!("invalid json: {e}"))).into_response(),
    };
    let base = state.engine.request_web_base(&headers);
    axum::Json(dispatch(&state.engine, &request, &identity, &base).await).into_response()
}

/// Answer one remote request as `identity`.
pub(crate) async fn dispatch(
    engine: &Arc<Engine>,
    request: &Value,
    identity: &McpIdentity,
    base: &WebBase,
) -> Value {
    let cmd = request.get("cmd").and_then(Value::as_str).unwrap_or("");
    if !REMOTE_COMMANDS.contains(&cmd) {
        return envelope_err(format!("'{cmd}' {NOT_REMOTE}"));
    }
    let scope = Scope::User {
        account: identity.name.clone(),
        admin: identity.admin,
    };
    let answer = match cmd {
        "status" => remote_status(engine, identity, &scope).await,
        "tool" => remote_tool(engine, request, identity, &scope, base).await,
        other => Err(format!("'{other}' {NOT_REMOTE}")),
    };
    match answer {
        Ok(data) => envelope_ok(data),
        Err(message) => envelope_err(message),
    }
}

/// What a connected Crystalline's `status` is told: this server's version and
/// mode, who the token is, and the domains that account may see with their
/// sizes. Nothing about the process or the machine it runs on.
async fn remote_status(
    engine: &Arc<Engine>,
    identity: &McpIdentity,
    scope: &Scope,
) -> Result<Value, String> {
    let listing = engine
        .list_domains(
            &ListDomainsParams {
                include_routing: false,
            },
            scope,
        )
        .await
        .map_err(|e| e.to_string())?;
    let domains: Vec<Value> = listing["domains"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| json!({ "name": row["name"], "engrams": row["engrams"] }))
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "version": crystalline_core::VERSION,
        "read_only": engine.read_only(),
        "account": identity.name,
        "admin": identity.admin,
        "domains": domains,
    }))
}

/// The client half the connected machine forwarded (decision D11), at most
/// 200 characters of it (the actor and the chip each budget their own share),
/// or [`REMOTE_CLIENT`] when it named none.
fn forwarded_client(request: &Value) -> String {
    request
        .get("agent")
        .and_then(|agent| agent.get("client"))
        .and_then(Value::as_str)
        .map(|client| client.chars().take(200).collect::<String>())
        .filter(|client| !client.trim().is_empty())
        .unwrap_or_else(|| REMOTE_CLIENT.to_string())
}

/// A remote `tool` call, through `McpServer::remote_tool` on a server object
/// built for this one request, as the forwarded agent for the account. The
/// account always comes from the token, never from the request. The holder is
/// the one a sessionless HTTP peer gets (decision D10): keyed by the account,
/// ended by `IDLE_JOIN_LIMIT` after the last use.
async fn remote_tool(
    engine: &Arc<Engine>,
    request: &Value,
    identity: &McpIdentity,
    scope: &Scope,
    base: &WebBase,
) -> Result<Value, String> {
    let tool = request.get("tool").and_then(Value::as_str).unwrap_or("");
    if !REMOTE_TOOLS.contains(&tool) {
        return Err(format!("the tool '{tool}' {NOT_REMOTE}"));
    }
    let args = request.get("args").cloned().unwrap_or_else(|| json!({}));
    let caller = crate::mcp::Caller {
        scope: scope.clone(),
        account: Some(identity.name.clone()),
        client: Some(forwarded_client(request)),
        holder: Some(crate::join::Holder::Token(identity.name.clone())),
        base: base.clone(),
    };
    crate::mcp::McpServer::new_http(engine.clone())
        .remote_tool(tool, args, &caller)
        .await
        .map_err(crate::mcp::RemoteToolError::into_message)
}
