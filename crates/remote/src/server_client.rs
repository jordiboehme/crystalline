//! The connection to one source: HTTPS plus the source's stored token, the
//! control protocol (`POST /api/v1/ctl`), and the OAuth refresh.
//!
//! # Why the refresh takes a file lock
//!
//! The server rotates the refresh token on every use and reads a rotated one
//! coming back as a stolen credential, which revokes the whole grant
//! (`AuthStore::refresh_oauth_grant`). Since 0.32.1 it forgives a rotated
//! token that comes back within 60 seconds, but the lock below keeps this
//! client from leaning on that: one refresh per expiry, not several. The
//! daemon's poller, the CLI and the
//! hooks are separate processes that can all find one source's access token
//! expired at the same moment. So a refresh runs under `refresh.lock` in the
//! source's host folder, re-reads the stored credential once it holds the
//! lock, and refreshes only when what it reads is still the expired or
//! refused token. The same lock and re-read make it single-flight inside one
//! process too: two callers on one connection both wait on the file lock, and
//! the second reads what the first saved. No in-memory lock is held while a
//! refresh runs, so a caller never waits past its own deadline for another
//! caller's refresh. Each source has its own folder, so a refresh of one
//! source never waits for another, and a refused one marks only its own
//! source.
//!
//! The locked part (lock, re-read, token request, save) runs as a task of
//! its own. A caller whose budget runs out stops waiting for it, and while
//! the async runtime lives the task still saves what the server rotated: in
//! the daemon a fan-out deadline does not throw a fresh refresh token away.
//! A short-lived process (the CLI, a hook) that returns from `main` while
//! the task runs shuts the runtime down and cancels it; that process has to
//! wait for an in-flight refresh before it exits, or it can lose the
//! rotated pair.
//!
//! If the save itself fails after the server rotated, the fresh pair is
//! still kept in memory and used, and the failure is logged, so this process
//! never presents the spent token again.
//!
//! One window stays: the server rotated and the process ended before the
//! new pair was saved. The next refresh then presents the old token, the
//! server revokes the grant, and the person is told to sign in again. That
//! is the server's replay rule doing its job, and it costs one sign-in.
//!
//! # A server that cannot be reached
//!
//! A server may be out of reach at any time (no VPN, bad network, a laptop
//! that just woke up). Four rules keep that from turning into a stuck state:
//!
//! - **Every request has an overall limit** that covers name resolution,
//!   connect, TLS and the answer (reqwest's per-request `timeout`, not only
//!   the connect timeout). A one-domain call has [`ONE_DOMAIN_LIMIT`]; a call
//!   over all domains passes the fan-out deadline through
//!   [`Connection::ctl_within`] or [`Connection::tool_within`]. The one
//!   longer limit is [`CTL_TIMEOUT`], for a forwarded `evolve_engrams`: it
//!   runs a maintenance sweep on the server, which can take longer than ten
//!   seconds on a large domain. A forwarded file upload or download has it
//!   too (a full-size file over a slow uplink), run as [`Budget::Cut`], and
//!   the capability question before it keeps [`ONE_DOMAIN_LIMIT`]. The
//!   token request of a refresh always has [`ONE_DOMAIN_LIMIT`], whatever
//!   the caller's budget (see above).
//! - **A network failure is never "sign in again".** It is
//!   [`RemoteFailure::Unreachable`] or [`RemoteFailure::TimedOut`], and a
//!   refresh that fails on the network leaves the stored refresh token as it
//!   was, so the next call tries again. Only the server's own refusal asks
//!   for a new sign-in: an OAuth `invalid_grant` or `invalid_client` from the
//!   token endpoint, or a `401` that a refresh cannot cure.
//! - **A write that went out and got no answer may have happened.** A
//!   forwarded write (a tool call or an automation verb that changes data)
//!   that fails after the connect finished, on the overall limit or a broken
//!   connection, is [`RemoteFailure::Unanswered`]: the caller reads that the
//!   write may or may not have happened and how to check, and the source
//!   stays up (it took the connection). A connect that failed never sent
//!   anything and stays [`RemoteFailure::TimedOut`] or
//!   [`RemoteFailure::Unreachable`].
//! - **A source that failed is down for a short window** ([`DOWN_WINDOW`]).
//!   While it lasts every call answers the recorded failure at once without
//!   touching the network; the first call after it tries again, and the first
//!   answer from the server clears it, even one to a call that was already
//!   on its way when the window opened. An outcome that never reached the
//!   server (an expired token on the quick path, nothing saved) neither opens
//!   nor clears it. The window lives in [`Health`], one
//!   per process by default, so every connection to a source in the daemon
//!   shares it. It is never written to disk, so no restart is ever needed.
//! - **Pooled connections are dropped after a network failure**, and before
//!   a request when the wall clock says the last one is longer ago than
//!   [`POOL_IDLE`] (a monotonic clock stands still while a laptop sleeps).
//!   reqwest cannot empty its pool, so the client is built anew; a laptop
//!   that woke or switched networks connects fresh.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime};

use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::server_token::{CredentialKind, ServerCredential, ServerCredentialStore};
use crate::sources::{REMOTE_TOKEN_ENV, SourceRecord};

/// How long a TCP and TLS connect may take. The overall limit of each
/// request bounds everything else.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The overall limit of a call for one domain, and of every token request.
pub const ONE_DOMAIN_LIMIT: Duration = Duration::from_secs(10);
/// The overall limit of a forwarded `evolve_engrams`, the one call that runs
/// a long operation on the server (a maintenance sweep).
pub const CTL_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a source that failed to answer is skipped.
pub const DOWN_WINDOW: Duration = Duration::from_secs(30);

/// The ctl command that reads one file of a mounted domain (0.27.0).
pub const FILE_COMMAND: &str = "file";

/// The capability a server lists in its `status` answer once an agent can send
/// a file through it: `write_engram` with `path` and `base64` or `content`,
/// `write_reflection` `write_asset`, and `read_engram` / `read_reflection` on
/// an `assets/` path. A server without it ignores `path` and `base64` on a
/// write, and with a title beside them writes an engram instead of a file, so
/// a call that carries a file is not sent to one.
pub const AGENT_FILES_CAPABILITY: &str = "agent_files";

/// The first release that lists [`AGENT_FILES_CAPABILITY`], named in the
/// sentence an agent reads when its server is older.
pub const AGENT_FILES_RELEASE: &str = "0.31.0";

/// The first release whose `write_engram` takes `expected_checksum`. An older
/// server ignores the field and writes last-write-wins, so a guarded write is
/// not sent to one.
pub const WRITE_CHECKSUM_RELEASE: &str = "0.32.0";

/// Whether a `major.minor.patch` version is at least `min`; it lives in core
/// so the Claude Desktop extension update (the service crate's `desktop_offer`) compares the same
/// way.
pub use crystalline_core::version::version_at_least;

/// The largest answer to a [`FILE_COMMAND`] this client reads: the base64 of
/// the largest file a domain holds ([`crystalline_core::MAX_ATTACHMENT_BYTES`],
/// which is also the JSON API's body ceiling) plus room for the envelope.
/// Every other answer is read whole, as before.
pub const FILE_ANSWER_CAP: usize =
    (crystalline_core::MAX_ATTACHMENT_BYTES as usize).div_ceil(3) * 4 + 64 * 1024;

/// Whose limit a call runs within, which decides what a timeout says about
/// the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Budget {
    /// The source's own limit: `remote.deadline_ms` for a call over all
    /// domains, [`ONE_DOMAIN_LIMIT`] for one domain, [`CTL_TIMEOUT`] where a
    /// long server-side operation needs it. A timeout here is the source not
    /// answering, and opens its down window.
    Own,
    /// A caller's budget shorter than the source's own limit (the per-prompt
    /// recall's cut, a hook's wait). Running out of it says nothing about the
    /// source: no down window, no dropped pool.
    Cut,
}

impl Budget {
    /// [`Budget::Cut`] when `limit` is shorter than `own`.
    pub fn of(limit: Duration, own: Duration) -> Budget {
        if limit < own {
            Budget::Cut
        } else {
            Budget::Own
        }
    }
}
/// How long a pooled connection may sit idle, by the wall clock.
pub const POOL_IDLE: Duration = Duration::from_secs(30);
/// The refresh lock, in the source's host folder.
pub const REFRESH_LOCK_FILE: &str = "refresh.lock";
/// How long a refresh waits for another process's refresh to finish.
const LOCK_WAIT: Duration = Duration::from_secs(15);
/// What every unreachable or timed-out failure says, and what
/// [`crate::stale_line`] recognises it by.
pub(crate) const UNREACHABLE_WORDS: &str = "cannot be reached right now";
/// What the failure of a server without the control protocol says, and
/// what [`crate::stale_line`] recognises it by.
pub(crate) const TOO_OLD_WORDS: &str = "does not serve the remote control protocol";
/// What a person can do about a source that does not answer.
const NETWORK_HINT: &str =
    "check the VPN or the network; it recovers by itself once the server answers again";

/// The `kind` of an envelope error that is an argument error: the one class
/// of a refusal a caller answers as an input error.
pub const INVALID_KIND: &str = "invalid";

/// Why a remote exchange did not produce an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteFailure {
    /// No answer at all: DNS, TCP, TLS, a broken connection, or a gateway
    /// that says the server behind it is down.
    Unreachable {
        /// The source's name.
        source: String,
        /// The server.
        url: String,
        /// The likely cause, in plain words.
        detail: String,
    },
    /// The credential is gone for good: revoked, a refused refresh, or none
    /// saved. That source is marked; nothing falls back anywhere.
    SignInAgain {
        /// The server to sign in to again.
        url: String,
    },
    /// The access token has expired and this call may not refresh it (the
    /// per-prompt recall). Not a sign-in problem: the next call that may
    /// refresh does.
    Expired {
        /// The server.
        url: String,
    },
    /// The local credential store failed.
    Credential(String),
    /// The server answered, but not with a result: an envelope error (the
    /// server's own words, word for word), or a status that is not the
    /// protocol.
    Refused(String),
    /// The server answered that an argument of the call is invalid (the
    /// envelope's `kind: invalid`: a missing page, an unparsable filter), in
    /// its own words. The same input error a local call gives, so a caller
    /// answers it as one. Never a failure of the source: the server answered,
    /// so it does not open the down window, whatever the [`Budget`]. A server
    /// older than 0.27.0 sends no kind, and its answer stays
    /// [`RemoteFailure::Refused`].
    Invalid(String),
    /// The server answered with an HTTP status that is not the protocol (a
    /// proxy's error page, a crash). The body is kept for `status` and
    /// `doctor` only ([`RemoteFailure::server_text`]); what an agent reads
    /// names the status and nothing the server wrote.
    Status {
        /// The server.
        url: String,
        /// The HTTP status code.
        status: u16,
        /// The start of the body on one line: control characters and line
        /// breaks become spaces, and it is cut at 200 characters.
        body: String,
    },
    /// This process was still setting up its network client when the
    /// caller's budget ran out, so the source was not asked. Nothing about
    /// the source; the next call asks it.
    Starting {
        /// The source's name.
        source: String,
    },
    /// The server did not answer within the limit.
    TimedOut {
        /// The source's name.
        source: String,
        /// The server.
        url: String,
        /// How long it waited.
        after: Duration,
    },
    /// A write went out and no answer came: the limit ran out, the
    /// connection broke after the connect had finished, or a gateway
    /// answered 504 or 502 ([`Silence`]). The server may have done the write
    /// or not. It does not open the down window, whatever the [`Budget`]
    /// (the server took the connection), except for a gateway's 502, which
    /// marks the source down with the gateway's own sentence
    /// ([`RemoteFailure::down_mark`]). A failure of the connect itself (name,
    /// TCP, TLS) never sent anything and stays [`RemoteFailure::TimedOut`]
    /// or [`RemoteFailure::Unreachable`]. Boxed, so every error that carries
    /// a failure stays small.
    Unanswered(Box<UnansweredWrite>),
}

impl RemoteFailure {
    /// Whether this source needs a new sign-in.
    pub fn is_sign_in(&self) -> bool {
        matches!(self, RemoteFailure::SignInAgain { .. })
    }

    /// What the server itself wrote, when this failure carries any that is
    /// not the protocol's own refusal: for `status` and `doctor`, never for
    /// anything an agent reads.
    pub fn server_text(&self) -> Option<&str> {
        match self {
            RemoteFailure::Status { body, .. } => Some(body),
            _ => None,
        }
    }

    /// The failure the down window records for this one, when it opens the
    /// window: an unreachable or timed-out source as it is, and a write a
    /// gateway answered with `502` as the gateway's plain sentence, so a
    /// later call inside the window is never told about a write it did not
    /// send.
    pub fn down_mark(&self) -> Option<RemoteFailure> {
        match self {
            failure if failure.is_unreachable() => Some(failure.clone()),
            RemoteFailure::Unanswered(write) if write.silence == Silence::Gateway(502) => {
                Some(RemoteFailure::Unreachable {
                    source: write.source.clone(),
                    url: write.url.clone(),
                    detail: gateway_detail(502),
                })
            }
            _ => None,
        }
    }

    /// Whether the server gave no answer at all: what opens the down window.
    pub fn is_unreachable(&self) -> bool {
        matches!(
            self,
            RemoteFailure::Unreachable { .. } | RemoteFailure::TimedOut { .. }
        )
    }
}

impl std::fmt::Display for RemoteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteFailure::Unreachable {
                source,
                url,
                detail,
            } => write!(
                f,
                "{source} ({url}) {UNREACHABLE_WORDS}: {detail} ({NETWORK_HINT})"
            ),
            RemoteFailure::SignInAgain { url } => write!(
                f,
                "the sign-in to {url} is no longer valid; sign in again: crystalline connect {url}"
            ),
            RemoteFailure::Expired { url } => write!(
                f,
                "the access token for {url} has expired; the next call that may refresh it renews it by itself"
            ),
            RemoteFailure::Credential(text)
            | RemoteFailure::Refused(text)
            | RemoteFailure::Invalid(text) => f.write_str(text),
            RemoteFailure::Starting { source } => write!(
                f,
                "{source} was not asked: this process was still setting up its network connection, and the next call asks it"
            ),
            RemoteFailure::Status { url, status, .. } => write!(
                f,
                "{url} answered with HTTP status {status} instead of the remote control protocol"
            ),
            RemoteFailure::TimedOut { source, url, after } => write!(
                f,
                "{source} ({url}) {UNREACHABLE_WORDS}: it did not answer within {} ({NETWORK_HINT})",
                seconds(*after)
            ),
            RemoteFailure::Unanswered(write) => {
                let (source, url, check) = (&write.source, &write.url, &write.check);
                match write.silence {
                    Silence::Waited(after) => write!(
                        f,
                        "{source} ({url}) did not answer within {}. {check}",
                        seconds(after)
                    ),
                    Silence::BrokeOff => write!(
                        f,
                        "{source} ({url}) broke off the connection before it answered. {check}"
                    ),
                    Silence::Gateway(status) if status == GATEWAY_TIMEOUT => write!(
                        f,
                        "{source} ({url}) did not answer in time: its gateway answered {}. {check}",
                        status_text(status)
                    ),
                    Silence::Gateway(status) => write!(
                        f,
                        "{source} ({url}) gave no answer: its gateway answered {}. \
                         {check}",
                        status_text(status)
                    ),
                }
            }
        }
    }
}

impl std::error::Error for RemoteFailure {}

/// A write that went out with no answer ([`RemoteFailure::Unanswered`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnansweredWrite {
    /// The source's name.
    pub source: String,
    /// The server.
    pub url: String,
    /// How the answer failed to come.
    pub silence: Silence,
    /// What may have happened and how to check, for the tool that was
    /// called.
    pub check: String,
}

/// How a write's answer failed to come ([`UnansweredWrite`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Silence {
    /// The limit ran out after this long.
    Waited(Duration),
    /// The connection broke while the request went out or while its answer
    /// was awaited.
    BrokeOff,
    /// A gateway in front of the server answered with this status: `504`
    /// (it passed the write on and the server did not answer it in time) or
    /// `502` (the connection to the server broke).
    Gateway(u16),
}

/// The status a gateway answers when the server behind it did not answer in
/// time.
const GATEWAY_TIMEOUT: u16 = 504;

/// `504 Gateway Timeout`: a status with its reason, as a person reads it.
fn status_text(status: u16) -> String {
    reqwest::StatusCode::from_u16(status)
        .map(|s| s.to_string())
        .unwrap_or_else(|_| status.to_string())
}

/// What a gateway's `502`, `503` or `504` says about the server behind it.
fn gateway_detail(status: u16) -> String {
    format!(
        "its gateway answered {}, so the server behind it is down",
        status_text(status)
    )
}

/// `3 s`, `0.7 s`: a deadline as a person reads it.
pub fn seconds(after: Duration) -> String {
    let millis = after.as_millis();
    if millis.is_multiple_of(1000) {
        format!("{} s", millis / 1000)
    } else {
        format!("{:.1} s", after.as_secs_f64())
    }
}

/// One ctl answer.
#[derive(Debug, Clone, PartialEq)]
pub enum CtlAnswer {
    /// A result, with its etag when the command carries one.
    Data {
        /// The `data` member.
        data: Value,
        /// The `etag` member.
        etag: Option<String>,
    },
    /// The request's `if_none_match` still holds.
    NotModified {
        /// The etag that still holds.
        etag: String,
    },
}

/// Who a forwarded call is for, beside the token's account: the agent's
/// client half (decision D11). `None` from the CLI and the hooks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForwardedAgent {
    /// `<name>/<version>` as the local session's `clientInfo` reported it.
    pub client: Option<String>,
}

/// Which sources are down right now, and why. Kept in memory only: one per
/// process by default ([`Health::shared`]), so every connection to a source
/// in the daemon skips it together, and a restart, a disconnect or a hook
/// process never inherits a stale state.
#[derive(Clone, Debug)]
pub struct Health {
    window: Duration,
    down: Arc<std::sync::Mutex<HashMap<String, (Instant, RemoteFailure)>>>,
}

static SHARED_HEALTH: LazyLock<Health> = LazyLock::new(|| Health::new(DOWN_WINDOW));

impl Health {
    /// A record of its own with `window` as the down window: for tests, which
    /// use a short one.
    pub fn new(window: Duration) -> Health {
        Health {
            window,
            down: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// This process's record, with [`DOWN_WINDOW`].
    pub fn shared() -> Health {
        SHARED_HEALTH.clone()
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, (Instant, RemoteFailure)>> {
        self.down.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The failure source `key` is skipped for, while its window lasts.
    pub fn down(&self, key: &str) -> Option<RemoteFailure> {
        let mut map = self.map();
        match map.get(key) {
            Some((until, failure)) if Instant::now() < *until => Some(failure.clone()),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    fn mark_down(&self, key: &str, failure: &RemoteFailure) {
        self.map().insert(
            key.to_string(),
            (Instant::now() + self.window, failure.clone()),
        );
    }

    fn clear(&self, key: &str) {
        self.map().remove(key);
    }
}

/// The HTTP client every remote exchange uses: the workspace's reqwest over
/// rustls, a short connect timeout, a short idle pool with TCP keepalive, and
/// a user agent naming this binary. Every request sets its own overall
/// limit. It never follows a redirect: a server that speaks this protocol
/// never sends one, and following it would carry a request body (a refresh
/// token, a sign-in code, a forwarded call) to an address nobody checked,
/// plain http included.
pub fn http_client() -> Result<reqwest::Client, RemoteFailure> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .pool_idle_timeout(POOL_IDLE)
        .tcp_keepalive(Duration::from_secs(15))
        .user_agent(concat!("crystalline/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| RemoteFailure::Refused(format!("could not build the HTTP client: {e}")))
}

/// Whether this process has built its first HTTP client.
static WARMED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// Build this process's first HTTP client on a blocking thread and wait for
/// it, once per process. The first build reads the system proxy settings,
/// and on macOS that lists the directory the executable sits in once per
/// process: a few milliseconds for an installed binary, seconds for one in a
/// crowded directory. Every later build takes well under a millisecond.
pub async fn warm_http_client() {
    WARMED
        .get_or_init(|| async {
            let _ = tokio::task::spawn_blocking(|| {
                let _ = http_client();
            })
            .await;
        })
        .await;
}

/// [`warm_http_client`] for a caller with a budget: wait for it at most
/// `limit`, and answer whether the client is ready. Past the limit the
/// build goes on in the background, so the next call finds it ready; the
/// caller answers [`RemoteFailure::Starting`] instead of spending more than
/// its budget. A cold start never reads as a sign-in that could not be used.
pub async fn warm_http_client_within(limit: Duration) -> bool {
    if WARMED.initialized() {
        return true;
    }
    tokio::spawn(warm_http_client());
    tokio::time::timeout(limit, warm_http_client())
        .await
        .is_ok()
}

/// What a form body leaves as it is: letters, digits and `*-._`, the
/// `application/x-www-form-urlencoded` byte set.
const FORM_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'*')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_');

/// A form body, encoded by hand: this workspace does not build reqwest's form
/// helper.
pub fn form_body(pairs: &[(&str, &str)]) -> String {
    use percent_encoding::utf8_percent_encode;
    pairs
        .iter()
        .map(|(name, value)| {
            format!(
                "{}={}",
                utf8_percent_encode(name, FORM_SET),
                utf8_percent_encode(value, FORM_SET)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Who a request failure is about, for its words.
#[derive(Clone)]
struct Peer {
    source: String,
    url: String,
}

impl Peer {
    /// A transport error, as the failure a person reads.
    fn failure(&self, e: &reqwest::Error, limit: Duration) -> RemoteFailure {
        if e.is_timeout() {
            // A connect that gave up waited the connect limit; anything else
            // ran into the request's overall limit.
            let after = if e.is_connect() {
                CONNECT_TIMEOUT.min(limit)
            } else {
                limit
            };
            return RemoteFailure::TimedOut {
                source: self.source.clone(),
                url: self.url.clone(),
                after,
            };
        }
        RemoteFailure::Unreachable {
            source: self.source.clone(),
            url: self.url.clone(),
            detail: cause(e, &self.url),
        }
    }

    /// A transport error on a request that changes data, `check` being what
    /// the caller reads about it. Every failure after the connect (the
    /// overall limit running out, a connection that broke while the request
    /// went out or while its answer was awaited) may have left the change
    /// done, so it says so and how to check. The connect has a timer of its
    /// own and its failures read as connect errors (name, TCP, TLS): those
    /// never sent anything and stay [`Peer::failure`], as does every request
    /// that only reads.
    fn sent_failure(
        &self,
        e: &reqwest::Error,
        limit: Duration,
        check: Option<&str>,
    ) -> RemoteFailure {
        match check {
            Some(check) if !e.is_connect() && !e.is_builder() => {
                RemoteFailure::Unanswered(Box::new(UnansweredWrite {
                    source: self.source.clone(),
                    url: self.url.clone(),
                    silence: if e.is_timeout() {
                        Silence::Waited(limit)
                    } else {
                        Silence::BrokeOff
                    },
                    check: check.to_string(),
                }))
            }
            _ => self.failure(e, limit),
        }
    }

    /// A redirect: refused, never followed ([`http_client`]).
    fn redirect(&self, status: reqwest::StatusCode) -> Option<RemoteFailure> {
        status
            .is_redirection()
            .then(|| RemoteFailure::Refused(redirect_sentence(&self.url)))
    }

    /// A gateway's `502`, `503` or `504`: the server behind it is down.
    fn gateway(&self, status: reqwest::StatusCode) -> Option<RemoteFailure> {
        matches!(status.as_u16(), 502..=504).then(|| RemoteFailure::Unreachable {
            source: self.source.clone(),
            url: self.url.clone(),
            detail: gateway_detail(status.as_u16()),
        })
    }

    /// A gateway's answer to a request that changes data, `check` being what
    /// the caller reads about it. A `504` (the gateway passed the request on
    /// and the server did not answer in time) and a `502` (the connection
    /// behind the gateway broke) may have left the change done. A `503` (no
    /// server behind the gateway, nothing passed on) and every read are
    /// [`Peer::gateway`].
    fn sent_gateway(
        &self,
        status: reqwest::StatusCode,
        check: Option<&str>,
    ) -> Option<RemoteFailure> {
        match (check, status.as_u16()) {
            (Some(check), code @ (502 | 504)) => {
                Some(RemoteFailure::Unanswered(Box::new(UnansweredWrite {
                    source: self.source.clone(),
                    url: self.url.clone(),
                    silence: Silence::Gateway(code),
                    check: check.to_string(),
                })))
            }
            _ => self.gateway(status),
        }
    }
}

/// What a server that answered with a redirect is told, on every path.
pub(crate) fn redirect_sentence(url: &str) -> String {
    format!("{url} answered with a redirect, which Crystalline does not follow; check the address")
}

/// The likely cause of a transport error, read off its chain of sources.
/// The chain starts below reqwest's own message, which carries the URL: a
/// host named `tls-gw` must not read as a TLS failure.
pub(crate) fn cause(e: &reqwest::Error, url: &str) -> String {
    let mut chain = Vec::new();
    let mut refused = false;
    let mut current = std::error::Error::source(e);
    while let Some(error) = current {
        if let Some(io) = error.downcast_ref::<std::io::Error>()
            && io.kind() == std::io::ErrorKind::ConnectionRefused
        {
            refused = true;
        }
        chain.push(error.to_string().to_lowercase());
        current = error.source();
    }
    let text = chain.join(": ");
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string());
    if text.contains("dns error") || text.contains("failed to lookup address") {
        format!("the name {host} does not resolve")
    } else if refused || text.contains("connection refused") {
        "nothing accepts connections at that address".to_string()
    } else if text.contains("certificate") || text.contains("tls") || text.contains("handshake") {
        "the secure connection could not be set up".to_string()
    } else if e.is_connect() {
        "the connection could not be opened".to_string()
    } else {
        "the connection broke off".to_string()
    }
}

enum Posted {
    Answer(CtlAnswer),
    Unauthorized,
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

/// The credential in hand, shared with a running refresh job so the job can
/// read the newest pair under the refresh lock and keep what it rotated even
/// when its caller stopped waiting.
type Held = Arc<std::sync::Mutex<Option<ServerCredential>>>;

fn read_held(held: &Held) -> Option<ServerCredential> {
    held.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn store_held(held: &Held, credential: ServerCredential) {
    *held.lock().unwrap_or_else(|e| e.into_inner()) = Some(credential);
}

/// The HTTP client in hand and when the wall clock last saw it used.
struct HttpSlot {
    client: reqwest::Client,
    used: SystemTime,
}

/// An open connection to one source.
pub struct Connection {
    source: SourceRecord,
    host_dir: PathBuf,
    store: ServerCredentialStore,
    http: std::sync::Mutex<HttpSlot>,
    health: Health,
    limit: Duration,
    /// The credential in hand. Only ever locked for a clone or a store,
    /// never across a wait: the refresh lock keeps a refresh single-flight.
    current: Held,
}

impl Connection {
    /// Open `source` over this process's environment.
    pub fn open(source: SourceRecord, remote_dir: &Path) -> Result<Connection, RemoteFailure> {
        Connection::open_with(
            source,
            remote_dir,
            crystalline_core::secret_env::process_var,
        )
    }

    /// Open `source`, loading its credential. No network yet. Blocking (one
    /// bounded keychain read); async callers that must not wait run it on
    /// `spawn_blocking`.
    pub fn open_with(
        source: SourceRecord,
        remote_dir: &Path,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Connection, RemoteFailure> {
        let host_dir = source.host_dir(remote_dir);
        let failed = |e: crate::error::RemoteError| RemoteFailure::Credential(e.to_string());
        let (store, credential) = if source.from_env {
            let store = ServerCredentialStore::env(
                env(REMOTE_TOKEN_ENV).unwrap_or_default().trim().to_string(),
                source.url.clone(),
            );
            let credential = store.load().map_err(failed)?;
            (store, credential)
        } else {
            ServerCredentialStore::resolve_and_load(&source.key(), &host_dir).map_err(failed)?
        };
        let Some(credential) = credential else {
            return Err(RemoteFailure::SignInAgain { url: source.url });
        };
        Ok(Connection {
            http: std::sync::Mutex::new(HttpSlot {
                client: http_client()?,
                used: SystemTime::now(),
            }),
            source,
            host_dir,
            store,
            health: Health::shared(),
            limit: ONE_DOMAIN_LIMIT,
            current: Arc::new(std::sync::Mutex::new(Some(credential))),
        })
    }

    /// The same connection with `limit` as the overall limit of a call that
    /// names none ([`ONE_DOMAIN_LIMIT`] by default). Tests use a short one.
    pub fn with_limit(mut self, limit: Duration) -> Connection {
        self.limit = limit;
        self
    }

    /// The same connection keeping its down window in `health` instead of
    /// this process's shared record. Tests use one with a short window.
    pub fn with_health(mut self, health: Health) -> Connection {
        self.health = health;
        self
    }

    /// The source.
    pub fn source(&self) -> &SourceRecord {
        &self.source
    }

    /// The source's host folder.
    pub fn host_dir(&self) -> &Path {
        &self.host_dir
    }

    /// The HTTP client, fresh when the last request is longer ago than
    /// [`POOL_IDLE`] by the wall clock. A clone shares the pool.
    pub fn http(&self) -> reqwest::Client {
        let mut slot = self.http.lock().unwrap_or_else(|e| e.into_inner());
        let now = SystemTime::now();
        let idle = now.duration_since(slot.used).unwrap_or(Duration::MAX);
        if idle > POOL_IDLE
            && let Ok(client) = http_client()
        {
            slot.client = client;
        }
        slot.used = now;
        slot.client.clone()
    }

    /// Build the client anew, so nothing reuses a connection that failed.
    fn drop_pool(&self) {
        if let Ok(client) = http_client() {
            let mut slot = self.http.lock().unwrap_or_else(|e| e.into_inner());
            slot.client = client;
            slot.used = SystemTime::now();
        }
    }

    /// `"keyring"`, `"file"` or `"environment"`.
    pub fn store_kind(&self) -> &'static str {
        self.store.kind()
    }

    /// The credential in hand, for `status` and `doctor`.
    pub async fn credential(&self) -> Option<ServerCredential> {
        self.held()
    }

    /// A copy of the credential in hand.
    fn held(&self) -> Option<ServerCredential> {
        read_held(&self.current)
    }

    /// The failure this source is skipped for while its down window lasts,
    /// for `status` and `doctor`.
    pub fn down(&self) -> Option<RemoteFailure> {
        self.health.down(&self.source.key())
    }

    /// The failure that says to sign in again.
    pub fn sign_in_again(&self) -> RemoteFailure {
        RemoteFailure::SignInAgain {
            url: self.source.url.clone(),
        }
    }

    fn peer(&self) -> Peer {
        Peer {
            source: self.source.name.clone(),
            url: self.source.url.clone(),
        }
    }

    /// Record how an exchange ended: no answer opens the down window and
    /// drops the pool. A refresh the caller stopped waiting for counts as no
    /// answer too; it was the token request that did not finish in time, or
    /// (rarely) another process holding the refresh lock that long. Closing
    /// the window is not done here but where an answer arrives
    /// ([`Connection::answered`]), so an outcome that never reached the
    /// server cannot erase a mark another call just set.
    pub(crate) fn note<T>(&self, result: &Result<T, RemoteFailure>) {
        if let Err(failure) = result
            && let Some(mark) = failure.down_mark()
        {
            self.health.mark_down(&self.source.key(), &mark);
            self.drop_pool();
        }
    }

    /// [`Connection::note`] for an exchange run within `budget`. A caller's
    /// budget that is shorter than the source's own limit (the recall's cut,
    /// session start's wait) says nothing about the source when it runs out:
    /// that timeout neither opens the down window nor drops the pool. A
    /// failure that is not a timeout (a refused connection, a name that does
    /// not resolve) counts whatever the budget.
    fn note_as<T>(&self, result: &Result<T, RemoteFailure>, budget: Budget) {
        if budget == Budget::Cut
            && let Err(RemoteFailure::TimedOut { .. }) = result
        {
            return;
        }
        self.note(result);
    }

    /// The server answered: whatever it said, it is reachable.
    fn answered(&self) {
        self.health.clear(&self.source.key());
    }

    /// The failure a call answers at once while the source is down.
    fn skip(&self) -> Result<(), RemoteFailure> {
        match self.down() {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }

    /// The bearer token for the next request, refreshed first when it is
    /// within the margin of its expiry.
    pub async fn bearer(&self) -> Result<String, RemoteFailure> {
        self.skip()?;
        let result = self
            .bearer_by(Instant::now() + self.limit, self.limit)
            .await;
        self.note(&result);
        result
    }

    async fn bearer_by(&self, deadline: Instant, limit: Duration) -> Result<String, RemoteFailure> {
        match self.held() {
            Some(credential) if !credential.needs_refresh(Utc::now()) => {
                Ok(credential.access_token)
            }
            Some(credential) => {
                self.refresh_by(Some(&credential.access_token), deadline, limit)
                    .await
            }
            None => Err(self.sign_in_again()),
        }
    }

    /// The bearer token as it stands, never refreshed: for the per-prompt
    /// recall, which has a one-second budget and must not spend a refresh
    /// token another process may be about to spend.
    pub async fn bearer_without_refresh(&self) -> Result<String, RemoteFailure> {
        match self.held() {
            Some(credential) if !credential.needs_refresh(Utc::now()) => {
                Ok(credential.access_token)
            }
            Some(_) => Err(RemoteFailure::Expired {
                url: self.source.url.clone(),
            }),
            None => Err(self.sign_in_again()),
        }
    }

    /// Refresh the OAuth access token. `rejected` is the token the caller saw
    /// expire or refused; a credential that already differs from it and is
    /// fresh was refreshed by somebody else, and is used as it is.
    pub async fn refresh(&self, rejected: Option<&str>) -> Result<String, RemoteFailure> {
        self.skip()?;
        let result = self
            .refresh_by(rejected, Instant::now() + self.limit, self.limit)
            .await;
        self.note(&result);
        result
    }

    async fn refresh_by(
        &self,
        rejected: Option<&str>,
        deadline: Instant,
        limit: Duration,
    ) -> Result<String, RemoteFailure> {
        if let Some(credential) = self.held()
            && settled(&credential, rejected)
        {
            return Ok(credential.access_token);
        }
        if self.source.kind == CredentialKind::Token || self.source.from_env {
            return Err(self.sign_in_again());
        }
        let job = RefreshJob {
            peer: self.peer(),
            store: self.store.clone(),
            http: self.http(),
            lock: self.host_dir.join(REFRESH_LOCK_FILE),
            endpoint: self.source.token_endpoint.clone(),
            rejected: rejected.map(str::to_string),
            health: self.health.clone(),
            key: self.source.key(),
            held: self.current.clone(),
        };
        // Its own task: a caller that stops waiting does not stop the save.
        // Counted from before the spawn, so a process that asks
        // [`settle_refreshes`] right after never misses one not yet polled.
        let counted = Refreshing::enter();
        let running = tokio::spawn(async move {
            let _counted = counted;
            job.run().await
        });
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, running).await {
            // The job already put it in hand.
            Ok(Ok(Ok(fresh))) => Ok(fresh.access_token),
            Ok(Ok(Err(failure))) => Err(failure),
            Ok(Err(stopped)) => Err(RemoteFailure::Credential(format!(
                "the refresh of the sign-in to {} stopped: {stopped}",
                self.source.url
            ))),
            Err(_) => Err(RemoteFailure::TimedOut {
                source: self.source.name.clone(),
                url: self.source.url.clone(),
                after: limit,
            }),
        }
    }

    /// One ctl exchange within this connection's limit: refresh ahead of
    /// expiry, and on a `401` refresh once and retry once.
    pub async fn ctl(&self, request: Value) -> Result<CtlAnswer, RemoteFailure> {
        self.ctl_within(request, self.limit).await
    }

    /// [`Connection::ctl`] within `limit` overall, refresh and retry
    /// included: the fan-out deadline for a call over all domains.
    pub async fn ctl_within(
        &self,
        request: Value,
        limit: Duration,
    ) -> Result<CtlAnswer, RemoteFailure> {
        self.ctl_within_as(request, limit, Budget::Own).await
    }

    /// [`Connection::ctl_within`] for a `limit` that is `budget`: the
    /// source's own, or a caller's shorter cut.
    pub async fn ctl_within_as(
        &self,
        request: Value,
        limit: Duration,
        budget: Budget,
    ) -> Result<CtlAnswer, RemoteFailure> {
        self.skip()?;
        let result = self.ctl_by(&request, Instant::now() + limit, limit).await;
        self.note_as(&result, budget);
        result
    }

    async fn ctl_by(
        &self,
        request: &Value,
        deadline: Instant,
        limit: Duration,
    ) -> Result<CtlAnswer, RemoteFailure> {
        let bearer = self.bearer_by(deadline, limit).await?;
        match self.post_ctl(request, &bearer, deadline, limit).await? {
            Posted::Answer(answer) => Ok(answer),
            Posted::Unauthorized => {
                let bearer = self.refresh_by(Some(&bearer), deadline, limit).await?;
                match self.post_ctl(request, &bearer, deadline, limit).await? {
                    Posted::Answer(answer) => Ok(answer),
                    Posted::Unauthorized => Err(self.sign_in_again()),
                }
            }
        }
    }

    /// [`Connection::ctl`] without any refresh.
    pub async fn ctl_without_refresh(&self, request: Value) -> Result<CtlAnswer, RemoteFailure> {
        self.ctl_without_refresh_within(request, self.limit).await
    }

    /// [`Connection::ctl_without_refresh`] within `limit` overall: the
    /// per-prompt recall passes its one second here, so a source that does
    /// not answer opens its down window like any other call.
    pub async fn ctl_without_refresh_within(
        &self,
        request: Value,
        limit: Duration,
    ) -> Result<CtlAnswer, RemoteFailure> {
        self.skip()?;
        let deadline = Instant::now() + limit;
        let result = async {
            let bearer = self.bearer_without_refresh().await?;
            match self.post_ctl(&request, &bearer, deadline, limit).await? {
                Posted::Answer(answer) => Ok(answer),
                Posted::Unauthorized if self.source.kind == CredentialKind::Oauth => {
                    Err(RemoteFailure::Expired {
                        url: self.source.url.clone(),
                    })
                }
                Posted::Unauthorized => Err(self.sign_in_again()),
            }
        }
        .await;
        self.note(&result);
        result
    }

    /// [`Connection::ctl`] for a command without an etag: its `data`.
    pub async fn ctl_data(&self, request: Value) -> Result<Value, RemoteFailure> {
        self.ctl_data_within(request, self.limit, Budget::Own).await
    }

    async fn ctl_data_within(
        &self,
        request: Value,
        limit: Duration,
        budget: Budget,
    ) -> Result<Value, RemoteFailure> {
        match self.ctl_within_as(request, limit, budget).await? {
            CtlAnswer::Data { data, .. } => Ok(data),
            CtlAnswer::NotModified { .. } => Ok(Value::Null),
        }
    }

    /// One forwarded tool call, in the source's own names, with the agent it
    /// is for, within `limit`: the caller picks it (the fan-out deadline for
    /// a call over all domains, or the long limit of a forwarded sweep). A
    /// refusal comes back as [`RemoteFailure::Refused`] carrying the
    /// server's sentence word for word.
    pub async fn tool_within(
        &self,
        tool: &str,
        args: Value,
        agent: &ForwardedAgent,
        limit: Duration,
    ) -> Result<Value, RemoteFailure> {
        self.tool_within_as(tool, args, agent, limit, Budget::Own)
            .await
    }

    /// [`Connection::tool_within`] for a `limit` that is `budget`.
    pub async fn tool_within_as(
        &self,
        tool: &str,
        args: Value,
        agent: &ForwardedAgent,
        limit: Duration,
        budget: Budget,
    ) -> Result<Value, RemoteFailure> {
        let mut request = json!({ "v": 1, "cmd": "tool", "tool": tool, "args": args });
        if let Some(client) = &agent.client {
            request["agent"] = json!({ "client": client });
        }
        self.ctl_data_within(request, limit, budget).await
    }

    /// Whether the server lists [`AGENT_FILES_CAPABILITY`] in its `status`
    /// answer. A server that answers no capability list, which every release
    /// before [`AGENT_FILES_RELEASE`] does, does not.
    pub async fn takes_agent_files(
        &self,
        limit: Duration,
        budget: Budget,
    ) -> Result<bool, RemoteFailure> {
        let status = self
            .ctl_data_within(json!({ "v": 1, "cmd": "status" }), limit, budget)
            .await?;
        Ok(status["capabilities"]
            .as_array()
            .is_some_and(|all| all.iter().any(|c| c == AGENT_FILES_CAPABILITY)))
    }

    /// The version the server names in its `status` answer, or `None` when
    /// it names none.
    pub async fn server_version(
        &self,
        limit: Duration,
        budget: Budget,
    ) -> Result<Option<String>, RemoteFailure> {
        let status = self
            .ctl_data_within(json!({ "v": 1, "cmd": "status" }), limit, budget)
            .await?;
        Ok(status["version"]
            .as_str()
            .filter(|v| !v.trim().is_empty())
            .map(str::to_string))
    }

    /// One file of a domain on the server, by its domain-relative path
    /// (`assets/...` or `reflections/assets/...`), within `limit`: the
    /// server's `{mime, size, base64}`, read with [`FILE_ANSWER_CAP`] as the
    /// ceiling. The server reads it under the account's own right for the
    /// kind of the file. A server older than 0.27.0 refuses the command
    /// ([`RemoteFailure::Refused`], in its words).
    ///
    /// Run as [`Budget::Cut`]: a large file over a slow link can outlast any
    /// limit, and running out of time on one file says nothing about the
    /// source, so a timeout here never opens its down window or drops the
    /// pool. A refused connection still counts, as for every call.
    pub async fn file_within(
        &self,
        domain: &str,
        path: &str,
        limit: Duration,
    ) -> Result<Value, RemoteFailure> {
        let request = json!({ "v": 1, "cmd": FILE_COMMAND, "domain": domain, "path": path });
        self.ctl_data_within(request, limit, Budget::Cut).await
    }

    async fn post_ctl(
        &self,
        request: &Value,
        bearer: &str,
        deadline: Instant,
        limit: Duration,
    ) -> Result<Posted, RemoteFailure> {
        let peer = self.peer();
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RemoteFailure::TimedOut {
                source: peer.source,
                url: peer.url,
                after: limit,
            });
        }
        // A request that changes data and got no answer after it went out
        // may have been done; what the caller checks depends on the request.
        let check = crate::unanswered::unanswered_check(request);
        let response = self
            .http()
            .post(format!("{}{}", self.source.url, crate::CTL_PATH))
            .bearer_auth(bearer)
            .header("content-type", "application/json")
            .timeout(remaining)
            .body(request.to_string())
            .send()
            .await
            .map_err(|e| peer.sent_failure(&e, limit, check.as_deref()))?;
        let status = response.status();
        if let Some(down) = peer.sent_gateway(status, check.as_deref()) {
            return Err(down);
        }
        self.answered();
        if let Some(redirect) = peer.redirect(status) {
            return Err(redirect);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(Posted::Unauthorized);
        }
        // The request's own timeout runs until the body has finished. A file
        // answer is read up to its ceiling, never whole.
        let text = if request.get("cmd").and_then(Value::as_str) == Some(FILE_COMMAND) {
            read_capped(response, FILE_ANSWER_CAP, &self.source.url)
                .await
                .map_err(|e| e.into_failure(&peer, limit))?
        } else {
            response
                .text()
                .await
                .map_err(|e| peer.sent_failure(&e, limit, check.as_deref()))?
        };
        if status == reqwest::StatusCode::NOT_FOUND
            || status == reqwest::StatusCode::METHOD_NOT_ALLOWED
        {
            return Err(RemoteFailure::Refused(format!(
                "{} {TOO_OLD_WORDS}; it needs Crystalline 0.23 or newer",
                self.source.url
            )));
        }
        if !status.is_success() {
            return Err(RemoteFailure::Status {
                url: self.source.url.clone(),
                status: status.as_u16(),
                body: crate::text::one_line(&text, 200),
            });
        }
        let envelope: Value = serde_json::from_str(&text).map_err(|_| {
            RemoteFailure::Refused(format!(
                "{} did not answer in the control protocol",
                self.source.url
            ))
        })?;
        if envelope.get("ok").and_then(Value::as_bool) != Some(true) {
            let text = envelope
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("ctl error")
                .to_string();
            return Err(match envelope.get("kind").and_then(Value::as_str) {
                Some(INVALID_KIND) => RemoteFailure::Invalid(text),
                _ => RemoteFailure::Refused(text),
            });
        }
        let etag = envelope
            .get("etag")
            .and_then(Value::as_str)
            .map(str::to_string);
        if envelope.get("not_modified").and_then(Value::as_bool) == Some(true) {
            return Ok(Posted::Answer(CtlAnswer::NotModified {
                etag: etag.unwrap_or_default(),
            }));
        }
        Ok(Posted::Answer(CtlAnswer::Data {
            data: envelope.get("data").cloned().unwrap_or(Value::Null),
            etag,
        }))
    }
}

/// Why a capped read stopped.
#[derive(Debug)]
enum CappedRead {
    /// The network failed while the body came in.
    Network(reqwest::Error),
    /// The body is over the ceiling, or not text.
    Refused(String),
}

impl CappedRead {
    fn into_failure(self, peer: &Peer, limit: Duration) -> RemoteFailure {
        match self {
            CappedRead::Network(e) => peer.failure(&e, limit),
            CappedRead::Refused(text) => RemoteFailure::Refused(text),
        }
    }
}

/// The body of `response` as text, refused once it grows past `cap` bytes:
/// a declared length over the cap is refused before anything is read, and a
/// body without one is read in chunks and stopped at the cap.
async fn read_capped(
    mut response: reqwest::Response,
    cap: usize,
    url: &str,
) -> Result<String, CappedRead> {
    let too_large =
        || CappedRead::Refused(format!("{url} answered more than {cap} bytes for one file"));
    if response.content_length().is_some_and(|n| n > cap as u64) {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(CappedRead::Network)? {
        if body.len() + chunk.len() > cap {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body)
        .map_err(|_| CappedRead::Refused(format!("{url} did not answer in the control protocol")))
}

/// Whether `credential` is somebody else's finished refresh: no longer the
/// token the caller saw rejected, and not due for a refresh itself.
fn settled(credential: &ServerCredential, rejected: Option<&str>) -> bool {
    Some(credential.access_token.as_str()) != rejected && !credential.needs_refresh(Utc::now())
}

/// How many refresh jobs of this process are still running.
static REFRESHING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One running refresh job, counted while it lives.
struct Refreshing;

impl Refreshing {
    fn enter() -> Refreshing {
        REFRESHING.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Refreshing
    }
}

impl Drop for Refreshing {
    fn drop(&mut self) {
        REFRESHING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Wait until every refresh this process started has finished, for at most
/// `limit`; whether none is left running. A short-lived process (a CLI
/// command, a hook) calls it before it leaves, so a token pair the server
/// rotated is saved even when the call that started the refresh stopped
/// waiting for it (see the module documentation).
pub async fn settle_refreshes(limit: Duration) -> bool {
    let until = Instant::now() + limit;
    loop {
        if REFRESHING.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The locked part of a refresh, owned so it runs as a task of its own.
struct RefreshJob {
    peer: Peer,
    store: ServerCredentialStore,
    http: reqwest::Client,
    lock: PathBuf,
    endpoint: Option<String>,
    rejected: Option<String>,
    health: Health,
    key: String,
    /// The connection's credential in hand, read under the refresh lock:
    /// newer than the stored one when a save after a rotation failed.
    held: Held,
}

impl RefreshJob {
    async fn run(self) -> Result<ServerCredential, RemoteFailure> {
        let sign_in = || RemoteFailure::SignInAgain {
            url: self.peer.url.clone(),
        };
        let _lock = RefreshLock::acquire(&self.lock).await?;
        let stored = self
            .store
            .load()
            .map_err(|e| RemoteFailure::Credential(e.to_string()))?;
        // Whichever pair is newer: a save that failed after a rotation left
        // the store holding a spent refresh token.
        let stored = match (stored, read_held(&self.held)) {
            (Some(stored), Some(held)) if held.created_at > stored.created_at => Some(held),
            (stored, _) => stored,
        };
        let Some(stored) = stored else {
            return Err(sign_in());
        };
        if settled(&stored, self.rejected.as_deref()) {
            store_held(&self.held, stored.clone());
            return Ok(stored);
        }
        let (Some(refresh_token), Some(client_id), Some(endpoint)) = (
            stored.refresh_token.clone(),
            stored.client_id.clone(),
            self.endpoint.clone(),
        ) else {
            return Err(sign_in());
        };
        // From here on a failure leaves the stored pair as it is, so the next
        // call presents the same refresh token again.
        let response = self
            .http
            .post(&endpoint)
            .header("content-type", "application/x-www-form-urlencoded")
            .timeout(ONE_DOMAIN_LIMIT)
            .body(form_body(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh_token),
                ("client_id", &client_id),
                ("resource", &stored.resource),
            ]))
            .send()
            .await
            .map_err(|e| self.peer.failure(&e, ONE_DOMAIN_LIMIT))?;
        let status = response.status();
        if let Some(down) = self.peer.gateway(status) {
            return Err(down);
        }
        self.health.clear(&self.key);
        if let Some(redirect) = self.peer.redirect(status) {
            return Err(redirect);
        }
        let text = response
            .text()
            .await
            .map_err(|e| self.peer.failure(&e, ONE_DOMAIN_LIMIT))?;
        if !status.is_success() {
            let error = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|body| {
                    body.get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_default();
            // invalid_grant: spent, expired, replayed or revoked.
            // invalid_client: the registration was pruned. Neither is
            // anything this machine can repair.
            if error == "invalid_grant" || error == "invalid_client" {
                return Err(sign_in());
            }
            // An OAuth error code is a short lowercase word; anything else
            // the server wrote is left out of a sentence an agent may read.
            let code_shaped = !error.is_empty()
                && error.len() <= 40
                && error.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
            let what = if !code_shaped {
                status.to_string()
            } else {
                format!("{status}, {error}")
            };
            return Err(RemoteFailure::Refused(format!(
                "{} answered a refresh with {what}; the sign-in is kept and the next call tries again",
                self.peer.url
            )));
        }
        let answer: TokenAnswer = serde_json::from_str(&text).map_err(|_| {
            RemoteFailure::Refused(format!(
                "the token endpoint of {} answered something that is not a token",
                self.peer.url
            ))
        })?;
        let fresh = ServerCredential::oauth(
            answer.access_token,
            answer.refresh_token,
            answer.expires_in,
            client_id,
            stored.resource.clone(),
            stored.account.clone(),
            Utc::now(),
        );
        // The server has spent the old refresh token. A failed save must not
        // lose the new pair as well: this process keeps using it, and the
        // next refresh here presents it, not the spent one.
        if let Err(e) = self.store.save(&fresh) {
            tracing::warn!(
                source = %self.peer.source,
                "could not save the refreshed sign-in to {}; it is kept in memory for now: {e}",
                self.peer.url
            );
        }
        // In hand before the lock is released: a job queued behind this one
        // reads it and never presents the spent refresh token, and a caller
        // that stopped waiting does not lose it.
        store_held(&self.held, fresh.clone());
        Ok(fresh)
    }
}

/// The cross-process refresh lock, polled so a wedged holder costs
/// [`LOCK_WAIT`] and not forever. Released on drop.
struct RefreshLock {
    file: std::fs::File,
}

impl RefreshLock {
    async fn acquire(path: &Path) -> Result<RefreshLock, RemoteFailure> {
        let failed = |e: std::io::Error| {
            RemoteFailure::Credential(format!("could not lock {}: {e}", path.display()))
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(failed)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(failed)?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match fs4::FileExt::try_lock(&file) {
                Ok(()) => return Ok(RefreshLock { file }),
                Err(fs4::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(fs4::TryLockError::WouldBlock) => {
                    return Err(RemoteFailure::Credential(
                        "another crystalline process is refreshing the sign-in and has not finished"
                            .to_string(),
                    ));
                }
                Err(fs4::TryLockError::Error(e)) => return Err(failed(e)),
            }
        }
    }
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        let _ = fs4::FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn settling_waits_for_a_running_refresh_and_gives_up_at_the_limit() {
        let held = Refreshing::enter();
        assert!(
            !settle_refreshes(Duration::from_millis(60)).await,
            "a refresh still running is not settled"
        );
        let job = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(held);
        });
        assert!(
            settle_refreshes(Duration::from_secs(5)).await,
            "settled once it finished"
        );
        job.await.unwrap();
    }

    fn connection(health: Health) -> Connection {
        let dir = std::env::temp_dir();
        let source = SourceRecord {
            url: "https://crystalline.acme.test".to_string(),
            name: "acme".to_string(),
            account: "keeper".to_string(),
            kind: CredentialKind::Oauth,
            token_endpoint: None,
            revocation_endpoint: None,
            connected_at: Utc::now(),
            mounts: Vec::new(),
            domains: None,
            from_env: false,
        };
        Connection {
            http: std::sync::Mutex::new(HttpSlot {
                client: http_client().unwrap(),
                used: SystemTime::now(),
            }),
            host_dir: source.host_dir(&dir),
            store: ServerCredentialStore::file(&dir.join("never-written")),
            health,
            limit: ONE_DOMAIN_LIMIT,
            current: Arc::new(std::sync::Mutex::new(None)),
            source,
        }
    }

    /// Review M4: an outcome that never reached the server leaves a down
    /// mark alone.
    #[test]
    fn a_local_outcome_neither_clears_nor_sets_the_down_mark() {
        let connection = connection(Health::new(Duration::from_secs(30)));
        let down = RemoteFailure::Unreachable {
            source: "acme".to_string(),
            url: connection.source.url.clone(),
            detail: "nothing accepts connections at that address".to_string(),
        };
        connection.health.mark_down(&connection.source.key(), &down);
        for local in [
            RemoteFailure::Expired {
                url: connection.source.url.clone(),
            },
            connection.sign_in_again(),
            RemoteFailure::Credential("the keychain did not answer".to_string()),
        ] {
            connection.note(&Err::<(), _>(local));
            assert_eq!(connection.down(), Some(down.clone()));
        }
        connection.note(&Ok::<(), RemoteFailure>(()));
        assert_eq!(connection.down(), Some(down), "only an answer clears it");

        let fresh = connection_with_no_mark();
        fresh.note(&Err::<(), _>(RemoteFailure::Expired {
            url: fresh.source.url.clone(),
        }));
        assert_eq!(fresh.down(), None, "and a local outcome never marks");
    }

    /// One HTTP answer on a loopback port: `head` is the status line and
    /// headers, `body` follows them.
    async fn answering(head: &'static str, body: Vec<u8>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(&body).await;
        });
        url
    }

    /// A file answer is read up to its ceiling: a declared length over it is
    /// refused at once, a body without one is stopped at the ceiling, and a
    /// body under it is read whole.
    #[tokio::test]
    async fn a_file_answer_is_read_up_to_its_ceiling() {
        let client = reqwest::Client::new();
        let url = answering(
            "HTTP/1.1 200 OK\r\ncontent-length: 20\r\nconnection: close\r\n\r\n",
            vec![b'a'; 20],
        )
        .await;
        let response = client.get(&url).send().await.unwrap();
        let refused = read_capped(response, 10, &url).await.unwrap_err();
        assert!(
            matches!(refused, CappedRead::Refused(ref t) if t.contains("more than 10 bytes")),
            "{refused:?}"
        );

        let url = answering(
            "HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n",
            vec![b'a'; 20],
        )
        .await;
        let response = client.get(&url).send().await.unwrap();
        let refused = read_capped(response, 10, &url).await.unwrap_err();
        assert!(matches!(refused, CappedRead::Refused(_)), "{refused:?}");

        let url = answering(
            "HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\n",
            b"hello".to_vec(),
        )
        .await;
        let response = client.get(&url).send().await.unwrap();
        assert_eq!(read_capped(response, 10, &url).await.unwrap(), "hello");
    }

    /// A file read that runs out of time says nothing about the source: a
    /// large file over a slow link is not a server that went away, so the
    /// down window stays shut.
    #[tokio::test]
    async fn a_timed_out_file_read_leaves_the_source_up() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let mut connection = connection(Health::new(Duration::from_secs(30)));
        connection.source.url = url.clone();
        connection.current = Arc::new(std::sync::Mutex::new(Some(ServerCredential::token(
            "cmt_live".to_string(),
            url,
            "keeper".to_string(),
            Utc::now(),
        ))));
        let failed = connection
            .file_within("open", "assets/shot.png", Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(
            matches!(failed, RemoteFailure::TimedOut { .. }),
            "{failed:?}"
        );
        assert_eq!(connection.down(), None);
    }

    /// A loopback server that reads each request whole, its body included,
    /// and then never answers, holding the connection open.
    async fn silent_after_the_body() -> String {
        after_the_body("", true).await
    }

    /// A loopback server that reads each request whole, its body included,
    /// and then drops the connection without an answer.
    async fn dropping_after_the_body() -> String {
        after_the_body("", false).await
    }

    /// Reads each request whole, writes `answer` (nothing, a whole answer or
    /// the start of one), then holds the connection (`hold`) or drops it.
    async fn after_the_body(answer: &'static str, hold: bool) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut seen = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let Ok(n) = socket.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        seen.extend_from_slice(&buf[..n]);
                        let text = String::from_utf8_lossy(&seen).to_lowercase();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let length = text[..end]
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            if seen.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    // The whole request is in; `answer` is all that goes out.
                    let _ = socket.write_all(answer.as_bytes()).await;
                    let _ = socket.flush().await;
                    if hold {
                        std::future::pending::<()>().await;
                    }
                    drop(socket);
                });
            }
        });
        url
    }

    /// A connection to `url` with a token in hand.
    fn signed_in(url: &str) -> Connection {
        let mut connection = connection(Health::new(Duration::from_secs(30)));
        connection.source.url = url.to_string();
        connection.current = Arc::new(std::sync::Mutex::new(Some(ServerCredential::token(
            "cmt_live".to_string(),
            url.to_string(),
            "keeper".to_string(),
            Utc::now(),
        ))));
        connection
    }

    async fn write(connection: &Connection, args: Value, limit: Duration) -> RemoteFailure {
        connection
            .tool_within_as(
                "write_engram",
                args,
                &ForwardedAgent { client: None },
                limit,
                Budget::Own,
            )
            .await
            .unwrap_err()
    }

    /// An upload the server took in and never answered may have been
    /// written: the agent is told so and how to check, not that the server
    /// cannot be reached, and the source stays up even on its own limit.
    #[tokio::test]
    async fn an_upload_sent_and_never_answered_may_have_been_written() {
        let url = silent_after_the_body().await;
        let connection = signed_in(&url);
        let failed = write(
            &connection,
            json!({ "domain": "open", "title": "Shot", "path": "assets/shot.png",
                    "base64": "aGVsbG8=" }),
            Duration::from_millis(300),
        )
        .await;
        assert!(matches!(failed, RemoteFailure::Unanswered(_)), "{failed:?}");
        assert_eq!(
            failed.to_string(),
            format!(
                "acme ({url}) did not answer within 0.3 s. The file may or may not be written. \
                 Read assets/shot.png with read_engram to check its sha256, or upload the same \
                 file again: the same bytes are accepted."
            )
        );
        assert!(!failed.to_string().contains(UNREACHABLE_WORDS));
        assert_eq!(connection.down(), None, "slow is not down");
    }

    /// An engram write that was sent and never answered says the engram may
    /// be written and to read it before writing it again.
    #[tokio::test]
    async fn an_engram_write_sent_and_never_answered_may_have_been_written() {
        let url = silent_after_the_body().await;
        let connection = signed_in(&url);
        let failed = write(
            &connection,
            json!({ "domain": "open", "title": "Night deploys", "content": "We deploy at night." }),
            Duration::from_millis(300),
        )
        .await;
        assert_eq!(
            failed.to_string(),
            format!(
                "acme ({url}) did not answer within 0.3 s. The engram may or may not be written. \
                 Read 'Night deploys' with read_engram to see whether it holds your text before \
                 you write it again."
            )
        );
        assert_eq!(connection.down(), None);
    }

    /// A write whose connection broke after the request went out may have
    /// been done too: it says so, never "cannot be reached", and the source
    /// stays up.
    #[tokio::test]
    async fn a_write_whose_connection_broke_after_sending_may_have_been_written() {
        let url = dropping_after_the_body().await;
        let connection = signed_in(&url);
        let failed = write(
            &connection,
            json!({ "domain": "open", "title": "Shot", "path": "assets/shot.png",
                    "base64": "aGVsbG8=" }),
            Duration::from_secs(3),
        )
        .await;
        assert_eq!(
            failed.to_string(),
            format!(
                "acme ({url}) broke off the connection before it answered. The file may or may \
                 not be written. Read assets/shot.png with read_engram to check its sha256, or \
                 upload the same file again: the same bytes are accepted."
            )
        );
        assert!(!failed.to_string().contains(UNREACHABLE_WORDS));
        assert_eq!(connection.down(), None);
    }

    /// An automation verb that changes data gets the same treatment; one
    /// that only reads keeps its old failure.
    #[tokio::test]
    async fn an_automation_write_never_answered_may_have_happened() {
        let url = silent_after_the_body().await;
        let connection = signed_in(&url);
        let failed = connection
            .ctl_within_as(
                json!({ "v": 1, "cmd": "automation_run_record", "loop": "evolve",
                        "domain": "open", "token": "t", "report": {} }),
                Duration::from_millis(300),
                Budget::Own,
            )
            .await
            .unwrap_err();
        assert_eq!(
            failed.to_string(),
            format!(
                "acme ({url}) did not answer within 0.3 s. The run may or may not be recorded. \
                 Run crystalline evolve status: the run still shows as running while it is not \
                 recorded."
            )
        );
        assert_eq!(connection.down(), None);

        let read = connection
            .ctl_within_as(
                json!({ "v": 1, "cmd": "automation_status", "loop": "evolve" }),
                Duration::from_millis(300),
                Budget::Own,
            )
            .await
            .unwrap_err();
        assert!(matches!(read, RemoteFailure::TimedOut { .. }), "{read:?}");
        assert!(connection.down().is_some());
    }

    /// A gateway that answers 504 passed the write on and the server behind
    /// it did not answer in time: it may have happened, and the source stays
    /// up. A 502 says the same to the caller but still opens the down window,
    /// with the gateway's plain sentence for every call that follows.
    #[tokio::test]
    async fn a_gateway_answer_to_a_write_says_it_may_have_happened() {
        for (status, said, down) in [
            (
                "504 Gateway Timeout",
                "did not answer in time: its gateway answered 504 Gateway Timeout",
                false,
            ),
            (
                "502 Bad Gateway",
                "gave no answer: its gateway answered 502 Bad Gateway",
                true,
            ),
        ] {
            let head: &'static str = Box::leak(
                format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .into_boxed_str(),
            );
            for (args, check) in [
                (
                    json!({ "domain": "open", "title": "Shot", "path": "assets/shot.png",
                            "base64": "aGVsbG8=" }),
                    "The file may or may not be written. Read assets/shot.png with read_engram \
                     to check its sha256, or upload the same file again: the same bytes are \
                     accepted.",
                ),
                (
                    json!({ "domain": "open", "title": "Night deploys", "content": "x" }),
                    "The engram may or may not be written. Read 'Night deploys' with \
                     read_engram to see whether it holds your text before you write it again.",
                ),
            ] {
                let url = after_the_body(head, false).await;
                let connection = signed_in(&url);
                let failed = write(&connection, args, Duration::from_secs(3)).await;
                assert_eq!(
                    failed.to_string(),
                    format!("acme ({url}) {said}. {check}"),
                    "{status}"
                );
                match connection.down() {
                    Some(mark) if down => {
                        assert!(mark.to_string().contains(UNREACHABLE_WORDS), "{mark}");
                        assert!(!mark.to_string().contains("may or may not"), "{mark}");
                    }
                    None if !down => {}
                    other => panic!("{status}: down window {other:?}"),
                }
            }
        }
    }

    /// A read through a gateway that answers 504 keeps today's sentence and
    /// opens the down window.
    #[tokio::test]
    async fn a_gateway_timeout_on_a_read_still_cannot_be_reached() {
        let url = after_the_body(
            "HTTP/1.1 504 Gateway Timeout\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            false,
        )
        .await;
        let connection = signed_in(&url);
        let failed = connection
            .tool_within_as(
                "read_engram",
                json!({ "identifier": "x", "domain": "open" }),
                &ForwardedAgent { client: None },
                Duration::from_secs(3),
                Budget::Own,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(failed, RemoteFailure::Unreachable { .. }),
            "{failed:?}"
        );
        assert!(
            failed.to_string().contains("gateway answered 504"),
            "{failed}"
        );
        assert!(connection.down().is_some());
    }

    /// The answer's headers came and its body did not: on a write that is
    /// the same unknown result, whether the body stalls or breaks off.
    #[tokio::test]
    async fn a_write_whose_answer_body_failed_may_have_happened() {
        let head = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"v\":1";
        for (hold, said) in [
            (true, "did not answer within 0.5 s"),
            (false, "broke off the connection before it answered"),
        ] {
            let url = after_the_body(head, hold).await;
            let connection = signed_in(&url);
            let failed = write(
                &connection,
                json!({ "domain": "open", "title": "Night deploys", "content": "x" }),
                Duration::from_millis(500),
            )
            .await;
            assert!(
                failed.to_string().starts_with(&format!(
                    "acme ({url}) {said}. The engram may or may not be written."
                )),
                "{failed}"
            );
            assert_eq!(connection.down(), None);
        }
    }

    /// A read that timed out is still a source that does not answer: it says
    /// so and opens the down window, as before.
    #[tokio::test]
    async fn a_read_sent_and_never_answered_still_cannot_be_reached() {
        let url = silent_after_the_body().await;
        let connection = signed_in(&url);
        let failed = connection
            .tool_within_as(
                "search_engrams",
                json!({ "query": "deploy" }),
                &ForwardedAgent { client: None },
                Duration::from_millis(300),
                Budget::Own,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(failed, RemoteFailure::TimedOut { .. }),
            "{failed:?}"
        );
        assert!(connection.down().is_some());
    }

    /// A connect that timed out (here a TLS handshake nobody answers) never
    /// sent the write: it still cannot be reached and opens the down window.
    #[tokio::test]
    async fn a_write_whose_connect_timed_out_cannot_be_reached() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let connection = signed_in(&url);
        connection.http.lock().unwrap().client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let failed = write(
            &connection,
            json!({ "domain": "open", "title": "Shot", "path": "assets/shot.png",
                    "base64": "aGVsbG8=" }),
            Duration::from_secs(3),
        )
        .await;
        assert!(
            matches!(failed, RemoteFailure::TimedOut { .. }),
            "{failed:?}"
        );
        assert!(failed.to_string().contains(UNREACHABLE_WORDS), "{failed}");
        assert!(connection.down().is_some());
    }

    /// A refused connect never sent the write either.
    #[tokio::test]
    async fn a_write_whose_connect_was_refused_cannot_be_reached() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let connection = signed_in(&url);
        let failed = write(
            &connection,
            json!({ "domain": "open", "title": "Night deploys", "content": "x" }),
            Duration::from_secs(3),
        )
        .await;
        assert!(
            matches!(failed, RemoteFailure::Unreachable { .. }),
            "{failed:?}"
        );
        assert!(failed.to_string().contains(UNREACHABLE_WORDS), "{failed}");
        assert!(connection.down().is_some());
    }

    /// The ceiling holds the largest file a domain keeps, as base64.
    #[test]
    fn the_file_ceiling_holds_the_largest_file_as_base64() {
        use base64::Engine as _;
        let largest = vec![0u8; crystalline_core::MAX_ATTACHMENT_BYTES as usize];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&largest);
        assert!(encoded.len() + 1024 < FILE_ANSWER_CAP);
    }

    fn connection_with_no_mark() -> Connection {
        connection(Health::new(Duration::from_secs(30)))
    }
}

#[cfg(test)]
mod version_tests {
    use super::version_at_least;

    #[test]
    fn a_version_is_compared_by_its_numbers_and_an_odd_one_is_never_enough() {
        assert!(version_at_least("0.32.0", "0.32.0"));
        assert!(version_at_least("0.32.1", "0.32.0"));
        assert!(version_at_least("1.0.0", "0.32.0"));
        assert!(version_at_least("0.40.0-rc.1", "0.32.0"));
        assert!(!version_at_least("0.31.9", "0.32.0"));
        assert!(!version_at_least("0.4.0", "0.32.0"));
        assert!(!version_at_least("", "0.32.0"));
        assert!(!version_at_least("next", "0.32.0"));
        assert!(!version_at_least("0.32", "0.32.0"));
    }
}
