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
//!   `crate::mcp_gate::resolve_bearer`, the one the MCP gate runs.
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

use crate::control::{envelope_err, envelope_err_kind, envelope_ok};
use crate::engine::{Engine, ShareActor};
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
pub const REMOTE_COMMANDS: &[&str] = &[
    "status",
    "tool",
    "routing_bullets",
    "hook_status",
    "origin_status",
    "origin_changes",
    crystalline_remote::FILE_COMMAND,
    "automation_status",
    "automation_lease_take",
    "automation_lease_renew",
    "automation_lease_release",
    "automation_run_record",
    "automation_share",
    "automation_runs",
];

/// The commands whose answer carries an etag, because a client caches it.
pub const ETAG_COMMANDS: &[&str] = &["routing_bullets", "hook_status"];

/// The etag of an answer: the sha256 of its JSON. `serde_json` sorts object
/// keys in this workspace (no `preserve_order`), so the same answer always
/// hashes the same.
pub fn etag_of(data: &Value) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(data).unwrap_or_default();
    crystalline_index::hex_lower(&Sha256::digest(&bytes))
}

/// `data` as an envelope with its etag, or the short `not_modified` answer
/// when the request already holds that etag.
fn conditional(request: &Value, data: Value) -> Value {
    let etag = etag_of(&data);
    if request.get("if_none_match").and_then(Value::as_str) == Some(etag.as_str()) {
        return json!({
            "v": crate::control::CTL_VERSION,
            "ok": true,
            "not_modified": true,
            "etag": etag,
        });
    }
    let mut envelope = envelope_ok(data);
    envelope["etag"] = json!(etag);
    envelope
}

/// The engine verbs and Reflection tools a remote `tool` call may run: the
/// fifteen engine verbs the control socket's `tool` command dispatches, and
/// the four Reflection tools (0.26.0), each with its arm in
/// `McpServer::remote_tool`, which runs what the MCP handler of that name
/// runs. Pinned exactly by
/// `tests/remote/ctl.rs::the_remote_allow_lists_are_pinned`. A connected
/// machine forwards exactly these (`route::FORWARDED_TOOLS` is this constant).
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
    "reflect",
    "read_reflection",
    "write_reflection",
    "discuss",
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

/// The route. The tool body ceiling ([`crate::rest::MAX_TOOL_BODY_BYTES`], room
/// for a 10 MiB file as base64) is applied by `handle` itself, after the bearer check, rather than by a
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
    let body = match axum::body::to_bytes(body, crate::rest::MAX_TOOL_BODY_BYTES).await {
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
        "routing_bullets" => routing_model(engine, identity, &scope).await,
        "hook_status" => hook_status(engine, identity, &scope).await,
        "origin_status" => origin_status(engine, request, &scope).await,
        "origin_changes" => origin_changes(engine, request, identity, &scope).await,
        // The two commands that run a verb: an argument error among their
        // failures is told apart by its kind.
        "tool" => {
            return verb_envelope(remote_tool(engine, request, identity, &scope, base).await);
        }
        crystalline_remote::FILE_COMMAND => {
            return verb_envelope(remote_file(engine, request, identity, &scope, base).await);
        }
        // A runner's verbs: the token's account holds the lease and is the
        // one whose MCP writes the share carries.
        automation if crate::automation_ctl::AUTOMATION_COMMANDS.contains(&automation) => {
            let mut request = request.clone();
            if let Some(typed) = request.get("domain").and_then(Value::as_str) {
                match localized(engine, typed, &scope).await {
                    Ok(local) => request["domain"] = Value::String(local),
                    Err(message) => return envelope_err(message),
                }
            }
            return crate::automation_ctl::answer(engine, automation, &request, &scope).await;
        }
        other => Err(format!("'{other}' {NOT_REMOTE}")),
    };
    match answer {
        Ok(data) if ETAG_COMMANDS.contains(&cmd) => conditional(request, data),
        Ok(data) => envelope_ok(data),
        Err(message) => envelope_err(message),
    }
}

/// The envelope of a verb's answer: an argument error carries its kind.
fn verb_envelope(verb: Result<Value, crate::mcp::RemoteToolError>) -> Value {
    match verb {
        Ok(data) => envelope_ok(data),
        Err(failure) if failure.is_invalid() => {
            envelope_err_kind(failure.into_message(), crystalline_remote::INVALID_KIND)
        }
        Err(failure) => envelope_err(failure.into_message()),
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
        // What this server can do that an older one cannot, for a client that
        // must not send such a call to a server without it.
        "capabilities": [crystalline_remote::AGENT_FILES_CAPABILITY],
        "read_only": engine.read_only(),
        "account": identity.name,
        "admin": identity.admin,
        "domains": domains,
    }))
}

/// The routing model for a connected machine: the whole block's content for
/// this account, because the machine has no MANIFEST of these domains on
/// disk, and each domain's origin identity, so it recognizes a domain it
/// already holds (decision D3). Each domain carries `domain_read_only`: whether this token cannot write there (the
/// domain lock, a viewer role, or a write list), taken from
/// `PromptDomain.read_only`, which `routing_model_scoped` already computed for
/// this token with the same `write_block` the write verbs ask, so no second
/// rights question is asked per row; the top-level `read_only` is the
/// instance's. Each row of a Reflection this account curates carries its open
/// reflect items; a row with nothing open carries no `reflect_open`, so the
/// answer, and its etag, is 0.25.0's until something is open.
async fn routing_model(
    engine: &Arc<Engine>,
    identity: &McpIdentity,
    scope: &Scope,
) -> Result<Value, String> {
    let output = engine
        .routing_model_scoped(scope)
        .await
        .map_err(|e| e.to_string())?;
    let curated = curated_for(engine, identity, scope).await;
    let open = crate::maintenance::ReflectStatus::from_state(&crate::maintenance::load(), &curated)
        .domains;
    let domains: Vec<Value> = output
        .domains
        .iter()
        .map(|d| {
            let mut row = json!({
                "name": d.name,
                "bullets": d.bullets,
                "origin": engine.origin_identity_of(&d.name),
                "domain_read_only": d.read_only,
            });
            if let Some(n) = open.get(&d.name) {
                row["reflect_open"] = json!(n);
            }
            row
        })
        .collect();
    Ok(json!({ "read_only": output.read_only, "domains": domains }))
}

/// The names this account may see, which is also the set of names still
/// registered: a pending domain that was removed since is not named.
async fn visible_names(
    engine: &Arc<Engine>,
    scope: &Scope,
) -> Result<std::collections::HashSet<String>, String> {
    let listing = engine
        .list_domains(
            &ListDomainsParams {
                include_routing: false,
            },
            scope,
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(listing["domains"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// The Reflections this account curates here, for the reflect counts.
/// Empty on any failure: a hook answer never fails on a Reflection.
async fn curated_for(
    engine: &Arc<Engine>,
    identity: &McpIdentity,
    scope: &Scope,
) -> std::collections::BTreeSet<String> {
    let caller = crate::ReflectionCaller::person(scope.clone(), &identity.name);
    match engine.curated_reflections(&caller).await {
        Ok(curated) => curated,
        Err(e) => {
            tracing::debug!("reflect counts left out, the curated Reflections are unreadable: {e}");
            std::collections::BTreeSet::new()
        }
    }
}

/// What a connected machine's Stop hook reads: this server's maintenance
/// record, narrowed to what the account may see (evolve) and to the
/// Reflections it curates (reflect). One read of the record, so the answer,
/// and with it the etag, changes exactly when the record does. A read-only
/// domain is left out of the backlog, as the local arms leave it out: nobody
/// can sweep it.
async fn hook_status(
    engine: &Arc<Engine>,
    identity: &McpIdentity,
    scope: &Scope,
) -> Result<Value, String> {
    let mut visible = visible_names(engine, scope).await?;
    let locked = engine.read_only_domain_names().await;
    visible.retain(|name| !locked.contains(name));
    let curated = curated_for(engine, identity, scope).await;
    let state = crate::maintenance::load();
    let mut status = crate::maintenance::HookStatus::from_state(&state, &visible);
    status.reflect = crate::maintenance::ReflectStatus::from_state(&state, &curated);
    serde_json::to_value(status).map_err(|e| e.to_string())
}

/// A domain the request names, spelled as this server's local name for this
/// account. A domain it may not see stays as typed, so it is refused in the
/// caller's own words.
async fn localized(engine: &Arc<Engine>, typed: &str, scope: &Scope) -> Result<String, String> {
    let hidden = engine.hidden_for(scope).await.map_err(|e| e.to_string())?;
    Ok(engine.localize_visible(typed, &hidden).await)
}

async fn origin_status(
    engine: &Arc<Engine>,
    request: &Value,
    scope: &Scope,
) -> Result<Value, String> {
    let domain = match request.get("domain").and_then(Value::as_str) {
        Some(typed) => Some(localized(engine, typed, scope).await?),
        None => None,
    };
    let detail = request
        .get("detail")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let diff = request
        .get("diff")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    engine
        .origin_status(domain.as_deref(), detail, diff, scope)
        .await
        .map_err(|e| e.to_string())
}

async fn origin_changes(
    engine: &Arc<Engine>,
    request: &Value,
    identity: &McpIdentity,
    scope: &Scope,
) -> Result<Value, String> {
    let typed = request.get("domain").and_then(Value::as_str).unwrap_or("");
    let domain = localized(engine, typed, scope).await?;
    engine
        .require_domain(&domain, scope)
        .await
        .map_err(|e| e.to_string())?;
    let path = request.get("path").and_then(Value::as_str);
    let sides = request
        .get("sides")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    crate::control::origin_changes_inline(
        engine,
        &domain,
        path,
        sides,
        &ShareActor::Account(identity.name.clone()),
    )
    .await
    .map_err(|e| e.to_string())
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

/// A remote `file` read (0.27.0): one file of a domain, `{domain, path}`
/// with `path` domain-relative, answered as `{mime, size, base64}` under the
/// account's own right for the kind of the file (engram rights for
/// `assets/...`, the Reflection read right for `reflections/assets/...`).
/// The bytes are the file's, never past `MAX_ATTACHMENT_BYTES`; the client
/// reads the answer with that ceiling (`crystalline_remote::FILE_ANSWER_CAP`).
async fn remote_file(
    engine: &Arc<Engine>,
    request: &Value,
    identity: &McpIdentity,
    scope: &Scope,
    base: &WebBase,
) -> Result<Value, crate::mcp::RemoteToolError> {
    let domain = request.get("domain").and_then(Value::as_str).unwrap_or("");
    let path = request.get("path").and_then(Value::as_str).unwrap_or("");
    crate::mcp::McpServer::new_http(engine.clone())
        .remote_file(domain, path, &remote_caller(request, identity, scope, base))
        .await
}

/// The caller of a remote call: the token's account, never anything the
/// request says, as the agent the connected machine forwarded. The holder is
/// the one a sessionless HTTP peer gets (decision D10): keyed by the
/// account, ended by `IDLE_JOIN_LIMIT` after the last use.
fn remote_caller(
    request: &Value,
    identity: &McpIdentity,
    scope: &Scope,
    base: &WebBase,
) -> crate::mcp::Caller {
    crate::mcp::Caller {
        scope: scope.clone(),
        account: Some(identity.name.clone()),
        client: Some(forwarded_client(request)),
        holder: Some(crate::join::Holder::Token(identity.name.clone())),
        base: base.clone(),
    }
}

/// A remote `tool` call, through `McpServer::remote_tool` on a server object
/// built for this one request, as the forwarded agent for the account
/// ([`remote_caller`]).
async fn remote_tool(
    engine: &Arc<Engine>,
    request: &Value,
    identity: &McpIdentity,
    scope: &Scope,
    base: &WebBase,
) -> Result<Value, crate::mcp::RemoteToolError> {
    let tool = request.get("tool").and_then(Value::as_str).unwrap_or("");
    if !REMOTE_TOOLS.contains(&tool) {
        return Err(crate::mcp::RemoteToolError::Refused(format!(
            "the tool '{tool}' {NOT_REMOTE}"
        )));
    }
    let args = request.get("args").cloned().unwrap_or_else(|| json!({}));
    crate::mcp::McpServer::new_http(engine.clone())
        .remote_tool(tool, args, &remote_caller(request, identity, scope, base))
        .await
}
