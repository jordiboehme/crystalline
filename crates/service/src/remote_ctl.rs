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

use axum::body::Bytes;
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

/// The commands a connected Crystalline may send. Pinned by
/// `tests/remote/answers.rs::the_remote_allow_list_is_exactly_six_commands`
/// (Task 7).
pub const REMOTE_COMMANDS: &[&str] = &["status"];

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

/// The route, with the JSON API's body ceiling.
pub fn route(state: CtlState) -> axum::Router {
    axum::Router::new()
        .route(CTL_PATH, axum::routing::post(handle))
        .layer(axum::extract::DefaultBodyLimit::max(
            crate::rest::MAX_BODY_BYTES,
        ))
        .with_state(state)
}

async fn handle(State(state): State<CtlState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(refused) = state.origin_rule.origin(&headers) {
        return refused.into_response();
    }
    let identity = match resolve_bearer(&state.auth, state.oauth.as_ref(), &headers).await {
        Bearer::Account(identity) => identity,
        Bearer::Refused(response) | Bearer::Unavailable(response) => return response,
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
    // Not read yet: the remote `tool` call builds its links on it.
    let _ = base;
    let answer = match cmd {
        "status" => remote_status(engine, identity, &scope).await,
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
