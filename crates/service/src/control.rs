//! The ctl control protocol: newline-delimited JSON over the socket.
//!
//! Each request is one JSON line `{ "v": 1, "cmd": ..., ... }`; each response is
//! one line `{ "v": 1, "ok": true, "data": ... }` or
//! `{ "v": 1, "ok": false, "error": ... }`. Commands: sync, status, reindex,
//! sessions, tool, configure, origin_add, origin_update, origin_status,
//! origin_share, origin_withdraw, origin_resolve, provision, forget_domain,
//! shutdown. This is the operator channel plus the `tool` command, which
//! dispatches a daemon-attached CLI data verb to the shared engine and
//! returns raw engine JSON; an MCP client's data operations still go over the
//! MCP handshake.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use interprocess::local_socket::tokio::Stream as IpcStream;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::daemon::Shared;
use crate::engine::ConfigureAction;

/// The protocol version carried on every ctl envelope.
pub const CTL_VERSION: u64 = 1;

/// Serve the ctl protocol on a connection until the peer disconnects or a
/// `shutdown` command is handled.
pub async fn serve_ctl(stream: IpcStream, shared: Arc<Shared>) {
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (response, shutdown) = match serde_json::from_str::<Value>(trimmed) {
            Ok(req) => handle(&req, &shared).await,
            Err(e) => (envelope_err(format!("invalid json: {e}")), false),
        };
        let mut out = serde_json::to_string(&response).unwrap_or_default();
        out.push('\n');
        if write.write_all(out.as_bytes()).await.is_err() {
            break;
        }
        let _ = write.flush().await;
        if shutdown {
            shared.trigger_shutdown();
            break;
        }
    }
}

/// Handle one ctl request, returning the response envelope and whether the
/// daemon should shut down after replying.
async fn handle(req: &Value, shared: &Arc<Shared>) -> (Value, bool) {
    let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or("");
    match cmd {
        "status" => {
            let mut data = json!({
                "pid": shared.pid,
                "version": crystalline_core::VERSION,
                "uptime_secs": shared.uptime_secs(),
                "sessions": shared.session_count(),
                "http": shared.http_addr.clone(),
                "http_sessions": shared.http_session_count(),
                "read_only": shared.engine.read_only(),
            });
            match shared.engine.status_report().await {
                Ok(report) => {
                    if let (Value::Object(a), Value::Object(b)) = (&mut data, report) {
                        a.extend(b);
                    }
                    (envelope_ok(data), false)
                }
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        "sessions" => (
            envelope_ok(json!({
                "count": shared.session_count(),
                "sessions": shared.sessions_json(),
            })),
            false,
        ),
        // Dispatch a data verb to the shared engine and return its JSON, so a
        // daemon-attached CLI data command receives engine JSON directly rather
        // than the MCP stream's format-dependent tool envelope (TOON by
        // default). The engine methods self-guard read-only mutations, so this
        // keeps the same refusals the MCP path gave.
        "tool" => {
            let tool = req.get("tool").and_then(Value::as_str).unwrap_or("");
            let args = req.get("args").cloned().unwrap_or_else(|| json!({}));
            match crate::client::dispatch_engine(&shared.engine, tool, args).await {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        "sync" => {
            let domain = req.get("domain").and_then(Value::as_str);
            let embed = req.get("embed").and_then(Value::as_bool).unwrap_or(false);
            let take_over = req
                .get("take_over")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match shared.engine.sync_take_over(domain, take_over).await {
                Ok(mut data) => {
                    maybe_embed(shared, embed, &mut data).await;
                    (envelope_ok(data), false)
                }
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        "reindex" => {
            let full = req.get("full").and_then(Value::as_bool).unwrap_or(false);
            let embed = req.get("embed").and_then(Value::as_bool).unwrap_or(false);
            match shared.engine.reindex(full).await {
                Ok(mut data) => {
                    maybe_embed(shared, embed, &mut data).await;
                    (envelope_ok(data), false)
                }
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Virtual-domain routing bullets for `prompt system`, served from the
        // daemon's warm state so the prompt stays inside its latency budget.
        "routing_bullets" => (
            envelope_ok(
                serde_json::to_value(shared.engine.virtual_routing_bullets().await)
                    .unwrap_or(Value::Null),
            ),
            false,
        ),
        // Scaffold a virtual domain's MANIFEST from markdown the CLI built.
        "scaffold_manifest" => {
            let domain = req.get("domain").and_then(Value::as_str).unwrap_or("");
            let markdown = req.get("markdown").and_then(Value::as_str).unwrap_or("");
            match shared
                .engine
                .scaffold_virtual_manifest(domain, markdown)
                .await
            {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Load engram files into a virtual domain verbatim.
        "domain_import" => {
            let domain = req.get("domain").and_then(Value::as_str).unwrap_or("");
            let path = req.get("path").and_then(Value::as_str).unwrap_or("");
            let overwrite = req
                .get("overwrite")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let dry_run = req.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
            match shared
                .engine
                .import_domain(domain, std::path::Path::new(path), overwrite, dry_run)
                .await
            {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Rename or merge a tag across the engrams that carry it.
        "retag" => {
            let old = req.get("old").and_then(Value::as_str).unwrap_or("");
            let new = req.get("new").and_then(Value::as_str).unwrap_or("");
            let domain = req.get("domain").and_then(Value::as_str);
            let merge = req.get("merge").and_then(Value::as_bool).unwrap_or(false);
            let dry_run = req.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
            // The merge records the fold as a tag alias unless the caller opted
            // out; an absent field defaults to recording, so existing callers
            // that never send it keep the recording behavior.
            let record_alias = !req
                .get("no_alias")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match shared
                .engine
                .retag(old, new, domain, merge, dry_run, record_alias)
                .await
            {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Export a domain's engrams to a filesystem folder.
        "domain_export" => {
            let domain = req.get("domain").and_then(Value::as_str).unwrap_or("");
            let path = req.get("path").and_then(Value::as_str).unwrap_or("");
            let force = req.get("force").and_then(Value::as_bool).unwrap_or(false);
            let dry_run = req.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
            match shared
                .engine
                .export_domain(domain, std::path::Path::new(path), force, dry_run)
                .await
            {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Show, set or reset a setting from the settings registry. `set` and
        // `unset` refuse on a read-only daemon (the engine method itself
        // checks); `show` is always allowed.
        "configure" => {
            let action = req.get("action").and_then(Value::as_str).unwrap_or("show");
            let outcome = match action {
                "show" => shared.engine.configure(&ConfigureAction::Show).await,
                "set" => {
                    let key = req.get("key").and_then(Value::as_str).unwrap_or("");
                    let value = req.get("value").and_then(Value::as_str).unwrap_or("");
                    shared
                        .engine
                        .configure(&ConfigureAction::Set {
                            key: key.to_string(),
                            value: value.to_string(),
                        })
                        .await
                }
                "unset" => {
                    let key = req.get("key").and_then(Value::as_str).unwrap_or("");
                    shared
                        .engine
                        .configure(&ConfigureAction::Unset {
                            key: key.to_string(),
                        })
                        .await
                }
                other => Err(crate::engine::EngineError::Invalid(format!(
                    "unknown configure action '{other}'; expected show, set or unset"
                ))),
            };
            match outcome {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Connect a new domain to a GitHub repository: downloads its tracked
        // subtree, registers it in the global config and indexes it.
        "origin_add" => {
            let repo = req.get("repo").and_then(Value::as_str).unwrap_or("");
            let domain = req.get("domain").and_then(Value::as_str);
            let path = req.get("path").and_then(Value::as_str);
            let branch = req.get("branch").and_then(Value::as_str);
            let folder = req.get("folder").and_then(Value::as_str);
            match shared
                .engine
                .origin_add(repo, domain, path, branch, folder)
                .await
            {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Pull one origin-connected domain (or every one) up to date.
        "origin_update" => {
            let domain = req.get("domain").and_then(Value::as_str);
            match shared.engine.origin_update(domain).await {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Report where one origin-connected domain (or every one) stands
        // relative to its origin, plus this machine's GitHub connection.
        "origin_status" => {
            let domain = req.get("domain").and_then(Value::as_str);
            match shared.engine.origin_status(domain).await {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Propose one domain's local changes as a pull request against its
        // origin.
        "origin_share" => {
            let domain = req.get("domain").and_then(Value::as_str).unwrap_or("");
            let title = req.get("title").and_then(Value::as_str);
            let description = req.get("description").and_then(Value::as_str);
            let proposal = match optional_number(req, "proposal") {
                Ok(proposal) => proposal,
                Err(message) => return (envelope_err(message), false),
            };
            match shared
                .engine
                .origin_share(domain, title, description, proposal)
                .await
            {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Withdraw a share proposal for one domain: close it on the forge,
        // optionally restore the shared files, record it withdrawn.
        "origin_withdraw" => {
            let domain = req.get("domain").and_then(Value::as_str).unwrap_or("");
            let proposal = match optional_number(req, "proposal") {
                Ok(proposal) => proposal,
                Err(message) => return (envelope_err(message), false),
            };
            let revert = req.get("revert").and_then(Value::as_bool).unwrap_or(false);
            match shared
                .engine
                .origin_withdraw(domain, proposal, revert)
                .await
            {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Resolve one recorded conflict for one domain. `content_b64` (a
        // caller-supplied merge) travels base64-encoded since the envelope
        // is JSON text and the resolved content may be binary.
        "origin_resolve" => {
            let domain = req.get("domain").and_then(Value::as_str).unwrap_or("");
            let path = req.get("path").and_then(Value::as_str).unwrap_or("");
            let keep = req.get("keep").and_then(Value::as_str);
            let content = match req.get("content_b64").and_then(Value::as_str) {
                Some(b64) => match BASE64.decode(b64) {
                    Ok(bytes) => Some(bytes),
                    Err(e) => return (envelope_err(format!("invalid content_b64: {e}")), false),
                },
                None => None,
            };
            match shared
                .engine
                .origin_resolve(domain, path, keep, content.as_deref())
                .await
            {
                Ok(data) => (envelope_ok(data), false),
                Err(e) => (envelope_err(e.to_string()), false),
            }
        }
        // Apply, inspect or record a decision for domain-declared artifact
        // provisioning. `status` is always allowed; `allow`, `deny` and
        // `apply` refuse on a read-only daemon (`Engine::provision` itself
        // checks).
        "provision" => {
            let action_str = req
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("status");
            let domain = req
                .get("domain")
                .and_then(Value::as_str)
                .map(str::to_string);
            let action = match action_str {
                "status" => Ok(crate::engine::ProvisionAction::Status),
                "allow" => domain
                    .map(|domain| crate::engine::ProvisionAction::Allow { domain })
                    .ok_or_else(|| "provision allow requires a domain".to_string()),
                "deny" => domain
                    .map(|domain| crate::engine::ProvisionAction::Deny { domain })
                    .ok_or_else(|| "provision deny requires a domain".to_string()),
                "apply" => Ok(crate::engine::ProvisionAction::Apply),
                other => Err(format!(
                    "unknown provision action '{other}'; expected status, allow, deny or apply"
                )),
            };
            match action {
                Ok(action) => match shared.engine.provision(&action).await {
                    Ok(data) => (envelope_ok(data), false),
                    Err(e) => (envelope_err(e.to_string()), false),
                },
                Err(msg) => (envelope_err(msg), false),
            }
        }
        "shutdown" => (envelope_ok(json!({ "stopping": true })), true),
        // Best-effort: `domain remove` calls this so a live daemon stops
        // watching the removed path right away instead of on its next
        // restart. Index rows are never touched here, only the watcher and
        // the engine's discovered-domain cache.
        "forget_domain" => {
            let domain = req.get("domain").and_then(Value::as_str).unwrap_or("");
            shared.engine.forget_domain(domain);
            (envelope_ok(json!({ "forgotten": domain })), false)
        }
        other => (
            envelope_err(format!(
                "unknown ctl command '{other}'; expected status, sessions, tool, sync, reindex, \
                 routing_bullets, scaffold_manifest, domain_import, domain_export, retag, \
                 configure, origin_add, origin_update, origin_status, origin_share, \
                 origin_withdraw, origin_resolve, provision, forget_domain or shutdown"
            )),
            false,
        ),
    }
}

/// Run a background-equivalent embed pass and record the count on the response.
async fn maybe_embed(shared: &Arc<Shared>, embed: bool, data: &mut Value) {
    if !embed {
        return;
    }
    match shared.engine.embed_pending().await {
        Ok(n) => {
            if let Value::Object(map) = data {
                map.insert("embedded_chunks".to_string(), json!(n));
            }
        }
        Err(e) => {
            if let Value::Object(map) = data {
                map.insert("embed_error".to_string(), json!(e.to_string()));
            }
        }
    }
}

/// An optional proposal number from the envelope: absent or null is `None`,
/// a whole non-negative number is that number, and anything else is a
/// refusal naming the key.
///
/// Deliberately not `and_then(Value::as_u64)`, which reads a negative, a
/// fraction or a string as absence. For `origin_share` that silent downgrade
/// would turn "amend layer #6" into "stack a new layer", which opens a
/// proposal the caller never asked for; for `origin_withdraw` it turns
/// "withdraw layer #6" into "withdraw the top layer", which closes a proposal
/// the caller never named. A refusal the caller can read is the only safe
/// answer to a value we cannot interpret.
fn optional_number(req: &Value, key: &str) -> Result<Option<u64>, String> {
    match req.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("invalid {key}: expected a proposal number, got {value}")),
    }
}

fn envelope_ok(data: Value) -> Value {
    json!({ "v": CTL_VERSION, "ok": true, "data": data })
}

fn envelope_err(message: impl Into<String>) -> Value {
    json!({ "v": CTL_VERSION, "ok": false, "error": message.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `origin_share`'s amend target is the parameter this guards: absent
    /// means "stack a new layer", a number means "amend that layer", and a
    /// value that is neither must refuse rather than quietly become the
    /// first of those and open a proposal nobody asked for. `origin_withdraw`
    /// reads the same key with the same stakes: absent means "the top layer",
    /// so an unreadable value there would close a layer nobody named.
    #[test]
    fn an_optional_number_refuses_a_present_but_unreadable_value() {
        assert_eq!(optional_number(&json!({}), "proposal"), Ok(None));
        assert_eq!(
            optional_number(&json!({ "proposal": Value::Null }), "proposal"),
            Ok(None)
        );
        assert_eq!(
            optional_number(&json!({ "proposal": 6 }), "proposal"),
            Ok(Some(6))
        );

        for bad in [json!(-1), json!(1.5), json!("6"), json!([6]), json!(true)] {
            let refused = optional_number(&json!({ "proposal": bad }), "proposal")
                .expect_err("a value that is not a proposal number is refused");
            assert!(refused.starts_with("invalid proposal:"), "{refused}");
            let envelope = envelope_err(refused);
            assert_eq!(envelope["ok"], false);
            assert!(
                envelope["error"]
                    .as_str()
                    .unwrap()
                    .contains("expected a proposal number"),
                "{envelope}"
            );
        }
    }

    /// Both origin arms that carry an amend or withdraw target read it
    /// through the guard rather than through `as_u64`, so neither can
    /// silently downgrade an unreadable value to "no target given".
    #[test]
    fn both_origin_arms_read_their_proposal_through_the_guard() {
        let source = include_str!("control.rs");
        for arm in ["\"origin_share\" =>", "\"origin_withdraw\" =>"] {
            let start = source.find(arm).unwrap_or_else(|| panic!("no {arm} arm"));
            let rest = &source[start + arm.len()..];
            // Up to the next arm at the same indentation, which is where this
            // one's body ends. That eight-space prefix is the assumption this
            // slice rests on: these arms sit two levels in, inside `match`
            // inside `handle`. Reindent them (another nesting level, a helper
            // function around them) and the slice runs past the arm into its
            // neighbours, where a stray `optional_number` would keep this
            // test green over an arm that no longer calls it - so move this
            // pattern with the code rather than leaving it to match by luck.
            let body = &rest[..rest.find("\n        \"").unwrap_or(rest.len())];
            assert!(
                body.contains("optional_number(req, \"proposal\")"),
                "{arm} must read its proposal through optional_number"
            );
            assert!(
                !body.contains("get(\"proposal\").and_then(Value::as_u64)"),
                "{arm} must not read its proposal as a bare as_u64"
            );
        }
    }
}
