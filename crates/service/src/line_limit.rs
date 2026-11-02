//! The size limit on one MCP message over stdio, and the answer a message over
//! it gets.
//!
//! rmcp's stdio transport reads at most [`STDIO_LINE_LIMIT`] bytes per line and
//! drops a longer line with nothing but a log line of its own
//! (`AsyncRwTransport::receive`, since rmcp 3.6.0). The client is never told:
//! it waits for an answer that never comes, so a large `write_engram` from a
//! local agent hangs the session. Over HTTP the same message meets a `413`
//! from the body ceiling ([`crate::rest::MAX_TOOL_BODY_BYTES`]); stdio had no
//! counterpart.
//!
//! [`BoundedLines`] closes that gap. It reads stdin in lines exactly the way
//! rmcp does, with the same limit counted the same way, but a line over the
//! limit comes back as [`Incoming::Oversized`] instead of vanishing, and
//! [`Oversized::refusal`] turns it into a JSON-RPC error the client reads. Both
//! stdio paths read stdin through it, so they cannot drift apart:
//!
//! - the bridge to the daemon (`client::relay_loop`) answers the refusal itself
//!   on stdout and never forwards the line;
//! - the embedded stack and the degraded status server ([`serve_guarded`])
//!   hand rmcp an in-memory pipe and run a pump in front of it, so every line
//!   rmcp sees is one it accepts, and the refusal is written by the single task
//!   that owns stdout.
//!
//! **Memory stays bounded.** A line is buffered only up to the limit; past it
//! the buffer is dropped and the rest of the line is read and discarded up to
//! its newline, and the session carries on with the next line. What the
//! refusal needs from the dropped line - its `id`, and whether it was a request
//! at all - is picked up by [`Scan`] in constant memory while the bytes stream
//! past.

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// The largest message, in bytes and counting its terminating newline, that
/// Crystalline accepts on a stdio transport: rmcp's own
/// [`DEFAULT_MAX_LINE_LENGTH`](rmcp::transport::async_rw::DEFAULT_MAX_LINE_LENGTH),
/// 16 MiB, imported rather than copied so the two cannot part.
///
/// **Why not [`crate::rest::MAX_BODY_BYTES`] (10 MiB), the HTTP ceiling.** A
/// stdio write between 10 and 16 MiB works today, so the HTTP number would
/// refuse messages that are served now. No byte cap applies to the text of a
/// `write_engram`, `edit_engram` or `write_reflection` anywhere below the
/// transport (the metadata index only leaves out an oversized frontmatter
/// key or value, it refuses no write), so such a call is bounded by the
/// transport alone, which on stdio is rmcp's 16 MiB line. Attachments are
/// not the deciding case either: an agent uploads a file with `write_engram`
/// (`path`) or `write_reflection` (`write_asset`), base64-encoded at up to
/// `4 * ceil(MAX_ATTACHMENT_BYTES / 3)` = 13,981,016 bytes (13.33 MiB) plus
/// the JSON-RPC envelope, which is under this limit; a file travels out the
/// same way in a read. The HTTP counterpart is
/// [`crate::rest::MAX_TOOL_BODY_BYTES`] (15 MiB). So the limit is
/// exactly the line rmcp would drop: every message that is served today is
/// still served, and every message rmcp would drop silently is answered.
///
/// The daemon's socket transport is built with the same constant
/// (`daemon::handle_conn`), so the bridge, which forwards only lines within
/// it, never hands the daemon a line the daemon would drop.
pub(crate) const STDIO_LINE_LIMIT: usize = rmcp::transport::async_rw::DEFAULT_MAX_LINE_LENGTH;

/// The JSON-RPC code for a message the server will not process as sent:
/// Invalid Request. The message may be perfectly formed; it is the transport
/// that refuses it, and this is the code rmcp itself uses for a line it reads
/// but cannot take.
const INVALID_REQUEST: i64 = -32600;

/// How many bytes of a top-level `id` value [`Scan`] keeps. JSON-RPC ids are
/// numbers and short strings; a longer one is treated as unreadable and the
/// refusal goes out with a `null` id, as JSON-RPC prescribes for an id that
/// could not be read.
const ID_CAPTURE_BYTES: usize = 256;

/// How many bytes of a top-level key [`Scan`] keeps: enough for the longest key
/// it looks for (`method`, `result`).
const KEY_CAPTURE_BYTES: usize = 16;

/// The capacity of the pipe between the pump and rmcp in [`serve_guarded`],
/// in each direction. Only a buffer: a line of any size up to the limit passes
/// through it in pieces.
const PIPE_CAPACITY: usize = 1 << 16;

/// One item read off stdin by [`BoundedLines`].
#[derive(Debug, PartialEq)]
pub(crate) enum Incoming {
    /// A line within the limit, without its `\n` or `\r\n`.
    Line(Vec<u8>),
    /// A line over the limit. Its bytes are gone; what is left is what the
    /// answer needs.
    Oversized(Oversized),
}

/// What is known about a line that was over the limit, collected by [`Scan`]
/// while it was discarded.
#[derive(Debug, PartialEq)]
pub(crate) struct Oversized {
    /// The limit the line exceeded, for the refusal's message.
    limit: usize,
    /// What the line was, as far as its top-level keys say.
    kind: OversizedKind,
}

/// The shape of an oversized line, read from its top-level keys.
#[derive(Debug, PartialEq)]
enum OversizedKind {
    /// A request, or anything that cannot be told apart from one (not an
    /// object, or no `method` at all). It is answered; `id` is the request's
    /// own id when one was read, else the answer carries `null`.
    Request { id: Option<Value> },
    /// A `method` with no `id`: a notification. JSON-RPC forbids an answer to
    /// one, so it is only logged.
    Notification,
    /// No `method` but a `result` or `error`: the client answering a request
    /// of ours. Answering an answer is not a thing, so it is only logged.
    Response,
}

impl Oversized {
    /// The JSON-RPC error line that answers this message, or `None` for a
    /// notification or a response, which get no answer. Either way the drop is
    /// logged, since for those two the log is all that is left of it.
    ///
    /// The id is written as `null` when it could not be read, which is what
    /// JSON-RPC 2.0 (section 5) asks for. rmcp's own error type would omit
    /// the key instead (the 2026-07-28 revision allows that), so the line is
    /// built here rather than through rmcp: `null` is read correctly by a
    /// client of either era, a missing id only by a modern one.
    pub(crate) fn refusal(&self) -> Option<String> {
        let id = match &self.kind {
            OversizedKind::Request { id } => id.clone().unwrap_or(Value::Null),
            OversizedKind::Notification | OversizedKind::Response => {
                tracing::warn!(
                    limit = self.limit,
                    kind = ?self.kind,
                    "dropped an incoming MCP message over the stdio line limit; \
                     it is not a request, so nothing is answered"
                );
                return None;
            }
        };
        tracing::warn!(
            limit = self.limit,
            id = %id,
            "refused an incoming MCP message over the stdio line limit"
        );
        let reply = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": INVALID_REQUEST,
                "message": format!(
                    "The message was not processed: one message on Crystalline's stdio \
                     transport may be at most {}, newline included, and this one is larger. \
                     Send less in one call, for example split a large write into several.",
                    describe_bytes(self.limit)
                ),
            },
        });
        Some(reply.to_string())
    }
}

/// A byte count as the refusal names it: in MiB when it is a whole number of
/// them, as the production limit is, else in bytes.
fn describe_bytes(bytes: usize) -> String {
    const MIB: usize = 1024 * 1024;
    if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
}

/// Lines off a reader, bounded the way rmcp bounds them: a line is accepted
/// when its bytes plus its terminating newline fit in `limit`
/// (`AsyncRwTransport::read_line` counts `line_buf.len() + take > max`, with
/// the newline inside `take`). So the longest line served is `limit - 1` bytes
/// of content and a `\n`; a `\r\n` line gets one byte less.
///
/// **Cancel safe**, like tokio's `Lines::next_line`, which it replaces in
/// `relay_loop`'s `select!`: every partial line, the discarding flag and the
/// scan live on the struct, and each `fill_buf` is followed by its `consume`
/// with no await between them, so a cancelled read resumes where it stopped
/// and loses no byte.
///
/// A last line without a newline is still returned at end of input, as
/// tokio's `Lines` returns it, with the newline its forwarder adds counted
/// against the limit.
pub(crate) struct BoundedLines<R> {
    reader: BufReader<R>,
    limit: usize,
    /// The line read so far, never longer than `limit`.
    line: Vec<u8>,
    /// Set from the moment a line crossed the limit until its newline.
    discarding: bool,
    /// The keys of the line being discarded.
    scan: Scan,
    /// An item handed back with [`BoundedLines::replay`], returned before
    /// anything else is read.
    replay: Option<Incoming>,
}

impl<R: AsyncRead + Unpin> BoundedLines<R> {
    /// Read `reader` in lines of at most `limit` bytes, newline included.
    /// Production passes [`STDIO_LINE_LIMIT`].
    pub(crate) fn new(reader: R, limit: usize) -> Self {
        BoundedLines {
            reader: BufReader::new(reader),
            limit,
            line: Vec::new(),
            discarding: false,
            scan: Scan::default(),
            replay: None,
        }
    }

    /// Hand `item` back so the next [`BoundedLines::next_line`] returns it.
    /// `client::run_mcp` reads the session opener before it knows who will
    /// serve the session, and gives it back for the serving path to read as
    /// its first line.
    pub(crate) fn replay(&mut self, item: Incoming) {
        debug_assert!(self.replay.is_none(), "one item is replayed at a time");
        self.replay = Some(item);
    }

    /// The next line, `Ok(None)` at end of input.
    pub(crate) async fn next_line(&mut self) -> std::io::Result<Option<Incoming>> {
        if let Some(item) = self.replay.take() {
            return Ok(Some(item));
        }
        loop {
            // The decision is made while `fill_buf`'s borrow is alive and
            // applied after it, so `consume` always follows in the same poll.
            let (consumed, done) = {
                let available = self.reader.fill_buf().await?;
                if available.is_empty() {
                    return Ok(self.at_end());
                }
                let newline = available.iter().position(|b| *b == b'\n');
                let take = newline.map_or(available.len(), |at| at + 1);
                let chunk = &available[..take];
                let done = if self.discarding {
                    self.scan.feed(chunk);
                    newline.map(|_| self.finish_oversized())
                } else if self.line.len().saturating_add(take) > self.limit {
                    // The line crossed the limit here: what was buffered is
                    // scanned and dropped, and so is the rest as it arrives.
                    self.scan = Scan::default();
                    self.scan.feed(&self.line);
                    self.scan.feed(chunk);
                    self.line = Vec::new();
                    if newline.is_some() {
                        Some(self.finish_oversized())
                    } else {
                        self.discarding = true;
                        None
                    }
                } else {
                    self.line.extend_from_slice(chunk);
                    newline.map(|_| Incoming::Line(self.take_line()))
                };
                (take, done)
            };
            self.reader.consume(consumed);
            if let Some(item) = done {
                return Ok(Some(item));
            }
        }
    }

    /// End of input: a line being discarded is answered like any other, a
    /// partial line is returned as it is, and after that there is nothing.
    ///
    /// A partial line has no newline yet, and both paths forward it with one
    /// added, so the newline is counted here: a last line of exactly `limit`
    /// bytes would reach rmcp as `limit + 1` and be dropped there in silence,
    /// so it is refused instead, like any line one byte over.
    fn at_end(&mut self) -> Option<Incoming> {
        if self.discarding {
            return Some(self.finish_oversized());
        }
        if self.line.is_empty() {
            return None;
        }
        if self.line.len().saturating_add(1) > self.limit {
            self.scan = Scan::default();
            self.scan.feed(&self.line);
            self.line = Vec::new();
            return Some(self.finish_oversized());
        }
        Some(Incoming::Line(self.take_line()))
    }

    fn finish_oversized(&mut self) -> Incoming {
        self.discarding = false;
        let scan = std::mem::take(&mut self.scan);
        Incoming::Oversized(Oversized {
            limit: self.limit,
            kind: scan.kind(),
        })
    }

    /// The buffered line without its `\n` or `\r\n`.
    fn take_line(&mut self) -> Vec<u8> {
        let mut line = std::mem::take(&mut self.line);
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        line
    }
}

/// Where [`Scan`] stands inside the current member of the top-level object.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum Member {
    /// Waiting for a key (after `{` or `,`).
    #[default]
    Key,
    /// Inside the key's string.
    InKey,
    /// After the key, waiting for its `:`.
    Colon,
    /// Inside the value, until the `,` or `}` that ends it at depth one.
    Value,
}

/// A constant-memory reader of a JSON object's top-level keys, fed the bytes
/// of a line too large to keep.
///
/// It learns the three things a refusal needs and nothing else: the value of
/// the top-level `id`, whether there is a top-level `method`, and whether
/// there is a top-level `result` or `error`. It reads the whole line, so key
/// order does not matter - a client that writes `id` after 16 MiB of params
/// still gets its id back - and it tracks depth and strings, so an `id` nested
/// inside the params is never taken for the request's own.
///
/// It is a tokenizer, not a validator: a line that is not JSON at all ends with
/// nothing found, and its refusal carries a `null` id.
#[derive(Debug, Default)]
struct Scan {
    /// Nesting depth; the top-level object's members are at depth one.
    depth: u32,
    /// Whether the line opened with `{`. A top-level array (a batch) has no
    /// keys to read.
    object: bool,
    in_string: bool,
    escaped: bool,
    member: Member,
    /// The current top-level key, up to [`KEY_CAPTURE_BYTES`].
    key: Vec<u8>,
    /// The raw bytes of the top-level `id` value while they stream past.
    capture: Option<Vec<u8>>,
    /// The top-level `id`, once its value ended and parsed.
    id: Option<Value>,
    /// Whether a top-level `id` key was seen at all, readable or not: an id
    /// too long to keep still means the message is a request.
    id_seen: bool,
    method: bool,
    outcome: bool,
}

impl Scan {
    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.step(b);
        }
    }

    fn at_top(&self) -> bool {
        self.object && self.depth == 1
    }

    fn step(&mut self, b: u8) {
        if self.in_string {
            if self.escaped {
                self.escaped = false;
            } else if b == b'\\' {
                self.escaped = true;
            } else if b == b'"' {
                self.in_string = false;
            }
            if self.member == Member::InKey && self.at_top() {
                if self.in_string {
                    if self.key.len() < KEY_CAPTURE_BYTES {
                        self.key.push(b);
                    }
                } else {
                    self.member = Member::Colon;
                }
            } else {
                self.capture(b);
            }
            return;
        }
        match b {
            b'"' => {
                self.in_string = true;
                if self.at_top() && self.member == Member::Key {
                    self.member = Member::InKey;
                    self.key.clear();
                } else {
                    self.capture(b);
                }
            }
            b'{' | b'[' => {
                if self.depth == 0 {
                    self.object = b == b'{';
                    self.member = Member::Key;
                } else {
                    self.capture(b);
                }
                self.depth = self.depth.saturating_add(1);
            }
            b'}' | b']' => {
                if self.at_top() {
                    self.end_member();
                } else {
                    self.capture(b);
                }
                self.depth = self.depth.saturating_sub(1);
            }
            b':' if self.at_top() && self.member == Member::Colon => {
                self.member = Member::Value;
                match self.key.as_slice() {
                    b"id" => {
                        self.id_seen = true;
                        self.capture = Some(Vec::new());
                    }
                    b"method" => self.method = true,
                    b"result" | b"error" => self.outcome = true,
                    _ => {}
                }
            }
            b',' if self.at_top() => {
                self.end_member();
                self.member = Member::Key;
            }
            _ => self.capture(b),
        }
    }

    /// Keep `b` when the `id` value is streaming past, and give up on an id
    /// longer than [`ID_CAPTURE_BYTES`].
    fn capture(&mut self, b: u8) {
        if let Some(raw) = &mut self.capture {
            if raw.len() < ID_CAPTURE_BYTES {
                raw.push(b);
            } else {
                self.capture = None;
            }
        }
    }

    /// The current top-level member ended: an `id` value being captured is
    /// parsed now that it is whole.
    fn end_member(&mut self) {
        if let Some(raw) = self.capture.take() {
            self.id = serde_json::from_slice(&raw).ok();
        }
    }

    fn kind(mut self) -> OversizedKind {
        // A line that ended inside the id's value (cut off, or no closing
        // brace) still offers what was captured.
        self.end_member();
        if self.method && !self.id_seen {
            OversizedKind::Notification
        } else if !self.method && self.outcome {
            OversizedKind::Response
        } else {
            OversizedKind::Request { id: self.id }
        }
    }
}

/// How many bytes the stdin thread of [`read_on_thread`] reads at once, and
/// how many such chunks may wait for the reader: a bound on what is held
/// between the two, so a client that writes faster than lines are served is
/// slowed down by the pipe rather than buffered without end.
const THREAD_CHUNK_BYTES: usize = 64 * 1024;
const THREAD_CHUNKS_QUEUED: usize = 4;

/// Read `source` on a dedicated OS thread and hand the bytes over as an
/// [`AsyncRead`]: what `client::run_mcp` reads stdin through.
///
/// **Why not `tokio::io::stdin()`.** tokio reads stdin on its blocking pool,
/// and a read there cannot be cancelled: dropping the future leaves the
/// thread inside `read` until the client writes or closes stdin. The runtime
/// waits for its blocking pool when it is dropped, so when rmcp ends a session
/// on its own while the client keeps stdin open (a notification where the
/// start-up loop expects a request, a failed write to stdout, a failed
/// handshake), the embedded stack and the degraded status server never got
/// to exit: [`serve_guarded`] keeps a read of stdin pending at all times,
/// which a plain rmcp transport only did while it was serving. A thread of our
/// own is not the runtime's to wait for; the process exit ends it wherever it
/// stands.
///
/// The thread ends by itself at the end of input, on a read error (handed on
/// as the reader's error) and once the reader is dropped.
pub(crate) fn read_on_thread<S>(source: S) -> std::io::Result<ThreadReader>
where
    S: std::io::Read + Send + 'static,
{
    let (chunks, received) = tokio::sync::mpsc::channel(THREAD_CHUNKS_QUEUED);
    std::thread::Builder::new()
        .name("crystalline-stdin".into())
        .spawn(move || pass_on(source, &chunks))?;
    Ok(ThreadReader {
        chunks: received,
        chunk: Vec::new(),
        at: 0,
    })
}

/// The loop of the thread [`read_on_thread`] starts.
fn pass_on<S: std::io::Read>(
    mut source: S,
    chunks: &tokio::sync::mpsc::Sender<std::io::Result<Vec<u8>>>,
) {
    loop {
        let mut chunk = vec![0; THREAD_CHUNK_BYTES];
        let sent = match source.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => {
                chunk.truncate(n);
                chunks.blocking_send(Ok(chunk))
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = chunks.blocking_send(Err(e));
                return;
            }
        };
        if sent.is_err() {
            // The reader is gone; nobody wants the rest.
            return;
        }
    }
}

/// The reading end of [`read_on_thread`].
pub(crate) struct ThreadReader {
    chunks: tokio::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>,
    /// The chunk being handed out, and how far into it.
    chunk: Vec<u8>,
    at: usize,
}

impl AsyncRead for ThreadReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;
        if self.at == self.chunk.len() {
            match self.chunks.poll_recv(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.chunk = chunk;
                    self.at = 0;
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                // The thread is done: end of input.
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
        let n = (self.chunk.len() - self.at).min(buf.remaining());
        let at = self.at;
        buf.put_slice(&self.chunk[at..at + n]);
        self.at += n;
        Poll::Ready(Ok(()))
    }
}

/// Serve `server` over stdio with every incoming line bounded by
/// [`BoundedLines`]: the embedded stack and the degraded status server.
///
/// rmcp is handed one end of an in-memory pipe instead of stdin and stdout. A
/// pump task feeds it the lines within the limit; a refused line never reaches
/// it. The answers from rmcp and the refusals both go to `stdout` from one
/// loop, line by line, because two writers on one stdout could interleave
/// inside a frame. Input and output are pumped by separate tasks so a large
/// request on its way in can never wait on a large answer on its way out.
///
/// It returns when rmcp's session ends, after the last answer was written:
/// rmcp drops its end of the pipe, the output loop reads to its end, writes
/// what is left and flushes.
pub(crate) async fn serve_guarded<S, R, W>(
    server: S,
    stdin: BoundedLines<R>,
    stdout: W,
) -> anyhow::Result<()>
where
    S: rmcp::Service<rmcp::RoleServer>,
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (rmcp_side, pump_side) = tokio::io::duplex(PIPE_CAPACITY);
    let pump = tokio::spawn(pump(stdin, pump_side, stdout));
    let served = async {
        let running = rmcp::serve_server(server, rmcp_side).await?;
        let _ = running.waiting().await;
        anyhow::Ok(())
    }
    .await;
    match pump.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("writing the MCP answers to stdout failed: {e}"),
        Err(e) => tracing::warn!("the stdio pump stopped: {e}"),
    }
    served
}

/// The two directions of [`serve_guarded`]'s pipe: stdin through the bound
/// into rmcp, and rmcp's answers plus the refusals out to `stdout`.
async fn pump<R, W>(
    mut stdin: BoundedLines<R>,
    rmcp: tokio::io::DuplexStream,
    mut stdout: W,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    let (from_rmcp, mut to_rmcp) = tokio::io::split(rmcp);
    let (refused, mut refusals) = tokio::sync::mpsc::unbounded_channel::<String>();
    let input = tokio::spawn(async move {
        if let Err(e) = feed(&mut stdin, &mut to_rmcp, &refused).await {
            tracing::warn!("reading the MCP client's stdin failed: {e}");
        }
        // End of input, or a read that failed: rmcp sees end of input either
        // way and ends the session, which is what closes the output loop.
        let _ = to_rmcp.shutdown().await;
    });
    let mut from_rmcp = BufReader::new(from_rmcp);
    let mut answer = Vec::new();
    loop {
        tokio::select! {
            // Refusals first: one queued before the input ended must be out
            // before the end of rmcp's output ends the loop.
            biased;
            Some(reply) = refusals.recv() => write_line(&mut stdout, reply.as_bytes()).await?,
            read = from_rmcp.read_until(b'\n', &mut answer) => {
                if read? == 0 {
                    break;
                }
                stdout.write_all(&answer).await?;
                stdout.flush().await?;
                answer.clear();
            }
        }
    }
    // rmcp is gone, so nothing more is fed to it; a refusal still queued is
    // answered all the same.
    input.abort();
    while let Ok(reply) = refusals.try_recv() {
        write_line(&mut stdout, reply.as_bytes()).await?;
    }
    stdout.flush().await
}

/// Feed every line within the limit to rmcp and queue a refusal for every
/// line over it, until the end of input.
async fn feed<R, W>(
    stdin: &mut BoundedLines<R>,
    to_rmcp: &mut W,
    refused: &tokio::sync::mpsc::UnboundedSender<String>,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    while let Some(item) = stdin.next_line().await? {
        match item {
            Incoming::Line(line) => write_line(to_rmcp, &line).await?,
            Incoming::Oversized(over) => {
                if let Some(reply) = over.refusal() {
                    let _ = refused.send(reply);
                }
            }
        }
    }
    Ok(())
}

async fn write_line<W: AsyncWrite + Unpin>(out: &mut W, line: &[u8]) -> std::io::Result<()> {
    out.write_all(line).await?;
    out.write_all(b"\n").await?;
    out.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: usize = 64;

    async fn all(input: &[u8], limit: usize) -> Vec<Incoming> {
        let mut lines = BoundedLines::new(input, limit);
        let mut out = Vec::new();
        while let Some(item) = lines.next_line().await.unwrap() {
            out.push(item);
        }
        out
    }

    fn line_of(len: usize) -> Vec<u8> {
        vec![b' '; len]
    }

    fn reply_of(item: &Incoming) -> Value {
        let Incoming::Oversized(over) = item else {
            panic!("expected an oversized line, got {item:?}");
        };
        serde_json::from_str(&over.refusal().expect("a request is answered")).unwrap()
    }

    fn kind_of(item: &Incoming) -> &OversizedKind {
        let Incoming::Oversized(over) = item else {
            panic!("expected an oversized line, got {item:?}");
        };
        &over.kind
    }

    /// A request padded with JSON whitespace to `len` bytes, newline not
    /// counted, so the size is exact and the line still parses.
    fn padded(id: u32, len: usize) -> Vec<u8> {
        let head = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/list","params":{{"#);
        let mut line = head.into_bytes();
        line.resize(len - 2, b' ');
        line.extend_from_slice(b"}}");
        line
    }

    /// The limit is counted as rmcp counts it, newline included: `limit - 1`
    /// bytes and a `\n` is the longest line served, one byte more is refused,
    /// and the line after a refused one is read as usual.
    #[tokio::test]
    async fn the_limit_counts_the_newline_like_rmcp() {
        let mut input = padded(1, LIMIT - 1);
        input.push(b'\n');
        input.extend_from_slice(&padded(2, LIMIT));
        input.push(b'\n');
        input.extend_from_slice(b"{\"id\":3}\n");
        let items = all(&input, LIMIT).await;
        assert_eq!(items.len(), 3, "{items:?}");
        assert_eq!(items[0], Incoming::Line(padded(1, LIMIT - 1)));
        assert_eq!(reply_of(&items[1])["id"], serde_json::json!(2));
        assert_eq!(items[2], Incoming::Line(b"{\"id\":3}".to_vec()));
    }

    /// A `\r\n` line strips both bytes and counts both, as rmcp does.
    #[tokio::test]
    async fn a_crlf_line_counts_its_carriage_return() {
        let mut input = line_of(LIMIT - 2);
        input.extend_from_slice(b"\r\n");
        input.extend_from_slice(&line_of(LIMIT - 1));
        input.extend_from_slice(b"\r\n");
        let items = all(&input, LIMIT).await;
        assert_eq!(items[0], Incoming::Line(line_of(LIMIT - 2)));
        assert!(matches!(items[1], Incoming::Oversized(_)), "{items:?}");
    }

    /// The refusal is an Invalid Request that names the limit and says the
    /// message was not processed; the production limit reads in MiB.
    #[test]
    fn the_refusal_names_the_limit_and_says_nothing_was_processed() {
        let over = Oversized {
            limit: STDIO_LINE_LIMIT,
            kind: OversizedKind::Request {
                id: Some(serde_json::json!("w-1")),
            },
        };
        let reply: Value = serde_json::from_str(&over.refusal().unwrap()).unwrap();
        assert_eq!(reply["jsonrpc"], serde_json::json!("2.0"));
        assert_eq!(reply["id"], serde_json::json!("w-1"));
        assert_eq!(reply["error"]["code"], serde_json::json!(-32600));
        let message = reply["error"]["message"].as_str().unwrap();
        assert!(message.contains("16 MiB"), "{message}");
        assert!(message.contains("not processed"), "{message}");
        assert_eq!(describe_bytes(LIMIT), "64 bytes");
    }

    /// The id is found wherever it sits among the top-level keys, after the
    /// params too, and an `id` nested in the params is never taken for it.
    #[tokio::test]
    async fn the_id_is_read_after_the_params_and_never_from_inside_them() {
        let mut input =
            br#"{"jsonrpc":"2.0","method":"tools/call","params":{"id":99,"s":"\"id\":98,"#.to_vec();
        input.extend_from_slice(&line_of(LIMIT));
        input.extend_from_slice(br#""},"id":7}"#);
        input.push(b'\n');
        let items = all(&input, LIMIT).await;
        assert_eq!(reply_of(&items[0])["id"], serde_json::json!(7));
    }

    /// No readable id - not JSON, a batch, or an id too long to keep on a
    /// line with a method - is answered with a `null` id; a request that
    /// lost its id to its length is still a request.
    #[tokio::test]
    async fn an_unreadable_id_is_answered_with_null() {
        let mut garbage = line_of(LIMIT + 5);
        garbage.push(b'\n');
        let items = all(&garbage, LIMIT).await;
        assert_eq!(reply_of(&items[0])["id"], Value::Null);

        let mut batch = br#"[{"id":1,"method":"ping"}"#.to_vec();
        batch.extend_from_slice(&line_of(LIMIT));
        batch.extend_from_slice(b"]\n");
        let items = all(&batch, LIMIT).await;
        assert_eq!(reply_of(&items[0])["id"], Value::Null);

        let long_id = "x".repeat(ID_CAPTURE_BYTES + 1);
        let input = format!(r#"{{"id":"{long_id}","method":"ping"}}"#) + "\n";
        let items = all(input.as_bytes(), LIMIT).await;
        assert_eq!(reply_of(&items[0])["id"], Value::Null);
    }

    /// A notification and a response over the limit get no answer.
    #[tokio::test]
    async fn a_notification_or_a_response_is_not_answered() {
        let mut note =
            br#"{"jsonrpc":"2.0","method":"notifications/progress","params":{"p":""#.to_vec();
        note.extend_from_slice(&line_of(LIMIT));
        note.extend_from_slice(b"\"}}\n");
        let items = all(&note, LIMIT).await;
        assert_eq!(kind_of(&items[0]), &OversizedKind::Notification);
        let Incoming::Oversized(over) = &items[0] else {
            unreachable!()
        };
        assert!(over.refusal().is_none());

        let mut answer = br#"{"jsonrpc":"2.0","id":4,"result":{"s":""#.to_vec();
        answer.extend_from_slice(&line_of(LIMIT));
        answer.extend_from_slice(b"\"}}\n");
        let items = all(&answer, LIMIT).await;
        assert_eq!(kind_of(&items[0]), &OversizedKind::Response);
    }

    /// A reader that hands out at most three bytes per read, so every line
    /// crosses many `fill_buf` chunks.
    struct Trickle(Vec<u8>);

    impl AsyncRead for Trickle {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let n = self.0.len().min(3).min(buf.remaining());
            buf.put_slice(&self.0[..n]);
            self.0.drain(..n);
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// Chunk boundaries change nothing: the limit, the discarding and the
    /// scan all carry across them.
    #[tokio::test]
    async fn chunk_boundaries_change_nothing() {
        let mut input = padded(1, LIMIT - 1);
        input.push(b'\n');
        let mut big = br#"{"params":{"id":5,"s":""#.to_vec();
        big.extend_from_slice(&line_of(LIMIT * 3));
        big.extend_from_slice(br#""},"id":"late"}"#);
        input.extend_from_slice(&big);
        input.push(b'\n');
        input.extend_from_slice(b"next\n");
        let mut lines = BoundedLines::new(Trickle(input), LIMIT);
        let mut items = Vec::new();
        while let Some(item) = lines.next_line().await.unwrap() {
            items.push(item);
        }
        assert_eq!(items.len(), 3, "{items:?}");
        assert_eq!(items[0], Incoming::Line(padded(1, LIMIT - 1)));
        assert_eq!(reply_of(&items[1])["id"], serde_json::json!("late"));
        assert_eq!(items[2], Incoming::Line(b"next".to_vec()));
    }

    /// A read cancelled halfway through a line, kept or discarded, resumes
    /// where it stopped: `relay_loop` polls stdin inside a `select!`.
    #[tokio::test]
    async fn a_cancelled_read_loses_nothing() {
        let (mut feed, read) = tokio::io::duplex(1024);
        let mut lines = BoundedLines::new(read, LIMIT);
        let short = std::time::Duration::from_millis(50);

        feed.write_all(b"{\"id\":1,").await.unwrap();
        assert!(
            tokio::time::timeout(short, lines.next_line())
                .await
                .is_err(),
            "half a line is not a line"
        );
        feed.write_all(b"\"method\":\"ping\"}\n").await.unwrap();
        assert_eq!(
            lines.next_line().await.unwrap(),
            Some(Incoming::Line(br#"{"id":1,"method":"ping"}"#.to_vec()))
        );

        feed.write_all(br#"{"id":2,"s":""#).await.unwrap();
        feed.write_all(&line_of(LIMIT)).await.unwrap();
        assert!(
            tokio::time::timeout(short, lines.next_line())
                .await
                .is_err(),
            "an oversized line is answered at its end"
        );
        feed.write_all(&line_of(LIMIT)).await.unwrap();
        feed.write_all(b"\"}\n").await.unwrap();
        let item = lines.next_line().await.unwrap().unwrap();
        assert_eq!(reply_of(&item)["id"], serde_json::json!(2));

        drop(feed);
        assert_eq!(lines.next_line().await.unwrap(), None);
    }

    /// The end of input ends a line: a partial line is returned as tokio's
    /// `Lines` returns it, and a line being discarded is still answered.
    #[tokio::test]
    async fn the_end_of_input_ends_a_line() {
        let items = all(b"{\"id\":1}", LIMIT).await;
        assert_eq!(items, vec![Incoming::Line(b"{\"id\":1}".to_vec())]);

        let mut cut = br#"{"id":3,"s":""#.to_vec();
        cut.extend_from_slice(&line_of(LIMIT));
        let items = all(&cut, LIMIT).await;
        assert_eq!(reply_of(&items[0])["id"], serde_json::json!(3));
    }

    /// A last line without a newline is forwarded with one added, so the
    /// newline counts: `limit` bytes at the end of input are refused, one
    /// byte less is served.
    #[tokio::test]
    async fn a_last_line_without_a_newline_counts_the_one_it_will_get() {
        let fits = padded(1, LIMIT - 1);
        assert_eq!(all(&fits, LIMIT).await, vec![Incoming::Line(fits.clone())]);

        let items = all(&padded(2, LIMIT), LIMIT).await;
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(reply_of(&items[0])["id"], serde_json::json!(2));
    }

    /// The thread reader hands on every byte in order and ends with its
    /// source.
    #[tokio::test]
    async fn the_thread_reader_hands_on_every_byte() {
        use tokio::io::AsyncReadExt;
        let source: Vec<u8> = (0..THREAD_CHUNK_BYTES * 3 + 17)
            .map(|i| (i % 251) as u8)
            .collect();
        let mut reader = read_on_thread(std::io::Cursor::new(source.clone())).unwrap();
        let mut read = Vec::new();
        reader.read_to_end(&mut read).await.unwrap();
        assert_eq!(read, source);
    }

    /// A replayed item comes back before anything else is read.
    #[tokio::test]
    async fn a_replayed_item_comes_first() {
        let input: &[u8] = b"second\n";
        let mut lines = BoundedLines::new(input, LIMIT);
        lines.replay(Incoming::Line(b"first".to_vec()));
        assert_eq!(
            lines.next_line().await.unwrap(),
            Some(Incoming::Line(b"first".to_vec()))
        );
        assert_eq!(
            lines.next_line().await.unwrap(),
            Some(Incoming::Line(b"second".to_vec()))
        );
    }
}
