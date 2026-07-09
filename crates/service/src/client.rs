//! Client-side entry points: the `crystalline mcp` stdio bridge, the CLI data
//! commands (over the socket when a daemon runs, else in-process) and the ctl
//! client used by the CLI operator commands.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use interprocess::local_socket::tokio::Stream as IpcStream;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader, ReadBuf};

use crate::daemon::{open_store, resolve_db};
use crate::engine::{Engine, open_standalone};
use crate::instance::{Connection, acquire_ownership, ensure_daemon, try_attach};
use crate::mcp::McpServer;
use crate::overlay;
use crate::params::*;

/// Whether a CLI verb may route to a running daemon instead of opening the
/// index (or config) directly. An explicit `db` or `config_path` override means
/// "operate on exactly this file/index"; a running daemon serves ITS OWN default
/// config and index, which may be entirely different ones, so with either
/// override the answer, or worse the write, would land in the wrong place. Any
/// override therefore bypasses the daemon and takes the direct in-process path;
/// only when BOTH are absent may the socket-first path run, which is the plain
/// `crystalline <verb>` invocation the daemon-first design is meant for. A verb
/// that takes only one of the two passes `None` for the other and so gates on
/// the override it actually has.
pub fn use_daemon(db: Option<&Path>, config_path: Option<&Path>) -> bool {
    db.is_none() && config_path.is_none()
}

/// If `line` is a JSON-RPC request for a known pre-`initialize` probe that
/// rmcp 2.x cannot handle gracefully in its init loop, return the JSON-RPC
/// `-32601 Method not found` response to send back so the client falls
/// back to plain `initialize` instead of seeing our stdio close and
/// classifying the connection as a network error.
///
/// The confirmed case is the TypeScript MCP SDK's dual-era negotiation
/// probe `server/discover`, added by the `versionNegotiation.mode = "auto"`
/// path and shipped by Claude Desktop chat mode as of July 2026. rmcp's
/// init loop returns `ExpectedInitializeRequest` for any pre-init request
/// that is not `ping` or `initialize` and does not send a response, so the
/// process exits and the client sees a closed connection. The TypeScript
/// SDK's probe classifier maps a closed connection to `network-error` and
/// aborts the session; a `-32601` reply would be classified as `legacy`
/// and trigger a normal `initialize` retry on the same pipe.
///
/// Only `server/discover` is intercepted; every other message flows to
/// rmcp unchanged so a real client bug is still visible. Broaden the set
/// here (or move to a "reply to any pre-init request" model) if further
/// probe methods are observed in the wild.
fn preinit_probe_reply(line: &str) -> Option<String> {
    let msg: Value = serde_json::from_str(line).ok()?;
    let method = msg.get("method")?.as_str()?;
    if method != "server/discover" {
        return None;
    }
    let id = msg.get("id")?;
    let reply = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32601,
            "message": format!("Method not found: {method}"),
        },
    });
    Some(reply.to_string())
}

/// Build a JSON-RPC error response to the `initialize` request in `init_line`,
/// answering it with `err_text` when the embedded startup fails before rmcp
/// ever takes over stdio. Without this the client would see nothing but a
/// closed pipe; the TypeScript SDK's negotiation window reads a mid-handshake
/// close as an unrecoverable network error and never retries, so a readable
/// `initialize` failure is strictly better than dying silently. Returns `None`
/// when the line carries no id to answer (malformed JSON, or a notification),
/// in which case the caller skips the write and just propagates the error.
fn initialize_error_reply(init_line: &str, err_text: &str) -> Option<String> {
    let msg: Value = serde_json::from_str(init_line).ok()?;
    let id = msg.get("id")?;
    let reply = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32000,
            "message": format!("crystalline mcp failed to start: {err_text}"),
        },
    });
    Some(reply.to_string())
}

/// An [`AsyncRead`] wrapper that yields a buffered `prefix` slice before
/// delegating to `inner`. [`run_mcp`] builds one after the pre-init probe
/// drain to re-front the `initialize` line it already read off stdin, together
/// with anything the underlying `BufReader` had buffered past it, so the
/// serving path (the daemon relay or the embedded rmcp server) sees the
/// `initialize` as its first line with no special replay.
struct Prefixed<R> {
    prefix: Vec<u8>,
    inner: R,
}

impl<R: AsyncRead + Unpin> AsyncRead for Prefixed<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.prefix.is_empty() {
            let n = std::cmp::min(self.prefix.len(), buf.remaining());
            buf.put_slice(&self.prefix[..n]);
            self.prefix.drain(..n);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

/// Read lines off `reader` until an `initialize` request arrives, replying
/// to any [`preinit_probe_reply`]-eligible line on `stdout` in the
/// meantime. Returns the raw `initialize` line so the caller can prepend
/// it to a wrapped reader before handing to `rmcp::serve_server`, or
/// `None` on stdin EOF before any `initialize`. Non-JSON, notifications
/// and unrecognized requests fall through untouched so rmcp still sees
/// them and rejects them the way it always has.
async fn drain_preinit_probes<R, W>(
    reader: &mut BufReader<R>,
    stdout: &mut W,
) -> std::io::Result<Option<String>>
where
    R: AsyncRead + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader.read_line(&mut buf).await?;
        if n == 0 {
            return Ok(None);
        }
        // read_line keeps the trailing newline; strip it to canonicalize.
        let line = buf.trim_end_matches(['\r', '\n']).to_string();
        if let Some(reply) = preinit_probe_reply(&line) {
            stdout.write_all(reply.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
            continue;
        }
        return Ok(Some(line));
    }
}

/// The `crystalline mcp` stdio entry: attach to (or spawn) a daemon and relay
/// the session, or run the full stack in-process when embedded or when no
/// daemon can be started. The relay survives a daemon restart (a version
/// takeover after an upgrade, a crash): it reconnects, replays the MCP
/// handshake and continues the session, so the harness never sees its stdio
/// transport die just because the daemon was replaced.
pub async fn run_mcp(
    embedded: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
    read_only: bool,
) -> anyhow::Result<()> {
    // Log to stderr from the start, for both modes: the relay's takeover and
    // reconnect notices and any embedded startup failure must be visible in the
    // harness's server log, not swallowed.
    init_tracing();

    // Answering the version-negotiation probe (Claude Desktop chat mode's
    // `server/discover`, see [`preinit_probe_reply`]) needs only stdin and
    // stdout, so drive the drain concurrently with daemon acquisition: the
    // probe is answered in milliseconds while a cold daemon spawns in parallel,
    // instead of the client waiting out the whole startup for its reply. A
    // transport close during that window is a terminal error the SDK never
    // retries, so a slow start is survivable but an exit here is not. Embedded
    // mode has no daemon half; it only drains.
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(tokio::io::stdin());
    let (drained, daemon) = if embedded {
        (drain_preinit_probes(&mut reader, &mut stdout).await, None)
    } else {
        // `read_only` is forwarded only to a daemon this call spawns; attaching
        // to an already-running daemon uses that daemon's own mode.
        let (drained, daemon) = tokio::join!(
            drain_preinit_probes(&mut reader, &mut stdout),
            ensure_daemon(true, db, config_path, read_only),
        );
        (drained, Some(daemon))
    };

    // Stdin EOF before any `initialize` means the client left mid-window; a
    // daemon this call spawned staying up is fine by design, so exit cleanly.
    // A real drain I/O error propagates.
    let Some(init_line) = drained? else {
        return Ok(());
    };
    // Re-front the drained `initialize` (plus anything buffered past it) so the
    // serving path reads it as its first stdin line with no special replay.
    let primed = prime_reader(&init_line, reader);

    // A daemon is up: relay through it. A failed `mcp` handshake falls through
    // to the embedded path rather than propagating, so an unreachable daemon
    // still yields a working in-process server instead of a mid-window close.
    if let Some(daemon) = daemon {
        match daemon {
            Ok(conn) => match conn.into_mcp().await {
                Ok(stream) => return pump_stdio(stream, primed, db, config_path, read_only).await,
                Err(e) => tracing::warn!("daemon MCP handshake failed ({e}); running embedded"),
            },
            Err(e) => tracing::warn!("no daemon available ({e}); running embedded"),
        }
    }

    // Embedded path: the explicit flag, or a daemon that could not be reached.
    // A terminal startup failure (lock held, config or store error) happens
    // before rmcp answers anything, so reply to the held `initialize` with a
    // JSON-RPC error rather than closing stdio, which the negotiation window
    // reads as an unrecoverable network error. The process still exits non-zero
    // (stderr carries the chain for the Desktop log).
    match run_embedded_stdio(primed, db, config_path, read_only).await {
        Ok(()) => Ok(()),
        Err(e) => {
            if let Some(reply) = initialize_error_reply(&init_line, &format!("{e:#}")) {
                let _ = stdout.write_all(reply.as_bytes()).await;
                let _ = stdout.write_all(b"\n").await;
                let _ = stdout.flush().await;
            }
            Err(e)
        }
    }
}

/// Re-front the drained `initialize` line ahead of stdin: the prefix is the
/// line, a newline and whatever the `BufReader` buffered past it, the inner is
/// the raw stdin. See [`Prefixed`].
fn prime_reader(
    init_line: &str,
    reader: BufReader<tokio::io::Stdin>,
) -> Prefixed<tokio::io::Stdin> {
    let buffered = reader.buffer().to_vec();
    let inner = reader.into_inner();
    let mut prefix = Vec::with_capacity(init_line.len() + 1 + buffered.len());
    prefix.extend_from_slice(init_line.as_bytes());
    prefix.push(b'\n');
    prefix.extend_from_slice(&buffered);
    Prefixed { prefix, inner }
}

/// How one relay session over a daemon socket ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionEnd {
    /// The MCP client closed its side; the bridge is done.
    StdinClosed,
    /// The daemon side closed or failed; the bridge should reconnect.
    SocketClosed,
}

/// What the relay remembers across daemon restarts: the client's handshake
/// lines to replay verbatim and the ids of requests still waiting for a
/// response, which get an error answer after a restart instead of silence.
#[derive(Default)]
struct RelayState {
    init_request: Option<String>,
    init_id: Option<Value>,
    initialized_note: Option<String>,
    outstanding: std::collections::HashMap<String, Value>,
}

impl RelayState {
    /// Record a client-to-daemon line: the initialize handshake, the
    /// initialized notification and every request id awaiting a response. A
    /// client line with an id but no method is the client answering a
    /// server-initiated request and is not tracked.
    fn note_client_line(&mut self, line: &str) {
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let method = msg.get("method").and_then(|m| m.as_str());
        let id = msg.get("id");
        match (method, id) {
            (Some("initialize"), Some(id)) => {
                self.init_request = Some(line.to_string());
                self.init_id = Some(id.clone());
                self.outstanding.insert(id.to_string(), id.clone());
            }
            (Some("notifications/initialized"), _) => {
                self.initialized_note = Some(line.to_string());
            }
            (Some(_), Some(id)) => {
                self.outstanding.insert(id.to_string(), id.clone());
            }
            _ => {}
        }
    }

    /// Record a daemon-to-client line: a response (an id without a method)
    /// settles its outstanding request.
    fn note_server_line(&mut self, line: &str) {
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if msg.get("method").is_none()
            && let Some(id) = msg.get("id")
        {
            self.outstanding.remove(&id.to_string());
        }
    }
}

/// One connected relay session: the daemon socket split into a line reader
/// and a writer that both live for the whole session, so no buffered bytes
/// are lost between the handshake replay and the relay loop.
struct Session<S> {
    sock_lines: tokio::io::Lines<BufReader<tokio::io::ReadHalf<S>>>,
    sock_write: tokio::io::WriteHalf<S>,
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> Session<S> {
    fn new(stream: S) -> Self {
        let (read, write) = tokio::io::split(stream);
        Session {
            sock_lines: BufReader::new(read).lines(),
            sock_write: write,
        }
    }
}

/// Relay lines both ways until one side ends. Returns how the session ended
/// and whether any daemon line was forwarded, the signal that the connection
/// was genuinely serving rather than dying straight after a reconnect.
async fn relay_loop<In, Out, S>(
    relay: &mut RelayState,
    stdin: &mut tokio::io::Lines<BufReader<In>>,
    stdout: &mut Out,
    session: &mut Session<S>,
) -> std::io::Result<(SessionEnd, bool)>
where
    In: tokio::io::AsyncRead + Unpin,
    Out: AsyncWriteExt + Unpin,
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut served_any = false;
    loop {
        tokio::select! {
            line = stdin.next_line() => match line? {
                None => {
                    let _ = session.sock_write.shutdown().await;
                    return Ok((SessionEnd::StdinClosed, served_any));
                }
                Some(line) => {
                    if let Some(reply) = preinit_probe_reply(&line) {
                        stdout.write_all(reply.as_bytes()).await?;
                        stdout.write_all(b"\n").await?;
                        stdout.flush().await?;
                        continue;
                    }
                    relay.note_client_line(&line);
                    let sent = session.sock_write.write_all(line.as_bytes()).await.is_ok()
                        && session.sock_write.write_all(b"\n").await.is_ok()
                        && session.sock_write.flush().await.is_ok();
                    if !sent {
                        return Ok((SessionEnd::SocketClosed, served_any));
                    }
                }
            },
            line = session.sock_lines.next_line() => match line {
                Ok(Some(line)) => {
                    relay.note_server_line(&line);
                    stdout.write_all(line.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                    served_any = true;
                }
                Ok(None) | Err(_) => return Ok((SessionEnd::SocketClosed, served_any)),
            },
        }
    }
}

/// Re-establish the MCP session on a fresh daemon connection: replay the
/// client's `initialize` verbatim and swallow the daemon's answer (the client
/// already holds one), replay `notifications/initialized`, then answer every
/// request the restart orphaned with a JSON-RPC error so the client can retry
/// instead of hanging.
async fn resync<Out, S>(
    relay: &mut RelayState,
    session: &mut Session<S>,
    stdout: &mut Out,
) -> std::io::Result<()>
where
    Out: AsyncWriteExt + Unpin,
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    if let Some(init) = relay.init_request.clone() {
        session.sock_write.write_all(init.as_bytes()).await?;
        session.sock_write.write_all(b"\n").await?;
        session.sock_write.flush().await?;
        let init_key = relay.init_id.as_ref().map(|id| id.to_string());
        loop {
            let Some(line) = session.sock_lines.next_line().await? else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "daemon closed during handshake replay",
                ));
            };
            let msg: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            let is_init_response =
                msg.get("method").is_none() && msg.get("id").map(|id| id.to_string()) == init_key;
            if is_init_response {
                break;
            }
            // Anything the daemon volunteers before answering the replayed
            // initialize predates the client's view of this session; drop it.
        }
        if let Some(note) = relay.initialized_note.clone() {
            session.sock_write.write_all(note.as_bytes()).await?;
            session.sock_write.write_all(b"\n").await?;
            session.sock_write.flush().await?;
        }
    }

    let orphaned: Vec<Value> = relay
        .outstanding
        .drain()
        .filter(|(key, _)| Some(key) != relay.init_id.as_ref().map(|id| id.to_string()).as_ref())
        .map(|(_, id)| id)
        .collect();
    for id in orphaned {
        let error = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32000,
                "message": "crystalline daemon restarted; retry this request",
            },
        });
        stdout.write_all(error.to_string().as_bytes()).await?;
        stdout.write_all(b"\n").await?;
    }
    stdout.flush().await?;
    Ok(())
}

/// Relay stdin and stdout to the daemon socket, reconnecting when the daemon
/// goes away mid-session. Gives up after several consecutive reconnects that
/// never manage to serve a line, so a crash-looping daemon fails the bridge
/// loudly instead of spinning forever.
async fn pump_stdio<R>(
    stream: IpcStream,
    reader: R,
    db: Option<&Path>,
    config_path: Option<&Path>,
    read_only: bool,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut relay = RelayState::default();
    let mut stdin = BufReader::new(reader).lines();
    let mut stdout = tokio::io::stdout();
    let mut session = Session::new(stream);
    let mut fruitless_reconnects = 0u32;

    loop {
        let (end, served_any) =
            relay_loop(&mut relay, &mut stdin, &mut stdout, &mut session).await?;
        if end == SessionEnd::StdinClosed {
            return Ok(());
        }
        if served_any {
            fruitless_reconnects = 0;
        }
        loop {
            fruitless_reconnects += 1;
            if fruitless_reconnects > 5 {
                anyhow::bail!(
                    "the crystalline daemon connection was lost and could not be re-established"
                );
            }
            tracing::warn!("daemon connection lost; reconnecting");
            let conn = match ensure_daemon(true, db, config_path, read_only).await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::warn!("reconnect failed: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    continue;
                }
            };
            let Ok(stream) = conn.into_mcp().await else {
                continue;
            };
            session = Session::new(stream);
            match resync(&mut relay, &mut session, &mut stdout).await {
                Ok(()) => break,
                Err(e) => {
                    tracing::warn!("session replay failed: {e}");
                    continue;
                }
            }
        }
    }
}

/// The full in-process stack over stdio. Takes the lock; refuses if held. The
/// effective mode is the explicit flag or `service.read_only`. `reader` is the
/// primed stdin [`run_mcp`] already fronted with the drained `initialize`, so
/// this path never touches the pre-init probe itself; it hands the reader
/// straight to `rmcp::serve_server`. A startup error returned here reaches
/// `run_mcp` before rmcp ever answers, which is where the terminal-failure
/// `initialize` reply is written.
async fn run_embedded_stdio<R>(
    reader: R,
    db: Option<&Path>,
    config_path: Option<&Path>,
    read_only: bool,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let ownership = acquire_ownership()
        .map_err(|e| anyhow::anyhow!("cannot run an embedded MCP server: {e}"))?;
    let loaded = overlay::load(config_path)?;
    let read_only = read_only || loaded.effective.read_only();
    let db_path = resolve_db(db)?;
    let store = open_store(&loaded.effective, Some(&db_path)).await?;
    // The provider is built in the background so the stdio session is ready and
    // text search works before any model download completes. There is no
    // watcher task in this mode, so the resolved config path only helps a domain
    // added mid-session resolve for data operations, not for picking up external
    // file changes. The engine holds the file config and the overlay apart, so
    // its effective config drives reads while persistence stays env-free.
    let engine = Arc::new(
        Engine::new(store, loaded.file.clone(), None, Some(loaded.path.clone()))
            .with_read_only(read_only)
            .with_env_overlay(loaded.overlay.clone()),
    );

    let bg = engine.clone();
    let bg_config = loaded.effective.clone();
    tokio::spawn(async move {
        let _ = bg.sync(None).await;
        if let Some(provider) = crate::engine::build_provider(&bg_config).await {
            bg.set_provider(provider);
            let _ = bg.embed_pending().await;
        }
    });

    // Prime the routing cache before serving so the very first `initialize`
    // renders complete instructions, never racing the background sync above.
    engine.refresh_routing_cache().await;

    let server = McpServer::new(engine);
    let stdout = tokio::io::stdout();
    let running = rmcp::serve_server(server, (reader, stdout)).await?;
    let _ = running.waiting().await;
    drop(ownership);
    Ok(())
}

/// Run a tool by name: over the socket when a daemon is up, else in-process
/// against a directly opened store.
pub async fn run_tool(
    tool: &str,
    args: Value,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    if use_daemon(db, config_path)
        && let Some(conn) = try_attach().await
    {
        let stream = conn.into_mcp().await?;
        return call_tool_over_stream(stream, tool, args).await;
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let want_embeddings = matches!(tool, "search_engrams");
    let engine = open_standalone(loaded, &db_path, want_embeddings).await?;
    dispatch_engine(&engine, tool, args).await
}

/// Scaffold a virtual domain's MANIFEST from prebuilt markdown: over the daemon
/// when one owns the index, else against a directly opened store.
pub async fn scaffold_virtual_manifest(
    domain: &str,
    markdown: &str,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "scaffold_manifest", "domain": domain, "markdown": markdown,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.scaffold_virtual_manifest(domain, markdown).await?)
}

/// Import engram files into a virtual domain: over the daemon when one owns the
/// index, else against a directly opened store.
pub async fn domain_import(
    domain: &str,
    src: &Path,
    overwrite: bool,
    dry_run: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "domain_import", "domain": domain,
            "path": src.display().to_string(), "overwrite": overwrite, "dry_run": dry_run,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine
        .import_domain(domain, src, overwrite, dry_run)
        .await?)
}

/// Export a domain's engrams to a filesystem folder: over the daemon when one
/// owns the index, else against a directly opened store.
pub async fn domain_export(
    domain: &str,
    dest: &Path,
    force: bool,
    dry_run: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "domain_export", "domain": domain,
            "path": dest.display().to_string(), "force": force, "dry_run": dry_run,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.export_domain(domain, dest, force, dry_run).await?)
}

/// Connect a new domain to a GitHub repository: over the daemon when one owns
/// the index, else against a directly opened store. `want_embeddings` is
/// `false` in the standalone fallback, matching `domain_import` and
/// `domain_export`: a one-shot command never triggers a surprise embedding
/// model download, and the domain is searchable via text immediately either
/// way; embedding follows whenever the daemon (or a later `sync --embed`)
/// gets to it.
pub async fn origin_add(
    repo: &str,
    domain: Option<&str>,
    path: Option<&str>,
    branch: Option<&str>,
    folder: Option<&str>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "origin_add", "repo": repo, "domain": domain,
            "path": path, "branch": branch, "folder": folder,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine
        .origin_add(repo, domain, path, branch, folder)
        .await?)
}

/// Bring one origin-connected domain (or every one) up to date: over the
/// daemon when one owns the index, else against a directly opened store.
pub async fn origin_update(
    domain: Option<&str>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) =
            ctl_if_running(json!({ "v": 1, "cmd": "origin_update", "domain": domain })).await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.origin_update(domain).await?)
}

/// Report where one origin-connected domain (or every one) stands relative to
/// its origin, plus this machine's GitHub connection: over the daemon when
/// one owns the index, else against a directly opened store.
pub async fn origin_status(
    domain: Option<&str>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) =
            ctl_if_running(json!({ "v": 1, "cmd": "origin_status", "domain": domain })).await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.origin_status(domain).await?)
}

/// Propose one team domain's local changes as a pull request against its
/// origin: over the daemon when one owns the index, else against a directly
/// opened store. `want_embeddings` is `false` in the standalone fallback: a
/// share never touches the working tree, so there is nothing new to embed.
pub async fn origin_share(
    domain: &str,
    title: Option<&str>,
    description: Option<&str>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "origin_share", "domain": domain,
            "title": title, "description": description,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.origin_share(domain, title, description).await?)
}

/// Discard a declined, or still-open, share proposal for one team domain,
/// restoring local files that were not changed since sharing them: over the
/// daemon when one owns the index, else against a directly opened store.
pub async fn origin_discard(
    domain: &str,
    proposal: u64,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "origin_discard", "domain": domain, "proposal": proposal,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.origin_discard(domain, proposal).await?)
}

/// Resolve one recorded conflict for a team domain: over the daemon when one
/// owns the index, else against a directly opened store. `content` (a
/// caller-supplied merge) travels over the ctl socket base64-encoded, since
/// the JSON envelope carries text only and a resolved asset may be binary;
/// the in-process fallback passes the bytes straight through.
pub async fn origin_resolve(
    domain: &str,
    path: &str,
    keep: Option<&str>,
    content: Option<&[u8]>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use serde_json::json;
    let content_b64 = content.map(|c| BASE64.encode(c));
    if use_daemon(db, config_path)
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "origin_resolve", "domain": domain, "path": path,
            "keep": keep, "content_b64": content_b64,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine.origin_resolve(domain, path, keep, content).await?)
}

/// Show, set or reset an agent-adjustable setting from the [`crate::settings`]
/// registry: over the daemon when one is running and no explicit config file
/// was named, else against the config file directly (no index store is opened
/// either way, unlike every data command above). `action` is `show`, `set` or
/// `unset`; `key` and `value` are required for `set`, `key` alone for
/// `unset`, and both are ignored for `show`.
pub async fn configure(
    action: &str,
    key: Option<&str>,
    value: Option<&str>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    // An explicit --config override names the exact file to operate on. A
    // running daemon answers for ITS config file, which may be a different
    // one entirely, so the override always takes the direct path and the
    // daemon is only consulted about the default config it actually serves.
    if config_path.is_none()
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "configure", "action": action, "key": key, "value": value,
        }))
        .await?
    {
        return Ok(data);
    }

    // The single load chokepoint resolves the config path and parses the
    // environment overlay: `show` reads the file config plus the overlay, and a
    // write mutates a clone of the file config and saves it to the resolved
    // path, so no environment value ever bakes into the saved file.
    let loaded = overlay::load(config_path)?;
    match action {
        "show" => Ok(json!({
            "settings": crate::settings::snapshot(&loaded.file, &loaded.overlay)
        })),
        "set" => {
            if loaded.effective.read_only() {
                anyhow::bail!("{}", crate::engine::EngineError::ReadOnly);
            }
            let key = key.ok_or_else(|| anyhow::anyhow!("configure set requires a key"))?;
            let value = value.ok_or_else(|| anyhow::anyhow!("configure set requires a value"))?;
            let mut file = loaded.file.clone();
            crate::settings::apply(&mut file, key, value)?;
            save_file(&loaded.path, &file)?;
            Ok(setting_view(&file, key, &loaded.overlay))
        }
        "unset" => {
            if loaded.effective.read_only() {
                anyhow::bail!("{}", crate::engine::EngineError::ReadOnly);
            }
            let key = key.ok_or_else(|| anyhow::anyhow!("configure unset requires a key"))?;
            let mut file = loaded.file.clone();
            crate::settings::unset(&mut file, key)?;
            save_file(&loaded.path, &file)?;
            Ok(setting_view(&file, key, &loaded.overlay))
        }
        other => anyhow::bail!("unknown configure action '{other}'; expected show, set or unset"),
    }
}

/// The just-applied setting's snapshot entry, as a JSON value, with a `note`
/// field attached when [`crate::settings::change_note`] has one (a
/// startup-effective reminder, an active env override, or both). `file` is the
/// freshly saved file config; the snapshot layers `overlay` on top, so an
/// env-overridden key reports its env value with `source: env`. `key` has
/// already been validated against the registry by `apply`/`unset`, so it is
/// always found.
fn setting_view(
    file: &crystalline_core::config::GlobalConfig,
    key: &str,
    overlay: &overlay::EnvOverlay,
) -> Value {
    crate::settings::snapshot(file, overlay)
        .into_iter()
        .find(|v| v.key == key)
        .map(|v| {
            let mut value = serde_json::to_value(v).unwrap_or(Value::Null);
            if let Some(note) = crate::settings::change_note(key, overlay)
                && let Value::Object(map) = &mut value
            {
                map.insert("note".to_string(), Value::String(note));
            }
            value
        })
        .unwrap_or(Value::Null)
}

/// Save a config to the path the load chokepoint already resolved.
fn save_file(path: &Path, config: &crystalline_core::config::GlobalConfig) -> anyhow::Result<()> {
    crystalline_core::config::save_yaml(path, config)
        .map_err(|e| anyhow::anyhow!("failed to save config {}: {e}", path.display()))
}

/// Resolve virtual-domain routing bullets for `prompt system`: over the daemon
/// when one is running (its warm state) and no explicit `--config`/`--db`
/// override was given, else against a directly opened store. Returns an empty
/// map when the config has no virtual domains, so the common all-file case never
/// opens a store or a socket. `config_path` is the raw `--config` override the
/// caller resolved `config` from, threaded through only so an override bypasses
/// the daemon (which serves its own default config) exactly like every other
/// verb.
pub async fn virtual_routing_bullets(
    config: &crystalline_core::config::GlobalConfig,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> std::collections::BTreeMap<String, Vec<String>> {
    use serde_json::json;
    if !config.domains.values().any(|e| e.is_virtual()) {
        return std::collections::BTreeMap::new();
    }
    if use_daemon(db, config_path)
        && let Ok(Some(data)) = ctl_if_running(json!({ "v": 1, "cmd": "routing_bullets" })).await
        && let Ok(map) = serde_json::from_value(data)
    {
        return map;
    }
    let db_path = match resolve_db(db) {
        Ok(p) => p,
        Err(_) => return std::collections::BTreeMap::new(),
    };
    // The caller already resolved the effective config (the overlay is applied
    // upstream in `run_prompt`), and this read-only path never persists, so a
    // no-op overlay over the given config is all `open_standalone` needs. The
    // path is only consulted on a post-startup domain re-read, which this
    // one-shot never performs.
    let loaded = overlay::LoadedConfig {
        path: crystalline_core::config::global_config_path().unwrap_or_default(),
        file: config.clone(),
        effective: config.clone(),
        overlay: overlay::EnvOverlay::default(),
    };
    match open_standalone(loaded, &db_path, false).await {
        Ok(engine) => engine.virtual_routing_bullets().await,
        Err(_) => std::collections::BTreeMap::new(),
    }
}

/// Call a tool over an MCP client on a socket stream and return its JSON value.
async fn call_tool_over_stream(
    stream: IpcStream,
    tool: &str,
    args: Value,
) -> anyhow::Result<Value> {
    let client = rmcp::serve_client((), stream).await?;
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    let result = client.peer().call_tool(params).await;
    let out = match result {
        Ok(result) => extract_tool_value(&result),
        Err(e) => Err(anyhow::anyhow!("{e}")),
    };
    let _ = client.cancel().await;
    out
}

/// Pull the tool's JSON body out of the single text content block.
fn extract_tool_value(result: &CallToolResult) -> anyhow::Result<Value> {
    let v = serde_json::to_value(result)?;
    if let Some(text) = v.pointer("/content/0/text").and_then(Value::as_str) {
        return Ok(serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())));
    }
    Ok(v)
}

/// Dispatch a tool to the in-process engine.
async fn dispatch_engine(engine: &Engine, tool: &str, args: Value) -> anyhow::Result<Value> {
    let v = match tool {
        "write_engram" => engine.write_engram(&decode::<WriteParams>(args)?).await?,
        "read_engram" => engine.read_engram(&decode::<ReadParams>(args)?).await?,
        "edit_engram" => engine.edit_engram(&decode::<EditParams>(args)?).await?,
        "move_engram" => engine.move_engram(&decode::<MoveParams>(args)?).await?,
        "delete_engram" => engine.delete_engram(&decode::<DeleteParams>(args)?).await?,
        "search_engrams" => {
            engine
                .search_engrams(&decode::<SearchParams>(args)?)
                .await?
        }
        "build_context" => {
            engine
                .build_context(&decode::<ContextParams>(args)?)
                .await?
        }
        "recent_activity" => {
            engine
                .recent_activity(&decode::<RecentParams>(args)?)
                .await?
        }
        "list_domains" => {
            engine
                .list_domains(&decode::<ListDomainsParams>(args)?)
                .await?
        }
        "browse_domain" => engine.browse_domain(&decode::<BrowseParams>(args)?).await?,
        "validate_engrams" => {
            engine
                .validate_engrams(&decode::<ValidateParams>(args)?)
                .await?
        }
        "infer_schema" => engine.infer_schema(&decode::<InferParams>(args)?).await?,
        other => anyhow::bail!("unknown tool '{other}'"),
    };
    Ok(v)
}

fn decode<T: DeserializeOwned>(args: Value) -> anyhow::Result<T> {
    serde_json::from_value(args).map_err(|e| anyhow::anyhow!("invalid arguments: {e}"))
}

// --- ctl client --------------------------------------------------------------

/// Send a ctl command if a daemon is running, else `None`.
pub async fn ctl_if_running(cmd: Value) -> anyhow::Result<Option<Value>> {
    match try_attach().await {
        Some(conn) => Ok(Some(ctl_exchange(conn, cmd).await?)),
        None => Ok(None),
    }
}

/// Send a ctl command, erroring when no daemon is running.
pub async fn ctl_required(cmd: Value) -> anyhow::Result<Value> {
    match try_attach().await {
        Some(conn) => ctl_exchange(conn, cmd).await,
        None => {
            anyhow::bail!("no Crystalline daemon is running; start one with `crystalline serve`")
        }
    }
}

async fn ctl_exchange(conn: Connection, cmd: Value) -> anyhow::Result<Value> {
    let stream = conn.into_ctl().await?;
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);
    let mut line = serde_json::to_string(&cmd)?;
    line.push('\n');
    write.write_all(line.as_bytes()).await?;
    write.flush().await?;

    let mut response = String::new();
    reader.read_line(&mut response).await?;
    let value: Value = serde_json::from_str(response.trim())?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(value.get("data").cloned().unwrap_or(Value::Null))
    } else {
        anyhow::bail!(
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("ctl error")
                .to_string()
        )
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn lines_from(bytes: &'static [u8]) -> tokio::io::Lines<BufReader<&'static [u8]>> {
        BufReader::new(bytes).lines()
    }

    #[test]
    fn relay_state_tracks_the_handshake_and_outstanding_requests() {
        let mut relay = RelayState::default();
        relay.note_client_line(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#);
        relay.note_client_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        relay.note_client_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#);
        relay.note_client_line(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#);

        assert!(relay.init_request.as_ref().unwrap().contains("initialize"));
        assert_eq!(relay.init_id, Some(serde_json::json!(0)));
        assert!(relay.initialized_note.is_some());
        assert!(relay.outstanding.contains_key("1"));
        assert!(
            !relay.outstanding.contains_key("9"),
            "a client response to a server request is not outstanding"
        );

        relay.note_server_line(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#);
        assert!(!relay.outstanding.contains_key("1"));
        relay.note_server_line(r#"{"jsonrpc":"2.0","id":2,"method":"sampling/createMessage"}"#);
        assert!(
            relay.outstanding.contains_key("0"),
            "a server request never settles an id"
        );
    }

    #[tokio::test]
    async fn relay_loop_forwards_both_ways_and_reports_socket_eof() {
        let (bridge_side, daemon_side) = tokio::io::duplex(4096);
        let daemon = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(daemon_side);
            let mut lines = BufReader::new(read).lines();
            let request = lines.next_line().await.unwrap().unwrap();
            assert!(request.contains("tools/call"));
            write
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n")
                .await
                .unwrap();
            write.flush().await.unwrap();
            // Then the daemon dies.
            drop(write);
            drop(lines);
        });

        // stdin stays open (the writer half is kept alive), so the loop ends
        // on the daemon's EOF, not on a client close racing the response.
        let (mut stdin_feed, stdin_read) = tokio::io::duplex(1024);
        stdin_feed
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\"}\n")
            .await
            .unwrap();
        let mut relay = RelayState::default();
        let mut stdin = BufReader::new(stdin_read).lines();
        let mut stdout = Vec::new();
        let mut session = Session::new(bridge_side);

        let (end, served) = relay_loop(&mut relay, &mut stdin, &mut stdout, &mut session)
            .await
            .unwrap();
        assert_eq!(end, SessionEnd::SocketClosed);
        assert!(served);
        let out = String::from_utf8(stdout).unwrap();
        assert!(out.contains("\"id\":1"), "{out}");
        assert!(relay.outstanding.is_empty(), "the response settled the id");
        daemon.await.unwrap();
        drop(stdin_feed);
    }

    #[tokio::test]
    async fn resync_replays_the_handshake_and_fails_orphaned_requests() {
        let mut relay = RelayState::default();
        relay.note_client_line(
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"clientInfo":{}}}"#,
        );
        relay.note_server_line(r#"{"jsonrpc":"2.0","id":0,"result":{"serverInfo":{}}}"#);
        relay.note_client_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        relay.note_client_line(r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{}}"#);

        let (bridge_side, daemon_side) = tokio::io::duplex(4096);
        let daemon = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(daemon_side);
            let mut lines = BufReader::new(read).lines();
            let init = lines.next_line().await.unwrap().unwrap();
            assert!(init.contains("\"initialize\""), "{init}");
            // A notification the fresh daemon volunteers before answering.
            write
                .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}\n")
                .await
                .unwrap();
            write
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"serverInfo\":{}}}\n")
                .await
                .unwrap();
            write.flush().await.unwrap();
            let note = lines.next_line().await.unwrap().unwrap();
            assert!(note.contains("notifications/initialized"), "{note}");
            (init, note)
        });

        let mut stdout = Vec::new();
        let mut session = Session::new(bridge_side);
        resync(&mut relay, &mut session, &mut stdout).await.unwrap();

        let out = String::from_utf8(stdout).unwrap();
        assert!(
            out.contains("\"id\":7") && out.contains("daemon restarted"),
            "the orphaned request gets an error answer: {out}"
        );
        assert!(
            !out.contains("serverInfo") && !out.contains("notifications/message"),
            "nothing from the replayed handshake reaches the client: {out}"
        );
        assert!(relay.outstanding.is_empty());
        daemon.await.unwrap();
    }

    #[tokio::test]
    async fn relay_loop_reports_stdin_closed_on_client_eof() {
        let (bridge_side, mut daemon_side) = tokio::io::duplex(4096);
        let mut relay = RelayState::default();
        let mut stdin = lines_from(b"");
        let mut stdout = Vec::new();
        let mut session = Session::new(bridge_side);

        let (end, served) = relay_loop(&mut relay, &mut stdin, &mut stdout, &mut session)
            .await
            .unwrap();
        assert_eq!(end, SessionEnd::StdinClosed);
        assert!(!served);
        // The daemon side sees EOF from the shutdown.
        let mut buf = Vec::new();
        daemon_side.read_to_end(&mut buf).await.unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn preinit_probe_reply_answers_server_discover_only() {
        // The TypeScript SDK dual-era probe with a numeric id.
        let reply = preinit_probe_reply(r#"{"jsonrpc":"2.0","id":0,"method":"server/discover"}"#)
            .expect("server/discover is intercepted");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["id"], serde_json::json!(0));
        assert_eq!(v["error"]["code"], serde_json::json!(-32601));
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("server/discover")
        );

        // A string id must round-trip verbatim.
        let reply = preinit_probe_reply(
            r#"{"jsonrpc":"2.0","id":"abc","method":"server/discover","params":{}}"#,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["id"], serde_json::json!("abc"));

        // initialize, notifications, tool calls, and garbage all fall through.
        assert!(preinit_probe_reply(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).is_none());
        assert!(
            preinit_probe_reply(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );
        assert!(preinit_probe_reply(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call"}"#).is_none());
        assert!(preinit_probe_reply("not json at all").is_none());
    }

    #[tokio::test]
    async fn drain_preinit_probes_answers_probes_and_returns_initialize() {
        let input: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"server/discover\"}\n\
            {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n\
            extra-buffered-past-initialize\n";
        let mut reader = BufReader::new(input);
        let mut stdout = Vec::new();
        let init = drain_preinit_probes(&mut reader, &mut stdout)
            .await
            .unwrap()
            .expect("initialize is returned");
        assert!(init.contains("\"initialize\""), "{init}");

        // The probe got a -32601 answer on stdout.
        let out = String::from_utf8(stdout).unwrap();
        let reply_line = out.lines().next().unwrap();
        let v: Value = serde_json::from_str(reply_line).unwrap();
        assert_eq!(v["id"], serde_json::json!(0));
        assert_eq!(v["error"]["code"], serde_json::json!(-32601));

        // The bytes buffered past `initialize` are still readable off the reader.
        let mut rest = String::new();
        reader.read_to_string(&mut rest).await.unwrap();
        assert!(rest.contains("extra-buffered-past-initialize"), "{rest}");
    }

    #[tokio::test]
    async fn drain_preinit_probes_returns_none_on_eof_before_initialize() {
        let input: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"server/discover\"}\n";
        let mut reader = BufReader::new(input);
        let mut stdout = Vec::new();
        let got = drain_preinit_probes(&mut reader, &mut stdout)
            .await
            .unwrap();
        assert!(got.is_none());
        // But the probe still got answered before EOF.
        let out = String::from_utf8(stdout).unwrap();
        assert!(out.contains("\"code\":-32601"), "{out}");
    }

    #[tokio::test]
    async fn prefixed_reader_yields_prefix_then_inner() {
        let inner: &[u8] = b"world";
        let mut reader = Prefixed {
            prefix: b"hello ".to_vec(),
            inner,
        };
        let mut out = String::new();
        reader.read_to_string(&mut out).await.unwrap();
        assert_eq!(out, "hello world");
    }

    #[tokio::test]
    async fn relay_loop_intercepts_server_discover_without_forwarding_to_daemon() {
        let (bridge_side, mut daemon_side) = tokio::io::duplex(4096);
        let (mut stdin_feed, stdin_read) = tokio::io::duplex(4096);
        stdin_feed
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"server/discover\"}\n")
            .await
            .unwrap();
        drop(stdin_feed);

        let mut relay = RelayState::default();
        let mut stdin = BufReader::new(stdin_read).lines();
        let mut stdout = Vec::new();
        let mut session = Session::new(bridge_side);

        let (end, _) = relay_loop(&mut relay, &mut stdin, &mut stdout, &mut session)
            .await
            .unwrap();
        assert_eq!(end, SessionEnd::StdinClosed);

        // The client saw the -32601 reply.
        let out = String::from_utf8(stdout).unwrap();
        assert!(out.contains("\"code\":-32601"), "{out}");
        assert!(out.contains("server/discover"), "{out}");

        // The daemon never saw the probe.
        drop(session);
        let mut buf = Vec::new();
        daemon_side.read_to_end(&mut buf).await.unwrap();
        assert!(buf.is_empty(), "the probe leaked to the daemon: {buf:?}");
        assert!(
            relay.init_request.is_none() && !relay.outstanding.contains_key("0"),
            "the probe polluted relay state"
        );
    }

    #[test]
    fn initialize_error_reply_builds_a_response_for_the_initialize_id() {
        // A numeric id is echoed exactly and the message carries the error text.
        let reply = initialize_error_reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            "another Crystalline instance owns the index (pid 42)",
        )
        .expect("an initialize carrying an id is answered");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["id"], serde_json::json!(1));
        assert_eq!(v["error"]["code"], serde_json::json!(-32000));
        let message = v["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("crystalline mcp failed to start"),
            "{message}"
        );
        assert!(
            message.contains("another Crystalline instance owns the index (pid 42)"),
            "{message}"
        );

        // A string id round-trips verbatim.
        let reply = initialize_error_reply(
            r#"{"jsonrpc":"2.0","id":"init-7","method":"initialize"}"#,
            "boom",
        )
        .unwrap();
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["id"], serde_json::json!("init-7"));
    }

    #[test]
    fn initialize_error_reply_skips_lines_without_an_id() {
        // Malformed JSON has no id to answer.
        assert!(initialize_error_reply("not json at all", "boom").is_none());
        // A notification (no id field) has nothing to reply to.
        assert!(
            initialize_error_reply(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                "boom",
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn primed_reader_hands_relay_loop_the_initialize_then_follow_up() {
        // The primed reader carries the drained `initialize` line as its prefix
        // and whatever the client sent next in its inner reader. Fed through
        // `BufReader::new(reader).lines()` exactly as `pump_stdio` does, the
        // relay must forward `initialize` first and the follow-up next, proving
        // the handoff preserves ordering with no special replay.
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let follow: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
        let mut prefix = Vec::with_capacity(init.len() + 1);
        prefix.extend_from_slice(init.as_bytes());
        prefix.push(b'\n');
        let primed = Prefixed {
            prefix,
            inner: follow,
        };
        let mut stdin = BufReader::new(primed).lines();

        let (bridge_side, daemon_side) = tokio::io::duplex(4096);
        let daemon = tokio::spawn(async move {
            let (read, _write) = tokio::io::split(daemon_side);
            let mut lines = BufReader::new(read).lines();
            let first = lines.next_line().await.unwrap().unwrap();
            let second = lines.next_line().await.unwrap().unwrap();
            (first, second)
        });

        let mut relay = RelayState::default();
        let mut stdout = Vec::new();
        let mut session = Session::new(bridge_side);
        let (end, _) = relay_loop(&mut relay, &mut stdin, &mut stdout, &mut session)
            .await
            .unwrap();
        assert_eq!(end, SessionEnd::StdinClosed);

        let (first, second) = daemon.await.unwrap();
        assert!(first.contains("\"initialize\""), "{first}");
        assert!(second.contains("tools/list"), "{second}");
        // The relay recorded the primed initialize as the handshake, so a later
        // daemon restart can replay it.
        assert!(relay.init_request.as_ref().unwrap().contains("initialize"));
        assert_eq!(relay.init_id, Some(serde_json::json!(1)));
    }
}
