//! Client-side entry points: the `crystalline mcp` stdio bridge, the CLI data
//! commands (over the socket when a daemon runs, else in-process) and the ctl
//! client used by the CLI operator commands.

use std::collections::HashSet;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use interprocess::local_socket::tokio::Stream as IpcStream;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader, ReadBuf};

use crate::daemon::{open_store, resolve_db};
use crate::engine::{CLI_ACTOR, Engine, ShareActor, open_standalone};
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

/// Build a JSON-RPC error response to the session opener in `opener_line`,
/// answering it with `err_text` when the embedded startup fails before rmcp
/// ever takes over stdio. Without this the client would see nothing but a
/// closed pipe; the TypeScript SDK's negotiation window reads a mid-handshake
/// close as an unrecoverable network error and never retries, so a readable
/// failure is strictly better than dying silently. Returns `None` when the line
/// carries no id to answer (malformed JSON, or a notification), in which case
/// the caller skips the write and just propagates the error.
///
/// This is the terminal path: it runs only when the embedded stack **and** the
/// degraded status server both failed, and the process exits non-zero straight
/// after. If the opener was a `server/discover` probe, this error tells the
/// client we are a legacy server - true only in the sense that we are about to
/// stop being any kind of server at all.
fn initialize_error_reply(opener_line: &str, err_text: &str) -> Option<String> {
    let msg: Value = serde_json::from_str(opener_line).ok()?;
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
/// delegating to `inner`. [`run_mcp`] builds one after reading the session
/// opener to re-front the line it already took off stdin, together with
/// anything the underlying `BufReader` had buffered past it, so the serving
/// path (the daemon relay or the embedded rmcp server) sees that opener as its
/// first line with no special replay.
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

/// Read the line that opens the session off `reader`: an `initialize` request
/// in the legacy era, a `server/discover` probe in the modern one, or whatever
/// else the client sent first. Returns it so the caller can prepend it to a
/// wrapped reader before handing that to the daemon relay or to
/// `rmcp::serve_server`, or `None` on stdin EOF before anything arrived.
///
/// Every line is returned exactly as it came, so rmcp sees and judges it the
/// way it always has - a `server/discover` probe included, which rmcp answers
/// itself since 3.1.4. **Nothing is answered here** - the function takes no
/// writer, which is the type-level version of that statement.
///
/// This used to drain and answer probes in a loop, which is why it could return
/// only an `initialize`. Forwarding replaces answering, so exactly one line is
/// read and the loop is gone.
async fn read_session_opener<R>(reader: &mut BufReader<R>) -> std::io::Result<Option<String>>
where
    R: AsyncRead + Unpin,
{
    let mut buf = String::new();
    let n = reader.read_line(&mut buf).await?;
    if n == 0 {
        return Ok(None);
    }
    // read_line keeps the trailing newline; strip it to canonicalize.
    Ok(Some(buf.trim_end_matches(['\r', '\n']).to_string()))
}

/// Whether the harness that spawned this process already has the shipped
/// skills on disk and onboards itself at session start, from the `--harness`
/// argument its MCP registration carries plus this machine's install receipt.
///
/// Neither input is the connecting client: one is the deployment's own
/// configuration, the other is machine state. That is what makes the answer
/// legal as a gate on a list endpoint (SEP-2567), where reading the client's
/// `initialize` name for the same purpose was not.
///
/// **The receipt is read here rather than persisted at install time**, so it
/// stays fresh: uninstalling the skills, or reinstalling with `--skip-hooks`,
/// flips the answer at the next spawn with nothing to re-register. And it is
/// read in this process rather than in the daemon because the state directory
/// can be overridden by the environment, which this process inherits from the
/// harness and the daemon does not.
///
/// Every uncertain input resolves to `false`, meaning serve: no argument (a
/// registration predating the flag), an id this binary does not know, a
/// missing or corrupt receipt, or a harness the receipt does not list with
/// hooks. An over-served client pays duplicated context; an under-served one
/// loses onboarding it cannot rediscover.
fn resolve_harness_onboarded(harness: Option<&str>) -> bool {
    let Ok(receipt) = crystalline_core::provision::install_receipt_path() else {
        return false;
    };
    resolve_harness_onboarded_at(harness, &receipt)
}

/// [`resolve_harness_onboarded`] against an explicit receipt path, so the
/// decision table can be tested without touching this machine's real state
/// directory or its environment.
fn resolve_harness_onboarded_at(harness: Option<&str>, receipt: &Path) -> bool {
    let Some(id) = harness else {
        return false;
    };
    // Parsed permissively rather than as a clap value enum: a downgraded
    // binary meeting a newer registration, or a harness id rolled out ahead of
    // the binaries, must degrade to serving rather than exit with a usage
    // error, which the harness would see as a server that will not start.
    let Some(kind) = crystalline_core::HarnessKind::from_id(id) else {
        tracing::warn!(
            harness = %id,
            "unknown --harness value; serving the full skill surface. \
             Upgrade crystalline if this harness is newer than this binary."
        );
        return false;
    };
    let onboarded = crystalline_core::harnesses_with_hooks(receipt).contains(&kind);
    tracing::debug!(
        harness = %id,
        onboarded,
        "resolved the skill surface for this session from the install receipt"
    );
    onboarded
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
    harness: Option<&str>,
) -> anyhow::Result<()> {
    // Log to stderr from the start, for both modes: the relay's takeover and
    // reconnect notices and any embedded startup failure must be visible in the
    // harness's server log, not swallowed.
    init_tracing();

    // Resolve the onboarding answer once, here, before anything connects: this
    // process serves exactly one MCP client for its whole life, so a value
    // fixed at startup is invariant for every connection it can ever serve,
    // which is what SEP-2567 needs of a gate that stays on a listing. It is
    // re-sent on each daemon reconnect and never re-derived daemon-side.
    let harness_onboarded = resolve_harness_onboarded(harness);

    // Read the client's first line concurrently with daemon acquisition, so a
    // cold daemon spawns while the client is still composing its opener rather
    // than afterwards.
    //
    // **What this no longer buys, since it used to buy more.** While the bridge
    // answered the `server/discover` probe itself the answer went out in
    // milliseconds regardless of how slow the daemon was. Forwarding means the
    // probe is answered by whoever ends up serving, so its reply now waits out
    // daemon acquisition or embedded startup. That matters
    // because branch three of the stdio rule is "any other error, **or does not
    // respond within a reasonable timeout**: the server is legacy", and that
    // verdict is cacheable for the server process's lifetime. Measured on this
    // machine, cold: see the transcripts in the migration ledger. A transport
    // close during the window is still the unsurvivable case, and nothing here
    // closes stdio.
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(tokio::io::stdin());
    let (opened, daemon) = if embedded {
        (read_session_opener(&mut reader).await, None)
    } else {
        // `read_only` is forwarded only to a daemon this call spawns; attaching
        // to an already-running daemon uses that daemon's own mode.
        let (opened, daemon) = tokio::join!(
            read_session_opener(&mut reader),
            ensure_daemon(true, db, config_path, read_only),
        );
        (opened, Some(daemon))
    };

    // Stdin EOF before the client said anything means it left mid-window; a
    // daemon this call spawned staying up is fine by design, so exit cleanly.
    // A real read error propagates.
    let Some(opener_line) = opened? else {
        return Ok(());
    };
    // Re-front the opener (plus anything buffered past it) so the serving path
    // reads it as its first stdin line with no special replay.
    let primed = prime_reader(&opener_line, reader);

    // A daemon is up: relay through it. A failed `mcp` handshake falls through
    // to the embedded path rather than propagating, so an unreachable daemon
    // still yields a working in-process server instead of a mid-window close.
    if let Some(daemon) = daemon {
        match daemon {
            Ok(conn) => match conn.into_mcp(harness_onboarded).await {
                Ok(stream) => {
                    return pump_stdio(
                        stream,
                        primed,
                        db,
                        config_path,
                        read_only,
                        harness_onboarded,
                    )
                    .await;
                }
                Err(e) => tracing::warn!("daemon MCP handshake failed ({e}); running embedded"),
            },
            Err(e) => tracing::warn!("no daemon available ({e}); running embedded"),
        }
    }

    // Embedded path: the explicit flag, or a daemon that could not be reached.
    // A terminal startup failure (lock held, config or store error) happens
    // before rmcp answers anything. Rather than closing stdio - which the
    // negotiation window reads as an unrecoverable network error - serve a
    // degraded status server: `initialize` succeeds with per-case instructions
    // and a `status` tool explaining the failure and its fix, so the model can
    // relay it instead of the session going dark. Every fallible step lives in
    // `build_embedded`, ahead of the primed reader, so on failure the reader is
    // still intact for the stub. Only if the stub itself fails to serve do we
    // fall back to the old `-32000` reply and a non-zero exit (stderr carries
    // the chain for the Desktop log).
    match build_embedded(db, config_path, read_only, harness_onboarded).await {
        Ok(stack) => run_embedded_stdio(stack, primed).await,
        Err(e) => {
            tracing::error!(
                "crystalline mcp cannot start ({e:#}); serving a degraded status server"
            );
            let status = crate::stub::StubStatus::gather(format!("{e:#}"));
            match serve_degraded_stub(status, primed).await {
                Ok(()) => Ok(()),
                Err(stub_err) => {
                    tracing::warn!("degraded status server failed ({stub_err:#})");
                    if let Some(reply) = initialize_error_reply(&opener_line, &format!("{e:#}")) {
                        let _ = stdout.write_all(reply.as_bytes()).await;
                        let _ = stdout.write_all(b"\n").await;
                        let _ = stdout.flush().await;
                    }
                    Err(e)
                }
            }
        }
    }
}

/// Re-front the session opener ahead of stdin: the prefix is the line, a
/// newline and whatever the `BufReader` buffered past it, the inner is the raw
/// stdin. See [`Prefixed`].
fn prime_reader(
    opener_line: &str,
    reader: BufReader<tokio::io::Stdin>,
) -> Prefixed<tokio::io::Stdin> {
    let buffered = reader.buffer().to_vec();
    let inner = reader.into_inner();
    let mut prefix = Vec::with_capacity(opener_line.len() + 1 + buffered.len());
    prefix.extend_from_slice(opener_line.as_bytes());
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
            // A cancellation settles its request as far as the client is
            // concerned: the daemon may never answer it, so the entry is
            // dropped here rather than waiting for a response that will not
            // come. Without this a cancelled-and-unanswered request would keep
            // its entry for the whole relay lifetime, and a restart would send
            // an error answer for a request the client already abandoned.
            (Some("notifications/cancelled"), _) => {
                if let Some(id) = msg.get("params").and_then(|p| p.get("requestId")) {
                    self.outstanding.remove(&id.to_string());
                }
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
                    // Every client line is forwarded verbatim, a
                    // `server/discover` probe included: the daemon's rmcp
                    // classifies and answers it. From here on it is an ordinary
                    // request: recorded, forwarded, settled by its response.
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
    harness_onboarded: bool,
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
            // Re-sent on every reconnect: the daemon builds a fresh
            // `McpServer` per accepted socket (`daemon.rs`), so a restart
            // would otherwise flip an onboarded harness back to served in the
            // middle of a live session.
            let Ok(stream) = conn.into_mcp(harness_onboarded).await else {
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

/// The built-and-ready in-process stack: the tool server plus the index
/// ownership it holds for the session. Every fallible startup step already ran
/// in [`build_embedded`], so [`run_embedded_stdio`] only has to serve and can
/// take the primed reader with no risk of failing after it is consumed.
struct EmbeddedStack {
    server: McpServer,
    ownership: crate::instance::Ownership,
}

/// Build the full in-process stack: take the index lock (refused if held),
/// load the config and overlay, open the store, construct the engine, launch
/// the background sync and embed workers and prime the routing cache. Every
/// step that can fail lives here, ahead of the reader ever being touched, so a
/// terminal failure leaves [`run_mcp`]'s primed `initialize` reader intact for
/// the degraded stub to serve. The effective read-only mode is the explicit
/// flag or `service.read_only`.
async fn build_embedded(
    db: Option<&Path>,
    config_path: Option<&Path>,
    read_only: bool,
    harness_onboarded: bool,
) -> anyhow::Result<EmbeddedStack> {
    let ownership = acquire_ownership()
        .map_err(|e| anyhow::anyhow!("cannot run an embedded MCP server: {e}"))?;
    let loaded = overlay::load(config_path)?;
    let read_only = read_only || loaded.effective.read_only();
    let db_path = resolve_db(db)?;
    let store = open_store(&loaded.effective, Some(&db_path)).await?;
    // A channel the embed worker (spawned below) listens on: connecting an
    // origin mid-session schedules its embedding pass here instead of running
    // it inline, so the connect request returns without waiting on the model.
    let (embed_tx, embed_rx) = tokio::sync::mpsc::unbounded_channel();
    // The provider is built in the background so the stdio session is ready and
    // text search works before any model download completes. There is no
    // watcher task in this mode, so the resolved config path only helps a domain
    // added mid-session resolve for data operations, not for picking up external
    // file changes. The engine holds the file config and the overlay apart, so
    // its effective config drives reads while persistence stays env-free.
    let engine = Arc::new(
        Engine::new(store, loaded.file.clone(), None, Some(loaded.path.clone()))
            .with_embed_channel(embed_tx)
            .with_read_only(read_only)
            .with_env_overlay(loaded.overlay.clone()),
    );
    tokio::spawn(crate::engine::run_embed_worker(engine.clone(), embed_rx));

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

    Ok(EmbeddedStack {
        server: McpServer::new(engine).with_onboarded_harness(harness_onboarded),
        ownership,
    })
}

/// Serve the built stack over stdio until the client closes stdin. `reader` is
/// the primed stdin [`run_mcp`] already fronted with the drained `initialize`,
/// so this path never touches the pre-init probe itself; it hands the reader
/// straight to `rmcp::serve_server`. This runs only after [`build_embedded`]
/// succeeded, so nothing here consumes the reader on a startup failure.
async fn run_embedded_stdio<R>(stack: EmbeddedStack, reader: R) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let stdout = tokio::io::stdout();
    let running = rmcp::serve_server(stack.server, (reader, stdout)).await?;
    let _ = running.waiting().await;
    drop(stack.ownership);
    Ok(())
}

/// Serve the degraded status server over stdio: a stand-in that answers
/// `initialize` with explanatory instructions and a single `status` tool when
/// the embedded stack could not start (see [`crate::stub`]). It holds no lock
/// and opens no store, so serving it cannot itself fail for the reasons that
/// forced the degradation; it ends when the client closes stdin, exactly like
/// [`run_embedded_stdio`].
async fn serve_degraded_stub<R>(status: crate::stub::StubStatus, reader: R) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let running = rmcp::serve_server(
        crate::stub::DegradedServer::new(status),
        (reader, tokio::io::stdout()),
    )
    .await?;
    let _ = running.waiting().await;
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
    use serde_json::json;
    // A running daemon owns the index, so route the data verb to its shared
    // engine over the ctl `tool` command, which always answers in JSON. The
    // MCP round trip is avoided on purpose: the list-shaped tools answer an MCP
    // call in the session's response format (TOON by default), which neither
    // the `--json` byte contract nor the human renderers can consume.
    if use_daemon(db, config_path)
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "tool", "tool": tool, "args": args.clone(),
        }))
        .await?
    {
        return Ok(data);
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

/// Rename or merge a tag across the engrams that carry it: over the daemon when
/// one owns the index, else against a directly opened store.
#[allow(clippy::too_many_arguments)]
pub async fn tags_retag(
    old: &str,
    new: &str,
    domain: Option<&str>,
    merge: bool,
    dry_run: bool,
    no_alias: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "retag", "old": old, "new": new,
            "domain": domain, "merge": merge, "dry_run": dry_run, "no_alias": no_alias,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine
        .retag(old, new, domain, merge, dry_run, !no_alias)
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
///
/// The standalone path shares as [`ShareActor::Owner`], as the three write
/// verbs below do: a CLI invocation has no account behind it, so the machine
/// owner is who it is, not a placeholder for one.
pub async fn origin_share(
    domain: &str,
    title: Option<&str>,
    description: Option<&str>,
    proposal: Option<u64>,
    files: Option<&[String]>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "origin_share", "domain": domain,
            "title": title, "description": description, "proposal": proposal,
            "files": files,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine
        .origin_share(
            domain,
            title,
            description,
            proposal,
            files,
            ShareActor::Owner,
        )
        .await?)
}

/// Withdraw a share proposal for one team domain: over the daemon when one
/// owns the index, else against a directly opened store.
pub async fn origin_withdraw(
    domain: &str,
    proposal: Option<u64>,
    revert: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "origin_withdraw", "domain": domain,
            "proposal": proposal, "revert": revert,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone(loaded, &db_path, false).await?;
    Ok(engine
        .origin_withdraw(domain, proposal, revert, ShareActor::Owner)
        .await?)
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
    Ok(engine
        .origin_resolve(domain, path, keep, content, ShareActor::Owner)
        .await?)
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

/// Apply, inspect or record a decision for domain-declared artifact
/// provisioning (the skills, commands, agents and MCP servers a domain's
/// `## Provisioning` section ships into a coding harness's own config
/// directory): over the daemon when one is running and no explicit
/// `--config` override was given, else against the config file and the
/// provisioning receipt directly, mirroring `configure`'s daemon-first
/// discipline (provisioning never opens the index either, so there is no
/// `--db` to gate on). `action` is one of `status`, `allow`, `deny` or
/// `apply`; `domain` is required for `allow`/`deny` and ignored otherwise.
/// `allow`, `deny` and `apply` refuse on a read-only effective config,
/// matching `Engine::provision`'s own guard; `status` is always answered.
///
/// The harnesses reconciled into always come from this machine's install
/// receipt (`crystalline install`'s own record of onboarded harnesses),
/// never a caller-supplied list.
pub async fn provision(
    action: &str,
    domain: Option<&str>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    // An explicit --config override names the exact file to operate on, the
    // same reasoning `configure` documents: a running daemon serves ITS OWN
    // default config, which may be a different one entirely.
    if config_path.is_none()
        && let Some(data) = ctl_if_running(json!({
            "v": 1, "cmd": "provision", "action": action, "domain": domain,
        }))
        .await?
    {
        return Ok(data);
    }

    let loaded = overlay::load(config_path)?;
    let install_receipt = crystalline_core::provision::install_receipt_path()
        .map_err(|e| anyhow::anyhow!("could not resolve the install receipt path: {e}"))?;
    let harnesses = crystalline_core::provision::installed_harnesses(&install_receipt);
    let receipt_path = crystalline_core::provision::receipt_path()
        .map_err(|e| anyhow::anyhow!("could not resolve the provisioning receipt path: {e}"))?;
    // Named so the pending block never nags about a domain whose decision can
    // never be recorded - see `crystalline_core::provision::apply`'s doc
    // comment.
    let env_domains: HashSet<&str> = loaded
        .overlay
        .env_domains()
        .map(|(name, _)| name.as_str())
        .collect();

    match action {
        "status" => {
            let report = crystalline_core::provision::status(
                &loaded.effective,
                &receipt_path,
                &harnesses,
                &env_domains,
            )?;
            Ok(crate::engine::status_report_json(&report))
        }
        "allow" | "deny" => {
            if loaded.effective.read_only() {
                anyhow::bail!("{}", crate::engine::EngineError::ReadOnly);
            }
            let name =
                domain.ok_or_else(|| anyhow::anyhow!("provision {action} requires a domain"))?;
            // An env-defined domain's source of truth is its variable: the
            // overlay re-inserts a fresh entry (provision unset) on every
            // read, so a decision written to the file would be silently
            // discarded. Checked before the registered-domain lookup so a
            // shadowed and an env-only name both get the env message.
            if let Some(env) = loaded.overlay.env_domain(name) {
                anyhow::bail!(
                    "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                    env.var
                );
            }
            let mut file = loaded.file.clone();
            crate::engine::set_domain_provision_decision(&mut file, name, action == "allow")?;
            save_file(&loaded.path, &file)?;
            let effective = loaded.overlay.apply(&file);
            let mut mcp = crate::harness_cli::SystemMcpRunner;
            let report = crystalline_core::provision::apply(
                &effective,
                &receipt_path,
                &harnesses,
                &mut mcp,
                &env_domains,
            )?;
            Ok(crate::engine::apply_report_json(&report))
        }
        "apply" => {
            if loaded.effective.read_only() {
                anyhow::bail!("{}", crate::engine::EngineError::ReadOnly);
            }
            let mut mcp = crate::harness_cli::SystemMcpRunner;
            let report = crystalline_core::provision::apply(
                &loaded.effective,
                &receipt_path,
                &harnesses,
                &mut mcp,
                &env_domains,
            )?;
            Ok(crate::engine::apply_report_json(&report))
        }
        other => {
            anyhow::bail!(
                "unknown provision action '{other}'; expected status, allow, deny or apply"
            )
        }
    }
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

/// Dispatch a tool to the shared engine by name, decoding `args` into the
/// tool's params type. Reachable from [`crate::control`] so the ctl `tool`
/// command dispatches a daemon-attached CLI data verb through the exact same
/// name-to-method mapping the standalone path uses.
pub(crate) async fn dispatch_engine(
    engine: &Engine,
    tool: &str,
    args: Value,
) -> anyhow::Result<Value> {
    let v = match tool {
        // A CLI-driven write is not an MCP client, so it identifies itself as
        // the CLI process; `identity.actor` still wins when it is set.
        "write_engram" => {
            engine
                .write_engram_as(&decode::<WriteParams>(args)?, Some(CLI_ACTOR))
                .await?
        }
        "read_engram" => engine.read_engram(&decode::<ReadParams>(args)?).await?,
        "edit_engram" => {
            engine
                .edit_engram_as(&decode::<EditParams>(args)?, Some(CLI_ACTOR))
                .await?
        }
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
        "vocabulary" => {
            engine
                .vocabulary(&decode::<VocabularyParams>(args)?)
                .await?
        }
        // Matched through the constant rather than a literal so the CLI verb,
        // this arm and the router name can never drift apart.
        t if t == crate::EVOLVE_TOOL_NAME => {
            engine
                .evolve_engrams(&decode::<EvolveParams>(args)?)
                .await?
        }
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

    // --- the resolved skill-surface answer ------------------------------------
    //
    // The whole decision table for `--harness`, against a receipt written by
    // hand in the shape `crystalline install` writes. Every uncertain row
    // resolves to false, meaning "serve the surface": an over-served client
    // pays some duplicated context, an under-served one loses onboarding it
    // has no way to rediscover.

    /// A receipt in the shape `crystalline install` writes, recording
    /// `harness` with its session hooks wired or skipped.
    fn write_receipt(path: &std::path::Path, harness: &str, hooks: bool) {
        std::fs::write(
            path,
            format!(
                r#"{{"format":1,"installs":[{{"harness":"{harness}","scope":"user","version":"0.13.0","parts":{{"mcp":true,"hooks":{hooks},"skills":true}},"skills":[]}}]}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn the_resolved_answer_is_the_named_harness_plus_this_machines_receipt() {
        let tmp = tempfile::tempdir().unwrap();
        let receipt = tmp.path().join("installs.json");

        // The row the feature exists for.
        write_receipt(&receipt, "claude-code", true);
        assert!(resolve_harness_onboarded_at(Some("claude-code"), &receipt));

        // Per harness, not per machine: the receipt knows claude-code, and a
        // codex-spawned process still gets the surface. This is the whole
        // reason the argument exists rather than the receipt alone.
        assert!(!resolve_harness_onboarded_at(Some("codex"), &receipt));

        // Hooks skipped: nothing onboards that session, so the block and the
        // surface both have to come from here.
        write_receipt(&receipt, "claude-code", false);
        assert!(!resolve_harness_onboarded_at(Some("claude-code"), &receipt));
    }

    #[test]
    fn every_uncertain_input_resolves_to_serving() {
        let tmp = tempfile::tempdir().unwrap();
        let receipt = tmp.path().join("installs.json");
        write_receipt(&receipt, "claude-code", true);

        // No argument at all: a registration written before `--harness`
        // existed. This is the pre-existing-install case and it must behave
        // exactly as it did before the flag.
        assert!(!resolve_harness_onboarded_at(None, &receipt));

        // An id this binary does not know: a downgrade meeting a newer
        // registration, or a harness rolled out ahead of the binaries. Warns
        // and serves; it must never be a usage error, which would leave the
        // harness with a server that will not start.
        assert!(!resolve_harness_onboarded_at(
            Some("nextgen-harness"),
            &receipt
        ));
        assert!(!resolve_harness_onboarded_at(Some(""), &receipt));

        // A missing receipt and a corrupt one both read as "nothing onboarded"
        // through the tolerant shallow reader, never as an error.
        let missing = tmp.path().join("nope.json");
        assert!(!resolve_harness_onboarded_at(Some("claude-code"), &missing));
        std::fs::write(&receipt, "not json at all").unwrap();
        assert!(!resolve_harness_onboarded_at(Some("claude-code"), &receipt));
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

    #[test]
    fn relay_state_drops_a_cancelled_request() {
        let mut relay = RelayState::default();
        relay.note_client_line(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#);
        relay.note_client_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#);
        relay.note_client_line(r#"{"jsonrpc":"2.0","id":"a2","method":"tools/call","params":{}}"#);
        assert!(relay.outstanding.contains_key("1"));

        relay.note_client_line(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1,"reason":"user"}}"#,
        );
        assert!(
            !relay.outstanding.contains_key("1"),
            "a cancelled request no longer waits for a response"
        );
        // A string id cancels the same way, and an unknown or malformed
        // cancellation leaves the rest of the map alone.
        relay.note_client_line(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"a2"}}"#,
        );
        assert!(!relay.outstanding.contains_key("\"a2\""));
        relay.note_client_line(r#"{"jsonrpc":"2.0","method":"notifications/cancelled"}"#);
        relay.note_client_line(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":42}}"#,
        );
        assert!(
            relay.outstanding.contains_key("0"),
            "the handshake id survives an unrelated cancellation"
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

    // --- the discover probe: forwarded, and answered by rmcp -----------------
    //
    // Nothing on this side classifies or rewrites a probe any more. rmcp 3.1.4
    // fixed rust-sdk #1157 (PR #1160), so its stdio init loop now answers a
    // first request that is not `initialize` instead of returning without ever
    // calling `transport.send`, and the normalization bridge that existed only
    // to work around that silence is gone.

    /// A bare probe as the TypeScript SDK's auto-negotiation window sends it:
    /// no `_meta` at all, so neither SEP-2575 required key is present.
    const BARE_PROBE: &str = r#"{"jsonrpc":"2.0","id":0,"method":"server/discover"}"#;

    /// The `_meta` a conforming probe carries: both `DRAFT_REQUIRED_KEYS`
    /// (`rmcp::model::RequestMetaObject`).
    fn complete_probe(id: Value, version: &str) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "server/discover",
            "params": { "_meta": {
                "io.modelcontextprotocol/protocolVersion": version,
                "io.modelcontextprotocol/clientCapabilities": {},
            }},
        })
        .to_string()
    }

    /// The pin that replaces `a_normalized_probe_reaches_our_discover_handler_
    /// and_a_bare_one_is_dropped`, over a real transport with no bridge in
    /// between:
    ///
    /// 1. A bare probe as the session's **first** message is answered by rmcp
    ///    itself rather than dropped into a closed pipe. The answer is a
    ///    JSON-RPC `-32602` naming the two missing keys, not a
    ///    `DiscoverResult`: #1160 fixed the silence, it did not make a
    ///    malformed request valid. That response is what made the
    ///    normalization removable - a client reads an error instead of a hang,
    ///    and the specification's stdio rule has a branch for an error and none
    ///    for a wedge.
    /// 2. The conforming shape - the one every client that actually probes
    ///    sends - reaches `McpServer::discover` and comes back as a
    ///    `DiscoverResult` carrying our routing block and exactly the revisions
    ///    we advertise.
    /// 3. The cost of a probe, pinned rather than argued: answering one arms
    ///    `peer.require_request_metadata()`, which is one-way, so a client that
    ///    ignores "do not fall back to `initialize`" is served the handshake and
    ///    then refused `-32602` on its first real call.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_bare_discover_probe_is_answered_by_rmcp_and_a_complete_one_returns_our_discover_result()
     {
        let tmp = tempfile::tempdir().unwrap();
        let build_engine = || {
            let config_path = tmp.path().join("config.yaml");
            async move {
                let store = crystalline_index::TursoStore::open_in_memory()
                    .await
                    .unwrap();
                Arc::new(Engine::new(
                    Arc::new(tokio::sync::Mutex::new(store)),
                    crystalline_core::config::GlobalConfig::default(),
                    None,
                    Some(config_path),
                ))
            }
        };

        // 1. The bare probe: rmcp answers it. Before #1160 the init loop
        //    returned without a `transport.send` and the client saw a closed
        //    pipe, which is the wedge the bridge existed to avoid.
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let server = tokio::spawn({
            let engine = build_engine().await;
            async move { rmcp::serve_server(McpServer::new(engine), server_io).await }
        });
        let (read, mut write) = tokio::io::split(client_io);
        let mut lines = BufReader::new(read).lines();
        write
            .write_all(format!("{BARE_PROBE}\n").as_bytes())
            .await
            .unwrap();
        write.flush().await.unwrap();
        let answer = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
            .await
            .expect("rmcp answers rather than hanging")
            .unwrap()
            .expect("a message rather than a closed pipe (upstream rust-sdk #1157/#1160)");
        let v: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(v["id"], serde_json::json!(0), "{answer}");
        assert_eq!(
            v["error"]["code"],
            serde_json::json!(-32602),
            "a malformed probe is invalid params, not silence: {answer}"
        );
        assert!(
            v["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("io.modelcontextprotocol/protocolVersion")
                    && m.contains("io.modelcontextprotocol/clientCapabilities")),
            "and it names both missing keys: {answer}"
        );
        drop(write);
        let _ = server.await;

        // 2. The conforming probe: our handler runs.
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let server = tokio::spawn({
            let engine = build_engine().await;
            async move { rmcp::serve_server(McpServer::new(engine), server_io).await }
        });
        let (read, mut write) = tokio::io::split(client_io);
        let mut lines = BufReader::new(read).lines();
        let probe = complete_probe(
            serde_json::json!(0),
            crate::mcp::newest_served_protocol_version().as_str(),
        );
        write
            .write_all(format!("{probe}\n").as_bytes())
            .await
            .unwrap();
        write.flush().await.unwrap();
        let answer = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
            .await
            .expect("the probe is answered")
            .unwrap()
            .expect("with a message rather than a closed pipe");
        let v: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(v["id"], serde_json::json!(0), "{answer}");
        assert!(v["error"].is_null(), "not an error of any kind: {answer}");
        assert!(
            v["result"]["instructions"]
                .as_str()
                .is_some_and(|s| s.starts_with("CRYSTALLINE KNOWLEDGE ROUTING")),
            "the DiscoverResult carries our routing block: {answer}"
        );
        let advertised: Vec<String> = crate::mcp::SERVED_PROTOCOL_VERSIONS
            .iter()
            .map(|v| v.as_str().to_string())
            .collect();
        assert_eq!(
            v["result"]["supportedVersions"],
            serde_json::json!(advertised),
            "and it advertises exactly what we serve: {answer}"
        );

        // 3. What a probe costs, pinned rather than argued. Answering one arms
        //    `peer.require_request_metadata()`, which is one-way. A client that
        //    ignores the specification's "do not fall back to `initialize`" and
        //    hands us a legacy handshake anyway is served it - `initialize` is
        //    exempt - and then dies on its first real call.
        write
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\
                   \"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\
                   \"clientInfo\":{\"name\":\"dual-era\",\"version\":\"0\"}}}\n",
            )
            .await
            .unwrap();
        write
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n")
            .await
            .unwrap();
        write.flush().await.unwrap();
        // Both answers are in flight at once, so index them by id rather than
        // by arrival: rmcp dispatches concurrently and the refusal is quicker
        // than the handshake.
        let mut answers = std::collections::HashMap::new();
        for _ in 0..2 {
            let v: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            answers.insert(v["id"].clone(), v);
        }
        let handshake = &answers[&serde_json::json!(1)];
        assert!(
            handshake["result"]["protocolVersion"].is_string(),
            "the legacy handshake still succeeds: {handshake}"
        );
        let refused = &answers[&serde_json::json!(2)];
        assert_eq!(
            refused["error"]["code"],
            serde_json::json!(-32602),
            "{refused}"
        );
        assert_eq!(
            refused["error"]["message"],
            "request _meta is missing or has malformed required fields: \
             io.modelcontextprotocol/protocolVersion, \
             io.modelcontextprotocol/clientCapabilities",
            "the latch is what refuses it, not our own guard: {refused}"
        );

        drop(write);
        let _ = server.await;
    }

    #[tokio::test]
    async fn the_session_opener_is_returned_verbatim_and_nothing_is_answered_here() {
        let input: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"server/discover\"}\n\
            extra-buffered-past-the-opener\n";
        let mut reader = BufReader::new(input);
        let opener = read_session_opener(&mut reader)
            .await
            .unwrap()
            .expect("the probe opens the session instead of being consumed");
        assert_eq!(opener, BARE_PROBE, "the opener read rewrites nothing");

        // The bytes buffered past it are still readable off the reader, so the
        // primed handoff is unchanged.
        let mut rest = String::new();
        reader.read_to_string(&mut rest).await.unwrap();
        assert!(rest.contains("extra-buffered-past-the-opener"), "{rest}");

        // An `initialize` opener comes back verbatim too: the legacy era is
        // untouched by any of this.
        let init: &[u8] =
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n";
        let mut reader = BufReader::new(init);
        let opener = read_session_opener(&mut reader).await.unwrap().unwrap();
        assert_eq!(
            opener,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#
        );
    }

    #[tokio::test]
    async fn read_session_opener_returns_none_on_eof() {
        let input: &[u8] = b"";
        let mut reader = BufReader::new(input);
        assert!(read_session_opener(&mut reader).await.unwrap().is_none());
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

    /// A `server/discover` probe on the relay path reaches the daemon exactly
    /// as the client sent it: the bridge classifies nothing and rewrites
    /// nothing, so the daemon's rmcp is what decides the probe's fate.
    #[tokio::test]
    async fn relay_loop_forwards_a_discover_probe_to_the_daemon_verbatim() {
        let (bridge_side, daemon_side) = tokio::io::duplex(4096);
        let (mut stdin_feed, stdin_read) = tokio::io::duplex(4096);
        stdin_feed
            .write_all(format!("{BARE_PROBE}\n").as_bytes())
            .await
            .unwrap();

        // The daemon end of the relay: read the one line the bridge sends,
        // answer it the way rmcp's inline branch does, then close. stdin stays
        // open (the writer half is held), so the loop ends on the daemon's EOF
        // and the answer cannot race a client close.
        let daemon = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(daemon_side);
            let mut lines = BufReader::new(read).lines();
            let seen = lines.next_line().await.unwrap();
            write
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"instructions\":\"x\"}}\n")
                .await
                .unwrap();
            write.flush().await.unwrap();
            drop(write);
            drop(lines);
            seen
        });

        let mut relay = RelayState::default();
        let mut stdin = BufReader::new(stdin_read).lines();
        let mut stdout = Vec::new();
        let mut session = Session::new(bridge_side);

        // Bounded: a bridge that answered the probe itself would leave both
        // sides waiting forever (stdin has no more lines, the daemon never
        // speaks), so that failure is a timeout with a message, not a hang.
        let looped = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            relay_loop(&mut relay, &mut stdin, &mut stdout, &mut session),
        )
        .await
        .expect("the probe was forwarded to the daemon, which answered and closed");
        let (end, _) = looped.unwrap();
        assert_eq!(end, SessionEnd::SocketClosed);
        drop(stdin_feed);

        // Byte for byte what the client wrote: no `_meta` is injected on the
        // way through, and no key order is disturbed.
        let seen = daemon
            .await
            .unwrap()
            .expect("the probe reaches the daemon rather than being answered here");
        assert_eq!(seen, BARE_PROBE, "{seen}");

        // Nothing was answered here: what stdout carries is the daemon's reply.
        let out = String::from_utf8(stdout).unwrap();
        assert!(out.contains("instructions"), "{out}");

        // The probe is an ordinary outstanding request while it is in flight,
        // and the daemon's answer settles it.
        assert!(relay.init_request.is_none(), "a probe is not a handshake");
        assert!(
            relay.outstanding.is_empty(),
            "the probe was tracked like any other request and the daemon's \
             answer settled it: {:?}",
            relay.outstanding
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
        // `BufReader::new(reader).lines()` and relayed from a plain
        // `RelayState::default()`, both exactly as `pump_stdio` does, the relay
        // must forward `initialize` first and the follow-up next, proving the
        // handoff preserves ordering with no special replay.
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
