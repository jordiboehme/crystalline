//! The ctl control protocol: newline-delimited JSON over the socket.
//!
//! Each request is one JSON line `{ "v": 1, "cmd": ..., ... }`; each response is
//! one line `{ "v": 1, "ok": true, "data": ... }` or
//! `{ "v": 1, "ok": false, "error": ... }`. Commands: sync, status, reindex,
//! sessions, forget_domain, shutdown. This is the operator channel; data
//! operations go over the MCP handshake instead.

use std::sync::Arc;

use interprocess::local_socket::tokio::Stream as IpcStream;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::daemon::Shared;

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
                "unknown ctl command '{other}'; expected status, sessions, sync, reindex, \
                 routing_bullets, scaffold_manifest, domain_import, domain_export, forget_domain or shutdown"
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

fn envelope_ok(data: Value) -> Value {
    json!({ "v": CTL_VERSION, "ok": true, "data": data })
}

fn envelope_err(message: impl Into<String>) -> Value {
    json!({ "v": CTL_VERSION, "ok": false, "error": message.into() })
}
