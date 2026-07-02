//! The ctl control protocol: newline-delimited JSON over the socket.
//!
//! Each request is one JSON line `{ "v": 1, "cmd": ..., ... }`; each response is
//! one line `{ "v": 1, "ok": true, "data": ... }` or
//! `{ "v": 1, "ok": false, "error": ... }`. Commands: sync, status, reindex,
//! sessions, shutdown. This is the operator channel; data operations go over the
//! MCP handshake instead.

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
            match shared.engine.sync(domain).await {
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
        "shutdown" => (envelope_ok(json!({ "stopping": true })), true),
        other => (
            envelope_err(format!(
                "unknown ctl command '{other}'; expected status, sessions, sync, reindex or shutdown"
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
