//! Client-side entry points: the `crystalline mcp` stdio bridge, the CLI data
//! commands (over the socket when a daemon runs, else in-process) and the ctl
//! client used by the CLI operator commands.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use interprocess::local_socket::tokio::Stream as IpcStream;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};

use crystalline_remote::ops::DiscardTarget;

use crate::daemon::{Departure, SHUTDOWN_DEADLINE, open_store, resolve_db};
use crate::engine::{CLI_ACTOR, Engine, ShareActor, open_standalone};
use crate::instance::{
    Connection, HarnessGate, acquire_ownership, acquire_standalone_ownership, ensure_daemon,
    try_attach,
};
use crate::line_limit::{BoundedLines, Incoming, STDIO_LINE_LIMIT, read_on_thread, serve_guarded};
use crate::mcp::McpServer;
use crate::overlay;
use crate::params::*;
use crate::scope::Scope;

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

/// Read the line that opens the session off `stdin`: an `initialize` request
/// in the legacy era, a `server/discover` probe in the modern one, or whatever
/// else the client sent first. Returns it so the caller can hand it back with
/// [`BoundedLines::replay`] before giving the reader to the daemon relay or to
/// [`serve_guarded`], or `None` on stdin EOF before anything arrived.
///
/// The read is bounded like every other one ([`crate::line_limit`]): an opener
/// over the limit comes back as [`Incoming::Oversized`] and is refused by
/// whichever path serves the session, exactly like a later line.
///
/// Every line is returned exactly as it came, so rmcp sees and judges it the
/// way it always has - a `server/discover` probe included, which rmcp answers
/// itself since 3.1.4. **Nothing is answered here** - the function takes no
/// writer, which is the type-level version of that statement.
///
/// This used to drain and answer probes in a loop, which is why it could return
/// only an `initialize`. Forwarding replaces answering, so exactly one line is
/// read and the loop is gone.
async fn read_session_opener<R>(stdin: &mut BoundedLines<R>) -> std::io::Result<Option<Incoming>>
where
    R: AsyncRead + Unpin,
{
    stdin.next_line().await
}

/// What the harness that spawned this process already does for itself, from
/// the `--harness` argument its MCP registration carries plus this machine's
/// install receipt: whether its session hook is installed (the routing block
/// probably arrives at session start) and whether its onboarding is verified
/// (the hook is installed **and** the harness's profile says a live check
/// confirmed that it loads the hook's block and the shipped skills as files).
/// See [`HarnessGate`].
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
/// Every uncertain input resolves to [`HarnessGate::default`], meaning serve
/// everything with the full instructions: no argument (a registration
/// predating the flag), an id this binary does not know, a missing or corrupt
/// receipt, or a harness the receipt does not list with hooks. An over-served
/// client pays duplicated context; an under-served one loses onboarding it
/// cannot rediscover.
fn resolve_harness_gate(harness: Option<&str>) -> HarnessGate {
    let Ok(receipt) = crystalline_core::provision::install_receipt_path() else {
        return HarnessGate::default();
    };
    resolve_harness_gate_at(harness, &receipt)
}

/// [`resolve_harness_gate`] against an explicit receipt path, so the decision
/// table can be tested without touching this machine's real state directory
/// or its environment.
fn resolve_harness_gate_at(harness: Option<&str>, receipt: &Path) -> HarnessGate {
    let Some(id) = harness else {
        return HarnessGate::default();
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
        return HarnessGate::default();
    };
    let hook_installed = crystalline_core::harnesses_with_hooks(receipt).contains(&kind);
    let gate = HarnessGate {
        hook_installed,
        onboarding_verified: hook_installed && kind.profile().onboarding_verified,
        unattended: false,
        run_domain: None,
    };
    tracing::debug!(
        harness = %id,
        hook_installed = gate.hook_installed,
        onboarding_verified = gate.onboarding_verified,
        "resolved the skill surface for this session from the install receipt"
    );
    gate
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
    unattended: bool,
    run_domain: Option<&str>,
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
    let harness_gate = HarnessGate {
        unattended: unattended || run_domain.is_some(),
        run_domain: run_domain.map(str::to_string),
        ..resolve_harness_gate(harness)
    };

    // Inside an app package (Claude Desktop's MSIX package on Windows) this
    // process is only a bridge to the daemon outside: every file it wrote
    // under AppData would land in the package's private copy. So it never
    // serves the embedded stack, not even when asked to.
    let packaged = crate::runs_in::PackageContext::here().is_packaged();
    if embedded && packaged {
        tracing::warn!(
            "--embedded is ignored inside an app package: this process only bridges to the daemon outside"
        );
    }
    let embedded = embedded && !packaged;

    // A newer Crystalline under an older Claude Desktop extension offers the
    // extension update, off the session's path (`desktop_update`). Outside
    // a package this process does it on a thread of its own; inside one it
    // asks the daemon once that is attached, below.
    let older_extension = crate::desktop_update::older_extension();
    if let Some(launcher) = &older_extension
        && !packaged
    {
        crate::desktop_update::offer_on_thread(launcher.clone());
    }

    // Read the client's first line concurrently with daemon acquisition, so a
    // cold daemon spawns while the client is still composing its opener rather
    // than afterwards.
    //
    // **What this no longer buys, since it used to buy more.** While the bridge
    // answered the `server/discover` probe itself the answer went out in
    // milliseconds regardless of how slow the daemon was. Forwarding means the
    // probe is answered by whoever ends up serving, so its reply now waits out
    // daemon acquisition or embedded startup. That matters because branch
    // three of the stdio rule is "any other error, **or does not
    // respond within a reasonable timeout**: the server is legacy", and that
    // verdict is cacheable for the server process's lifetime. Measured on this
    // machine, cold: see the transcripts in the migration ledger. A transport
    // close during the window is still the unsurvivable case, and nothing here
    // closes stdio.
    let mut stdout = tokio::io::stdout();
    // Read on a thread of its own rather than tokio's blocking pool, so a
    // session rmcp ends while stdin stays open does not keep the process
    // alive at the runtime's drop (see `read_on_thread`).
    let mut stdin = BoundedLines::new(read_on_thread(std::io::stdin())?, STDIO_LINE_LIMIT);
    let (opened, daemon) = if embedded {
        (read_session_opener(&mut stdin).await, None)
    } else {
        // `read_only` is forwarded only to a daemon this call spawns; attaching
        // to an already-running daemon uses that daemon's own mode. The `mcp`
        // handshake goes out the moment the daemon is attached, ahead of the
        // client's opener: the daemon counts a session from that line, and a
        // daemon with a bounded life (`--exit-when-idle`) must see this stub
        // as a client while the client is still composing its first request.
        let (opened, daemon) = tokio::join!(read_session_opener(&mut stdin), async {
            let conn = ensure_daemon(true, db, config_path, read_only).await?;
            conn.into_mcp(harness_gate.clone())
                .await
                .map_err(|e| anyhow::Error::new(e).context("daemon MCP handshake failed"))
        });
        (opened, Some(daemon))
    };

    // Stdin EOF before the client said anything means it left mid-window; a
    // daemon this call spawned staying up is fine by design, so exit cleanly.
    // A real read error propagates.
    let Some(opener) = opened? else {
        return Ok(());
    };
    // Kept for the last-resort error reply below; an oversized opener has no
    // text left to answer from, and the serving path refuses it anyway.
    let opener_line = match &opener {
        Incoming::Line(line) => Some(String::from_utf8_lossy(line).into_owned()),
        Incoming::Oversized(_) => None,
    };
    // Hand the opener back so the serving path reads it as its first line.
    stdin.replay(opener);

    // A daemon is up: relay through it. A failed attach or handshake falls
    // through to the embedded path rather than propagating, so an unreachable
    // daemon still yields a working in-process server instead of a mid-window
    // close.
    if let Some(daemon) = daemon {
        match daemon {
            Ok(stream) => {
                let update_offered =
                    crate::desktop_update::offer_for_bridge(older_extension.as_deref(), packaged)
                        .await;
                return pump_stdio(
                    stream,
                    stdin,
                    db,
                    config_path,
                    read_only,
                    harness_gate,
                    update_offered,
                )
                .await;
            }
            Err(e) if packaged => {
                let bridge = crate::daemon_task::BridgeFailure::of(&e);
                tracing::warn!(
                    "this process runs inside an app package and only bridges to the daemon: {bridge}"
                );
                let status = crate::stub::StubStatus::for_bridge(format!("{e:#}"), bridge);
                return serve_degraded_stub(status, stdin).await;
            }
            Err(e) => tracing::warn!("no daemon available ({e:#}); running embedded"),
        }
    }

    // Embedded path: the explicit flag, or a daemon that could not be reached.
    // A terminal startup failure (lock held, config or store error) happens
    // before rmcp answers anything. Rather than closing stdio - which the
    // negotiation window reads as an unrecoverable network error - serve a
    // degraded status server: `initialize` succeeds with per-case instructions
    // and a `status` tool explaining the failure and its fix, so the model can
    // relay it instead of the session going dark. Every fallible step lives in
    // `build_embedded`, ahead of the bounded reader, so on failure the reader is
    // still intact for the stub. Only if the stub itself fails to serve do we
    // fall back to the old `-32000` reply and a non-zero exit (stderr carries
    // the chain for the Desktop log).
    match build_embedded(db, config_path, read_only, harness_gate).await {
        Ok(stack) => run_embedded_stdio(stack, stdin).await,
        Err(e) => {
            tracing::error!(
                "crystalline mcp cannot start ({e:#}); serving a degraded status server"
            );
            let status = crate::stub::StubStatus::gather(format!("{e:#}")).await;
            match serve_degraded_stub(status, stdin).await {
                Ok(()) => Ok(()),
                Err(stub_err) => {
                    tracing::warn!("degraded status server failed ({stub_err:#})");
                    if let Some(reply) = opener_line
                        .as_deref()
                        .and_then(|line| initialize_error_reply(line, &format!("{e:#}")))
                    {
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

/// The oldest Claude Desktop extension launcher this binary works with, as a
/// numeric triple. Raised only when a launcher fix matters: the launcher hands
/// every message to the installed binary, so users update Crystalline, not the
/// extension, and this is how the binary says when the extension itself needs
/// an update. After a raise the user runs `crystalline install claude-desktop`,
/// which builds a launcher of this binary's own version.
pub const MINIMUM_LAUNCHER: (u32, u32, u32) = (0, 24, 0);

/// The line the bridge adds to the onboarding instructions when the launcher
/// that started it is older than [`MINIMUM_LAUNCHER`].
const LAUNCHER_NOTE: &str = "The Claude Desktop extension that starts Crystalline is older than this Crystalline supports. Ask the user to run `crystalline install claude-desktop` in a terminal, then confirm the install dialog in Claude Desktop.";

/// The launcher note when this Crystalline's extension update was offered
/// in Claude Desktop and Claude Desktop took it (`desktop_update`).
const LAUNCHER_OFFERED_NOTE: &str = "The Claude Desktop extension that starts Crystalline is older than this Crystalline supports. Crystalline offered the extension update in Claude Desktop. Ask the user to confirm the update dialog there. If no dialog shows, ask them to run `crystalline install claude-desktop` in a terminal.";

/// The numeric triple of a launcher version such as `0.25.1` or
/// `0.26.0-rc.1`, read like the launcher's own `parseVersion`: a pre-release
/// suffix is dropped. `None` for anything else.
fn launcher_version(text: &str) -> Option<(u32, u32, u32)> {
    let text = text.trim();
    let core = text.split_once('-').map_or(text, |(core, _)| core);
    let mut parts = core.split('.');
    let mut next = || -> Option<u32> {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let triple = (next()?, next()?, next()?);
    parts.next().is_none().then_some(triple)
}

/// Whether the launcher that started this bridge is older than
/// [`MINIMUM_LAUNCHER`]. Only the standard Claude Desktop extension (channel
/// `desktop`) has a launcher; any other channel is never too old. On that
/// channel an absent value is a launcher from 0.24.0 to 0.25.0, which did not
/// pass its version yet, judged as 0.24.0. A value that cannot be read is
/// never too old: the bridge does not nag on a guess.
fn launcher_too_old(env_value: Option<&str>, channel: Option<&str>) -> bool {
    if channel != Some("desktop") {
        return false;
    }
    let version = match env_value {
        None => (0, 24, 0),
        Some(text) => match launcher_version(text) {
            Some(version) => version,
            None => return false,
        },
    };
    version < MINIMUM_LAUNCHER
}

/// The launcher note for a launcher of `env_value` on `channel`: none when
/// it is not too old, else [`LAUNCHER_OFFERED_NOTE`] when `update_offered`
/// and [`LAUNCHER_NOTE`] otherwise.
fn launcher_note(
    env_value: Option<&str>,
    channel: Option<&str>,
    update_offered: bool,
) -> Option<&'static str> {
    if !launcher_too_old(env_value, channel) {
        return None;
    }
    Some(if update_offered {
        LAUNCHER_OFFERED_NOTE
    } else {
        LAUNCHER_NOTE
    })
}

/// The note to add to this session's onboarding instructions, read once from
/// this process's environment when the bridge starts.
fn launcher_note_from_env(update_offered: bool) -> Option<&'static str> {
    let version = std::env::var(overlay::LAUNCHER_VERSION_ENV).ok();
    let channel = std::env::var(overlay::CHANNEL_ENV).ok();
    launcher_note(version.as_deref(), channel.as_deref(), update_offered)
}

/// How long the bridge waits for the daemon's `status` behind the split
/// note.
const SPLIT_NOTE_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// The split note for this session, when Claude Desktop's package keeps a
/// second state with domains: the daemon this bridge attached to is asked
/// (passively, a second connection) which domains it serves, so the note
/// names only the private ones it does not. When it cannot be asked the
/// note says what may be wrong.
async fn split_note_here() -> Option<String> {
    let states = crate::desktop_state::scan_here();
    if !states.iter().any(|s| s.domains > 0) {
        return None;
    }
    // Bounded: a daemon that accepts and never answers must not hold up
    // the session's onboarding; no answer reads as "could not be asked".
    let registered: Option<Vec<String>> = tokio::time::timeout(
        SPLIT_NOTE_WAIT,
        ctl_if_running_passive(serde_json::json!({ "v": 1, "cmd": "status" })),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .flatten()
    .and_then(|status| serde_json::from_value(status.get("registered")?.clone()).ok());
    crate::desktop_state::split_note(&states, registered.as_deref())
}

/// What a command says when the daemon it reached is older than the
/// command and does not know `cmd`: an "unknown ctl command" answer turned
/// into a plain sentence. Any other error is kept as it is.
pub fn older_daemon_words(error: anyhow::Error, cmd: &str) -> anyhow::Error {
    if format!("{error:#}").contains(&format!("unknown ctl command '{cmd}'")) {
        return anyhow::anyhow!(
            "the running Crystalline daemon is older than this command and cannot do this yet. Restart it from a terminal outside Claude Desktop: crystalline ctl shutdown, then crystalline status"
        );
    }
    error
}

/// The note this bridge adds to the session's onboarding instructions: the
/// launcher note and, when Claude Desktop's app package keeps a second
/// state that registers domains, the split note
/// ([`crate::desktop_state::split_note`]), a blank line apart. `None`
/// when there is neither.
fn onboarding_note(launcher: Option<&str>, split: Option<String>) -> Option<String> {
    let parts: Vec<String> = launcher
        .map(str::to_string)
        .into_iter()
        .chain(split)
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// `line` with `note` added after a blank line at the end of
/// `result.instructions`, where both an `initialize` answer and a
/// `server/discover` answer carry the onboarding text. `None` when the line is
/// not JSON or has no instructions text, so it is forwarded unchanged.
fn with_launcher_note(line: &str, note: &str) -> Option<String> {
    let mut msg: Value = serde_json::from_str(line).ok()?;
    let Some(Value::String(text)) = msg.get_mut("result")?.get_mut("instructions") else {
        return None;
    };
    text.push_str("\n\n");
    text.push_str(note);
    Some(msg.to_string())
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
/// It also holds the launcher note, when there is one, and the ids of the
/// requests whose answers carry the onboarding instructions it goes into.
#[derive(Default)]
struct RelayState {
    init_request: Option<String>,
    init_id: Option<Value>,
    initialized_note: Option<String>,
    outstanding: std::collections::HashMap<String, Value>,
    /// The line to add to the onboarding instructions, set when the launcher
    /// that started this bridge is too old (see [`launcher_too_old`]).
    /// With the split note after it when there is one (see
    /// [`onboarding_note`]).
    launcher_note: Option<String>,
    /// The ids of `initialize` and `server/discover` requests still waiting
    /// for their answer, keyed like `outstanding`.
    onboarding: HashSet<String>,
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
                self.onboarding.insert(id.to_string());
            }
            (Some("server/discover"), Some(id)) => {
                self.outstanding.insert(id.to_string(), id.clone());
                self.onboarding.insert(id.to_string());
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
                    self.onboarding.remove(&id.to_string());
                }
            }
            (Some(_), Some(id)) => {
                self.outstanding.insert(id.to_string(), id.clone());
            }
            _ => {}
        }
    }

    /// Record a daemon-to-client line: a response (an id without a method)
    /// settles its outstanding request. Returns the line to write instead
    /// when it answers an `initialize` or `server/discover` request and a
    /// launcher note is held; every other line is forwarded verbatim.
    fn note_server_line(&mut self, line: &str) -> Option<String> {
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return None;
        };
        if msg.get("method").is_none()
            && let Some(id) = msg.get("id")
        {
            let key = id.to_string();
            self.outstanding.remove(&key);
            if self.onboarding.remove(&key)
                && let Some(note) = &self.launcher_note
            {
                return with_launcher_note(line, note);
            }
        }
        None
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
///
/// A client line over the stdio limit ([`crate::line_limit`]) is answered
/// here, on stdout, and never reaches the daemon: the daemon's rmcp would drop
/// it without a word and the client would wait forever. It is not recorded in
/// [`RelayState`] either, since no daemon will ever answer it, so a restart
/// does not answer it a second time.
async fn relay_loop<In, Out, S>(
    relay: &mut RelayState,
    stdin: &mut BoundedLines<In>,
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
                Some(Incoming::Oversized(over)) => {
                    if let Some(reply) = over.refusal() {
                        stdout.write_all(reply.as_bytes()).await?;
                        stdout.write_all(b"\n").await?;
                        stdout.flush().await?;
                    }
                }
                Some(Incoming::Line(line)) => {
                    // What tokio's `Lines` did with a line that is not UTF-8,
                    // which this reader replaced: the relay ends with an error.
                    let line = String::from_utf8(line).map_err(|e| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
                    })?;
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
                    let line = relay.note_server_line(&line).unwrap_or(line);
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
    // Every onboarding request is now either the replayed `initialize`, whose
    // answer was swallowed above, or orphaned and answered with an error
    // below: none is waiting for the note any more.
    relay.onboarding.clear();
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
    mut stdin: BoundedLines<R>,
    db: Option<&Path>,
    config_path: Option<&Path>,
    read_only: bool,
    harness_gate: HarnessGate,
    update_offered: bool,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut relay = RelayState {
        launcher_note: onboarding_note(
            launcher_note_from_env(update_offered),
            split_note_here().await,
        ),
        ..RelayState::default()
    };
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
            let Ok(stream) = conn.into_mcp(harness_gate.clone()).await else {
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
/// take the bounded reader with no risk of failing after it is consumed.
struct EmbeddedStack {
    server: McpServer,
    /// The engine the server runs on, kept here too so the end of the session
    /// can take its store before the process leaves.
    engine: Arc<Engine>,
    ownership: crate::instance::Ownership,
    /// Stops the source poller when the session ends.
    stop_sources: tokio::sync::watch::Sender<bool>,
}

/// Build the full in-process stack: take the index lock (refused if held),
/// load the config and overlay, open the store, construct the engine, launch
/// the background sync and embed workers and prime the routing cache. Every
/// step that can fail lives here, ahead of the reader ever being touched, so a
/// terminal failure leaves [`run_mcp`]'s bounded reader, opener and all, intact for
/// the degraded stub to serve. The effective read-only mode is the explicit
/// flag or `service.read_only`.
async fn build_embedded(
    db: Option<&Path>,
    config_path: Option<&Path>,
    read_only: bool,
    harness_gate: HarnessGate,
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
    let mut engine = Engine::new(store, loaded.file.clone(), None, Some(loaded.path.clone()))
        .with_embed_channel(embed_tx)
        .with_read_only(read_only)
        .with_env_overlay(loaded.overlay.clone())
        .with_machine_owner_lookup(machine_owner_for_engine());
    // Told where the state directory is, as the daemon and the standalone
    // opener are; see `open_standalone`.
    if let Ok(state) = crystalline_core::config::state_dir() {
        engine = engine.with_state_dir(state);
    }
    // On a database other instances share, this stack is this machine's
    // instance, as the daemon it stands in for would be: it claims the file
    // domains it walks and serves, heartbeats those claims and releases them
    // when the session ends. It holds the state directory a daemon holds, so
    // this machine's daemon is not running beside it.
    let shared_database = engine.store().lock().await.shares_database();
    if shared_database {
        let name = engine.config().instance_name();
        engine = engine
            .with_instance_id(crystalline_core::config::read_or_create_instance_id()?)
            .with_instance_label(name);
    }
    let engine = Arc::new(engine);
    // Who reads which domain, known before the stack answers anything: a
    // Reflection page this stack writes reaches every reader of its domain
    // and the backup branch, whether or not anybody reads it over HTTP here.
    install_domain_access(&engine).await;
    // Env-defined team domains that should be private are closed here too,
    // without a word: the daemon's start says it once.
    engine.apply_env_readers_quietly().await?;
    tokio::spawn(crate::engine::run_embed_worker(engine.clone(), embed_rx));
    // A rename a stopped daemon left half done is finished before the first
    // sync and before the routing cache reads a name, as the daemon does.
    engine.finish_leftover_rename().await;
    // This machine's sources, once that rename is done (its recovery reads
    // names the mount table could hide). An explicit --db or --config serves
    // this machine's own index only.
    //
    // With them, the daemon's source poller, run here because this stack is
    // the one serving the session: a `connect` or `disconnect` made while it
    // runs, and a hand edit of sources.json or config.yaml, show within the
    // look, and the routing caches refresh on the poll as they do in a
    // daemon. Its own task, so the start never waits on a server.
    let (stop_sources, stopped) = tokio::sync::watch::channel(false);
    if shared_database {
        let (e, rx, secs) = (
            engine.clone(),
            stop_sources.subscribe(),
            engine.heartbeat_secs(),
        );
        tokio::spawn(async move { crate::daemon::run_heartbeat(e, secs, rx).await });
    }
    if use_daemon(db, config_path) {
        crate::route::install_sources(&engine);
        let e = engine.clone();
        tokio::spawn(async move {
            crate::daemon::run_source_poller(
                e,
                crate::daemon::SOURCE_POLL,
                crate::daemon::SOURCE_LOOK,
                stopped,
            )
            .await;
        });
    }

    let bg = engine.clone();
    let bg_config = loaded.effective.clone();
    tokio::spawn(async move {
        let _ = bg.sync(None).await;
        // As at the daemon's startup, once the first sync has returned.
        bg.settle_after_initial_sync().await;
        if let Some(provider) = crate::engine::build_provider(&bg_config).await {
            bg.set_provider(provider);
            // Schedule on the worker spawned just above rather than embedding
            // inline beside it, the same shape the daemon's startup task uses:
            // two passes over one backlog shadow each other instead of sharing
            // it. The inline fallback is for an engine with no worker wired,
            // which this stack never is.
            if !bg.request_embed()
                && let Err(err) = bg.embed_pending().await
            {
                tracing::warn!("initial embed failed: {err}");
            }
        }
    });

    // Prime the routing cache before serving so the very first `initialize`
    // renders complete instructions, never racing the background sync above.
    engine.refresh_routing_cache().await;

    Ok(EmbeddedStack {
        server: McpServer::new(engine.clone()).with_harness_gate(harness_gate),
        engine,
        ownership,
        stop_sources,
    })
}

/// Serve the built stack over stdio until the client closes stdin. `stdin` is
/// the bounded reader [`run_mcp`] handed the session opener back to, so this
/// path never touches the opener itself; it hands the reader to
/// [`serve_guarded`], which answers a line over the limit and feeds rmcp the
/// rest. This runs only after [`build_embedded`] succeeded, so nothing here
/// consumes the reader on a startup failure.
///
/// A served session ends the process the way a daemon's shutdown does, in
/// `std::process::exit` through [`Departure`], and never by returning: this
/// stack owns the index exactly like a daemon, runs the same background model
/// download in a blocking task, and a return would leave the process waiting
/// for that task in the runtime's drop with its lock file already removed and
/// the index still held.
async fn run_embedded_stdio<R>(stack: EmbeddedStack, stdin: BoundedLines<R>) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let session = serve_guarded(stack.server, stdin, tokio::io::stdout());
    // The host locks this stack claimed go back when the session ends, as a
    // daemon's do at its shutdown, so another instance can take the domains
    // at once; a session that failed to start gives them back too.
    serve_then_release(&stack.engine, &stack.stop_sources, session).await?;
    tracing::info!(
        "stopping (the client closed the session); exiting within {}s",
        SHUTDOWN_DEADLINE.as_secs()
    );
    let departure = Departure::begin(stack.ownership, SHUTDOWN_DEADLINE);
    // Held to the exit, as in the daemon: an operation already inside the
    // store finishes and no new one starts.
    departure.step("waiting for the store");
    let store = stack.engine.store();
    let _held = store.lock().await;
    // Then a sign-in refresh a source call started, which runs as a task of
    // its own, is waited for with what is left of the deadline, so a token
    // pair the server rotated is saved.
    departure.settle_sign_ins().await;
    departure.finish(None)
}

/// Run the embedded stack's session, then stop its background tasks and
/// give back every host lock it claimed, whether the session ended or
/// failed: a lock left behind would make every other instance wait out the
/// stale threshold. The session's own outcome is the answer.
async fn serve_then_release(
    engine: &Engine,
    stop_sources: &tokio::sync::watch::Sender<bool>,
    session: impl std::future::Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<()> {
    let served = session.await;
    let _ = stop_sources.send(true);
    engine.release_hosts().await;
    served
}

/// Serve the degraded status server over stdio: a stand-in that answers
/// `initialize` with explanatory instructions and a single `status` tool when
/// the embedded stack could not start (see [`crate::stub`]). It holds no lock
/// and opens no store, so serving it cannot itself fail for the reasons that
/// forced the degradation; it ends when the client closes stdin, exactly like
/// [`run_embedded_stdio`], and bounds its lines the same way.
async fn serve_degraded_stub<R>(
    status: crate::stub::StubStatus,
    stdin: BoundedLines<R>,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    serve_guarded(
        crate::stub::DegradedServer::new(status),
        stdin,
        tokio::io::stdout(),
    )
    .await
}

/// Open the index in-process for a standalone command, or say why it could
/// not be opened.
///
/// Every verb below falls through to here when no daemon answered, and one of
/// the states that reaches it is a daemon that is alive, owns the index and
/// answers nothing on its socket. What a person met there was the backend's
/// raw lock text, with no daemon named and no command to run; a colleague's
/// agent spent a session on the wrong diagnosis reading exactly that.
///
/// The sentence is [`crate::instance::index_unreachable_words`], the one the
/// CLI's own opener already uses (`cmd::reach_index`), so the same state reads
/// the same way whichever crate happened to open the index. The backend's own
/// words are kept, last, inside it: nothing is hidden, it just stops leading.
///
/// An override that opens the index successfully here gets
/// [`crate::instance::BYPASS_NOTE`] on stderr too, the same sentence
/// `status` prints for a bypass: a read verb's empty answer from a
/// deliberately different index must never be mistaken for a genuine miss.
/// A failed open says nothing besides the composed error below, which
/// already names the override as the reason nothing was asked of the
/// daemon - printing the plain note first there would claim a direct read
/// that the very next line reports could not happen.
async fn open_standalone_reporting(
    loaded: overlay::LoadedConfig,
    db_path: &Path,
    want_embeddings: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Engine> {
    open_standalone_finishing(loaded, db_path, want_embeddings, db, config_path)
        .await
        .map(|(engine, _)| engine)
}

/// [`open_standalone_reporting`], also answering the report of a rename an
/// earlier run left half done that the opener finished, so a command that
/// was about to send that same rename again answers with it instead.
async fn open_standalone_finishing(
    loaded: overlay::LoadedConfig,
    db_path: &Path,
    want_embeddings: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<(Engine, Option<Value>)> {
    open_standalone_noting(loaded, db_path, want_embeddings, db, config_path, true).await
}

/// [`open_standalone_finishing`], printing the bypass note only when `note`
/// is set: a caller that opens a copy of an index on purpose has nothing to
/// tell the person about the daemon.
async fn open_standalone_noting(
    loaded: overlay::LoadedConfig,
    db_path: &Path,
    want_embeddings: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
    note: bool,
) -> anyhow::Result<(Engine, Option<Value>)> {
    // Postgres has no local file, so naming one in a failure would point at a
    // path nothing lives at.
    let location = if loaded.effective.database().backend
        == crystalline_core::config::DatabaseBackend::Turso
    {
        db_path.display().to_string()
    } else {
        "the configured database".to_string()
    };
    // Inside an app package no local engine is opened, not even for a read
    // or with an override outside AppData: see `ctl_routed`.
    if crate::runs_in::PackageContext::here().is_packaged() {
        anyhow::bail!(packaged_local_engine_refusal());
    }
    // An explicit --db or --config never asked the daemon in the first place,
    // which changes the remedy rather than the diagnosis.
    let bypassed = !use_daemon(db, config_path);
    let engine = open_standalone(loaded, db_path, want_embeddings)
        .await
        .map_err(|e| {
            anyhow::anyhow!(crate::instance::index_unreachable_words(
                &location,
                &format!("{e:#}"),
                bypassed
            ))
        })?
        .with_machine_owner_lookup(machine_owner_for_engine());
    if bypassed && note {
        eprintln!("Daemon: {}", crate::instance::BYPASS_NOTE);
    }
    let finished = finish_leftover_rename_owned(&engine).await;
    Ok((engine, finished))
}

/// Finish a rename an earlier run left half done, from a standalone opener,
/// only while this process holds the ownership of the state directory a
/// daemon holds for its whole life ([`acquire_ownership`]): the journal lives
/// in that directory, and two processes running it at once would move the
/// same state folders and write the same configuration, on either backend.
/// The engine itself then runs a journal only against the index it belongs
/// to. No journal: nothing is locked. The lock held by someone else: the
/// rename is left to that process and a log line says so. The report of a
/// rename it finished is answered.
async fn finish_leftover_rename_owned(engine: &Engine) -> Option<Value> {
    if !engine.has_leftover_rename() {
        return None;
    }
    match acquire_standalone_ownership("crystalline command finishing an earlier domain rename") {
        Ok(ownership) => {
            engine.set_holds_state_dir(true);
            let finished = engine.finish_leftover_rename().await;
            engine.set_holds_state_dir(false);
            drop(ownership);
            finished
        }
        Err(_) => {
            tracing::warn!(
                "a domain rename an earlier run left half done is not finished by this command: \
                 another Crystalline process (the daemon) holds this machine's state directory, \
                 and it finishes the rename itself; if it does not, stop it with `crystalline \
                 ctl shutdown` and run this again"
            );
            None
        }
    }
}

/// The local name `spelling` means, once an engine is open: a canonical name
/// or a machine-local alias resolves to its registration's own local name,
/// the same resolution `control::localized_request` gives the daemon path
/// for every command `control::DOMAIN_REFERENCE_COMMANDS` lists, before the
/// request ever reaches the shared engine. A name nothing answers to comes
/// back unchanged, so the caller's own not-found wording still fires.
///
/// The standalone branch below has no pre-pass of its own - only the daemon
/// path gets one for free - so a function here that opens its own engine and
/// then calls one that expects a registration's own local name outright
/// (unlike a data verb's `DomainArgs` params struct, which localizes itself
/// through `Engine::localized_for`) does this once, right after opening it.
async fn localize_standalone(engine: &Engine, spelling: &str) -> String {
    engine
        .localize_visible(spelling, &std::collections::HashSet::new())
        .await
}

/// [`localize_standalone`] for the handful of verbs (`provision` among them)
/// that never open an engine at all: built straight from the loaded
/// configuration through [`overlay::LoadedConfig::name_table`],
/// the same inputs [`Engine::name_table_now`] itself reads for a file domain
/// (a virtual domain's own declared name lives in the database, out of reach
/// here, and is not needed by anything that calls this: none of them ever
/// names a virtual domain by its canonical spelling, so an empty
/// `virtual_names` map is always correct here).
fn localize_in_config(loaded: &overlay::LoadedConfig, spelling: &str) -> String {
    loaded
        .name_table()
        .resolve(spelling)
        .unwrap_or(spelling)
        .to_string()
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
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "tool", "tool": tool, "args": args.clone(),
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let want_embeddings = matches!(tool, "search_engrams");
    let engine =
        open_standalone_reporting(loaded, &db_path, want_embeddings, db, config_path).await?;
    // An explicit --db or --config keeps every verb on this machine's own
    // index, as it keeps it off the daemon; otherwise the sources are this
    // machine's, read from disk for this one command. The opener above has
    // already finished any rename an earlier run left half done.
    let engine = Arc::new(engine);
    // A tool run here can write a Reflection page, which reaches every
    // reader of its domain: this command knows them as a daemon would.
    install_domain_access(&engine).await;
    engine.apply_env_readers_quietly().await?;
    if use_daemon(db, config_path) {
        crate::route::install_sources(&engine);
    }
    let answer = crate::route::run_tool_routed(
        &engine,
        tool,
        args,
        &crystalline_remote::ForwardedAgent::default(),
        None,
    )
    .await;
    // A write into a file domain of a shared database claimed its host lock
    // as this machine; a command sends no heartbeat, so it gives it back.
    engine.release_command_locks().await;
    // A refresh a source call started runs as a task of its own: this
    // process waits for it before it leaves, so a token pair the server
    // rotated is saved.
    crystalline_remote::settle_refreshes(crystalline_remote::ONE_DOMAIN_LIMIT).await;
    answer
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
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "scaffold_manifest", "domain": domain, "markdown": markdown,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let domain = localize_standalone(&engine, domain).await;
    Ok(engine.scaffold_virtual_manifest(&domain, markdown).await?)
}

/// Import engram files into a virtual domain: over the daemon when one owns the
/// index, else against a directly opened store. `complete` completes notes
/// that lack a field a reader needs; a move of Crystalline's own engrams
/// passes `false` so nothing it moves is changed.
pub async fn domain_import(
    domain: &str,
    src: &Path,
    overwrite: bool,
    dry_run: bool,
    complete: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "domain_import", "domain": domain,
            "path": src.display().to_string(), "overwrite": overwrite, "dry_run": dry_run,
            "complete": complete,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let domain = localize_standalone(&engine, domain).await;
    Ok(engine
        .import_domain_with(&domain, src, overwrite, dry_run, complete)
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
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "retag", "old": old, "new": new,
            "domain": domain, "merge": merge, "dry_run": dry_run, "no_alias": no_alias,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let domain = match domain {
        Some(d) => Some(localize_standalone(&engine, d).await),
        None => None,
    };
    Ok(engine
        .retag(old, new, domain.as_deref(), merge, dry_run, !no_alias)
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
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "domain_export", "domain": domain,
            "path": dest.display().to_string(), "force": force, "dry_run": dry_run,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let domain = localize_standalone(&engine, domain).await;
    Ok(engine.export_domain(&domain, dest, force, dry_run).await?)
}

/// [`domain_export`] from an index file that is not this machine's own (a
/// copy taken on purpose), opened directly and never through the daemon,
/// without the bypass note an explicit `--db` prints.
pub async fn domain_export_from_copy(
    domain: &str,
    dest: &Path,
    db: &Path,
    config_path: &Path,
) -> anyhow::Result<Value> {
    let loaded = overlay::load(Some(config_path))?;
    let (engine, _) =
        open_standalone_noting(loaded, db, false, Some(db), Some(config_path), false).await?;
    let domain = localize_standalone(&engine, domain).await;
    Ok(engine.export_domain(&domain, dest, true, false).await?)
}

/// What the index still holds for domains nobody registers any more, and,
/// unless `dry_run`, the removal of those rows: over the daemon when one owns
/// the index, else against a directly opened store.
///
/// The grace period is never applied on this route. It exists to wait for a
/// person to say the domain is really gone, and a person running `crystalline
/// doctor` has said it - so an index inherited from a version that stranded
/// its rows clears on first contact rather than a week after it. The daemon's
/// own timer is the path that waits.
///
/// Unlike the orphan *file* rows doctor also reports, this works with a daemon
/// running: the removal goes through the daemon that owns the index rather
/// than needing it stopped.
pub async fn collect_orphaned_domains(
    dry_run: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "collect_orphaned_domains", "dry_run": dry_run,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    Ok(engine.collect_orphaned_domains(None, dry_run).await?)
}

/// The domain name findings `crystalline doctor` shows
/// ([`Engine::name_report`]), after, when `fix` is set, finishing every
/// rename a MANIFEST made that nobody answered
/// ([`Engine::finish_rename_questions`]) and then respelling every link that
/// names a domain by a name only this machine uses
/// ([`Engine::fix_local_spellings`]). The report then carries `fixed`, how
/// many links the second changed, and `renames_finished`, the domains the
/// first finished. A fix that is refused (a read-only instance) keeps the
/// report and carries `fix_error` or `rename_fix_errors` instead. Over the
/// daemon when one owns the index (the ctl commands
/// `finish_rename_questions`, `fix_local_spellings` and `name_report`), else
/// against one directly opened store.
pub async fn name_report(
    fix: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path) {
        let renamed = if fix {
            match ctl_routed(json!({ "v": 1, "cmd": "finish_rename_questions" })).await {
                Ok(answer) => answer.map(Ok),
                Err(e) => Some(Err(format!("{e:#}"))),
            }
        } else {
            None
        };
        let fixed = if fix {
            match ctl_routed(json!({ "v": 1, "cmd": "fix_local_spellings" })).await {
                Ok(answer) => {
                    answer.map(|data| Ok(data.get("fixed").and_then(Value::as_u64).unwrap_or(0)))
                }
                Err(e) => Some(Err(format!("{e:#}"))),
            }
        } else {
            None
        };
        if (!fix || fixed.is_some())
            && let Some(mut data) = ctl_routed(json!({ "v": 1, "cmd": "name_report" })).await?
        {
            attach_fix_outcome(&mut data, fixed);
            attach_rename_outcome(&mut data, renamed);
            return Ok(data);
        }
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let renamed = if fix {
        Some(
            engine
                .finish_rename_questions()
                .await
                .map_err(|e| e.to_string()),
        )
    } else {
        None
    };
    let fixed = if fix {
        Some(
            engine
                .fix_local_spellings()
                .await
                .map_err(|e| e.to_string()),
        )
    } else {
        None
    };
    let mut report = engine.name_report().await?;
    attach_fix_outcome(&mut report, fixed);
    attach_rename_outcome(&mut report, renamed);
    Ok(report)
}

/// Put what `--fix` did about MANIFEST renames into a name report:
/// `renames_finished`, the domains finished, and `rename_fix_errors`, one
/// `domain: reason` per domain that could not be, or the one reason nothing
/// could be.
fn attach_rename_outcome(report: &mut Value, renamed: Option<Result<Value, String>>) {
    let Value::Object(map) = report else {
        return;
    };
    let (finished, errors): (Vec<Value>, Vec<Value>) = match renamed {
        None => return,
        Some(Err(e)) => (Vec::new(), vec![Value::from(e)]),
        Some(Ok(answer)) => (
            answer["finished"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|report| report.get("domain").cloned())
                .collect(),
            answer["failed"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|failed| {
                    Value::from(format!(
                        "{}: {}",
                        failed["domain"].as_str().unwrap_or_default(),
                        failed["error"].as_str().unwrap_or_default()
                    ))
                })
                .collect(),
        ),
    };
    map.insert("renames_finished".to_string(), Value::from(finished));
    map.insert("rename_fix_errors".to_string(), Value::from(errors));
}

/// Put what a `--fix` did into a name report: `fixed`, or `fix_error`.
fn attach_fix_outcome(report: &mut Value, fixed: Option<Result<u64, String>>) {
    let Value::Object(map) = report else {
        return;
    };
    match fixed {
        Some(Ok(n)) => {
            map.insert("fixed".to_string(), Value::from(n));
        }
        Some(Err(e)) => {
            map.insert("fix_error".to_string(), Value::from(e));
        }
        None => {}
    }
}

/// Unregister a domain: over the daemon when one owns the index, else against a
/// directly opened store.
///
/// The CLI is the machine owner, so it passes [`Scope::Unrestricted`] and the
/// gate resolves to `Own` on every domain - whoever can run this already holds
/// the files. What the CLI must NOT skip is the rest of the entry point: the
/// join fence, the co-editing sweep, the index clear and the retirement of the
/// domain's visibility and membership records. That last one is why this
/// function exists at all. The CLI used to be a config-file edit of its own,
/// so a `domain_acl` row outlived every removal and a domain later registered
/// under the same name came back private, owned by whoever owned the old one.
///
/// **The standalone branch installs the resolver itself.** A one-shot engine
/// has none, and without one the record sweep is a silent no-op - which is the
/// bug, not a mitigation of it. The accounts database is opened only when the
/// file is already there: a machine that has never had an account has no
/// records to retire, and creating an accounts database as a side effect of a
/// removal would be a surprising thing for this command to do.
///
/// **It does open the index, though, and that is a real change from the config
/// edit this replaced.** The removal clears the domain's engram rows, so it
/// needs the store the way `write` and `reindex` do: an embedded index file is
/// created if this machine never had one, and a removal against an unreachable
/// Postgres fails instead of quietly editing the YAML. The second is the
/// important one and it is the right answer rather than a cost: in a shared
/// database the rows are every instance's, so a config-only removal here would
/// leave the domain live for everybody else while this machine stopped serving
/// it. The error says so, and names the file an operator can edit by hand if
/// their database is gone for good.
///
/// **The standalone branch still tells a running daemon**, and that is not
/// belt-and-braces: [`use_daemon`] is false whenever `--db` or `--config` is
/// given, and `--db` is a GLOBAL flag, so `crystalline --db /somewhere.db
/// domain remove eng` takes this branch while a daemon is live. With
/// `--config` absent, `overlay::load(None)` then edits the very config file
/// that daemon is serving, and without the notify nothing would tell it: it
/// would go on watching a root that is no longer registered. Best effort and
/// never fatal, exactly as before this function existed - by the time it runs
/// the removal has already happened, and a daemon that is not there has
/// nothing to be told.
///
/// Be exact about what that buys, because the notify is not a substitute for
/// the daemon doing the removal itself. `Engine::forget_domain` drops the
/// discovered-domain entry and stops the watcher; a domain that was in the
/// daemon's STARTUP config stays in that daemon's in-memory effective config
/// until it restarts, because only the daemon's own `unregister_domain` writes
/// that snapshot. So the ordinary path - no `--db`, no `--config` - is the
/// daemon branch above, which has none of this residue, and this is the
/// best-effort tail for the flag combination that bypasses it.
pub async fn domain_remove(
    name: &str,
    purge: bool,
    end_drafts: &[String],
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "domain_remove", "domain": name, "purge": purge,
            "end_drafts": end_drafts,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let config_file = loaded.path.clone();
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{e:#}\n\nUnregistering a domain clears its rows from the index, so it needs the \
                 index open. Start the database (or the daemon) and try again; if it is gone for \
                 good, remove the domain's entry from {} by hand.",
                config_file.display()
            )
        })?;
    let engine = std::sync::Arc::new(engine);
    // This machine's sources, as the daemon has them: a local copy a
    // connected server hides is refused here too (decision D19).
    if use_daemon(db, config_path) {
        crate::route::install_sources(&engine);
    }
    let name = localize_standalone(&engine, name).await;
    let name = name.as_str();
    if let Ok(auth_path) = crystalline_core::config::web_auth_db_path()
        && auth_path.exists()
    {
        let auth = std::sync::Arc::new(crate::rest::AuthStore::open(&auth_path).await?);
        engine.set_domain_access(std::sync::Arc::new(
            crate::scope::DomainAccess::new(auth).with_governor(engine.knowledge_governor()),
        ));
    }
    let report = engine
        .unregister_domain(name, &Scope::Unrestricted, purge, end_drafts)
        .await?;
    // Only when this removal edited the daemon's own config file: an explicit
    // --config edited a different one, whose domains that daemon never served.
    if config_path.is_none() {
        let _ = ctl_if_running(json!({ "v": 1, "cmd": "forget_domain", "domain": name })).await;
    }
    Ok(report)
}

/// Rename a domain everywhere this caller can write, or (`local_only`) on
/// this machine alone: over the daemon when one owns the index, else against
/// a directly opened store.
///
/// The same shape [`domain_remove`] has, and for the same reason: the CLI is
/// the machine owner, so it passes [`Scope::Unrestricted`] and the gate
/// resolves to `Own` on every domain. Unlike `domain_remove`, the standalone
/// branch localizes nothing itself: [`crate::engine::Engine::rename_domain`]
/// resolves `domain` against its own name table as the very first thing it
/// does, before the privacy and ownership checks that follow, so a canonical
/// name or an alias already works here exactly as it does over the daemon
/// (`control::DOMAIN_REFERENCE_COMMANDS` never lists `domain_rename` for the
/// same reason). It also needs no accounts database installed ahead of time:
/// [`crate::engine::Engine::rename_domain`]'s own `rename_access` opens
/// `web-auth.db` under the state directory itself when one exists and none
/// was installed, the same fallback `domain_remove` has no equivalent of.
pub async fn domain_rename(
    domain: &str,
    new: &str,
    local_only: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "domain_rename", "domain": domain, "new": new,
            "local_only": local_only,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    // Refused before the index is even opened, so nothing can move.
    if !use_daemon(db, config_path) {
        refuse_rename_off_this_machine(domain, new, local_only, &loaded, &db_path)?;
    }
    let (engine, finished) =
        open_standalone_finishing(loaded, &db_path, false, db, config_path).await?;
    // The opener just finished this very rename, which an earlier run left
    // half done: that is the answer, and sending it again would only be
    // refused as a rename onto the name the domain already has.
    if let Some(report) = finished
        && report["previous"] == domain
        && report["domain"] == new
    {
        return Ok(report);
    }
    // This machine's sources, as the daemon has them: a local copy a
    // connected server hides is refused here too (decision D19). Installed
    // only now, after the opener finished any rename an earlier run left
    // half done, because that recovery reads names the table would hide.
    if use_daemon(db, config_path) {
        crate::route::install_sources(&engine);
    }
    // A rename runs only while this process holds the state directory, as a
    // daemon does; the engine refuses it otherwise, in words that name the
    // holder.
    let ownership = acquire_standalone_ownership("crystalline domain rename").ok();
    engine.set_holds_state_dir(ownership.is_some());
    let report = engine
        .rename_domain(domain, new, local_only, &Scope::Unrestricted)
        .await;
    engine.set_holds_state_dir(false);
    drop(ownership);
    Ok(report?)
}

/// Refuse a rename through `--db` or `--config` that did not open this
/// machine's own index and configuration.
///
/// A rename moves this machine's state folders, auth records and provision
/// receipt, and writes the configuration it persists to. Run against another
/// index, it would move all of that while this machine's own index keeps the
/// old name, and a virtual domain's engrams would stay behind on the old row.
/// So the index, configuration and state directory the command opened are
/// compared with the ones a plain command opens, the identity a rename
/// journal records ([`crystalline_engine::RenameOwner`]), and any part that
/// differs is named. `--db` or `--config` spelling this machine's own index
/// or configuration differs in nothing and is allowed. The engine refuses the
/// same rename too ([`crystalline_engine::Engine::with_machine_owner`]); this
/// check runs first so the refusal can name the command, and so the opener
/// never even looks at a journal.
fn refuse_rename_off_this_machine(
    domain: &str,
    new: &str,
    local_only: bool,
    loaded: &overlay::LoadedConfig,
    db_path: &Path,
) -> anyhow::Result<()> {
    let state_dir = crystalline_core::config::state_dir()?;
    let opened = crystalline_engine::RenameOwner::for_opened(loaded, db_path, &state_dir);
    let home = machine_rename_owner().map_err(|e| {
        anyhow::anyhow!(
            "this machine's own index and configuration cannot be named ({e:#}), so renames are \
             off. Nothing was renamed. Fix the configuration so it loads, and run this again"
        )
    })?;
    let differences = opened.differences_from(&home);
    if differences.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "a rename moves this machine's state folders and writes its configuration, so it runs \
         only against this machine's own index and configuration, and {}. Nothing was renamed. \
         Run `crystalline domain rename {domain} {new}{}` without --db and --config",
        differences.join(", and "),
        if local_only { " --local" } else { "" }
    )
}

pub use crystalline_engine::machine_rename_owner;

/// [`machine_rename_owner`] for an opener to hand its engine
/// ([`Engine::with_machine_owner_lookup`]): the daemon, the embedded stack,
/// the standalone opener and the direct adoption. A lookup that fails (this
/// machine's default configuration does not load, say) is handed on as it
/// is, so the engine refuses a rename and skips a name adoption rather than
/// running either unchecked; a warning says so once here.
/// Install the private-domain resolver on a serving stack, over this
/// machine's accounts store, and hand the store back for the HTTP surface to
/// share. Every serving stack calls it before it answers anything: the
/// daemon (HTTP on or off), the embedded stdio stack and a one-shot tool
/// run, so a stack knows its readers whether or not anybody signs in here.
/// A store that is not there is not created: with no account there is no
/// private domain, and the engine refuses other domains' sources should the
/// file appear while it runs.
///
/// Fails closed: when the store cannot be opened, the engine is told so
/// ([`Engine::set_domain_access_unavailable`]) and then
/// refuses every Reflection source outside its page's domain, never lets
/// one through.
pub(crate) async fn install_domain_access(
    engine: &Arc<Engine>,
) -> Option<Arc<crate::rest::AuthStore>> {
    // Who reads a domain connected through the GitHub App follows GitHub,
    // and the App is loaded off the request path, starting now. The
    // governor is built first, so the load warms it up when it lands.
    engine.github_governor();
    engine.preload_github_app();
    let opened = match crystalline_core::config::web_auth_db_path() {
        // No store yet: no account and no private domain exist, so nothing
        // is opened (or created, which would change this machine's state
        // from a stack that only reads it). The engine watches for the file
        // and fails closed should it appear.
        Ok(path) if !path.exists() => {
            engine.set_domain_access_absent(path);
            return None;
        }
        Ok(path) => crate::rest::AuthStore::open(&path)
            .await
            .map_err(|e| format!("{e:#}")),
        Err(e) => Err(format!("{e:#}")),
    };
    match opened {
        Ok(auth) => {
            let auth = Arc::new(auth);
            engine.set_domain_access(Arc::new(
                crate::scope::DomainAccess::new(auth.clone())
                    .with_governor(engine.knowledge_governor()),
            ));
            Some(auth)
        }
        Err(why) => {
            tracing::warn!(
                "the accounts store could not be opened ({why}); Reflection sources outside a page's own domain are refused until it opens"
            );
            engine.set_domain_access_unavailable(why);
            None
        }
    }
}

pub(crate) fn machine_owner_for_engine() -> anyhow::Result<crystalline_engine::RenameOwner> {
    let lookup = machine_rename_owner();
    if let Err(e) = &lookup {
        tracing::warn!(
            "this machine's own index could not be named ({e:#}); renames and the lining up \
             of domain names are off in this process. Fix the configuration so it loads, then \
             restart the daemon or the MCP server"
        );
    }
    lookup
}

/// Delete this machine's pending rename journal, leaving every step it
/// already ran as it is, and answer what it recorded; `None` when no rename
/// is pending. Done only while this process holds the state directory, so a
/// daemon that is running the journal right now is never cut off halfway:
/// with the lock held elsewhere the journal stays and the error says why.
pub fn discard_rename_journal() -> anyhow::Result<Option<crystalline_engine::PendingRename>> {
    let state_dir = crystalline_core::config::state_dir()?;
    let Some(pending) = crystalline_engine::pending_rename(&state_dir)? else {
        return Ok(None);
    };
    let _ownership =
        acquire_standalone_ownership("crystalline doctor --discard-rename").map_err(|_| {
            anyhow::anyhow!(
                "another Crystalline process (the daemon) holds this machine's state directory, \
                 so the rename journal was left as it is; stop it with `crystalline ctl \
                 shutdown` and run this again"
            )
        })?;
    crystalline_engine::discard_pending_rename(&state_dir)?;
    Ok(Some(pending))
}

/// The socket request one `domain_review` call sends.
///
/// Its own function so the shape can be asserted without a daemon: what is
/// load bearing is the ABSENCE of `folds` when there are no answers, and an
/// absent key is exactly what a test of a live round trip would not notice.
fn review_request(
    name: &str,
    overlay_mode: bool,
    preview: bool,
    folds: &[(String, bool)],
) -> Value {
    use serde_json::json;
    let mut request = json!({
        "v": 1, "cmd": "domain_review", "domain": name,
        "mode": if overlay_mode { "overlay" } else { "direct" },
        "preview": preview,
    });
    if !folds.is_empty() {
        request["folds"] = folds
            .iter()
            .map(|(actor, fold)| {
                (
                    actor.clone(),
                    Value::from(if *fold { "fold" } else { "discard" }),
                )
            })
            .collect::<serde_json::Map<String, Value>>()
            .into();
    }
    request
}

/// Turn review mode on for a domain, or take it off and settle every actor's
/// drafts: over the daemon when one owns the index, else against a directly
/// opened store.
///
/// The same shape [`domain_remove`] has, and for the same reasons: the CLI is
/// the machine owner, so it passes [`Scope::Unrestricted`] and the gate resolves
/// to `Own` on every domain - whoever can run this already holds the files. It
/// opens the index because the verb reads every actor's drafts out of it and
/// writes the folds back through a sync, and because a shared database's rows
/// are every instance's: a config-only edit here would leave the domain
/// reviewing for everybody else while this machine stopped.
///
/// `folds` carries the per-actor answers when there are any; an empty slice
/// with `preview` false is the answer for a domain nobody is drafting in.
/// `preview` asks for the plan and writes nothing.
///
/// The socket request omits the key entirely when there are no answers, which
/// is how the daemon reads "none": the surfaces agree that fold answers are a
/// question about LEAVING review mode, so a request on the way IN must carry
/// no `folds` at all rather than an empty one.
pub async fn domain_review(
    name: &str,
    overlay_mode: bool,
    preview: bool,
    folds: &[(String, bool)],
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    if use_daemon(db, config_path)
        && let Some(data) = ctl_routed(review_request(name, overlay_mode, preview, folds)).await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let name = localize_standalone(&engine, name).await;
    let name = name.as_str();
    let mode = overlay_mode.then_some(crystalline_core::config::ReviewMode::Overlay);
    let confirm = if preview {
        crate::review::ReviewModeConfirm::Preview
    } else {
        crate::review::ReviewModeConfirm::Confirmed {
            folds: folds
                .iter()
                .map(|(actor, fold)| {
                    (
                        actor.clone(),
                        if *fold {
                            crate::review::FoldChoice::Fold
                        } else {
                            crate::review::FoldChoice::Discard
                        },
                    )
                })
                .collect(),
        }
    };
    Ok(engine
        .set_review_mode(name, mode, confirm, &Scope::Unrestricted)
        .await?)
}

/// Connect a new domain to a GitHub repository: over the daemon when one owns
/// the index, else against a directly opened store. `want_embeddings` is
/// `false` in the standalone fallback, matching `domain_import` and
/// `domain_export`: a one-shot command never triggers a surprise embedding
/// model download, and the domain is searchable via text immediately either
/// way; embedding follows whenever the daemon (or a later `sync --embed`)
/// gets to it.
///
/// `readers` says who may read the domain on an instance with a GitHub App
/// (see [`Engine::origin_add`]); the standalone fallback installs this
/// machine's accounts store first, so a private answer is recorded there as
/// the daemon would record it.
#[allow(clippy::too_many_arguments)]
pub async fn origin_add(
    repo: &str,
    domain: Option<&str>,
    path: Option<&str>,
    branch: Option<&str>,
    folder: Option<&str>,
    readers: Option<crate::params::Readers>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "origin_add", "repo": repo, "domain": domain,
            "path": path, "branch": branch, "folder": folder,
            "readers": readers.map(crate::params::Readers::as_str),
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine =
        Arc::new(open_standalone_reporting(loaded, &db_path, false, db, config_path).await?);
    install_domain_access(&engine).await;
    engine.apply_env_readers_quietly().await?;
    Ok(engine
        .origin_add(repo, domain, path, branch, folder, readers)
        .await?)
}

/// The name adoption a sync run without a daemon owes once it has returned
/// ([`Engine::adopt_domain_names`]), over the store that sync opened: the
/// daemon's ctl `sync` runs the same adoption after its own sync. A failure
/// is logged and answered as an empty list; the sync has landed.
///
/// An adoption renames this machine's state folders and configuration keys,
/// so it runs only where a daemon's would: never against an index `--db` or
/// `--config` named instead of the daemon's, and only while this process
/// holds the ownership of the state directory. Either way a skipped adoption
/// says so in a log line and answers an empty list.
pub async fn adopt_domain_names_direct(
    store: Arc<tokio::sync::Mutex<dyn crystalline_index::Store>>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    if !use_daemon(db, config_path) {
        // At warn, which a command's own log filter shows by default: the
        // person who ran this sync reads that the names were not lined up
        // and which command does it.
        tracing::warn!(
            "domain names are not lined up with their MANIFESTs after this sync: --db or \
             --config named the index, and lining them up renames this machine's own state \
             and configuration; run `crystalline sync` without --db and --config to do it"
        );
        return Ok(Value::Array(Vec::new()));
    }
    let Ok(ownership) = acquire_standalone_ownership("crystalline sync") else {
        tracing::warn!(
            "domain names are not lined up with their MANIFESTs after this sync: another \
             Crystalline process (the daemon) holds this machine's state directory, and it \
             lines them up after its own next sync"
        );
        return Ok(Value::Array(Vec::new()));
    };
    let loaded = overlay::load(config_path)?;
    let read_only = loaded.effective.read_only();
    let mut engine = Engine::new(store, loaded.file, None, Some(loaded.path))
        .with_read_only(read_only)
        .with_env_overlay(loaded.overlay)
        .with_machine_owner_lookup(machine_owner_for_engine());
    // Where a rename keeps its journal, as every other opener says.
    if let Ok(state) = crystalline_core::config::state_dir() {
        engine = engine.with_state_dir(state);
    }
    let adopted = engine.adopt_domain_names_after("the sync").await;
    drop(ownership);
    Ok(adopted)
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
            ctl_routed(json!({ "v": 1, "cmd": "origin_update", "domain": domain })).await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let domain = match domain {
        Some(d) => Some(localize_standalone(&engine, d).await),
        None => None,
    };
    Ok(engine
        .origin_update(domain.as_deref(), &Scope::Unrestricted)
        .await?)
}

/// Report where one origin-connected domain (or every one) stands relative to
/// its origin, plus this machine's GitHub connection: over the daemon when
/// one owns the index, else against a directly opened store.
///
/// `detail` asks for each domain's unshared files to be named and grouped by
/// kind rather than only counted, at the cost of a second walk of the working
/// tree per domain. `diff` adds both sides of every one of those files, needs
/// a domain and implies `detail`.
pub async fn origin_status(
    domain: Option<&str>,
    detail: bool,
    diff: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_routed(json!({
            "v": 1,
            "cmd": "origin_status",
            "domain": domain,
            "detail": detail,
            "diff": diff,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let domain = match domain {
        Some(d) => Some(localize_standalone(&engine, d).await),
        None => None,
    };
    Ok(engine
        .origin_status(domain.as_deref(), detail, diff, &Scope::Unrestricted)
        .await?)
}

/// Propose one team domain's local changes as a pull request against its
/// origin: over the daemon when one owns the index, else against a directly
/// opened store. `want_embeddings` is `false` in the standalone fallback: a
/// share never touches the working tree, so there is nothing new to embed.
///
/// The standalone path shares as [`ShareActor::Owner`], as the three write
/// verbs below do: a CLI invocation has no account behind it, so the machine
/// owner is who it is, not a placeholder for one.
#[allow(clippy::too_many_arguments)]
pub async fn origin_share(
    domain: &str,
    title: Option<&str>,
    description: Option<&str>,
    proposal: Option<u64>,
    files: Option<&[String]>,
    as_proposal: Option<bool>,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    if use_daemon(db, config_path)
        && let Some(data) = ctl_routed(origin_share_request(
            domain,
            title,
            description,
            proposal,
            files,
            as_proposal,
        ))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let domain = localize_standalone(&engine, domain).await;
    Ok(engine
        .origin_share_with(
            &domain,
            title,
            description,
            proposal,
            files,
            as_proposal,
            ShareActor::Owner,
        )
        .await?)
}

/// The control-socket request [`origin_share`] sends, built in one place so
/// the daemon's reader (`control::share_request`) is tested against exactly
/// these keys: a key spelled differently on one side would read as absent,
/// and an absent `as_proposal` lets a share commit directly.
pub(crate) fn origin_share_request(
    domain: &str,
    title: Option<&str>,
    description: Option<&str>,
    proposal: Option<u64>,
    files: Option<&[String]>,
    as_proposal: Option<bool>,
) -> Value {
    serde_json::json!({
        "v": 1, "cmd": "origin_share", "domain": domain,
        "title": title, "description": description, "proposal": proposal,
        "files": files, "as_proposal": as_proposal,
    })
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
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "origin_withdraw", "domain": domain,
            "proposal": proposal, "revert": revert,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let domain = localize_standalone(&engine, domain).await;
    Ok(engine
        .origin_withdraw(&domain, proposal, revert, ShareActor::Owner)
        .await?)
}

/// One team domain's unshared changes, offline, for `origin diff`: over the
/// daemon when one owns the index, else against a directly opened store.
/// `path` narrows to one change, `sides` inlines both texts per entry.
pub async fn origin_changes(
    domain: &str,
    path: Option<&str>,
    sides: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    if use_daemon(db, config_path)
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "origin_changes", "domain": domain, "path": path, "sides": sides,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let domain = localize_standalone(&engine, domain).await;
    Ok(
        crate::control::origin_changes_inline(&engine, &domain, path, sides, &ShareActor::Owner)
            .await?,
    )
}

/// Put named paths of one team domain back the way the team has them, for
/// `origin discard`: over the daemon when one owns the index, else against a
/// directly opened store. The standalone path discards as
/// [`ShareActor::Owner`], as every origin write verb does.
pub async fn origin_discard(
    domain: &str,
    targets: &[(String, Option<String>)],
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Value> {
    use serde_json::json;
    let wire: Vec<Value> = targets
        .iter()
        .map(|(path, sha)| json!({ "path": path, "sha": sha }))
        .collect();
    if use_daemon(db, config_path)
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "origin_discard", "domain": domain, "targets": wire,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let domain = localize_standalone(&engine, domain).await;
    let targets: Vec<DiscardTarget> = targets
        .iter()
        .map(|(path, sha)| DiscardTarget {
            path: path.clone(),
            sha256: sha.clone(),
        })
        .collect();
    Ok(engine
        .discard_local_changes(&domain, &targets, &ShareActor::Owner)
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
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "origin_resolve", "domain": domain, "path": path,
            "keep": keep, "content_b64": content_b64,
        }))
        .await?
    {
        return Ok(data);
    }
    let loaded = overlay::load(config_path)?;
    let db_path = resolve_db(db)?;
    let engine = open_standalone_reporting(loaded, &db_path, false, db, config_path).await?;
    let domain = localize_standalone(&engine, domain).await;
    Ok(engine
        .origin_resolve(&domain, path, keep, content, ShareActor::Owner)
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
        && let Some(data) = ctl_routed(json!({
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
            crate::settings::apply_checked(&mut file, &loaded.overlay, key, value)?;
            save_file(&loaded.path, &file)?;
            Ok(setting_view(&file, key, &loaded.overlay))
        }
        "unset" => {
            if loaded.effective.read_only() {
                anyhow::bail!("{}", crate::engine::EngineError::ReadOnly);
            }
            let key = key.ok_or_else(|| anyhow::anyhow!("configure unset requires a key"))?;
            let mut file = loaded.file.clone();
            crate::settings::unset_checked(&mut file, &loaded.overlay, key)?;
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
        && let Some(data) = ctl_routed(json!({
            "v": 1, "cmd": "provision", "action": action, "domain": domain,
        }))
        .await?
    {
        return Ok(data);
    }

    let loaded = overlay::load(config_path)?;
    // `provision` opens no engine at all (see the doc comment above), so it
    // has no [`localize_standalone`] to call: a canonical name or alias is
    // resolved straight from the loaded configuration instead, the same
    // inputs an engine's own name table would read for a file domain.
    let domain = domain.map(|d| localize_in_config(&loaded, d));
    let domain = domain.as_deref();
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

/// The key of the `routing_bullets` answer that lists the read-only virtual
/// domains. `$` is not a legal domain name character: no domain name passes
/// the validator; a hand-registered $ name is left out of the answer. An
/// older client parses the key as one more entry it never asks for.
pub const ROUTING_READ_ONLY_KEY: &str = "$read_only";

/// The virtual domains' routing facts for `prompt system`: each one's bullets,
/// and which of them are read-only.
#[derive(Debug, Default)]
pub struct VirtualRouting {
    pub bullets: std::collections::BTreeMap<String, Vec<String>>,
    pub read_only: std::collections::BTreeSet<String>,
}

/// Split a daemon's `routing_bullets` answer into the bullets and the
/// read-only names. An answer from before 0.25.1 has no such key and marks
/// nothing.
fn split_routing_answer(
    mut map: std::collections::BTreeMap<String, Vec<String>>,
) -> VirtualRouting {
    let read_only = map
        .remove(ROUTING_READ_ONLY_KEY)
        .unwrap_or_default()
        .into_iter()
        .collect();
    VirtualRouting {
        bullets: map,
        read_only,
    }
}

/// Resolve virtual-domain routing bullets for `prompt system`: over the daemon
/// when one is running (its warm state) and no explicit `--config`/`--db`
/// override was given, else against a directly opened store, with the
/// virtual domains that are read-only beside them. Returns nothing when the
/// config has no virtual domains, so the common all-file case never opens a
/// store or a socket. `config_path` is the raw `--config` override the
/// caller resolved `config` from, threaded through only so an override bypasses
/// the daemon (which serves its own default config) exactly like every other
/// verb.
pub async fn virtual_routing_bullets(
    config: &crystalline_core::config::GlobalConfig,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> VirtualRouting {
    use serde_json::json;
    if !config.domains.values().any(|e| e.is_virtual()) {
        return VirtualRouting::default();
    }
    if use_daemon(db, config_path)
        && let Ok(Some(data)) = ctl_if_running(json!({ "v": 1, "cmd": "routing_bullets" })).await
        && let Ok(map) = serde_json::from_value(data)
    {
        return split_routing_answer(map);
    }
    // Inside an app package the index is never opened here (see
    // `ctl_routed`): the routing block goes out without these bullets.
    if crate::runs_in::PackageContext::here().is_packaged() {
        return VirtualRouting::default();
    }
    let db_path = match resolve_db(db) {
        Ok(p) => p,
        Err(_) => return VirtualRouting::default(),
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
    // No rename recovery here: this read-only render takes no ownership of
    // the state directory, so a rename left half done waits for the next
    // opener that does.
    match open_standalone(loaded, &db_path, false).await {
        Ok(engine) => {
            let (bullets, read_only) = engine.virtual_routing().await;
            VirtualRouting { bullets, read_only }
        }
        Err(_) => VirtualRouting::default(),
    }
}

/// Dispatch a tool to the shared engine by name, decoding `args` into the
/// tool's params type. Reachable from [`crate::control`] so the ctl `tool`
/// command dispatches a daemon-attached CLI data verb through the exact same
/// name-to-method mapping the standalone path uses.
///
/// Every read here is [`Scope::Unrestricted`], and that is the answer rather
/// than a placeholder: this dispatcher is reached from the CLI and the control
/// socket, and whoever can run either already holds the files on disk.
pub(crate) async fn dispatch_engine(
    engine: &Engine,
    tool: &str,
    args: Value,
) -> anyhow::Result<Value> {
    // Where a caller on this machine opens a page: the CLI and the control
    // socket are both same-machine, so the daemon's own bind is the answer and
    // there is no request to derive one from. Read once, because every arm
    // below that hands back one engram hands back the same base with it.
    let base = engine.local_web_base();
    let v = match tool {
        // A CLI-driven write is not an MCP client, so it identifies itself as
        // the CLI process; `identity.actor` still wins when it is set.
        "write_engram" => {
            let mut v = engine
                .write_engram_as(
                    &decode::<WriteParams>(args)?,
                    Some(CLI_ACTOR),
                    &Scope::Unrestricted,
                )
                .await?;
            // The close-folder hint the MCP receipt carries, so `crystalline
            // write` can print it too. After the write returned, for the same
            // store-lock reason as there.
            let folders = Engine::close_folder_probe(&v);
            let close = engine
                .close_folders_for(folders.as_ref(), &Scope::Unrestricted)
                .await;
            Engine::attach_close_folders(&mut v, &close);
            crate::web_url::attach_engram_url(&mut v, &base);
            v
        }
        "read_engram" => {
            let mut v = engine
                .read_engram(&decode::<ReadParams>(args)?, &Scope::Unrestricted)
                .await?;
            crate::web_url::attach_engram_url(&mut v, &base);
            v
        }
        "edit_engram" => {
            let mut v = engine
                .edit_engram_as(
                    &decode::<EditParams>(args)?,
                    Some(CLI_ACTOR),
                    &Scope::Unrestricted,
                )
                .await?;
            crate::web_url::attach_engram_url(&mut v, &base);
            v
        }
        "move_engram" => {
            let mut v = engine
                .move_engram(&decode::<MoveParams>(args)?, &Scope::Unrestricted)
                .await?;
            // The destination and nowhere else: `from` is the address the
            // engram stopped answering to.
            if let Some(to) = v.get_mut("to") {
                crate::web_url::attach_engram_url(to, &base);
            }
            v
        }
        "split_engram" => {
            engine
                .split_engram_as(
                    &decode::<SplitParams>(args)?,
                    Some(CLI_ACTOR),
                    &Scope::Unrestricted,
                )
                .await?
        }
        "delete_engram" => {
            engine
                .delete_engram_as(
                    &decode::<DeleteParams>(args)?,
                    Some(CLI_ACTOR),
                    &Scope::Unrestricted,
                )
                .await?
        }
        "search_engrams" => {
            engine
                .search_engrams(&decode::<SearchParams>(args)?, &Scope::Unrestricted)
                .await?
        }
        "build_context" => {
            engine
                .build_context(&decode::<ContextParams>(args)?, &Scope::Unrestricted)
                .await?
        }
        "recent_activity" => {
            engine
                .recent_activity(&decode::<RecentParams>(args)?, &Scope::Unrestricted)
                .await?
        }
        "list_domains" => {
            engine
                .list_domains(&decode::<ListDomainsParams>(args)?, &Scope::Unrestricted)
                .await?
        }
        "browse_domain" => {
            engine
                .browse_domain(&decode::<BrowseParams>(args)?, &Scope::Unrestricted)
                .await?
        }
        "validate_engrams" => {
            engine
                .validate_engrams(&decode::<ValidateParams>(args)?, &Scope::Unrestricted)
                .await?
        }
        "infer_schema" => {
            engine
                .infer_schema(&decode::<InferParams>(args)?, &Scope::Unrestricted)
                .await?
        }
        "vocabulary" => {
            engine
                .vocabulary(&decode::<VocabularyParams>(args)?, &Scope::Unrestricted)
                .await?
        }
        // Matched through the constant rather than a literal so the CLI verb,
        // this arm and the router name can never drift apart.
        t if t == crate::EVOLVE_TOOL_NAME => {
            engine
                .evolve_engrams(&decode::<EvolveParams>(args)?, &Scope::Unrestricted)
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
///
/// A call that displaced an older daemon on the way does not answer `None`.
/// A displacement means a daemon was running, so a daemon answers after it:
/// either the successor a respawning MCP bridge has already started from the
/// new binary, or one this call spawns. Going standalone there collides with
/// that successor, which holds the index by the time the open runs, and the
/// first command after an in-place upgrade failed on the index lock (gate
/// item 9 of 0.22). `None` stays the answer when no daemon was running, and
/// when no daemon became ready after the displacement, so the caller falls
/// back as before.
///
/// A successor this call starts runs with the displaced daemon's own start
/// options from its record (its `--db`, config file and read-only mode among
/// them), never with this command's: see
/// [`crate::instance::attach_after_displacement`].
pub async fn ctl_if_running(cmd: Value) -> anyhow::Result<Option<Value>> {
    let conn = match crate::instance::try_attach_displacing().await {
        (Some(conn), _) => conn,
        (None, Some(start)) => match crate::instance::attach_after_displacement(start).await {
            Some(conn) => conn,
            None => return Ok(None),
        },
        (None, None) => return Ok(None),
    };
    Ok(Some(ctl_exchange(conn, cmd).await?))
}

/// Send a ctl command the way this process may: outside an app package this
/// is [`ctl_if_running`], a running daemon's answer or `None`, and the
/// caller falls back to its local path as before.
///
/// Inside an app package (Claude Desktop's MSIX package on Windows) a
/// daemon is required, because every new file this process writes under
/// AppData lands in the package's private copy. So it attaches, or runs the
/// daemon task and waits for the daemon the way a packaged bridge does
/// ([`crate::instance::ensure_daemon`]), and answers the daemon's reply. It
/// never answers `None` there: without a daemon it fails with
/// [`packaged_refusal`], and the caller never opens a local engine.
pub async fn ctl_routed(cmd: Value) -> anyhow::Result<Option<Value>> {
    ctl_routed_waiting(cmd, crate::instance::PACKAGED_TASK_WAIT).await
}

/// [`ctl_routed`], waiting at most `wait` for the daemon the task starts
/// inside a package: `prompt system` runs in a SessionStart hook with a
/// budget shorter than [`crate::instance::PACKAGED_TASK_WAIT`].
///
/// A process that found no daemon once says so again at once on every
/// later call, so a command that asks several times (doctor) does not wait
/// out the task each time.
pub async fn ctl_routed_waiting(
    cmd: Value,
    wait: std::time::Duration,
) -> anyhow::Result<Option<Value>> {
    static NO_DAEMON: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let here = crate::runs_in::PackageContext::here();
    if !here.is_packaged() {
        return ctl_if_running(cmd).await;
    }
    if let Some(refusal) = NO_DAEMON.get() {
        anyhow::bail!(refusal.clone());
    }
    let task = crate::daemon_task::for_this_process();
    let options = crate::instance::SpawnOptions::default();
    let conn = match crate::instance::ensure_daemon_in(here, &*task, &options, wait).await {
        Ok(conn) => conn,
        Err(e) => {
            let bridge = crate::daemon_task::BridgeFailure::of(&e);
            let refusal = NO_DAEMON.get_or_init(|| packaged_refusal(&bridge));
            anyhow::bail!(refusal.clone());
        }
    };
    Ok(Some(ctl_exchange(conn, cmd).await?))
}

/// What a command inside an app package says when no daemon answers: why,
/// the fix and the terminal outside Claude Desktop.
pub fn packaged_refusal(bridge: &crate::daemon_task::BridgeFailure) -> String {
    let fix = crate::stub::StubStatus::for_bridge(bridge.to_string(), bridge.clone()).fix();
    format!(
        "Crystalline cannot run this command here: {bridge}. A command inside Claude Desktop's app package works only through the Crystalline daemon outside it. {fix} Or run the command in a terminal outside Claude Desktop."
    )
}

/// The refusal of a local engine (or a direct index open) inside an app
/// package: it would create a private index, write-ahead log and ownership
/// record, and index the same knowledge folders as the daemon outside.
pub fn packaged_local_engine_refusal() -> String {
    format!(
        "Crystalline does not open its index from here: {}.",
        crate::runs_in::PACKAGED_WRITE_REASON
    )
}

/// [`ctl_if_running`] over a passive attach: the answer of a daemon that is
/// already there, `None` when none is, and never a displacement on the way.
/// For callers on a latency budget that must not pay for a takeover - the
/// per-prompt recall hook is the one today.
pub async fn ctl_if_running_passive(cmd: Value) -> anyhow::Result<Option<Value>> {
    match crate::instance::try_attach_passive().await {
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

/// The `crystalline mcp` client's log: stderr only, since stdout is the MCP
/// stdio stream, filtered by RUST_LOG with a default of `warn`.
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// The embedded stack gives back the host locks it claimed whether its
    /// session ended or failed to start: a lock left behind would make every
    /// other instance wait out the stale threshold.
    #[tokio::test]
    async fn the_embedded_stack_releases_its_locks_when_the_session_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("eng");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("MANIFEST.md"), "# eng\n").unwrap();
        let mut cfg = crystalline_core::config::GlobalConfig::default();
        cfg.domains.insert(
            "eng".to_string(),
            crystalline_core::config::DomainEntry::file(root),
        );
        let store = crystalline_index::TursoStore::open_in_memory()
            .await
            .unwrap();
        let engine = Engine::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            cfg,
            None,
            Some(tmp.path().join("config.yaml")),
        )
        .with_instance_id("this-machine".to_string());
        assert!(engine.claim_host("eng", false).await.unwrap().is_acquired());
        let holder = || async {
            let store = engine.store();
            let store = store.lock().await;
            let id = store.domain_id("eng").await.unwrap().unwrap();
            store.domain_host(id).await.unwrap()
        };
        assert!(holder().await.is_some(), "the stack holds the lock");
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let served = serve_then_release(&engine, &stop, async {
            Err(anyhow::anyhow!("the session could not start"))
        })
        .await;
        assert!(served.is_err(), "the session's failure is the answer");
        assert!(*stopped.borrow(), "the background tasks are told to stop");
        assert!(holder().await.is_none(), "the lock is given back");
    }

    /// An answer from a daemon before 0.25.1 has no `$read_only` key: every
    /// bullet is kept and nothing is marked.
    #[test]
    fn an_old_routing_bullets_answer_still_parses() {
        let answer = serde_json::json!({ "scratch": ["- x"] });
        let routing = split_routing_answer(serde_json::from_value(answer).unwrap());
        assert_eq!(routing.bullets["scratch"], vec!["- x".to_string()]);
        assert!(routing.read_only.is_empty());
    }

    /// A 0.25.1 answer carries the locked virtual domains under a key no
    /// domain can be named, and the key is not a domain row.
    #[test]
    fn the_read_only_key_is_split_off() {
        let answer = serde_json::json!({
            "scratch": ["- x"],
            "vault": ["- y"],
            (ROUTING_READ_ONLY_KEY): ["vault"],
        });
        let routing = split_routing_answer(serde_json::from_value(answer).unwrap());
        assert_eq!(routing.bullets.len(), 2);
        assert!(!routing.bullets.contains_key(ROUTING_READ_ONLY_KEY));
        assert_eq!(
            routing.read_only,
            ["vault".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn the_read_only_key_is_never_a_domain_name() {
        assert!(
            crystalline_core::config::registration::validate_domain_name(ROUTING_READ_ONLY_KEY)
                .is_err()
        );
    }

    fn lines_from(bytes: &'static [u8]) -> BoundedLines<&'static [u8]> {
        BoundedLines::new(bytes, STDIO_LINE_LIMIT)
    }

    /// Turning review ON carries no fold answers, and the request has to say
    /// that the way the daemon and the JSON API both read it: by not carrying
    /// the key at all. An empty map is a map, and the surfaces refuse any map
    /// here - so a client that sent one would meet a refusal for a question it
    /// never asked.
    #[test]
    fn a_request_on_the_way_in_carries_no_folds_at_all() {
        let going_in = review_request("team", true, false, &[]);
        assert_eq!(going_in["mode"], serde_json::json!("overlay"));
        assert!(
            going_in.get("folds").is_none(),
            "a domain on its way in holds no drafts to answer for: {going_in}"
        );

        let leaving = review_request("team", false, false, &[("ada".to_string(), true)]);
        assert_eq!(
            leaving["folds"],
            serde_json::json!({ "ada": "fold" }),
            "and on the way out the answers are the request: {leaving}"
        );

        let plan = review_request("team", false, true, &[]);
        assert_eq!(plan["preview"], serde_json::json!(true));
        assert!(
            plan.get("folds").is_none(),
            "asking for the plan answers nothing yet: {plan}"
        );
    }

    // --- the resolved skill-surface answer ------------------------------------
    //
    // The whole decision table for `--harness`, against a receipt written by
    // hand in the shape `crystalline install` writes. Every uncertain row
    // resolves to the default gate, meaning "serve the surface and the full
    // instructions": an over-served client
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

    /// The gate a verified harness with its hooks wired resolves to.
    const VERIFIED: HarnessGate = HarnessGate {
        hook_installed: true,
        onboarding_verified: true,
        unattended: false,
        run_domain: None,
    };

    #[test]
    fn the_resolved_answer_is_the_named_harness_plus_this_machines_receipt() {
        let tmp = tempfile::tempdir().unwrap();
        let receipt = tmp.path().join("installs.json");

        // The row the feature exists for.
        write_receipt(&receipt, "claude-code", true);
        assert_eq!(
            resolve_harness_gate_at(Some("claude-code"), &receipt),
            VERIFIED
        );

        // Per harness, not per machine: the receipt knows claude-code, and a
        // codex-spawned process still gets the surface. This is the whole
        // reason the argument exists rather than the receipt alone.
        assert_eq!(
            resolve_harness_gate_at(Some("codex"), &receipt),
            HarnessGate::default()
        );

        // Hooks skipped: nothing onboards that session, so the block and the
        // surface both have to come from here.
        write_receipt(&receipt, "claude-code", false);
        assert_eq!(
            resolve_harness_gate_at(Some("claude-code"), &receipt),
            HarnessGate::default()
        );
    }

    /// The gate is two facts. A hook in the receipt is enough for the short
    /// instructions with a conditional pointer, but the skills stay served
    /// until the harness's profile says a live check confirmed that it loads
    /// the hook's routing block and the skills as files.
    #[test]
    fn the_gate_is_two_facts_and_only_a_verified_profile_hides_the_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let receipt = tmp.path().join("installs.json");
        write_receipt(&receipt, "claude-code", true);
        assert_eq!(
            resolve_harness_gate_at(Some("claude-code"), &receipt),
            HarnessGate {
                hook_installed: true,
                onboarding_verified: true,
                unattended: false,
                run_domain: None,
            }
        );
        for id in ["codex", "copilot"] {
            write_receipt(&receipt, id, true);
            assert_eq!(
                resolve_harness_gate_at(Some(id), &receipt),
                VERIFIED,
                "{id} is verified"
            );
        }
        for id in ["cursor", "kiro", "gemini", "qwen"] {
            write_receipt(&receipt, id, true);
            assert_eq!(
                resolve_harness_gate_at(Some(id), &receipt),
                HarnessGate {
                    hook_installed: true,
                    onboarding_verified: false,
                    unattended: false,
                    run_domain: None,
                },
                "{id} is unverified"
            );
            write_receipt(&receipt, id, false);
            assert_eq!(
                resolve_harness_gate_at(Some(id), &receipt),
                HarnessGate::default(),
                "{id} with --skip-hooks"
            );
        }
    }

    #[test]
    fn every_uncertain_input_resolves_to_serving() {
        let tmp = tempfile::tempdir().unwrap();
        let receipt = tmp.path().join("installs.json");
        write_receipt(&receipt, "claude-code", true);
        let serve = HarnessGate::default();

        // No argument at all: a registration written before `--harness`
        // existed. This is the pre-existing-install case and it must behave
        // exactly as it did before the flag.
        assert_eq!(resolve_harness_gate_at(None, &receipt), serve);

        // An id this binary does not know: a downgrade meeting a newer
        // registration, or a harness rolled out ahead of the binaries. Warns
        // and serves; it must never be a usage error, which would leave the
        // harness with a server that will not start.
        assert_eq!(
            resolve_harness_gate_at(Some("nextgen-harness"), &receipt),
            serve
        );
        assert_eq!(resolve_harness_gate_at(Some(""), &receipt), serve);

        // A missing receipt and a corrupt one both read as "nothing onboarded"
        // through the tolerant shallow reader, never as an error.
        let missing = tmp.path().join("nope.json");
        assert_eq!(
            resolve_harness_gate_at(Some("claude-code"), &missing),
            serve
        );
        std::fs::write(&receipt, "not json at all").unwrap();
        assert_eq!(
            resolve_harness_gate_at(Some("claude-code"), &receipt),
            serve
        );
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
    fn launcher_too_old_reads_the_version_only_on_the_desktop_channel() {
        assert_eq!(MINIMUM_LAUNCHER, (0, 24, 0));
        // A launcher from 0.24.0 to 0.25.0 passed no version: judged 0.24.0.
        assert!(!launcher_too_old(None, Some("desktop")));
        assert!(launcher_too_old(Some("0.23.9"), Some("desktop")));
        assert!(launcher_too_old(Some("0.23.0-rc.1"), Some("desktop")));
        assert!(!launcher_too_old(Some("0.24.0"), Some("desktop")));
        assert!(!launcher_too_old(Some("0.25.1"), Some("desktop")));
        assert!(!launcher_too_old(Some("1.0.0"), Some("desktop")));
        // Never nag on a value that cannot be read.
        for garbage in ["garbage", "", "0.25", "0.25.1.2", "v0.23.0", "0.x.1"] {
            assert!(
                !launcher_too_old(Some(garbage), Some("desktop")),
                "{garbage:?}"
            );
        }
        // Only the standard extension has a launcher.
        for channel in [Some("mcpb"), Some("other"), None] {
            assert!(!launcher_too_old(Some("0.1.0"), channel), "{channel:?}");
            assert!(!launcher_too_old(None, channel), "{channel:?}");
        }
    }

    #[test]
    fn the_launcher_note_says_when_the_update_was_offered() {
        assert_eq!(
            launcher_note(Some("0.23.0"), Some("desktop"), false),
            Some(LAUNCHER_NOTE)
        );
        assert_eq!(
            launcher_note(Some("0.23.0"), Some("desktop"), true),
            Some(LAUNCHER_OFFERED_NOTE)
        );
        assert_eq!(launcher_note(Some("0.25.1"), Some("desktop"), true), None);
        assert_eq!(launcher_note(Some("0.23.0"), Some("mcpb"), true), None);
    }

    const LAUNCHER_INIT: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#;
    const LAUNCHER_DISCOVER: &str =
        r#"{"jsonrpc":"2.0","id":"d","method":"server/discover","params":{}}"#;

    fn instructions_of(line: &str) -> String {
        let msg: Value = serde_json::from_str(line).unwrap();
        msg["result"]["instructions"].as_str().unwrap().to_string()
    }

    #[test]
    fn an_old_launcher_adds_its_note_to_the_initialize_and_discover_answers() {
        let mut relay = RelayState {
            launcher_note: Some(LAUNCHER_NOTE.to_string()),
            ..RelayState::default()
        };
        relay.note_client_line(LAUNCHER_INIT);
        relay.note_client_line(LAUNCHER_DISCOVER);
        relay.note_client_line(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);

        let init = relay
            .note_server_line(r#"{"jsonrpc":"2.0","id":0,"result":{"instructions":"Route."}}"#)
            .expect("the initialize answer gets the note");
        assert_eq!(instructions_of(&init), format!("Route.\n\n{LAUNCHER_NOTE}"));
        let discover = relay
            .note_server_line(
                r#"{"jsonrpc":"2.0","id":"d","result":{"resultType":"complete","instructions":"Route."}}"#,
            )
            .expect("the server/discover answer gets the note");
        assert!(instructions_of(&discover).ends_with(&format!("\n\n{LAUNCHER_NOTE}")));
        assert!(
            relay
                .note_server_line(r#"{"jsonrpc":"2.0","id":3,"result":{"instructions":"x"}}"#)
                .is_none(),
            "a tools/list answer is forwarded as it is"
        );
        // A second answer to the same id (a replay) gets nothing more.
        assert!(
            relay
                .note_server_line(r#"{"jsonrpc":"2.0","id":0,"result":{"instructions":"x"}}"#)
                .is_none()
        );
        assert!(relay.onboarding.is_empty());
    }

    #[test]
    fn the_onboarding_note_joins_the_launcher_note_and_the_split_note() {
        assert_eq!(onboarding_note(None, None), None);
        assert_eq!(
            onboarding_note(Some(LAUNCHER_NOTE), None).as_deref(),
            Some(LAUNCHER_NOTE)
        );
        let split = "Claude Desktop keeps a second Crystalline state".to_string();
        assert_eq!(
            onboarding_note(None, Some(split.clone())),
            Some(split.clone())
        );
        assert_eq!(
            onboarding_note(Some(LAUNCHER_NOTE), Some(split.clone())),
            Some(format!("{LAUNCHER_NOTE}\n\n{split}"))
        );
    }

    #[test]
    fn the_launcher_note_leaves_other_answers_alone() {
        // No note held: the initialize answer is forwarded as it is.
        let mut relay = RelayState::default();
        relay.note_client_line(LAUNCHER_INIT);
        assert!(
            relay
                .note_server_line(r#"{"jsonrpc":"2.0","id":0,"result":{"instructions":"x"}}"#)
                .is_none()
        );
        // A note held, but the answer has no instructions or is an error.
        let mut relay = RelayState {
            launcher_note: Some(LAUNCHER_NOTE.to_string()),
            ..RelayState::default()
        };
        relay.note_client_line(LAUNCHER_INIT);
        relay.note_client_line(LAUNCHER_DISCOVER);
        assert!(
            relay
                .note_server_line(r#"{"jsonrpc":"2.0","id":0,"result":{"serverInfo":{}}}"#)
                .is_none()
        );
        assert!(
            relay
                .note_server_line(
                    r#"{"jsonrpc":"2.0","id":"d","error":{"code":-32602,"message":"m"}}"#
                )
                .is_none()
        );
        assert!(with_launcher_note("not json", LAUNCHER_NOTE).is_none());
        assert!(with_launcher_note(r#"{"result":{"instructions":7}}"#, LAUNCHER_NOTE).is_none());
    }

    #[tokio::test]
    async fn the_launcher_note_reaches_the_client_through_the_relay_loop() {
        let (bridge_side, daemon_side) = tokio::io::duplex(4096);
        let daemon = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(daemon_side);
            let mut lines = BufReader::new(read).lines();
            let request = lines.next_line().await.unwrap().unwrap();
            assert!(request.contains("initialize"));
            write
                .write_all(
                    b"{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"instructions\":\"Route.\"}}\n",
                )
                .await
                .unwrap();
            write.flush().await.unwrap();
            drop(write);
            drop(lines);
        });
        let (mut stdin_feed, stdin_read) = tokio::io::duplex(1024);
        stdin_feed
            .write_all(format!("{LAUNCHER_INIT}\n").as_bytes())
            .await
            .unwrap();
        let mut relay = RelayState {
            launcher_note: Some(LAUNCHER_NOTE.to_string()),
            ..RelayState::default()
        };
        let mut stdin = BoundedLines::new(stdin_read, STDIO_LINE_LIMIT);
        let mut stdout = Vec::new();
        let mut session = Session::new(bridge_side);
        relay_loop(&mut relay, &mut stdin, &mut stdout, &mut session)
            .await
            .unwrap();
        let out = String::from_utf8(stdout).unwrap();
        assert!(
            instructions_of(out.trim_end()).ends_with(LAUNCHER_NOTE),
            "{out}"
        );
        daemon.await.unwrap();
        drop(stdin_feed);
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
        // A cancelled onboarding request no longer waits for the note either.
        relay.note_client_line(r#"{"jsonrpc":"2.0","id":5,"method":"server/discover"}"#);
        assert!(relay.onboarding.contains("5"));
        relay.note_client_line(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":0}}"#,
        );
        relay.note_client_line(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":5}}"#,
        );
        assert!(relay.onboarding.is_empty());
        assert!(relay.outstanding.is_empty());
    }

    /// A JSON-RPC batch (an array line) is neither tracked nor settled, so no
    /// id from it can be left behind: a batch from the client records nothing,
    /// and a batched answer is forwarded verbatim, without the launcher note.
    #[test]
    fn relay_state_leaves_a_batch_untracked() {
        let mut relay = RelayState {
            launcher_note: Some(LAUNCHER_NOTE.to_string()),
            ..RelayState::default()
        };
        relay.note_client_line(
            r#"[{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}},{"jsonrpc":"2.0","id":1,"method":"server/discover"}]"#,
        );
        assert!(relay.outstanding.is_empty());
        assert!(relay.onboarding.is_empty());
        assert!(relay.init_request.is_none());
        let answer = r#"[{"jsonrpc":"2.0","id":0,"result":{"instructions":"x"}},{"jsonrpc":"2.0","id":1,"result":{"instructions":"y"}}]"#;
        assert_eq!(relay.note_server_line(answer), None);
        assert!(relay.outstanding.is_empty());
        assert!(relay.onboarding.is_empty());
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
        let mut stdin = BoundedLines::new(stdin_read, STDIO_LINE_LIMIT);
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
    /// 3. A probe costs nothing since rmcp 3.5.1 (#1248): answering one no
    ///    longer arms the one-way metadata latch, so a client that probes and
    ///    then falls back to `initialize` gets a whole legacy session, and its
    ///    first real call without `_meta` is served. Up to rmcp 3.5.0 that call
    ///    was refused `-32602`, which is what the dual-era residue in AGENTS.md
    ///    described.
    ///
    /// Step 1 also pins that a bare probe no longer ends the session: rmcp
    /// keeps waiting for either lifecycle, so a conforming probe on the same
    /// connection is still answered, and only the end of input ends it.
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
        // The session carries on after the refusal (rmcp 3.5.1, #1248): a
        // conforming probe on the same connection is answered too.
        let probe = complete_probe(
            serde_json::json!(1),
            crate::mcp::newest_served_protocol_version().as_str(),
        );
        write
            .write_all(format!("{probe}\n").as_bytes())
            .await
            .unwrap();
        write.flush().await.unwrap();
        let answer = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
            .await
            .expect("the session is still open after the bare probe")
            .unwrap()
            .expect("a message rather than a closed pipe");
        let v: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(v["id"], serde_json::json!(1), "{answer}");
        assert!(v["result"]["instructions"].is_string(), "{answer}");
        // Only the end of input ends it, still on neither lifecycle.
        write.shutdown().await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), server)
                .await
                .expect("the session ends with its input")
                .unwrap()
                .is_err(),
            "a session that chose no lifecycle ends as an error"
        );

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

        // 3. What a probe costs: nothing since rmcp 3.5.1 (#1248). The
        //    discover armed no latch, so a client that ignores the
        //    specification's "do not fall back to `initialize`" and hands us a
        //    legacy handshake anyway gets a whole legacy session, and its first
        //    real call without `_meta` is served.
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
        // by arrival: rmcp dispatches concurrently.
        let mut answers = std::collections::HashMap::new();
        for _ in 0..2 {
            let v: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            answers.insert(v["id"].clone(), v);
        }
        let handshake = &answers[&serde_json::json!(1)];
        assert_eq!(
            handshake["result"]["protocolVersion"],
            serde_json::json!(crate::mcp::newest_legacy_handshake_version().as_str()),
            "the legacy handshake still succeeds, at the revision rmcp negotiates \
             for one: {handshake}"
        );
        let listed = &answers[&serde_json::json!(2)];
        assert!(listed["error"].is_null(), "not refused: {listed}");
        assert!(
            listed["result"]["tools"].is_array(),
            "the legacy session serves a call without _meta: {listed}"
        );

        drop(write);
        let _ = server.await;
    }

    #[tokio::test]
    async fn the_session_opener_is_returned_verbatim_and_nothing_is_answered_here() {
        let input: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"server/discover\"}\n\
            extra-buffered-past-the-opener\n";
        let mut stdin = BoundedLines::new(input, STDIO_LINE_LIMIT);
        let opener = read_session_opener(&mut stdin)
            .await
            .unwrap()
            .expect("the probe opens the session instead of being consumed");
        assert_eq!(
            opener,
            Incoming::Line(BARE_PROBE.as_bytes().to_vec()),
            "the opener read rewrites nothing"
        );

        // Handed back, it is read first, and the line after it is still there,
        // so the serving path sees the session from its first line.
        stdin.replay(opener);
        assert_eq!(
            stdin.next_line().await.unwrap(),
            Some(Incoming::Line(BARE_PROBE.as_bytes().to_vec()))
        );
        assert_eq!(
            stdin.next_line().await.unwrap(),
            Some(Incoming::Line(b"extra-buffered-past-the-opener".to_vec()))
        );

        // An `initialize` opener comes back verbatim too: the legacy era is
        // untouched by any of this.
        let init: &[u8] =
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n";
        let mut stdin = BoundedLines::new(init, STDIO_LINE_LIMIT);
        let opener = read_session_opener(&mut stdin).await.unwrap().unwrap();
        assert_eq!(
            opener,
            Incoming::Line(
                br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_vec()
            )
        );
    }

    #[tokio::test]
    async fn read_session_opener_returns_none_on_eof() {
        let input: &[u8] = b"";
        let mut stdin = BoundedLines::new(input, STDIO_LINE_LIMIT);
        assert!(read_session_opener(&mut stdin).await.unwrap().is_none());
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
        let mut stdin = BoundedLines::new(stdin_read, STDIO_LINE_LIMIT);
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
    async fn a_replayed_opener_reaches_relay_loop_before_the_follow_up() {
        // `run_mcp` reads the `initialize` line before it knows who serves the
        // session and hands it back to the bounded reader, ahead of whatever
        // the client sent next. Relayed from a plain `RelayState::default()`,
        // exactly as `pump_stdio` does, the relay must forward `initialize`
        // first and the follow-up next, proving the handoff preserves
        // ordering.
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let follow: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
        let mut stdin = BoundedLines::new(follow, STDIO_LINE_LIMIT);
        stdin.replay(Incoming::Line(init.as_bytes().to_vec()));

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
        // The relay recorded the replayed initialize as the handshake, so a later
        // daemon restart can replay it.
        assert!(relay.init_request.as_ref().unwrap().contains("initialize"));
        assert_eq!(relay.init_id, Some(serde_json::json!(1)));
    }

    // --- a message over the stdio line limit: answered, never dropped -------
    //
    // rmcp drops a line over its limit with only a log line, so a client that
    // sent one waited forever. Both stdio paths now answer it with an Invalid
    // Request naming the limit, and carry on with the next line.

    /// A request of exactly `len` bytes (newline not counted), padded with
    /// JSON whitespace inside its params so it still parses.
    fn padded_request(id: u32, method: &str, len: usize) -> Vec<u8> {
        let head = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{{"#);
        let mut line = head.into_bytes();
        assert!(line.len() + 2 <= len, "the head fits in {len} bytes");
        line.resize(len - 2, b' ');
        line.extend_from_slice(b"}}");
        line
    }

    /// A notification over `len` bytes.
    fn oversized_notification(len: usize) -> Vec<u8> {
        let mut line =
            br#"{"jsonrpc":"2.0","method":"notifications/progress","params":{"p":""#.to_vec();
        line.resize(len + 1, b'x');
        line.extend_from_slice(br#""}}"#);
        line
    }

    const STEP: std::time::Duration = std::time::Duration::from_secs(20);

    /// Read stdout lines until every id in `ids` was answered, keyed by id.
    /// Every line must carry one of them: a notification over the limit
    /// leaves no answer behind.
    async fn answers_for<R: AsyncRead + Unpin>(
        lines: &mut tokio::io::Lines<BufReader<R>>,
        ids: &[Value],
    ) -> std::collections::HashMap<Value, Value> {
        let mut answers = std::collections::HashMap::new();
        while answers.len() < ids.len() {
            let line = tokio::time::timeout(STEP, lines.next_line())
                .await
                .expect("an answer instead of silence")
                .unwrap()
                .expect("an answer instead of a closed pipe");
            let v: Value = serde_json::from_str(&line).unwrap();
            assert!(ids.contains(&v["id"]), "an answer nobody asked for: {line}");
            answers.insert(v["id"].clone(), v);
        }
        answers
    }

    fn assert_refused(answer: &Value, limit: usize) {
        assert_eq!(
            answer["error"]["code"],
            serde_json::json!(-32600),
            "{answer}"
        );
        let message = answer["error"]["message"].as_str().unwrap();
        assert!(message.contains("not processed"), "{message}");
        if limit == STDIO_LINE_LIMIT {
            assert!(message.contains("16 MiB"), "{message}");
        }
    }

    /// The bridge: `lines` go in on stdin, the fake daemon answers every line
    /// it receives with an empty result for its id, and the relay runs until
    /// the daemon closes after `forwarded` lines. Returns stdout and what the
    /// daemon saw.
    async fn relay_through_bridge(
        input: Vec<u8>,
        limit: usize,
        forwarded: usize,
    ) -> (Vec<u8>, Vec<String>, RelayState) {
        let (bridge_side, daemon_side) = tokio::io::duplex(1 << 16);
        let daemon = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(daemon_side);
            let mut lines = BufReader::new(read).lines();
            let mut seen = Vec::new();
            for _ in 0..forwarded {
                let line = lines.next_line().await.unwrap().unwrap();
                let v: Value = serde_json::from_str(&line).unwrap();
                let answer = serde_json::json!({"jsonrpc": "2.0", "id": v["id"], "result": {}});
                write
                    .write_all(format!("{answer}\n").as_bytes())
                    .await
                    .unwrap();
                write.flush().await.unwrap();
                seen.push(line);
            }
            seen
        });
        let (mut stdin_feed, stdin_read) = tokio::io::duplex(1 << 16);
        let feeder = tokio::spawn(async move {
            stdin_feed.write_all(&input).await.unwrap();
            stdin_feed.flush().await.unwrap();
            // Held open: the relay ends on the daemon's close, not on stdin's.
            stdin_feed
        });
        let mut relay = RelayState::default();
        let mut stdin = BoundedLines::new(stdin_read, limit);
        let mut stdout = Vec::new();
        let mut session = Session::new(bridge_side);
        let (end, _) = tokio::time::timeout(
            STEP,
            relay_loop(&mut relay, &mut stdin, &mut stdout, &mut session),
        )
        .await
        .expect("the relay reads past the oversized lines")
        .unwrap();
        assert_eq!(end, SessionEnd::SocketClosed);
        let seen = daemon.await.unwrap();
        drop(feeder.await.unwrap());
        (stdout, seen, relay)
    }

    /// At the real limit on the bridge: a line of exactly the limit (newline
    /// included) reaches the daemon whole, one byte more is answered here and
    /// never forwarded, and the next request is relayed as usual.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_bridge_answers_a_line_over_the_limit_and_relays_the_next() {
        let limit = STDIO_LINE_LIMIT;
        let mut input = padded_request(1, "tools/list", limit - 1);
        input.push(b'\n');
        input.extend_from_slice(&padded_request(2, "tools/call", limit));
        input.push(b'\n');
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n");

        let (stdout, seen, relay) = relay_through_bridge(input, limit, 2).await;

        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[0].len(),
            limit - 1,
            "the line at the limit is forwarded whole"
        );
        assert!(seen[1].contains("\"id\":3"), "{}", seen[1]);
        let out = String::from_utf8(stdout).unwrap();
        let answers: Vec<Value> = out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(answers.len(), 3, "{out}");
        let refused = answers
            .iter()
            .find(|a| a["id"] == serde_json::json!(2))
            .expect("the oversized request is answered");
        assert_refused(refused, limit);
        assert!(
            relay.outstanding.is_empty(),
            "the refused request never waits for the daemon: {:?}",
            relay.outstanding
        );
    }

    /// The bridge at a small limit: a notification over it is dropped with no
    /// answer, and the request after it is relayed.
    #[tokio::test]
    async fn the_bridge_drops_a_notification_over_the_limit_without_an_answer() {
        let limit = 256;
        let mut input = oversized_notification(limit);
        input.push(b'\n');
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"ping\"}\n");

        let (stdout, seen, _) = relay_through_bridge(input, limit, 1).await;

        assert_eq!(seen.len(), 1);
        assert!(seen[0].contains("\"id\":5"), "{}", seen[0]);
        let out = String::from_utf8(stdout).unwrap();
        assert_eq!(out.lines().count(), 1, "only the ping's answer: {out}");
    }

    async fn in_memory_engine(tmp: &Path) -> Arc<Engine> {
        let store = crystalline_index::TursoStore::open_in_memory()
            .await
            .unwrap();
        Arc::new(Engine::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            crystalline_core::config::GlobalConfig::default(),
            None,
            Some(tmp.join("config.yaml")),
        ))
    }

    const INITIALIZE: &[u8] =
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\
        \"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\
        \"clientInfo\":{\"name\":\"limit\",\"version\":\"0\"}}}\n";

    /// The embedded path, through [`serve_guarded`] with a real server:
    /// `initialize`, a `tools/list` of exactly the limit (answered), one a
    /// byte over (refused), a notification over it (no answer) and a
    /// `tools/list` after them (answered). Then stdin closes and the serve
    /// returns.
    async fn serve_embedded_around_the_limit(limit: usize) {
        let tmp = tempfile::tempdir().unwrap();
        let engine = in_memory_engine(tmp.path()).await;
        let (mut stdin_feed, stdin_read) = tokio::io::duplex(1 << 16);
        let (stdout_write, stdout_read) = tokio::io::duplex(1 << 16);
        let served = tokio::spawn(serve_guarded(
            McpServer::new(engine),
            BoundedLines::new(stdin_read, limit),
            stdout_write,
        ));
        let mut out = BufReader::new(stdout_read).lines();

        let mut input = INITIALIZE.to_vec();
        input.extend_from_slice(&padded_request(2, "tools/list", limit - 1));
        input.push(b'\n');
        input.extend_from_slice(&padded_request(3, "tools/list", limit));
        input.push(b'\n');
        input.extend_from_slice(&oversized_notification(limit));
        input.push(b'\n');
        input.extend_from_slice(
            b"{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/list\",\"params\":{}}\n",
        );
        let feeder = tokio::spawn(async move {
            stdin_feed.write_all(&input).await.unwrap();
            stdin_feed.flush().await.unwrap();
            stdin_feed
        });

        let ids: Vec<Value> = (1..=4).map(|id| serde_json::json!(id)).collect();
        let answers = answers_for(&mut out, &ids).await;
        assert!(
            answers[&ids[0]]["result"].is_object(),
            "{:?}",
            answers[&ids[0]]
        );
        assert!(
            answers[&ids[1]]["result"]["tools"].is_array(),
            "the request at the limit is served: {:?}",
            answers[&ids[1]]
        );
        assert_refused(&answers[&ids[2]], limit);
        assert!(
            answers[&ids[3]]["result"]["tools"].is_array(),
            "the session carries on: {:?}",
            answers[&ids[3]]
        );

        // The end of input ends the session, with nothing else written: the
        // notification left no answer behind.
        drop(feeder.await.unwrap());
        let rest = tokio::time::timeout(STEP, out.next_line())
            .await
            .expect("stdout closes with the session")
            .unwrap();
        assert!(rest.is_none(), "nothing after the answers: {rest:?}");
        tokio::time::timeout(STEP, served)
            .await
            .expect("the serve returns at the end of input")
            .unwrap()
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_embedded_stack_answers_a_line_over_the_limit_and_serves_the_next() {
        serve_embedded_around_the_limit(STDIO_LINE_LIMIT).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_embedded_stack_bounds_lines_at_any_limit() {
        serve_embedded_around_the_limit(4096).await;
    }

    /// A last line of exactly the limit with no newline is refused by the
    /// bridge, never forwarded: with the newline the relay adds it would be one
    /// byte over at the daemon, which drops it in silence.
    #[tokio::test]
    async fn the_bridge_refuses_a_last_line_of_the_limit_without_a_newline() {
        let limit = 256;
        let input = padded_request(9, "tools/call", limit);
        let (bridge_side, mut daemon_side) = tokio::io::duplex(4096);
        let mut relay = RelayState::default();
        let mut stdin = BoundedLines::new(input.as_slice(), limit);
        let mut stdout = Vec::new();
        let mut session = Session::new(bridge_side);
        let (end, _) = relay_loop(&mut relay, &mut stdin, &mut stdout, &mut session)
            .await
            .unwrap();
        assert_eq!(end, SessionEnd::StdinClosed);
        let mut forwarded = Vec::new();
        daemon_side.read_to_end(&mut forwarded).await.unwrap();
        assert!(forwarded.is_empty(), "nothing reaches the daemon");
        let answer: Value = serde_json::from_slice(stdout.trim_ascii_end()).unwrap();
        assert_eq!(answer["id"], serde_json::json!(9));
        assert_refused(&answer, limit);
    }

    /// The same on the embedded path: the last line, `limit` bytes with no
    /// newline, is refused rather than handed to rmcp one byte over.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_embedded_stack_refuses_a_last_line_of_the_limit_without_a_newline() {
        let limit = 4096;
        let tmp = tempfile::tempdir().unwrap();
        let engine = in_memory_engine(tmp.path()).await;
        let mut input = INITIALIZE.to_vec();
        input.extend_from_slice(&padded_request(2, "tools/list", limit));
        let (stdout_write, stdout_read) = tokio::io::duplex(1 << 16);
        let served = tokio::spawn(serve_guarded(
            McpServer::new(engine),
            BoundedLines::new(std::io::Cursor::new(input), limit),
            stdout_write,
        ));
        let mut out = BufReader::new(stdout_read).lines();
        let ids = [serde_json::json!(1), serde_json::json!(2)];
        let answers = answers_for(&mut out, &ids).await;
        assert!(
            answers[&ids[0]]["result"].is_object(),
            "{:?}",
            answers[&ids[0]]
        );
        assert_refused(&answers[&ids[1]], limit);
        let _ = tokio::time::timeout(STEP, served)
            .await
            .expect("the serve returns at the end of input");
    }

    /// When rmcp gives a session up on its own while the client keeps stdin
    /// open, [`serve_guarded`] returns and the runtime it ran on can be
    /// dropped at once: stdin is read through [`read_on_thread`], so no read
    /// of it is parked on the runtime's blocking pool, which a runtime waits
    /// for at its drop. The session is a modern `server/discover` and then a
    /// notification, which rmcp's start-up loop refuses.
    #[test]
    fn a_session_rmcp_ends_lets_the_runtime_go_with_stdin_still_open() {
        let (stdin_read, mut stdin_write) = std::io::pipe().unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let out = runtime.block_on(async move {
                let tmp = tempfile::tempdir().unwrap();
                let engine = in_memory_engine(tmp.path()).await;
                let (stdout_write, mut stdout_read) = tokio::io::duplex(1 << 16);
                let collected = tokio::spawn(async move {
                    let mut out = Vec::new();
                    stdout_read.read_to_end(&mut out).await.unwrap();
                    out
                });
                let stdin =
                    BoundedLines::new(read_on_thread(stdin_read).unwrap(), STDIO_LINE_LIMIT);
                let _ = serve_guarded(McpServer::new(engine), stdin, stdout_write).await;
                collected.await.unwrap()
            });
            drop(runtime);
            let _ = done.send(out);
        });

        let probe = complete_probe(
            serde_json::json!(1),
            crate::mcp::newest_served_protocol_version().as_str(),
        );
        std::io::Write::write_all(
            &mut stdin_write,
            format!("{probe}\n{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}\n")
                .as_bytes(),
        )
        .unwrap();
        let out = finished
            .recv_timeout(STEP)
            .expect("the session ends and the runtime drops with stdin still open");
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains("\"id\":1"),
            "the probe was answered first: {out}"
        );
        drop(stdin_write);
    }
}
