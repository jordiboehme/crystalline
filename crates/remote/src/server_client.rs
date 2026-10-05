//! The connection to one source: HTTPS plus the source's stored token, the
//! control protocol (`POST /api/v1/ctl`), and the OAuth refresh.
//!
//! # Why the refresh takes a file lock
//!
//! The server rotates the refresh token on every use and reads a rotated one
//! coming back as a stolen credential, which revokes the whole grant
//! (`AuthStore::refresh_oauth_grant`). The daemon's poller, the CLI and the
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
//!   seconds on a large domain. The token request of a refresh always has
//!   [`ONE_DOMAIN_LIMIT`], whatever the caller's budget (see above).
//! - **A network failure is never "sign in again".** It is
//!   [`RemoteFailure::Unreachable`] or [`RemoteFailure::TimedOut`], and a
//!   refresh that fails on the network leaves the stored refresh token as it
//!   was, so the next call tries again. Only the server's own refusal asks
//!   for a new sign-in: an OAuth `invalid_grant` or `invalid_client` from the
//!   token endpoint, or a `401` that a refresh cannot cure.
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
/// How long a pooled connection may sit idle, by the wall clock.
pub const POOL_IDLE: Duration = Duration::from_secs(30);
/// The refresh lock, in the source's host folder.
pub const REFRESH_LOCK_FILE: &str = "refresh.lock";
/// How long a refresh waits for another process's refresh to finish.
const LOCK_WAIT: Duration = Duration::from_secs(15);
/// The forwarded tool that runs a sweep on the server.
const EVOLVE_TOOL: &str = "evolve_engrams";
/// What every unreachable or timed-out failure says, and what
/// [`crate::stale_line`] recognises it by.
pub(crate) const UNREACHABLE_WORDS: &str = "cannot be reached right now";
/// What the failure of a server without the control protocol says, and
/// what [`crate::stale_line`] recognises it by.
pub(crate) const TOO_OLD_WORDS: &str = "does not serve the remote control protocol";
/// What a person can do about a source that does not answer.
const NETWORK_HINT: &str =
    "check the VPN or the network; it recovers by itself once the server answers again";

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
    /// The server did not answer within the limit.
    TimedOut {
        /// The source's name.
        source: String,
        /// The server.
        url: String,
        /// How long it waited.
        after: Duration,
    },
}

impl RemoteFailure {
    /// Whether this source needs a new sign-in.
    pub fn is_sign_in(&self) -> bool {
        matches!(self, RemoteFailure::SignInAgain { .. })
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
            RemoteFailure::Credential(text) | RemoteFailure::Refused(text) => f.write_str(text),
            RemoteFailure::TimedOut { source, url, after } => write!(
                f,
                "{source} ({url}) {UNREACHABLE_WORDS}: it did not answer within {} ({NETWORK_HINT})",
                seconds(*after)
            ),
        }
    }
}

impl std::error::Error for RemoteFailure {}

/// `3 s`, `0.7 s`: a deadline as a person reads it.
pub(crate) fn seconds(after: Duration) -> String {
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
/// limit.
pub fn http_client() -> Result<reqwest::Client, RemoteFailure> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .pool_idle_timeout(POOL_IDLE)
        .tcp_keepalive(Duration::from_secs(15))
        .user_agent(concat!("crystalline/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| RemoteFailure::Refused(format!("could not build the HTTP client: {e}")))
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

    /// A gateway's `502`, `503` or `504`: the server behind it is down.
    fn gateway(&self, status: reqwest::StatusCode) -> Option<RemoteFailure> {
        matches!(status.as_u16(), 502..=504).then(|| RemoteFailure::Unreachable {
            source: self.source.clone(),
            url: self.url.clone(),
            detail: format!("its gateway answered {status}, so the server behind it is down"),
        })
    }
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
        Connection::open_with(source, remote_dir, |name| std::env::var(name).ok())
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
            && failure.is_unreachable()
        {
            self.health.mark_down(&self.source.key(), failure);
            self.drop_pool();
        }
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
        let running = tokio::spawn(job.run());
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
        self.skip()?;
        let result = self.ctl_by(&request, Instant::now() + limit, limit).await;
        self.note(&result);
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
        self.ctl_data_within(request, self.limit).await
    }

    async fn ctl_data_within(
        &self,
        request: Value,
        limit: Duration,
    ) -> Result<Value, RemoteFailure> {
        match self.ctl_within(request, limit).await? {
            CtlAnswer::Data { data, .. } => Ok(data),
            CtlAnswer::NotModified { .. } => Ok(Value::Null),
        }
    }

    /// One forwarded tool call for one domain, in the source's own names,
    /// with the agent it is for: within this connection's limit, or
    /// [`CTL_TIMEOUT`] for `evolve_engrams`. A refusal comes back as
    /// [`RemoteFailure::Refused`] carrying the server's sentence word for
    /// word.
    pub async fn tool(
        &self,
        tool: &str,
        args: Value,
        agent: &ForwardedAgent,
    ) -> Result<Value, RemoteFailure> {
        let limit = if tool == EVOLVE_TOOL {
            CTL_TIMEOUT.max(self.limit)
        } else {
            self.limit
        };
        self.tool_within(tool, args, agent, limit).await
    }

    /// [`Connection::tool`] within `limit`: the fan-out deadline for a call
    /// over all domains.
    pub async fn tool_within(
        &self,
        tool: &str,
        args: Value,
        agent: &ForwardedAgent,
        limit: Duration,
    ) -> Result<Value, RemoteFailure> {
        let mut request = json!({ "v": 1, "cmd": "tool", "tool": tool, "args": args });
        if let Some(client) = &agent.client {
            request["agent"] = json!({ "client": client });
        }
        self.ctl_data_within(request, limit).await
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
        let response = self
            .http()
            .post(format!("{}{}", self.source.url, crate::CTL_PATH))
            .bearer_auth(bearer)
            .header("content-type", "application/json")
            .timeout(remaining)
            .body(request.to_string())
            .send()
            .await
            .map_err(|e| peer.failure(&e, limit))?;
        let status = response.status();
        if let Some(down) = peer.gateway(status) {
            return Err(down);
        }
        self.answered();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(Posted::Unauthorized);
        }
        // The request's own timeout runs until the body has finished.
        let text = response.text().await.map_err(|e| peer.failure(&e, limit))?;
        if status == reqwest::StatusCode::NOT_FOUND
            || status == reqwest::StatusCode::METHOD_NOT_ALLOWED
        {
            return Err(RemoteFailure::Refused(format!(
                "{} {TOO_OLD_WORDS}; it needs Crystalline 0.23 or newer",
                self.source.url
            )));
        }
        if !status.is_success() {
            let excerpt: String = text.chars().take(200).collect();
            return Err(RemoteFailure::Refused(format!(
                "{} answered {status}: {excerpt}",
                self.source.url
            )));
        }
        let envelope: Value = serde_json::from_str(&text).map_err(|_| {
            RemoteFailure::Refused(format!(
                "{} did not answer in the control protocol",
                self.source.url
            ))
        })?;
        if envelope.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(RemoteFailure::Refused(
                envelope
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("ctl error")
                    .to_string(),
            ));
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

/// Whether `credential` is somebody else's finished refresh: no longer the
/// token the caller saw rejected, and not due for a refresh itself.
fn settled(credential: &ServerCredential, rejected: Option<&str>) -> bool {
    Some(credential.access_token.as_str()) != rejected && !credential.needs_refresh(Utc::now())
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
            let what = if error.is_empty() {
                status.to_string()
            } else {
                format!("{status}, {error}")
            };
            return Err(RemoteFailure::Refused(format!(
                "{} answered a refresh with {what}; the sign-in is kept and the next call tries again",
                self.peer.url
            )));
        }
        let answer: TokenAnswer = serde_json::from_str(&text).map_err(|e| {
            RemoteFailure::Refused(format!(
                "the token endpoint of {} answered something that is not a token: {e}",
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

    fn connection_with_no_mark() -> Connection {
        connection(Health::new(Duration::from_secs(30)))
    }
}
