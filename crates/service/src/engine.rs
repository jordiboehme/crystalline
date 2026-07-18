//! The shared service engine.
//!
//! Every data operation (the MCP tools, the CLI data commands and the ctl
//! sync and reindex) runs through one [`Engine`]. It owns a single boxed
//! [`Store`] (`dyn Store`) behind a [`tokio::sync::Mutex`] so the backend's
//! single-connection model is honoured across the daemon's many tasks, the
//! optional embedding provider (built once), the resolved config and the chunk
//! parameters. The concrete backend is chosen at open time by the store factory
//! from the `database` config block.
//!
//! Files are the source of truth: every mutation writes the file first, then
//! upserts that single file into the store using the on-disk file stamp, so the
//! daemon's debounced watcher classifies the file as unchanged and never
//! reprocesses it (the idempotency guard, see `research/single-instance-ipc.md`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Duration, FixedOffset, Utc};
use crystalline_core::config::{
    DomainEntry, DomainKind as CoreDomainKind, GlobalConfig, OriginConfig,
};
use crystalline_core::emit::{
    append_body, insert_after_section, insert_before_section, prepend_body,
    remove_frontmatter_field, replace_section, touch_timestamp,
};
use crystalline_core::schema::{self, Schema};
use crystalline_core::{
    CrystallineUrl, Engram, Frontmatter, HarnessKind, Manifest, YamlValue, parse_engram, slugify,
};
use crystalline_index::{
    ChunkParams, DomainHost, DomainId, DomainKind, EmbeddingProvider, EngramDescriptor, EngramId,
    EngramRecord, FileStamp, HostClaim, RecentFilter, SearchMode, SearchQuery, Store, SyncReport,
    apply_scan, chunk_engram, configured_model_id, order_jobs_for_batching, parse_metadata_filters,
    provider_from_config, scan_domain, scan_paths,
};
use crystalline_remote::ops;
use crystalline_remote::{
    GitHubProvider, OriginSpec, Provider, RemoteError, StoredToken, TokenStore,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::origin;
use crate::overlay::{self, EnvOverlay, LoadedConfig};
use crate::params::*;
use crate::poller;
use crate::settings;

/// How many chunks are embedded per background batch.
const EMBED_BATCH: usize = 16;

/// The default host-lock heartbeat interval, seconds. Overridable via
/// `CRYSTALLINE_HEARTBEAT_SECS` (used to drive fast multi-instance verification).
const DEFAULT_HEARTBEAT_SECS: i64 = 30;
/// The default host-lock stale threshold, seconds (three missed heartbeats). A
/// lock whose last heartbeat is older than this is takeable by another instance.
/// Overridable via `CRYSTALLINE_STALE_SECS`.
const DEFAULT_STALE_SECS: i64 = 90;

/// Read a positive-integer seconds value from an environment variable, falling
/// back to `default` when unset, empty, unparseable or non-positive.
fn env_secs(key: &str, default: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

/// An error from an engine operation, mapped to actionable tool errors.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// A referenced domain is not registered.
    #[error("domain '{domain}' not registered; registered: [{}]", .registered.join(", "))]
    UnknownDomain {
        /// The requested domain.
        domain: String,
        /// The registered domain names.
        registered: Vec<String>,
    },
    /// The engram or section was not found.
    #[error("{0}")]
    NotFound(String),
    /// A bare identifier matched engrams in more than one domain.
    #[error("{0}")]
    Ambiguous(String),
    /// A write would clobber an existing engram without `overwrite`.
    #[error("{0}")]
    Conflict(String),
    /// The request was malformed.
    #[error("{0}")]
    Invalid(String),
    /// A content mutation was attempted against a read-only instance.
    #[error("this instance is read-only; content mutations are disabled")]
    ReadOnly,
    /// An interactive connect action (`connect_with_token`,
    /// `start_device_connect`) was attempted while `CRYSTALLINE_GITHUB_TOKEN`
    /// is set. This machine's identity is fixed by the environment, so there
    /// is nothing for a sign-in to change until the variable is unset.
    #[error(
        "this machine's GitHub identity comes from CRYSTALLINE_GITHUB_TOKEN; unset it to sign in interactively"
    )]
    EnvTokenConnect,
    /// A filesystem error.
    #[error("io error at {path}: {source}")]
    Io {
        /// The path involved.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// An error from the storage or parse layer.
    #[error("{0}")]
    Internal(String),
    /// A GitHub collaboration error from the remote origin engine, surfaced
    /// with its message verbatim: every `RemoteError` variant is already
    /// actionable product copy (see `crystalline_remote::error`), so this
    /// never re-wraps or restates it. Deliberately not `#[from]`: thiserror
    /// would then also derive `source()` pointing back at the same
    /// `RemoteError`, and since its text is identical to this variant's own
    /// `Display`, a top-level `anyhow` printer would show the message twice
    /// (once as the error, once as its "caused by"). The manual `From` impl
    /// below converts without that, mirroring `IndexError`'s and
    /// `SettingsError`'s conversions in this file.
    #[error("{0}")]
    Remote(crystalline_remote::RemoteError),
}

impl From<crystalline_remote::RemoteError> for EngineError {
    fn from(e: crystalline_remote::RemoteError) -> Self {
        EngineError::Remote(e)
    }
}

impl From<crystalline_index::IndexError> for EngineError {
    fn from(e: crystalline_index::IndexError) -> Self {
        match e {
            crystalline_index::IndexError::Constraint(m) => EngineError::Conflict(m),
            crystalline_index::IndexError::NotFound(m) => EngineError::NotFound(m),
            crystalline_index::IndexError::Invalid(m) => EngineError::Invalid(m),
            // A stale compare-and-swap surfaces as a conflict, mirroring the
            // expected_replacements ergonomics: re-read and retry.
            crystalline_index::IndexError::StaleEdit { expected, found } => {
                EngineError::Conflict(format!(
                    "engram changed since you last read it (expected checksum {expected}, found {found}); re-read and retry"
                ))
            }
            other => EngineError::Internal(other.to_string()),
        }
    }
}

/// The result type used across the engine.
pub type Result<T> = std::result::Result<T, EngineError>;

/// A stage-boundary progress callback for a long connect:
/// (step, total steps, message). Sync and cheap by contract; the MCP
/// layer bridges it onto async notifications through a channel.
pub type OriginProgress = std::sync::Arc<dyn Fn(u64, u64, &str) + Send + Sync>;

// --- connect auth (a testable seam over crystalline_remote::github::auth) ---

/// The GitHub identity calls the `configure` tool's connect actions need:
/// validating a token and running a device flow to completion. Production
/// always uses [`RealConnectAuth`], a thin pass-through to
/// `crystalline_remote::github::auth`; tests inject a fake so the
/// pending-connect state machine (one flow at a time, a landed outcome
/// reported once, the slot cleared after) can be driven deterministically,
/// with no real device flow, network access or OS keychain interaction.
#[async_trait::async_trait]
pub trait ConnectAuth: Send + Sync {
    /// Starts a device-flow sign-in, returning the code to show the user.
    async fn start_device_flow(
        &self,
        auth_base: &str,
        client_id: &str,
    ) -> std::result::Result<crystalline_remote::DeviceFlowStart, RemoteError>;

    /// Runs a started device flow to completion, returning the access token.
    async fn run_device_flow(
        &self,
        auth_base: &str,
        client_id: &str,
        start: &crystalline_remote::DeviceFlowStart,
    ) -> std::result::Result<String, RemoteError>;

    /// Validates a token (freshly issued by a device flow, or a pasted
    /// personal access token), returning the signed-in login.
    async fn validate_token(
        &self,
        api_url: Option<&str>,
        token: &str,
    ) -> std::result::Result<String, RemoteError>;
}

/// The production [`ConnectAuth`]: delegates straight to
/// `crystalline_remote::github::auth`.
struct RealConnectAuth;

#[async_trait::async_trait]
impl ConnectAuth for RealConnectAuth {
    async fn start_device_flow(
        &self,
        auth_base: &str,
        client_id: &str,
    ) -> std::result::Result<crystalline_remote::DeviceFlowStart, RemoteError> {
        crystalline_remote::github::auth::start_device_flow(auth_base, client_id).await
    }

    async fn run_device_flow(
        &self,
        auth_base: &str,
        client_id: &str,
        start: &crystalline_remote::DeviceFlowStart,
    ) -> std::result::Result<String, RemoteError> {
        crystalline_remote::github::auth::run_device_flow(auth_base, client_id, start).await
    }

    async fn validate_token(
        &self,
        api_url: Option<&str>,
        token: &str,
    ) -> std::result::Result<String, RemoteError> {
        crystalline_remote::github::auth::validate_token(api_url, token).await
    }
}

/// A message from the engine to the daemon's file watcher: a domain root to
/// start or stop watching, raised when a domain registered after the daemon
/// started is first resolved (see [`Engine::domain_entry`]) or removed (see
/// [`Engine::forget_domain`]). Only the daemon's watcher task consumes these;
/// embedded stdio and standalone CLI commands never install a receiver.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// Start watching this domain's root.
    Add(String, PathBuf),
    /// Stop watching this domain's root.
    Remove(String),
}

/// Where a domain's engram content comes from and goes to: files on disk for a
/// file domain, or the database for a virtual domain. This is the one seam every
/// content mutation branches on; everything after `parse_engram` is shared (see
/// [`Engine::index_markdown`]).
enum ContentSource {
    /// A file domain rooted at this filesystem path.
    File {
        /// The tilde-expanded domain root.
        root: PathBuf,
    },
    /// A virtual domain whose engrams live only in the database.
    Virtual,
}

/// The shared service engine.
pub struct Engine {
    store: Arc<Mutex<dyn Store>>,
    // The effective config: the file config with the environment overlay
    // applied. Every runtime read goes through this, so the ~30 read sites stay
    // untouched by the file/effective split. Behind a lock (not an immutable
    // snapshot) so `configure` can update a setting and every later read
    // (including a concurrent one) sees it, mirroring the
    // `discovered_domains`/`provider` interior-mutability pattern below.
    config: std::sync::RwLock<GlobalConfig>,
    // The persisted file config, the truth `persist_config` writes back. Kept
    // apart from `config` so an environment value never bakes itself into
    // `config.yaml`: `configure show` and `set`/`unset` read and mutate this,
    // and the effective `config` above is recomputed from it plus the overlay.
    // The lock order is always `file_config` then `config`.
    file_config: std::sync::RwLock<GlobalConfig>,
    // The parsed environment overlay layered on top of `file_config` to produce
    // `config`. Empty by default (the standalone construction path and every
    // existing test); the daemon and the standalone loader install the real one
    // via `with_env_overlay`.
    overlay: EnvOverlay,
    // The `--config` override this engine was started with, so a domain
    // registered after startup (`domain add` only ever touches the file on
    // disk) can be found by re-reading the same file. See `refresh_domain`.
    config_path: Option<PathBuf>,
    // Domains discovered by re-reading the global config after startup,
    // layered on top of the immutable `config` snapshot taken at construction.
    // The full entry is kept (kind plus optional path) so a virtual domain
    // added mid-session is served from the database, not mistaken for a file
    // domain with an empty root.
    discovered_domains: std::sync::RwLock<HashMap<String, DomainEntry>>,
    // Told about domains discovered this way so the daemon's watcher can pick
    // them up without a restart. `None` outside the daemon.
    watch_tx: Option<tokio::sync::mpsc::UnboundedSender<WatchEvent>>,
    // The channel a background embed worker listens on, so long-running verbs
    // schedule an embed pass there instead of running it inline and blocking
    // the caller on the model. `None` when no worker is wired (standalone
    // one-shot commands and most tests), which keeps the inline pass.
    embed_tx: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    // Swappable so the daemon can build the (possibly downloading) provider in the
    // background without blocking readiness or text search.
    provider: std::sync::RwLock<Option<Arc<dyn EmbeddingProvider>>>,
    model_id: String,
    chunk_params: ChunkParams,
    // When true the four content-mutating methods refuse early with
    // `EngineError::ReadOnly`. Set at construction from the effective mode
    // (explicit flag or `service.read_only`). Index maintenance is unaffected.
    read_only: bool,
    // This instance's stable id for shared-database collaboration, or empty when
    // collaboration is off (standalone commands and the embedded stdio stack).
    // Only a non-empty id claims host locks, scopes embedding and refuses a
    // non-host sync; the `serve` daemon sets it via `with_instance_id`.
    instance_id: String,
    // The human label recorded alongside the host lock (currently the instance
    // id; a stable, greppable handle in a shared database).
    label: String,
    // The file domains this instance currently hosts, name to id, populated by a
    // successful `claim_domain_host` and renewed by the heartbeat timer. Drives
    // embed scoping, heartbeat renewal and graceful release.
    hosted: std::sync::RwLock<HashMap<String, DomainId>>,
    // The heartbeat interval and stale threshold, seconds. Defaults 30 and 90,
    // overridable via `CRYSTALLINE_HEARTBEAT_SECS`/`CRYSTALLINE_STALE_SECS` (a
    // short threshold makes multi-instance stale-takeover verification fast).
    heartbeat_secs: i64,
    stale_secs: i64,
    // Per-domain lock serializing `origin_add`, `origin_update` and
    // `origin_status` against each other for one domain, so a connect and a
    // pull racing on the same domain never interleave. Created lazily, one
    // `tokio::sync::Mutex` per domain name ever operated on; held across the
    // whole call rather than reasoning about which sub-step actually needs
    // it, simplest and cheap since these calls are already rare and short.
    origin_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    // A fixed provider used by every origin operation instead of the
    // production per-operation `GitHubProvider` build, for tests: an engine
    // built this way never reads config or the token store to decide who to
    // talk to, and `origin_status`'s connection block reflects the injected
    // provider's own identity rather than a real, untestable OS credential
    // store. Production code never sets this.
    origin_provider_override: Option<Arc<dyn Provider>>,
    // Overrides where per-domain origin state (the base snapshot, conflict
    // records, `state.json`) is read and written, for tests: `None` means the
    // real `crystalline_core::config::origin_state_dir`, a real machine path
    // no test may touch.
    origins_dir_override: Option<PathBuf>,
    // The `configure` tool's connect actions: production always resolves a
    // fresh `RealConnectAuth`; tests inject a fake so the pending-connect
    // state machine runs with no real device flow or network access.
    connect_auth: Arc<dyn ConnectAuth>,
    // The one in-flight device-flow sign-in this engine is tracking, if any.
    // See `PendingConnect` and `Engine::start_device_connect`.
    pending_connect: std::sync::Mutex<Option<PendingConnect>>,
    // Forces the GitHub token store to a plain file under this directory
    // instead of the real OS keychain, for tests: connect and configure tests
    // must never read, write or prompt for the developer's actual credential
    // store. `None` (production) resolves through `TokenStore::resolve_and_load`
    // and `save_resolving`, cached per process in `github_tokens`.
    token_store_dir_override: Option<PathBuf>,
    // A process-lifetime cache of the resolved GitHub token store and the
    // token it holds, keyed by token host ("" for GitHub.com, the bare host
    // for a GitHub Enterprise Server). The point is that one machine reads its
    // OS keychain at most once per process: the first `github_credential`
    // touch for a host performs the single keychain read and every later one
    // is served from here, so a daemon polling N team domains prompts the
    // keychain once, not once per domain per tick. A std (not tokio) mutex on
    // purpose: the critical section never awaits, and holding the lock across
    // that one keychain read single-flights concurrent first touches into a
    // single prompt rather than a race of N. Only present-token outcomes are
    // cached (an entry existing means a token exists); a `None` stays live so
    // a `connect` landing later - in this process or a standalone CLI writing
    // the same keychain item - is picked up on the very next call.
    github_tokens: Arc<std::sync::Mutex<HashMap<String, CachedGithub>>>,
    // The background origin poller's observable state: every domain's poll
    // schedule and most recent result, plus the poller's one shared
    // rate-limit pause. Always present (not an `Option`), whether or not
    // `run_origin_poller` is actually spawned, so `status_report`'s offline
    // `origins` block reads the same field in a daemon or a one-shot
    // standalone engine alike; it simply stays at its empty default when no
    // poller ever ticks.
    origin_poller: poller::OriginPollerState,
    // The routing bullets of every virtual domain, keyed by domain name, cached
    // for the SYNC `routing_text` path. A virtual domain's bullets live in the
    // database (its MANIFEST engram), so they cannot be read from `routing_text`
    // without an await; this cache is recomputed off the async path by
    // `refresh_routing_cache` at each MCP connection's initialize and after every
    // virtual-source write, and read here under the lock. Empty at construction
    // and for an engine that never serves MCP.
    routing_virtual: std::sync::RwLock<BTreeMap<String, Vec<String>>>,
    // A live view of what this engine is doing (sync, embed, reindex), fed by
    // RAII guards from the maintenance operations and read by `status_report`'s
    // activity block. Behind an `Arc` so a guard owns its own handle and a
    // panicking or early-returning operation still clears its entry on drop.
    activity: Arc<std::sync::Mutex<ActivityState>>,
}

/// One host's cached GitHub credential: the resolved store and the token it
/// held at the single keychain read this process ever does for that host. The
/// token is non-optional - only a present-token outcome is ever cached, so an
/// entry existing in [`Engine::github_tokens`] means a token exists - and the
/// type carries no `Debug` impl, so a cached secret cannot reach a log line or
/// panic message through the engine's own `Debug`.
struct CachedGithub {
    store: TokenStore,
    token: StoredToken,
}

/// The engine's observable activity: what is running now and what finished
/// last. Fed exclusively through [`ActivityGuard`]s.
#[derive(Default)]
pub(crate) struct ActivityState {
    next_token: u64,
    current: Vec<(u64, ActivityEntry)>,
    last_done: Option<(ActivityEntry, chrono::DateTime<chrono::Utc>)>,
}

#[derive(Clone)]
pub(crate) struct ActivityEntry {
    kind: &'static str,
    domain: Option<String>,
    started_at: chrono::DateTime<chrono::Utc>,
}

impl ActivityState {
    /// Register an operation and hand back the guard that ends it.
    pub(crate) fn begin(
        state: &Arc<std::sync::Mutex<ActivityState>>,
        kind: &'static str,
        domain: Option<&str>,
    ) -> ActivityGuard {
        let mut inner = state.lock().unwrap();
        inner.next_token += 1;
        let token = inner.next_token;
        inner.current.push((
            token,
            ActivityEntry {
                kind,
                domain: domain.map(str::to_string),
                started_at: chrono::Utc::now(),
            },
        ));
        ActivityGuard {
            state: Arc::clone(state),
            token,
        }
    }

    /// The status-report shape: `now` lists running operations with their
    /// elapsed seconds, `last` the most recently finished one.
    pub(crate) fn snapshot_json(&self) -> Value {
        let now = chrono::Utc::now();
        let current: Vec<Value> = self
            .current
            .iter()
            .map(|(_, e)| {
                json!({
                    "kind": e.kind,
                    "domain": e.domain,
                    "for_secs": (now - e.started_at).num_seconds().max(0),
                })
            })
            .collect();
        let last = self.last_done.as_ref().map(|(e, at)| {
            json!({
                "kind": e.kind,
                "domain": e.domain,
                "finished_at": at.to_rfc3339(),
            })
        });
        json!({ "now": current, "last": last })
    }
}

/// Ends the activity it belongs to on drop, recording it as the last
/// finished operation.
pub(crate) struct ActivityGuard {
    state: Arc<std::sync::Mutex<ActivityState>>,
    token: u64,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        if let Some(pos) = state.current.iter().position(|(t, _)| *t == self.token) {
            let (_, entry) = state.current.remove(pos);
            state.last_done = Some((entry, chrono::Utc::now()));
        }
    }
}

impl Engine {
    /// Build an engine around an already-open store, an optional provider and a
    /// config. A `None` provider can be installed later with [`Engine::set_provider`].
    /// `config_path` is the `--config` override (if any) this engine started
    /// with, used to re-read the config file when a domain is not in the
    /// startup snapshot; pass `None` when the caller never re-reads (a
    /// one-shot standalone CLI command already sees a fresh config).
    pub fn new(
        store: Arc<Mutex<dyn Store>>,
        config: GlobalConfig,
        provider: Option<Arc<dyn EmbeddingProvider>>,
        config_path: Option<PathBuf>,
    ) -> Engine {
        let model_id = configured_model_id(config.embeddings.as_ref());
        let chunk_params = ChunkParams::for_model(model_id.clone());
        // No overlay yet: file and effective start identical, so an engine
        // built without `with_env_overlay` behaves exactly as before the split.
        let file_config = config.clone();
        Engine {
            store,
            config: std::sync::RwLock::new(config),
            file_config: std::sync::RwLock::new(file_config),
            overlay: EnvOverlay::default(),
            config_path,
            discovered_domains: std::sync::RwLock::new(HashMap::new()),
            watch_tx: None,
            embed_tx: None,
            provider: std::sync::RwLock::new(provider),
            model_id,
            chunk_params,
            read_only: false,
            instance_id: String::new(),
            label: String::new(),
            hosted: std::sync::RwLock::new(HashMap::new()),
            heartbeat_secs: env_secs("CRYSTALLINE_HEARTBEAT_SECS", DEFAULT_HEARTBEAT_SECS),
            stale_secs: env_secs("CRYSTALLINE_STALE_SECS", DEFAULT_STALE_SECS),
            origin_locks: std::sync::Mutex::new(HashMap::new()),
            origin_provider_override: None,
            origins_dir_override: None,
            connect_auth: Arc::new(RealConnectAuth),
            pending_connect: std::sync::Mutex::new(None),
            token_store_dir_override: None,
            github_tokens: Arc::default(),
            origin_poller: poller::OriginPollerState::default(),
            routing_virtual: std::sync::RwLock::new(BTreeMap::new()),
            activity: Arc::default(),
        }
    }

    /// Turn on shared-database collaboration for this engine by giving it a
    /// stable instance id (the `serve` daemon supplies the persisted one from
    /// `config::read_or_create_instance_id`). With an id set, syncing a file
    /// domain first claims its host lock: acquired domains sync and embed here, a
    /// domain held by another live instance is skipped on a full sync and refused
    /// on a named one and this instance renews its locks on the heartbeat timer.
    /// An empty id (the default) leaves collaboration off.
    pub fn with_instance_id(mut self, instance_id: String) -> Engine {
        self.label = instance_id.clone();
        self.instance_id = instance_id;
        self
    }

    /// Install the channel the daemon's watcher listens on for domains
    /// discovered after startup. Only wired by `run_serve`.
    pub fn with_watch_channel(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<WatchEvent>,
    ) -> Engine {
        self.watch_tx = Some(tx);
        self
    }

    /// Wires the channel a background embed worker listens on. When present,
    /// long-running verbs schedule embedding there instead of embedding
    /// inline, so a connect request returns without waiting on the model.
    pub fn with_embed_channel(mut self, tx: tokio::sync::mpsc::UnboundedSender<()>) -> Engine {
        self.embed_tx = Some(tx);
        self
    }

    /// Set the read-only mode. In read-only mode the four content-mutating
    /// methods refuse with `EngineError::ReadOnly`; every read path and all
    /// index maintenance (sync, reindex, embedding) run unchanged.
    pub fn with_read_only(mut self, read_only: bool) -> Engine {
        self.read_only = read_only;
        self
    }

    /// Whether this engine serves the content API read-only.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Install the environment overlay and recompute the effective config from
    /// the file config plus this overlay. The daemon and the standalone loader
    /// call this with the overlay parsed at startup; every existing call site
    /// leaves the default empty overlay in place, so file and effective stay
    /// identical there.
    pub fn with_env_overlay(mut self, overlay: EnvOverlay) -> Engine {
        let effective = overlay.apply(&self.file_config.read().unwrap());
        *self.config.write().unwrap() = effective;
        self.overlay = overlay;
        self
    }

    /// Inject a fixed provider for every origin operation (`origin_add`,
    /// `origin_update`, `origin_status`), bypassing the production
    /// per-operation `GitHubProvider` build from config and the token store.
    /// Test-only: production code always leaves this unset so the provider is
    /// built from the cached GitHub token (read from the keychain at most once
    /// per process, see [`Engine::github_credential`]) and a new `connect` is
    /// still picked up without a restart.
    pub fn with_origin_provider(mut self, provider: Arc<dyn Provider>) -> Engine {
        self.origin_provider_override = Some(provider);
        self
    }

    /// Override the base directory per-domain origin state is read and
    /// written under, in place of the real
    /// `crystalline_core::config::origin_state_dir`. Test-only: lets origin
    /// tests use a tempdir instead of touching the real machine's state
    /// directory.
    pub fn with_origins_dir(mut self, dir: PathBuf) -> Engine {
        self.origins_dir_override = Some(dir);
        self
    }

    /// Inject a fake [`ConnectAuth`] for the `configure` tool's connect
    /// actions, bypassing the real device flow and token validation.
    /// Test-only: production code always leaves this at the default
    /// `RealConnectAuth`.
    pub fn with_connect_auth(mut self, auth: Arc<dyn ConnectAuth>) -> Engine {
        self.connect_auth = auth;
        self
    }

    /// Force the GitHub token store to a plain file under `dir`, never the
    /// real OS keychain. Test-only: a connect or configure test must never
    /// read, write or prompt for the developer's actual credential store.
    pub fn with_token_store_dir(mut self, dir: PathBuf) -> Engine {
        self.token_store_dir_override = Some(dir);
        self
    }

    /// The shared store handle, for the daemon's watcher and embed loop.
    pub fn store(&self) -> Arc<Mutex<dyn Store>> {
        self.store.clone()
    }

    /// The active embedding provider, if one has been installed.
    pub fn provider(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        self.provider.read().unwrap().clone()
    }

    /// Install (or replace) the embedding provider. Used by the daemon after it
    /// builds the provider in the background.
    pub fn set_provider(&self, provider: Arc<dyn EmbeddingProvider>) {
        *self.provider.write().unwrap() = Some(provider);
    }

    /// A snapshot of the registered config as of now, reflecting any
    /// `configure` set or unset applied since construction.
    pub fn config(&self) -> GlobalConfig {
        self.config.read().unwrap().clone()
    }

    /// Whether team collaboration is enabled, read fresh under the config guard
    /// without cloning the whole config. `config()` stays for callers that need
    /// a full snapshot.
    pub fn github_enabled(&self) -> bool {
        self.config.read().unwrap().github_enabled()
    }

    /// The active embedding model id.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// This instance's collaboration id, or empty when collaboration is off.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// The host-lock heartbeat interval in seconds, for the daemon's timer.
    pub fn heartbeat_secs(&self) -> u64 {
        self.heartbeat_secs.max(1) as u64
    }

    // --- host locks ----------------------------------------------------------

    /// Claim the host lock for one file domain against a locked store. Records
    /// the domain in `hosted` on success and drops it on a loss, so the heartbeat
    /// timer and embed scoping stay in step with what this instance actually
    /// hosts.
    async fn claim_file_host(
        &self,
        store: &dyn Store,
        name: &str,
        root: &Path,
        take_over: bool,
    ) -> Result<HostClaim> {
        let id = store
            .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
            .await?;
        let now = now_offset().to_rfc3339();
        let stale_before = (now_offset() - Duration::seconds(self.stale_secs)).to_rfc3339();
        let claim = store
            .claim_domain_host(
                id,
                &self.instance_id,
                &self.label,
                &now,
                &stale_before,
                take_over,
            )
            .await?;
        match &claim {
            HostClaim::Acquired => {
                self.hosted.write().unwrap().insert(name.to_string(), id);
            }
            HostClaim::HeldByOther(_) => {
                self.hosted.write().unwrap().remove(name);
            }
        }
        Ok(claim)
    }

    /// Claim the host lock for a file domain by name (resolving its root and
    /// locking the store), for the daemon's watch-arming path. A no-op that
    /// reports `Acquired` when collaboration is off or the domain is virtual, so
    /// the caller arms the watch uniformly.
    pub async fn claim_host(&self, name: &str, take_over: bool) -> Result<HostClaim> {
        if self.instance_id.is_empty() {
            return Ok(HostClaim::Acquired);
        }
        let ContentSource::File { root } = self.content_source(name)? else {
            return Ok(HostClaim::Acquired);
        };
        let store = self.store.lock().await;
        self.claim_file_host(&*store, name, &root, take_over).await
    }

    /// Renew this instance's heartbeat on every host lock it holds. A lock that
    /// no longer belongs to this instance (another took it over) is dropped from
    /// `hosted` so this instance stops renewing and hosting it. Called on the
    /// daemon's periodic timer and a no-op when collaboration is off.
    pub async fn renew_hosts(&self) {
        if self.instance_id.is_empty() {
            return;
        }
        let hosted: Vec<(String, DomainId)> = self
            .hosted
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        if hosted.is_empty() {
            return;
        }
        let now = now_offset().to_rfc3339();
        let store = self.store.lock().await;
        for (name, id) in hosted {
            match store.renew_domain_host(id, &self.instance_id, &now).await {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(
                        "lost the host lock for domain '{name}'; another instance took over"
                    );
                    self.hosted.write().unwrap().remove(&name);
                }
                Err(e) => tracing::warn!("failed to renew the host lock for '{name}': {e}"),
            }
        }
    }

    /// Release every host lock this instance holds, for a graceful shutdown, so a
    /// successor acquires immediately instead of waiting out the stale threshold.
    /// A no-op when collaboration is off.
    pub async fn release_hosts(&self) {
        if self.instance_id.is_empty() {
            return;
        }
        let hosted: Vec<(String, DomainId)> = self
            .hosted
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        if hosted.is_empty() {
            return;
        }
        {
            let store = self.store.lock().await;
            for (_, id) in &hosted {
                let _ = store.release_domain_host(*id, &self.instance_id).await;
            }
        }
        self.hosted.write().unwrap().clear();
    }

    // --- domain helpers ------------------------------------------------------

    /// Resolve a registered domain to its content source: a filesystem root for
    /// a file domain, or the database for a virtual domain. Errors when the
    /// domain is not registered (the write path wants that), the layered lookup
    /// mirroring [`Engine::domain_entry`].
    fn content_source(&self, name: &str) -> Result<ContentSource> {
        let entry = self.domain_entry(name)?;
        Ok(self.source_of(&entry))
    }

    /// The content source implied by a domain entry: its filesystem root when it
    /// is a file domain with a path, else the database. A file domain with no
    /// path (an impossible config, but defended) falls back to the database.
    fn source_of(&self, entry: &DomainEntry) -> ContentSource {
        match entry.file_path() {
            Some(root) if !entry.is_virtual() => ContentSource::File { root },
            _ => ContentSource::Virtual,
        }
    }

    /// The content source to read a resolved engram through: a locally
    /// registered file domain's root, or the database. Never errors, so a
    /// database-only domain (virtual, or a file domain whose rows this instance
    /// sees but whose files it does not hold) still resolves for reading.
    fn read_source(&self, name: &str) -> ContentSource {
        match self.domain_entry(name) {
            Ok(entry) => self.source_of(&entry),
            Err(_) => ContentSource::Virtual,
        }
    }

    /// The registered entry for a domain: the startup snapshot, then the
    /// discovered overlay, then a fresh re-read of the config from disk (a
    /// `domain add` only ever edits the file, never this in-memory snapshot).
    fn domain_entry(&self, name: &str) -> Result<DomainEntry> {
        if let Some(entry) = self.config.read().unwrap().domains.get(name) {
            return Ok(entry.clone());
        }
        if let Some(entry) = self.discovered_domains.read().unwrap().get(name) {
            return Ok(entry.clone());
        }
        if let Some(entry) = self.refresh_domain(name) {
            return Ok(entry);
        }
        Err(EngineError::UnknownDomain {
            domain: name.to_string(),
            registered: self.known_domain_names(),
        })
    }

    /// Re-read the global config from disk looking for a domain registered
    /// after this engine started. A hit is cached in `discovered_domains` and,
    /// for a file domain on the daemon, reported over `watch_tx` so the watcher
    /// starts watching its root without a restart. A virtual domain has no root,
    /// so it is cached but never watched.
    fn refresh_domain(&self, name: &str) -> Option<DomainEntry> {
        // Re-read the same file this engine persists to (its `--config`
        // override, else the default global path) and layer the overlay back
        // on, so a post-startup re-read sees the same effective config a fresh
        // load would, environment overrides included.
        let path = match &self.config_path {
            Some(p) => p.clone(),
            None => crystalline_core::config::global_config_path().ok()?,
        };
        let file = overlay::load_file(&path).ok()?;
        let fresh = self.overlay.apply(&file);
        let entry = fresh.domains.get(name)?.clone();
        self.discovered_domains
            .write()
            .unwrap()
            .insert(name.to_string(), entry.clone());
        if let Some(tx) = &self.watch_tx
            && let Some(root) = entry.file_path()
            && !entry.is_virtual()
        {
            let _ = tx.send(WatchEvent::Add(name.to_string(), root));
        }
        Some(entry)
    }

    /// Every domain name this engine currently knows about: the startup
    /// snapshot plus anything discovered since.
    fn known_domain_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .config
            .read()
            .unwrap()
            .domains
            .keys()
            .cloned()
            .collect();
        names.extend(self.discovered_domains.read().unwrap().keys().cloned());
        names
    }

    /// Forget a domain removed by `domain remove` while this engine is live:
    /// drop it from the discovered overlay and, on the daemon, tell the
    /// watcher to stop watching its root. The index rows are never touched
    /// here; they are left for the next full reindex.
    pub fn forget_domain(&self, name: &str) {
        self.discovered_domains.write().unwrap().remove(name);
        if let Some(tx) = &self.watch_tx {
            let _ = tx.send(WatchEvent::Remove(name.to_string()));
        }
    }

    /// Resolve an identifier to a descriptor and the content source to read
    /// it through. The grammar is deliberately two-form: a bare permalink or
    /// title is domain-relative (within the passed `domain`, or across all
    /// domains when none is passed) and a `crystalline://` URL is the one
    /// absolute, cross-domain form - mirroring the `[[target]]` /
    /// `[[domain:target]]` wikilink pair. A scheme-less `domain/permalink`
    /// composite is not part of the grammar, since domain names are per-user
    /// configuration and must never ride inside an identifier. Resolution
    /// goes through the store, so a virtual domain (or any database-only
    /// domain) resolves without a filesystem root.
    async fn resolve(
        &self,
        identifier: &str,
        domain: Option<&str>,
    ) -> Result<(EngramDescriptor, ContentSource)> {
        if let Some(url) = CrystallineUrl::parse(identifier) {
            let store = self.store.lock().await;
            let d = store
                .find_engram(&url.domain, &url.permalink)
                .await?
                .ok_or_else(|| {
                    EngineError::NotFound(format!(
                        "no engram '{}' in domain '{}'",
                        url.permalink, url.domain
                    ))
                })?;
            drop(store);
            let source = self.read_source(&url.domain);
            return Ok((d, source));
        }

        if let Some(dom) = domain {
            let store = self.store.lock().await;
            let d = store.find_engram(dom, identifier).await?.ok_or_else(|| {
                // The one wrong shape agents keep producing is the domain
                // glued onto the permalink; the error teaches the fix so a
                // stumble recovers in one step.
                match identifier
                    .strip_prefix(dom)
                    .and_then(|r| r.strip_prefix('/'))
                    .filter(|r| !r.is_empty())
                {
                    Some(rest) => EngineError::NotFound(format!(
                        "no engram '{identifier}' in domain '{dom}'. An identifier without crystalline:// is domain-relative - retry with '{rest}'"
                    )),
                    None => EngineError::NotFound(format!(
                        "no engram '{identifier}' in domain '{dom}'"
                    )),
                }
            })?;
            drop(store);
            let source = self.read_source(dom);
            return Ok((d, source));
        }

        // Bare identifier across all domains.
        let store = self.store.lock().await;
        let mut matches = store.find_engram_any(identifier).await?;
        drop(store);
        match matches.len() {
            0 => Err(EngineError::NotFound(format!(
                "no engram matches '{identifier}'"
            ))),
            1 => {
                let d = matches.remove(0);
                let source = self.read_source(&d.domain);
                Ok((d, source))
            }
            _ => {
                let doms: Vec<String> = matches.iter().map(|d| d.domain.clone()).collect();
                Err(EngineError::Ambiguous(format!(
                    "'{identifier}' matches engrams in multiple domains: [{}]; pass a domain",
                    doms.join(", ")
                )))
            }
        }
    }

    /// Parse and index one markdown document into a domain, whatever its origin.
    /// This is the content-agnostic tail shared by every mutation: file writes
    /// pass the on-disk stamp and `None`; virtual writes pass a synthesized stamp
    /// and, for an edit, the CAS `expected_sha`. Everything after `parse_engram`
    /// (upsert, chunk, resolve refs) is identical, and it all runs in one
    /// transaction.
    ///
    /// `store_full` controls what lands in the `content` column. A file domain
    /// stores the body only (its source of truth is the file on disk, read back
    /// verbatim), matching the historical projection. A virtual domain has no
    /// file, so it stores the full markdown (frontmatter plus body): that is the
    /// exact document a read, edit, export or CAS checksum must round-trip, and
    /// `virtual_stamp` hashed the same full markdown.
    #[allow(clippy::too_many_arguments)]
    async fn index_markdown(
        &self,
        store: &dyn Store,
        domain_id: DomainId,
        rel: &str,
        text: &str,
        stamp: FileStamp,
        expected_sha: Option<&str>,
        store_full: bool,
    ) -> Result<EngramId> {
        let engram = parse_engram(text).map_err(|e| EngineError::Invalid(e.to_string()))?;
        let mut record = EngramRecord::from_engram(&engram, rel, stamp);
        if store_full {
            record.content = text.to_string();
        }

        store.begin().await?;
        let result = async {
            let id = store
                .upsert_engram_checked(domain_id, &record, expected_sha)
                .await?;
            let chunks = chunk_engram(
                &record.title,
                record.description.as_deref(),
                &record.content,
                &self.chunk_params,
            );
            store.replace_chunks(id, &chunks).await?;
            store.resolve_pending_relations(domain_id).await?;
            store.resolve_pending_links(domain_id).await?;
            Ok::<EngramId, EngineError>(id)
        }
        .await;
        match result {
            Ok(id) => {
                store.commit().await?;
                Ok(id)
            }
            Err(e) => {
                let _ = store.rollback().await;
                Err(e)
            }
        }
    }

    /// Upsert a single file into the store from disk, carrying the on-disk stamp
    /// so the watcher does not reprocess it. The file-origin wrapper over
    /// [`Engine::index_markdown`].
    async fn reindex_file(
        &self,
        store: &dyn Store,
        domain_id: DomainId,
        root: &Path,
        rel: &str,
    ) -> Result<EngramId> {
        let abs = join_rel(root, rel);
        let bytes = std::fs::read(&abs).map_err(|source| EngineError::Io {
            path: abs.display().to_string(),
            source,
        })?;
        let meta = std::fs::metadata(&abs).map_err(|source| EngineError::Io {
            path: abs.display().to_string(),
            source,
        })?;
        let stamp = FileStamp {
            mtime: mtime_secs(&meta),
            size: meta.len(),
            sha256: sha256_hex(&bytes),
        };
        let text = String::from_utf8(bytes)
            .map_err(|_| EngineError::Invalid(format!("{} is not valid UTF-8", abs.display())))?;
        // A file domain stores the body only; its source of truth is the file.
        self.index_markdown(store, domain_id, rel, &text, stamp, None, false)
            .await
    }

    /// Load an engram's parsed form through a content source: the file on disk
    /// for a file domain, or the stored `content` column for a virtual domain.
    /// Backs validation and schema inference across both kinds.
    async fn load_engram(
        &self,
        source: &ContentSource,
        domain_id: DomainId,
        rel: &str,
    ) -> Option<Engram> {
        match source {
            ContentSource::File { root } => read_engram_file(root, rel),
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let content = store.engram_content(domain_id, rel).await.ok().flatten()?;
                parse_engram(&content).ok()
            }
        }
    }

    /// Load an engram's full markdown through the read-path policy: the local
    /// file when a file domain holds it on disk, else the stored `content`
    /// column. This keeps files-are-truth for the host while serving virtual and
    /// non-host reads from the database.
    async fn load_content(
        &self,
        source: &ContentSource,
        desc: &EngramDescriptor,
    ) -> Result<String> {
        if let ContentSource::File { root } = source {
            let abs = join_rel(root, &desc.path);
            if let Ok(text) = std::fs::read_to_string(&abs) {
                return Ok(text);
            }
        }
        let store = self.store.lock().await;
        store
            .engram_content(desc.domain_id, &desc.path)
            .await?
            .ok_or_else(|| {
                EngineError::NotFound(format!(
                    "no content stored for '{}' in domain '{}'",
                    desc.permalink, desc.domain
                ))
            })
    }

    // --- write ---------------------------------------------------------------

    /// Create or overwrite an engram, then index it. A file domain writes the
    /// markdown file first (files-are-truth) then reindexes it from disk; a
    /// virtual domain builds the markdown in memory and indexes it straight into
    /// the database, touching no filesystem.
    pub async fn write_engram(&self, p: &WriteParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let source = self.content_source(&p.domain)?;
        let engram_type = p
            .engram_type
            .clone()
            .unwrap_or_else(|| "engram".to_string());
        let status = p.status.clone().unwrap_or_else(|| "current".to_string());
        let tags = p.tags.clone();

        let folder = p.folder.clone().unwrap_or_default();
        let title_slug = slugify(&p.title);
        if title_slug.is_empty() {
            return Err(EngineError::Invalid(
                "title does not slugify to a permalink; provide a title with letters or digits"
                    .into(),
            ));
        }
        let rel = if folder.trim_matches('/').is_empty() {
            format!("{title_slug}.md")
        } else {
            format!("{}/{title_slug}.md", folder.trim_matches('/'))
        };
        let permalink = slugify(&rel);

        // Enforce overwrite semantics against the existing permalink.
        {
            let store = self.store.lock().await;
            if let Some(existing) = store.find_engram(&p.domain, &permalink).await?
                && !p.overwrite
            {
                return Err(EngineError::Conflict(format!(
                    "permalink '{permalink}' already exists in domain '{}' (at {}); pass overwrite=true to replace",
                    p.domain, existing.path
                )));
            }
        }

        let today = chrono::Utc::now().date_naive();
        let now = now_offset();
        let markdown = build_markdown(
            &engram_type,
            &p.title,
            &permalink,
            &tags,
            &status,
            &today.format("%Y-%m-%d").to_string(),
            &now.to_rfc3339(),
            p.metadata.as_ref(),
            &p.content,
        )?;

        match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &rel);
                write_file(&abs, &markdown)?;
                let store = self.store.lock().await;
                let domain_id = store
                    .upsert_domain(&p.domain, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?;
                self.reindex_file(&*store, domain_id, root, &rel).await?;
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let domain_id = store
                    .upsert_domain(&p.domain, None, DomainKind::Virtual)
                    .await?;
                let stamp = virtual_stamp(&markdown);
                self.index_markdown(&*store, domain_id, &rel, &markdown, stamp, None, true)
                    .await?;
            }
        }

        // A virtual write may have landed or replaced this domain's MANIFEST
        // engram, the source of its routing bullets, so refresh the cache the
        // sync `routing_text` reads. The store locks above are all released.
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }

        Ok(json!({
            "domain": p.domain,
            "permalink": permalink,
            "path": rel,
            "title": p.title,
            "type": engram_type,
            "status": status,
            "action": if p.overwrite { "written" } else { "created" },
        }))
    }

    // --- read ----------------------------------------------------------------

    /// Read an engram's full markdown and resolved frontmatter. The content
    /// comes from the local file when a file domain holds it, else from the
    /// database (virtual domains, and non-host reads over a shared database). The
    /// returned `checksum` is the CAS token an `edit_engram` can pass back as
    /// `expected_checksum` to detect a change since this read.
    pub async fn read_engram(&self, p: &ReadParams) -> Result<Value> {
        let (desc, source) = self.resolve(&p.identifier, p.domain.as_deref()).await?;
        let content = self.load_content(&source, &desc).await?;
        let engram = parse_engram(&content).map_err(|e| EngineError::Invalid(e.to_string()))?;
        let checksum = sha256_hex(content.as_bytes());
        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "title": desc.title,
            "type": desc.engram_type,
            "status": desc.status,
            "path": desc.path,
            "url": format!("crystalline://{}/{}", desc.domain, desc.permalink),
            "content": content,
            "checksum": checksum,
            "frontmatter": engram.frontmatter,
            "observations": engram.observations,
            "relations": engram.relations,
        }))
    }

    // --- edit ----------------------------------------------------------------

    /// Apply a surgical edit to an engram, then reindex it. A file domain edits
    /// the file on disk and reindexes it; a virtual domain reads the current
    /// content from the database, applies the same edit and writes it back under
    /// a compare-and-swap guard so a stale edit is refused rather than silently
    /// clobbering a concurrent change (see `expected_checksum`).
    pub async fn edit_engram(&self, p: &EditParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let (desc, source) = self.resolve(&p.identifier, Some(&p.domain)).await?;

        match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &desc.path);
                let current = std::fs::read_to_string(&abs).map_err(|source| EngineError::Io {
                    path: abs.display().to_string(),
                    source,
                })?;
                let edited = self.apply_edit(&current, p, &desc.permalink)?;
                let edited = touch_timestamp(&edited, now_offset());
                let edited = Self::enforce_temporal(edited)?;
                write_file(&abs, &edited)?;
                let store = self.store.lock().await;
                self.reindex_file(&*store, desc.domain_id, root, &desc.path)
                    .await?;
            }
            ContentSource::Virtual => {
                let current = {
                    let store = self.store.lock().await;
                    store
                        .engram_content(desc.domain_id, &desc.path)
                        .await?
                        .ok_or_else(|| {
                            EngineError::NotFound(format!(
                                "no content stored for '{}' in domain '{}'",
                                desc.permalink, desc.domain
                            ))
                        })?
                };
                // The CAS token: the caller's expected_checksum when supplied
                // (guarding against a change since their read), else the sha of
                // what we just read (last-write-wins, matching the settled
                // semantics).
                let expected = p
                    .expected_checksum
                    .clone()
                    .unwrap_or_else(|| sha256_hex(current.as_bytes()));
                let edited = self.apply_edit(&current, p, &desc.permalink)?;
                let edited = touch_timestamp(&edited, now_offset());
                let edited = Self::enforce_temporal(edited)?;
                let stamp = virtual_stamp(&edited);
                let store = self.store.lock().await;
                self.index_markdown(
                    &*store,
                    desc.domain_id,
                    &desc.path,
                    &edited,
                    stamp,
                    Some(&expected),
                    true,
                )
                .await?;
            }
        }

        // A virtual edit may have rewritten this domain's MANIFEST engram, so
        // refresh the routing cache. The store locks above are all released.
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }

        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "path": desc.path,
            "operation": p.operation,
        }))
    }

    /// Apply one edit operation to an engram's markdown, returning the edited
    /// text. Content-agnostic: the same logic serves file and virtual edits.
    fn apply_edit(&self, source: &str, p: &EditParams, permalink: &str) -> Result<String> {
        Ok(match p.operation.as_str() {
            "append" => append_body(source, &p.content),
            "prepend" => prepend_body(source, &p.content),
            "find_replace" => {
                let find = p.find_text.as_deref().ok_or_else(|| {
                    EngineError::Invalid("find_replace requires find_text".into())
                })?;
                if find.is_empty() {
                    return Err(EngineError::Invalid("find_text must not be empty".into()));
                }
                let count = source.matches(find).count();
                if count == 0 {
                    return Err(EngineError::NotFound(format!(
                        "find_text '{find}' not found in '{permalink}'"
                    )));
                }
                if let Some(expected) = p.expected_replacements
                    && expected != count
                {
                    return Err(EngineError::Invalid(format!(
                        "expected {expected} replacements of '{find}' but found {count}"
                    )));
                }
                source.replace(find, &p.content)
            }
            "replace_section" => {
                let section = self.require_section(p)?;
                replace_section(source, section, &p.content, p.include_subsections)
                    .map_err(section_err)?
            }
            "insert_before_section" => {
                let section = self.require_section(p)?;
                insert_before_section(source, section, &p.content).map_err(section_err)?
            }
            "insert_after_section" => {
                let section = self.require_section(p)?;
                insert_after_section(source, section, &p.content).map_err(section_err)?
            }
            other => {
                return Err(EngineError::Invalid(format!(
                    "unknown edit operation '{other}'; expected append, prepend, find_replace, replace_section, insert_before_section or insert_after_section"
                )));
            }
        })
    }

    fn require_section<'a>(&self, p: &'a EditParams) -> Result<&'a str> {
        p.section
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                EngineError::Invalid(format!("operation '{}' requires a section", p.operation))
            })
    }

    /// Enforce the temporal write contract on post-edit markdown: reject a date
    /// field left malformed and surgically drop sentinel or null bounds,
    /// matching write_engram and import. Post-edit rather than per-argument
    /// because find_replace can rewrite frontmatter text directly. A parse
    /// failure passes through unchanged; indexing reports it.
    fn enforce_temporal(edited: String) -> Result<String> {
        let Ok(engram) = parse_engram(&edited) else {
            return Ok(edited);
        };
        let mut fm = engram.frontmatter;
        let dropped = crystalline_core::temporal::normalize_temporal_fields(&mut fm)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        let mut out = edited;
        for field in dropped {
            out = remove_frontmatter_field(&out, field);
        }
        Ok(out)
    }

    // --- move ----------------------------------------------------------------

    /// Move an engram to a new path or domain, rewriting inbound bare links on a
    /// cross-domain move. Source and destination may each be a file or virtual
    /// domain, so a move carries content between the two truths: a same-domain
    /// move is a rename (no reparse), a cross-domain move reads the source
    /// content and re-indexes it into the destination's source.
    pub async fn move_engram(&self, p: &MoveParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let (src, src_source) = self.resolve(&p.identifier, Some(&p.domain)).await?;
        let dest_domain = p
            .destination_domain
            .clone()
            .unwrap_or_else(|| p.domain.clone());
        let dest_source = self.content_source(&dest_domain)?;
        let dest_rel = normalize_md(&p.destination);
        if dest_rel.is_empty() {
            return Err(EngineError::Invalid("destination path is empty".into()));
        }
        let cross = dest_domain != p.domain;

        // Destination collision check, on disk or in the database.
        self.ensure_dest_free(&dest_source, &dest_domain, &dest_rel)
            .await?;

        // Gather inbound refs before the move while `to_id` still points at src.
        let inbound = if cross && p.update_links.unwrap_or(true) {
            let store = self.store.lock().await;
            store
                .inbound_refs(src.id, src.domain_id, &src.permalink, &src.title)
                .await?
        } else {
            Vec::new()
        };

        if cross {
            // Read the source content (file when present, else database), index
            // it into the destination source, then remove the source.
            let content = self.load_content(&src_source, &src).await?;
            match &dest_source {
                ContentSource::File { root } => {
                    let dest_abs = join_rel(root, &dest_rel);
                    write_file(&dest_abs, &content)?;
                    let store = self.store.lock().await;
                    let dest_id = store
                        .upsert_domain(
                            &dest_domain,
                            Some(&root.to_string_lossy()),
                            DomainKind::File,
                        )
                        .await?;
                    self.reindex_file(&*store, dest_id, root, &dest_rel).await?;
                }
                ContentSource::Virtual => {
                    let store = self.store.lock().await;
                    let dest_id = store
                        .upsert_domain(&dest_domain, None, DomainKind::Virtual)
                        .await?;
                    let stamp = virtual_stamp(&content);
                    self.index_markdown(&*store, dest_id, &dest_rel, &content, stamp, None, true)
                        .await?;
                }
            }
            if let ContentSource::File { root } = &src_source {
                let src_abs = join_rel(root, &src.path);
                if let Err(e) = std::fs::remove_file(&src_abs) {
                    tracing::warn!(
                        "could not remove moved source {}: {e}; leaving it in place",
                        src_abs.display()
                    );
                }
            }
            let store = self.store.lock().await;
            store.delete_engram(src.domain_id, &src.path).await?;
        } else {
            // Same-domain rename: move the file when file-backed, then rename the
            // row in place with no reparse (the permalink follows only when it
            // was path-derived).
            if let ContentSource::File { root } = &src_source {
                let src_abs = join_rel(root, &src.path);
                let dest_abs = join_rel(root, &dest_rel);
                let content = std::fs::read(&src_abs).map_err(|source| EngineError::Io {
                    path: src_abs.display().to_string(),
                    source,
                })?;
                write_bytes(&dest_abs, &content)?;
                std::fs::remove_file(&src_abs).map_err(|source| EngineError::Io {
                    path: src_abs.display().to_string(),
                    source,
                })?;
            }
            let store = self.store.lock().await;
            store
                .rename_engram(src.domain_id, &src.path, &dest_rel)
                .await?;
        }

        // Rewrite inbound bare links from other domains to the prefixed form.
        let mut rewritten = 0usize;
        for r in inbound {
            if r.src_domain == dest_domain || r.to_target.contains(':') {
                continue;
            }
            let needle = format!("[[{}]]", r.to_target);
            let prefixed = format!("[[{dest_domain}:{}]]", r.to_target);
            match self.read_source(&r.src_domain) {
                ContentSource::File { root } => {
                    let linker_abs = join_rel(&root, &r.src_path);
                    let Ok(text) = std::fs::read_to_string(&linker_abs) else {
                        continue;
                    };
                    if !text.contains(&needle) {
                        continue;
                    }
                    let replaced = touch_timestamp(&text.replace(&needle, &prefixed), now_offset());
                    write_file(&linker_abs, &replaced)?;
                    let store = self.store.lock().await;
                    self.reindex_file(&*store, r.src_domain_id, &root, &r.src_path)
                        .await?;
                    rewritten += 1;
                }
                ContentSource::Virtual => {
                    let current = {
                        let store = self.store.lock().await;
                        store.engram_content(r.src_domain_id, &r.src_path).await?
                    };
                    let Some(text) = current else { continue };
                    if !text.contains(&needle) {
                        continue;
                    }
                    let replaced = touch_timestamp(&text.replace(&needle, &prefixed), now_offset());
                    let stamp = virtual_stamp(&replaced);
                    let store = self.store.lock().await;
                    self.index_markdown(
                        &*store,
                        r.src_domain_id,
                        &r.src_path,
                        &replaced,
                        stamp,
                        None,
                        true,
                    )
                    .await?;
                    rewritten += 1;
                }
            }
        }

        // When either end of the move is a virtual domain, a MANIFEST engram
        // may have moved into or out of it, so refresh the routing cache. Every
        // store lock taken above is released by here.
        if matches!(src_source, ContentSource::Virtual)
            || matches!(dest_source, ContentSource::Virtual)
        {
            self.refresh_routing_cache().await;
        }

        Ok(json!({
            "from": { "domain": p.domain, "permalink": src.permalink, "path": src.path },
            "to": { "domain": dest_domain, "path": dest_rel },
            "cross_domain": cross,
            "links_rewritten": rewritten,
        }))
    }

    /// Refuse a move whose destination path is already taken, checking disk for
    /// a file domain and the database for a virtual one.
    async fn ensure_dest_free(
        &self,
        dest_source: &ContentSource,
        dest_domain: &str,
        dest_rel: &str,
    ) -> Result<()> {
        let taken = match dest_source {
            ContentSource::File { root } => join_rel(root, dest_rel).exists(),
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let dest_id = store
                    .upsert_domain(dest_domain, None, DomainKind::Virtual)
                    .await?;
                store.engram_content(dest_id, dest_rel).await?.is_some()
            }
        };
        if taken {
            return Err(EngineError::Conflict(format!(
                "destination '{dest_rel}' already exists in domain '{dest_domain}'"
            )));
        }
        Ok(())
    }

    // --- delete --------------------------------------------------------------

    /// Delete an engram and its index rows. A file domain also removes the file
    /// on disk; a virtual domain only drops the database rows.
    pub async fn delete_engram(&self, p: &DeleteParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let (desc, source) = self.resolve(&p.identifier, Some(&p.domain)).await?;
        if let ContentSource::File { root } = &source {
            let abs = join_rel(root, &desc.path);
            std::fs::remove_file(&abs).map_err(|source| EngineError::Io {
                path: abs.display().to_string(),
                source,
            })?;
        }
        let store = self.store.lock().await;
        store.delete_engram(desc.domain_id, &desc.path).await?;
        drop(store);

        // Deleting a virtual domain's MANIFEST engram empties its routing
        // bullets, so refresh the cache once the store lock is released.
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }

        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "path": desc.path,
            "deleted": true,
        }))
    }

    // --- search --------------------------------------------------------------

    /// Search across domains, embedding the query when the mode needs it.
    pub async fn search_engrams(&self, p: &SearchParams) -> Result<Value> {
        let requested = parse_mode(p.search_type.as_deref())?;
        let text = p.query.clone().filter(|s| !s.trim().is_empty());
        let mut query = SearchQuery {
            text: text.clone(),
            domains: Some(p.domains.clone()).filter(|d| !d.is_empty()),
            engram_type: p.engram_type.clone(),
            status: p.status.clone(),
            tags: Some(p.tags.clone()).filter(|t| !t.is_empty()),
            after: p.after.clone(),
            min_similarity: p.min_similarity,
            limit: p.limit.unwrap_or(10).max(1),
            page: p.page.unwrap_or(1).max(1),
            ..SearchQuery::default()
        };
        if let Some(mf) = &p.metadata_filters {
            query.metadata_filters =
                parse_metadata_filters(mf).map_err(|e| EngineError::Invalid(e.to_string()))?;
        }

        // Phase the store lock so it is never held across the provider embed
        // call, the same discipline as `embed_pending`: fetch the provider once,
        // hold the store lock only to resolve the effective mode (which reads
        // embedding coverage), then drop it before embedding the query and
        // relock for the search. The coverage snapshot can go stale between the
        // mode decision and the search, an accepted race of the same class as
        // already exists across two separate search calls.
        let provider = self.provider();
        let effective = {
            let store = self.store.lock().await;
            self.effective_mode(&*store, requested, text.is_some(), provider.is_some())
                .await?
        };
        query.mode = effective;
        if matches!(effective, SearchMode::Semantic | SearchMode::Hybrid)
            && let Some(provider) = &provider
        {
            let q = text.clone().unwrap_or_default();
            let vecs = provider
                .embed_queries(&[q])
                .await
                .map_err(|e| EngineError::Internal(e.to_string()))?;
            query.query_embedding = vecs.into_iter().next();
            query.active_model = Some(self.model_id.clone());
        }

        let store = self.store.lock().await;
        let page = store.search(&query).await?;
        Ok(json!({
            "mode": mode_str(effective),
            "total": page.total,
            "page": page.page,
            "limit": page.limit,
            "count": page.items.len(),
            "hits": serde_json::to_value(&page.items).unwrap_or(Value::Null),
        }))
    }

    async fn effective_mode(
        &self,
        store: &dyn Store,
        requested: SearchMode,
        has_text: bool,
        has_provider: bool,
    ) -> Result<SearchMode> {
        if !matches!(requested, SearchMode::Semantic | SearchMode::Hybrid) {
            return Ok(requested);
        }
        if !has_text || !has_provider {
            return Ok(SearchMode::Text);
        }
        let coverage = store.embedding_coverage().await?;
        if coverage.has_active_embeddings(&self.model_id) {
            Ok(requested)
        } else {
            Ok(SearchMode::Text)
        }
    }

    // --- context -------------------------------------------------------------

    /// Traverse the graph around a `crystalline://` anchor.
    pub async fn build_context(&self, p: &ContextParams) -> Result<Value> {
        let url = CrystallineUrl::parse(&p.anchor).ok_or_else(|| {
            EngineError::Invalid(format!("anchor '{}' is not a crystalline:// URL", p.anchor))
        })?;
        let depth = p.depth.unwrap_or(1).clamp(1, 3);
        let max_related = p.max_related.unwrap_or(10);
        let domain_filter = Some(p.domains.clone()).filter(|d| !d.is_empty());

        let store = self.store.lock().await;
        let seeds: Vec<EngramDescriptor> = if url.glob {
            store
                .list_engrams(&url.domain, None, None)
                .await?
                .into_iter()
                .filter(|d| url.matches(&d.domain, &d.permalink))
                .collect()
        } else {
            match store.find_engram(&url.domain, &url.permalink).await? {
                Some(d) => vec![d],
                None => {
                    return Err(EngineError::NotFound(format!(
                        "no engram '{}' in domain '{}'",
                        url.permalink, url.domain
                    )));
                }
            }
        };
        if seeds.is_empty() {
            return Err(EngineError::NotFound(format!(
                "anchor '{}' matched no engrams",
                p.anchor
            )));
        }
        let seed_ids: HashSet<i64> = seeds.iter().map(|d| d.id.0).collect();
        let ids: Vec<EngramId> = seeds.iter().map(|d| d.id).collect();
        let slice = store.neighbors(&ids, depth).await?;

        // Keep every seed, cap related nodes and apply the optional domain filter.
        let mut kept: HashSet<i64> = HashSet::new();
        let mut nodes = Vec::new();
        let mut related = 0usize;
        for node in &slice.nodes {
            if let Some(filter) = &domain_filter
                && !filter.contains(&node.domain)
            {
                continue;
            }
            let is_seed = seed_ids.contains(&node.id.0);
            if !is_seed {
                if related >= max_related {
                    continue;
                }
                related += 1;
            }
            kept.insert(node.id.0);
            nodes.push(json!({
                "id": node.id.0,
                "domain": node.domain,
                "permalink": node.permalink,
                "title": node.title,
                "type": node.engram_type,
                "seed": is_seed,
            }));
        }
        let edges: Vec<Value> = slice
            .edges
            .iter()
            .filter(|e| kept.contains(&e.from.0) && kept.contains(&e.to.0))
            .map(|e| {
                json!({
                    "from": e.from.0,
                    "to": e.to.0,
                    "rel_type": e.rel_type,
                    "kind": match e.kind {
                        crystalline_index::EdgeKind::Relation => "relation",
                        crystalline_index::EdgeKind::Link => "link",
                    },
                })
            })
            .collect();

        Ok(json!({
            "anchor": url.to_url(),
            "depth": depth,
            "timeframe": p.timeframe,
            "nodes": nodes,
            "edges": edges,
        }))
    }

    // --- recent --------------------------------------------------------------

    /// Recent engrams within a timeframe.
    pub async fn recent_activity(&self, p: &RecentParams) -> Result<Value> {
        let timeframe = p.timeframe.clone().unwrap_or_else(|| "7d".to_string());
        let filter = RecentFilter {
            domains: Some(p.domains.clone()).filter(|d| !d.is_empty()),
            after: timeframe_cutoff(&timeframe),
            engram_types: Some(p.types.clone()).filter(|t| !t.is_empty()),
            limit: 50,
        };
        let store = self.store.lock().await;
        let items = store.recent(&filter).await?;
        Ok(json!({
            "timeframe": timeframe,
            "count": items.len(),
            "engrams": serde_json::to_value(&items).unwrap_or(Value::Null),
        }))
    }

    // --- list domains --------------------------------------------------------

    /// List registered domains with counts and optional routing bullets. A file
    /// domain reports its path and reads routing bullets from its `MANIFEST.md`
    /// on disk; a virtual domain reports a null path, its kind and reads routing
    /// bullets from its MANIFEST engram in the database.
    pub async fn list_domains(&self, p: &ListDomainsParams) -> Result<Value> {
        let store = self.store.lock().await;
        let stats = store.domain_stats().await.unwrap_or_default();
        drop(store);

        let mut out = Vec::new();
        // Cloned out from behind the lock before any `.await` below, matching
        // the `hosted`/`discovered_domains` convention elsewhere in this file.
        let domains = self.config.read().unwrap().domains.clone();
        for (name, entry) in &domains {
            let source = self.source_of(entry);
            let s = stats.iter().find(|d| &d.name == name);
            let mut obj = json!({
                "name": name,
                "kind": if entry.is_virtual() { "virtual" } else { "file" },
                "path": entry.file_path().map(|r| r.display().to_string()),
                "engrams": s.map(|d| d.engrams),
                "observations": s.map(|d| d.observations),
                "relations": s.map(|d| d.relations),
                "last_sync": s.and_then(|d| d.last_sync.clone()),
            });
            // In a shared database a file domain names its current host so an
            // agent and an operator see who syncs what; `hosted_here` is true when
            // this instance holds the lock.
            if let Some(host) = s.and_then(|d| d.host_instance_id.clone()) {
                let hosted_here = !self.instance_id.is_empty() && host == self.instance_id;
                obj["host"] = json!({
                    "instance_id": host,
                    "heartbeat_at": s.and_then(|d| d.host_heartbeat_at.clone()),
                    "hosted_here": hosted_here,
                });
            }
            if p.include_routing {
                let bullets = match &source {
                    ContentSource::File { root } => routing_bullets(root),
                    ContentSource::Virtual => self.virtual_routing_bullets_for(name).await,
                };
                obj["when_to_use"] = json!(bullets);
            }
            out.push(obj);
        }
        Ok(json!({ "domains": out }))
    }

    /// Routing bullets for one virtual domain, read from its `MANIFEST.md`
    /// engram in the database. Empty when there is no MANIFEST engram yet.
    async fn virtual_routing_bullets_for(&self, name: &str) -> Vec<String> {
        let content = {
            let store = self.store.lock().await;
            match store.find_engram(name, "manifest").await.ok().flatten() {
                Some(d) => store
                    .engram_content(d.domain_id, &d.path)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        };
        let Some(source) = content else {
            return Vec::new();
        };
        let Ok(engram) = parse_engram(&source) else {
            return Vec::new();
        };
        Manifest::from_engram(&engram, &source)
            .routing_bullets()
            .to_vec()
    }

    /// Routing bullets for every virtual domain, keyed by domain name. Supplied
    /// to `crystalline_core::generate_prompt` (which never touches a database),
    /// served over the `routing_bullets` ctl request so `prompt system` stays
    /// inside its latency budget for virtual domains too and snapshotted by
    /// [`Engine::refresh_routing_cache`] for the MCP server instructions.
    pub async fn virtual_routing_bullets(&self) -> BTreeMap<String, Vec<String>> {
        let mut out = BTreeMap::new();
        let domains = self.config.read().unwrap().domains.clone();
        for (name, entry) in &domains {
            if entry.is_virtual() {
                out.insert(name.clone(), self.virtual_routing_bullets_for(name).await);
            }
        }
        out
    }

    // --- routing instructions ------------------------------------------------

    /// Recompute the cached virtual-domain routing bullets from the database.
    /// The async companion to [`Engine::routing_text`]: a virtual domain's
    /// bullets live in its MANIFEST engram in the store, so they need an await
    /// to read, but `routing_text` is sync and must not block. The daemon and
    /// the embedded stdio stack call this off the async path (at each MCP
    /// connection's initialize, and after every write that touches a virtual
    /// source) so the sync render only ever reads the cache under the lock.
    pub async fn refresh_routing_cache(&self) {
        let bullets = self.virtual_routing_bullets().await;
        *self.routing_virtual.write().unwrap() = bullets;
    }

    /// The routing instructions a fresh MCP connection is handed at initialize:
    /// the "CRYSTALLINE KNOWLEDGE ROUTING" block over every registered domain.
    /// Synchronous, because rmcp's `get_info` is sync and runs once per
    /// connection; it never blocks on async work, so the virtual bullets come
    /// from the [`Engine::routing_virtual`] cache alone (refreshed off the async
    /// path by [`Engine::refresh_routing_cache`]) and the file bullets are read
    /// straight from each domain's `MANIFEST.md` on disk.
    ///
    /// There is no workspace over MCP: a server serves one index to every
    /// connecting agent, so `prompt.rules` path-glob filters and repo-local
    /// `preferred_domains` never apply here (both need a workspace path). The
    /// effective config is composed live: with a `--config` override this
    /// re-reads that file and re-applies the environment overlay (mirroring
    /// [`Engine::refresh_domain`]) so a domain registered after startup shows up
    /// on the next connection; without one (tests and standalone) it takes the
    /// in-memory config plus any domain discovered since, and never touches the
    /// default global config path. Staleness is bounded to one connection: the
    /// block is an initialize-time snapshot, and the virtual bullets are only as
    /// fresh as the last cache refresh.
    ///
    /// This re-read looks redundant with `self.config` (in-memory), and mostly
    /// is: `configure`'s `Set`/`Unset` (Engine::configure), `domain_add`'s file
    /// and virtual arms and `origin_add` all persist to disk and then write
    /// `self.config` in the same call, under `file_config`-then-`config` lock
    /// order, before returning - a concurrent reader sees the new value the
    /// instant the write lock releases, no re-read needed. `domain remove`
    /// (`cmd::domain_remove` in the CLI crate) is the one path that does not:
    /// it is a free function with no `Engine` reference at all, so it mutates
    /// the config file directly regardless of whether a daemon is live; the
    /// only in-process signal a running daemon gets is the `forget_domain` ctl
    /// call, and `Engine::forget_domain` only drops the name from
    /// `discovered_domains` and tells the watcher to stop - it never touches
    /// `self.config`. Serving from `self.config` alone would therefore keep a
    /// removed domain in every connection's routing block until the daemon
    /// restarts, not just for one racing connection - a real regression, not
    /// the already-accepted bounded staleness this comment describes for the
    /// `None` branch below. So the re-read stays for as long as `domain
    /// remove` is the one mutation path that does not refresh `self.config`.
    pub fn routing_text(&self) -> String {
        // (1) The effective config, composed the same way a fresh load would
        // see it. With a config path this is a fresh file read plus the overlay;
        // a read error falls back to the in-memory effective config.
        let global = match &self.config_path {
            Some(path) => match overlay::load_file(path) {
                Ok(file) => self.overlay.apply(&file),
                Err(_) => self.config(),
            },
            None => {
                // No config path to re-read (tests, standalone): start from the
                // in-memory config and append any domain discovered since
                // startup that it does not already carry, sorted for
                // determinism. Never touch the default global config path.
                let mut global = self.config();
                let discovered = self.discovered_domains.read().unwrap().clone();
                let mut extra: Vec<(String, DomainEntry)> = discovered
                    .into_iter()
                    .filter(|(name, _)| !global.domains.contains_key(name))
                    .collect();
                extra.sort_by(|a, b| a.0.cmp(&b.0));
                for (name, entry) in extra {
                    global.domains.insert(name, entry);
                }
                global
            }
        };

        // (2) Generate over every registered domain from the cached virtual map,
        // (3) force the engine's effective read-only mode, then (4) render.
        let virtual_bullets = self.routing_virtual.read().unwrap().clone();
        let mut output = crystalline_core::generate_prompt_unscoped(&global, &virtual_bullets);
        output.read_only = self.read_only();
        crystalline_core::render_instructions(&output)
    }

    // --- browse --------------------------------------------------------------

    /// Browse a domain's engrams under a folder path. Works for any registered
    /// domain, file or virtual, since it lists rows from the store rather than
    /// walking a filesystem.
    pub async fn browse_domain(&self, p: &BrowseParams) -> Result<Value> {
        // A domain-exists check, not a filesystem-root requirement, so a virtual
        // domain browses.
        self.domain_entry(&p.domain)?;
        let raw = p.path.clone().unwrap_or_else(|| "/".to_string());
        let prefix = raw.trim_start_matches("./").trim_matches('/').to_string();
        let depth = p.depth.unwrap_or(1).max(1);
        let matcher = match &p.glob {
            Some(g) => Some(
                globset::Glob::new(g)
                    .map_err(|e| EngineError::Invalid(format!("invalid glob '{g}': {e}")))?
                    .compile_matcher(),
            ),
            None => None,
        };

        let prefix_pat = if prefix.is_empty() {
            None
        } else {
            Some(format!("{prefix}/"))
        };
        let store = self.store.lock().await;
        let all = store
            .list_engrams(&p.domain, prefix_pat.as_deref(), None)
            .await?;
        drop(store);

        let mut entries = Vec::new();
        let mut folders: HashSet<String> = HashSet::new();
        for d in &all {
            let rel: &str = if prefix.is_empty() {
                d.path.as_str()
            } else {
                d.path
                    .strip_prefix(&format!("{prefix}/"))
                    .unwrap_or(&d.path)
            };
            if let Some(m) = &matcher
                && !m.is_match(&d.path)
            {
                continue;
            }
            let segments: Vec<&str> = rel.split('/').collect();
            if segments.len() > 1 {
                folders.insert(segments[0].to_string());
            }
            if segments.len() <= depth {
                entries.push(json!({
                    "permalink": d.permalink,
                    "title": d.title,
                    "type": d.engram_type,
                    "path": d.path,
                }));
            }
        }
        let mut folders: Vec<String> = folders.into_iter().collect();
        folders.sort();

        Ok(json!({
            "domain": p.domain,
            "path": raw,
            "folders": folders,
            "engrams": entries,
        }))
    }

    // --- validate ------------------------------------------------------------

    /// Validate a domain's engrams against its schema engrams. Engram content is
    /// loaded from disk for a file domain and from the database for a virtual
    /// domain, so validation covers both kinds.
    pub async fn validate_engrams(&self, p: &ValidateParams) -> Result<Value> {
        let source = self.content_source(&p.domain)?;
        let store = self.store.lock().await;
        let schema_descs = store.list_engrams(&p.domain, None, Some("schema")).await?;
        let targets = if let Some(id) = &p.identifier {
            match store.find_engram(&p.domain, id).await? {
                Some(d) => vec![d],
                None => {
                    return Err(EngineError::NotFound(format!(
                        "no engram '{id}' in domain '{}'",
                        p.domain
                    )));
                }
            }
        } else {
            store
                .list_engrams(&p.domain, None, p.engram_type.as_deref())
                .await?
        };
        drop(store);

        let mut schemas: Vec<Schema> = Vec::new();
        for d in &schema_descs {
            if let Some(engram) = self.load_engram(&source, d.domain_id, &d.path).await
                && let Some(schema) = Schema::from_engram(&engram)
            {
                schemas.push(schema);
            }
        }

        let mut issues = Vec::new();
        let mut checked = 0usize;
        for d in &targets {
            let Some(engram) = self.load_engram(&source, d.domain_id, &d.path).await else {
                continue;
            };
            checked += 1;
            if let Some(schema) = schema::select_schema(&engram, &schemas) {
                for issue in schema::validate(&engram, &schema) {
                    issues.push(json!({
                        "permalink": d.permalink,
                        "path": d.path,
                        "severity": issue.severity,
                        "kind": issue.kind,
                        "field": issue.field,
                        "message": issue.message,
                        "line": issue.line,
                    }));
                }
            }
            for issue in crystalline_core::verify::check_temporal(Path::new(&d.path), &engram) {
                let message = match issue.fix {
                    Some(fix) => format!("{} (fix: {fix})", issue.message),
                    None => issue.message,
                };
                issues.push(json!({
                    "permalink": d.permalink,
                    "path": d.path,
                    "severity": issue.severity,
                    "kind": issue.rule,
                    "field": Value::Null,
                    "message": message,
                    "line": issue.line,
                }));
            }
        }

        Ok(json!({
            "domain": p.domain,
            "checked": checked,
            "schemas": schemas.len(),
            "issue_count": issues.len(),
            "issues": issues,
        }))
    }

    // --- infer schema --------------------------------------------------------

    /// Infer a Picoschema from a domain's engrams of a type. Engram content is
    /// loaded from disk for a file domain and from the database for a virtual
    /// domain.
    pub async fn infer_schema(&self, p: &InferParams) -> Result<Value> {
        let source = self.content_source(&p.domain)?;
        let store = self.store.lock().await;
        let descs = store
            .list_engrams(&p.domain, None, Some(&p.engram_type))
            .await?;
        drop(store);

        let mut engrams = Vec::new();
        for d in &descs {
            if let Some(engram) = self.load_engram(&source, d.domain_id, &d.path).await {
                engrams.push(engram);
            }
        }
        let threshold = p.threshold.unwrap_or(0.25);
        let schema = schema::infer(&engrams, threshold);
        Ok(json!({
            "domain": p.domain,
            "type": p.engram_type,
            "count": engrams.len(),
            "threshold": threshold,
            "schema": schema,
        }))
    }

    // --- domain import / export / scaffold -----------------------------------

    /// Scaffold a MANIFEST engram into a virtual domain from prebuilt markdown,
    /// unless one already exists. A no-op that reports `created: false` when the
    /// domain already has a `MANIFEST.md`. Refuses on a file domain (its MANIFEST
    /// belongs on disk via `domain init`).
    pub async fn scaffold_virtual_manifest(&self, domain: &str, markdown: &str) -> Result<Value> {
        if let ContentSource::File { .. } = self.content_source(domain)? {
            return Err(EngineError::Invalid(format!(
                "domain '{domain}' is a file domain; scaffold its MANIFEST on disk with `crystalline domain init`"
            )));
        }
        let store = self.store.lock().await;
        let domain_id = store
            .upsert_domain(domain, None, DomainKind::Virtual)
            .await?;
        let existing = store.engram_content(domain_id, "MANIFEST.md").await?;
        drop(store);
        if existing.is_some() {
            return Ok(json!({ "domain": domain, "manifest": "MANIFEST.md", "created": false }));
        }
        let stamp = virtual_stamp(markdown);
        let store = self.store.lock().await;
        self.index_markdown(
            &*store,
            domain_id,
            "MANIFEST.md",
            markdown,
            stamp,
            None,
            true,
        )
        .await?;
        drop(store);

        // The MANIFEST engram just landed; its Scope and When to Use bullets are
        // exactly what the routing block reads for this virtual domain, so
        // refresh the cache the sync `routing_text` serves.
        self.refresh_routing_cache().await;

        Ok(json!({ "domain": domain, "manifest": "MANIFEST.md", "created": true }))
    }

    /// Import already-well-formed engram `.md` files from `src` into a virtual
    /// domain verbatim. Refuses a file target (that would desync the DB from its
    /// files). Collisions on an existing path or permalink are skipped unless
    /// `overwrite`; `dry_run` reports without writing.
    pub async fn import_domain(
        &self,
        domain: &str,
        src: &Path,
        overwrite: bool,
        dry_run: bool,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if let ContentSource::File { .. } = self.content_source(domain)? {
            return Err(EngineError::Invalid(format!(
                "domain import loads into a virtual domain; '{domain}' is a file domain. \
                 Use `crystalline import` then `crystalline sync` for a file domain."
            )));
        }
        if !src.is_dir() {
            return Err(EngineError::Invalid(format!(
                "import source '{}' is not a directory",
                src.display()
            )));
        }

        let files = walk_markdown(src);
        let store = self.store.lock().await;
        let domain_id = store
            .upsert_domain(domain, None, DomainKind::Virtual)
            .await?;
        let existing = store.all_engram_contents(domain_id).await?;
        drop(store);
        let existing_paths: HashSet<String> = existing.iter().map(|e| e.path.clone()).collect();
        let existing_perms: HashSet<String> =
            existing.iter().map(|e| e.permalink.clone()).collect();

        let mut written = 0usize;
        let mut skipped = 0usize;
        let mut collisions: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        let mut changes: Vec<Value> = Vec::new();

        for (rel, abs) in files {
            let text = match std::fs::read_to_string(&abs) {
                Ok(t) => t,
                Err(e) => {
                    warnings.push(format!("{rel}: could not read: {e}"));
                    continue;
                }
            };
            let engram = match parse_engram(&text) {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(format!("{rel}: could not parse: {e}"));
                    continue;
                }
            };
            let record = EngramRecord::from_engram(&engram, &rel, virtual_stamp(&text));
            let collides = (existing_paths.contains(&rel)
                || existing_perms.contains(&record.permalink))
                && !overwrite;
            if collides {
                collisions.push(rel.clone());
                skipped += 1;
                continue;
            }
            if dry_run {
                changes.push(json!({ "path": rel, "permalink": record.permalink }));
                written += 1;
                continue;
            }
            let stamp = virtual_stamp(&text);
            let store = self.store.lock().await;
            match self
                .index_markdown(&*store, domain_id, &rel, &text, stamp, None, true)
                .await
            {
                Ok(_) => {
                    changes.push(json!({ "path": rel, "permalink": record.permalink }));
                    written += 1;
                }
                Err(e) => {
                    warnings.push(format!("{rel}: {e}"));
                    skipped += 1;
                }
            }
        }

        Ok(json!({
            "domain": domain,
            "dry_run": dry_run,
            "files_written": written,
            "files_skipped": skipped,
            "collisions": collisions,
            "warnings": warnings,
            "files": changes,
        }))
    }

    /// Export every engram of a domain (file or virtual) from the database to
    /// `dest` as a normal filesystem engram folder. Refuses to write into a
    /// non-empty directory unless `force`; `dry_run` reports without writing.
    pub async fn export_domain(
        &self,
        domain: &str,
        dest: &Path,
        force: bool,
        dry_run: bool,
    ) -> Result<Value> {
        let entry = self.domain_entry(domain)?;
        let store = self.store.lock().await;
        let domain_id = match self.source_of(&entry) {
            ContentSource::File { root } => {
                store
                    .upsert_domain(domain, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?
            }
            ContentSource::Virtual => {
                store
                    .upsert_domain(domain, None, DomainKind::Virtual)
                    .await?
            }
        };
        let all = store.all_engram_contents(domain_id).await?;
        drop(store);

        if !dry_run && dir_is_nonempty(dest) && !force {
            return Err(EngineError::Conflict(format!(
                "destination '{}' is not empty; pass force to overwrite",
                dest.display()
            )));
        }

        let mut written = 0usize;
        let mut files: Vec<Value> = Vec::new();
        for e in &all {
            files.push(json!({ "path": e.path, "permalink": e.permalink }));
            if dry_run {
                continue;
            }
            let abs = join_rel(dest, &e.path);
            write_file(&abs, &e.content)?;
            written += 1;
        }

        Ok(json!({
            "domain": domain,
            "dest": dest.display().to_string(),
            "dry_run": dry_run,
            "files_written": if dry_run { all.len() } else { written },
            "files": files,
        }))
    }

    // --- sync / reindex (ctl + CLI) ------------------------------------------

    /// Sync one or all registered domains, returning per-domain reports.
    pub async fn sync(&self, only: Option<&str>) -> Result<Value> {
        self.sync_take_over(only, false).await
    }

    /// Sync like [`Engine::sync`], but with an explicit host-takeover flag for the
    /// `sync --take-over` and `serve --take-over` migration paths. In
    /// collaboration mode (a non-empty instance id) each file domain is claimed
    /// before syncing: an acquired domain syncs, a domain held by another live
    /// instance is skipped on a full sync (`only` is `None`) and refused on a
    /// named one and `take_over` forces the claim. Outside collaboration mode
    /// (standalone, single-instance) nothing is claimed and every target syncs.
    pub async fn sync_take_over(&self, only: Option<&str>, take_over: bool) -> Result<Value> {
        let _activity = ActivityState::begin(&self.activity, "sync", only);
        let targets = self.sync_targets(only)?;
        let collab = !self.instance_id.is_empty();
        let mut reports = Vec::new();
        let mut skipped = Vec::new();
        // Two short store-lock windows per domain with the scan in between, so the
        // walk-hash-parse pass of a large domain no longer blocks every concurrent
        // read behind the mutex. The first window claims the host, resolves the
        // domain id and snapshots its stamps; the second applies transactionally
        // with the TOCTOU guards. The claim stays live across the lock-free scan:
        // the heartbeat timer renews it on its own task (30 s cadence, 90 s stale
        // threshold), and unlike the old single-lock sync that scan no longer holds
        // the store lock the timer needs, so a long scan cannot starve the
        // heartbeat into staleness. The apply window is bounded db work, so no
        // extra renew before it is needed.
        for (name, root) in &targets {
            let (domain, snapshot) = {
                let store = self.store.lock().await;
                if collab {
                    match self.claim_file_host(&*store, name, root, take_over).await? {
                        HostClaim::Acquired => {}
                        HostClaim::HeldByOther(host) => {
                            if only.is_some() {
                                return Err(EngineError::Conflict(host_refusal(name, &host)));
                            }
                            tracing::info!(
                                "domain '{name}' is hosted by instance {} (last heartbeat {}); serving it read-from-database only",
                                host.instance_id,
                                host.heartbeat_at
                            );
                            skipped.push(json!({
                                "domain": name,
                                "hosted_by": host.instance_id,
                                "heartbeat_at": host.heartbeat_at,
                            }));
                            continue;
                        }
                    }
                }
                let domain = store
                    .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?;
                let snapshot = store.file_stamps(domain).await?;
                (domain, snapshot)
            };
            let scan = scan_domain(name, root, snapshot, &self.chunk_params).await;
            let report = {
                let store = self.store.lock().await;
                apply_scan(&*store, domain, scan)
                    .await
                    .map_err(|e| EngineError::Internal(format!("sync of '{name}' failed: {e}")))?
            };
            reports.push(report);
        }
        Ok(json!({
            "reports": serde_json::to_value(&reports).unwrap_or(Value::Null),
            "skipped": skipped,
        }))
    }

    /// Sync only the given relative paths of one file domain: the targeted path
    /// the daemon's watcher takes for a small debounced batch instead of a full
    /// rescan, so a one-file edit in a large domain costs one stat and one hash,
    /// not a walk of every entry.
    ///
    /// The two-lock-window shape mirrors [`Engine::sync_take_over`]'s per-domain
    /// body - claim the host, snapshot the stamps and release the lock, run the
    /// lock-free path scan, then re-lock to apply through the same [`apply_scan`]
    /// with its TOCTOU guards - so a targeted pass never holds the store mutex
    /// across the scan either. Only the watcher calls this; it is intentionally
    /// not exposed over MCP or the control socket, where a full sync is always
    /// wanted. A domain hosted by another live instance in collaboration mode is
    /// skipped silently, exactly as the watcher's full-sync path skips it today,
    /// so a non-host never writes the host's rows. A missed or mis-targeted event
    /// is caught by the full fallback, the startup sync or a manual sync, so the
    /// targeted pass only has to be convergent, never perfect.
    pub async fn sync_paths(&self, name: &str, paths: Vec<String>) -> Result<SyncReport> {
        let ContentSource::File { root } = self.content_source(name)? else {
            // A virtual domain has no files on disk; there is nothing to scan.
            return Ok(SyncReport {
                domain: name.to_string(),
                ..SyncReport::default()
            });
        };
        let collab = !self.instance_id.is_empty();
        let (domain, snapshot) = {
            let store = self.store.lock().await;
            if collab {
                match self.claim_file_host(&*store, name, &root, false).await? {
                    HostClaim::Acquired => {}
                    HostClaim::HeldByOther(host) => {
                        tracing::info!(
                            "targeted sync skipped: domain '{name}' is hosted by instance {}",
                            host.instance_id
                        );
                        return Ok(SyncReport {
                            domain: name.to_string(),
                            ..SyncReport::default()
                        });
                    }
                }
            }
            let domain = store
                .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
                .await?;
            let snapshot = store.file_stamps(domain).await?;
            (domain, snapshot)
        };
        let scan = scan_paths(name, &root, snapshot, paths, &self.chunk_params).await;
        let report = {
            let store = self.store.lock().await;
            apply_scan(&*store, domain, scan).await.map_err(|e| {
                EngineError::Internal(format!("targeted sync of '{name}' failed: {e}"))
            })?
        };
        Ok(report)
    }

    /// Reindex all file domains. `full` clears each file domain's rows first
    /// (per-domain, not a global wipe) and resyncs from disk, so virtual-domain
    /// rows, whose only source of truth is the database, are never destroyed. In
    /// collaboration mode a domain hosted by another live instance is left
    /// untouched (neither cleared nor resynced), so a non-host never rebuilds the
    /// host's rows out from under it.
    pub async fn reindex(&self, full: bool) -> Result<Value> {
        let _activity = ActivityState::begin(&self.activity, "reindex", None);
        let targets = self.sync_targets(None)?;
        let collab = !self.instance_id.is_empty();
        let mut reports = Vec::new();
        // Two short store-lock windows per domain with the scan in between, the
        // same shape as `sync_take_over`, so a large domain's walk-hash-parse pass
        // no longer holds the mutex. The first window claims the host, clears the
        // domain when `full` and snapshots the stamps; the snapshot is taken AFTER
        // the clear so the scan classifies every file as new against empty stamps -
        // the correct full-rebuild semantics. In collaboration mode a domain hosted
        // by another live instance is left untouched (neither cleared nor scanned).
        for (name, root) in targets {
            let (domain, snapshot) = {
                let store = self.store.lock().await;
                if collab {
                    match self.claim_file_host(&*store, &name, &root, false).await? {
                        HostClaim::Acquired => {}
                        HostClaim::HeldByOther(host) => {
                            tracing::info!(
                                "skipping reindex of '{name}' hosted by instance {}",
                                host.instance_id
                            );
                            continue;
                        }
                    }
                }
                let domain = store
                    .upsert_domain(&name, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?;
                if full {
                    store.clear_domain(domain).await?;
                }
                let snapshot = store.file_stamps(domain).await?;
                (domain, snapshot)
            };
            let scan = scan_domain(&name, &root, snapshot, &self.chunk_params).await;
            let report = {
                let store = self.store.lock().await;
                apply_scan(&*store, domain, scan).await.map_err(|e| {
                    EngineError::Internal(format!("reindex of '{name}' failed: {e}"))
                })?
            };
            reports.push(report);
        }
        Ok(json!({
            "full": full,
            "reports": serde_json::to_value(&reports).unwrap_or(Value::Null),
        }))
    }

    /// The file domains to sync, as `(name, root)` pairs. Virtual domains have
    /// no files, so they are skipped everywhere sync and reindex walk domains; a
    /// named sync of a virtual domain is a clean no-op.
    fn sync_targets(&self, only: Option<&str>) -> Result<Vec<(String, PathBuf)>> {
        match only {
            Some(name) => match self.content_source(name)? {
                ContentSource::File { root } => Ok(vec![(name.to_string(), root)]),
                ContentSource::Virtual => Ok(Vec::new()),
            },
            None => {
                let mut targets: Vec<(String, PathBuf)> = Vec::new();
                let config = self.config.read().unwrap();
                for (name, entry) in &config.domains {
                    if let Some(root) = entry.file_path().filter(|_| !entry.is_virtual()) {
                        targets.push((name.clone(), root));
                    }
                }
                // A domain registered after startup and already resolved once
                // (e.g. by a named `ctl sync`) rides along on a full sync too.
                for (name, entry) in self.discovered_domains.read().unwrap().iter() {
                    if config.domains.contains_key(name) {
                        continue;
                    }
                    if let Some(root) = entry.file_path().filter(|_| !entry.is_virtual()) {
                        targets.push((name.clone(), root));
                    }
                }
                Ok(targets)
            }
        }
    }

    /// Diagnostics for ctl `status`: per-domain stats, embedding coverage and the
    /// active full-text mode.
    pub async fn status_report(&self) -> Result<Value> {
        let store = self.store.lock().await;
        let info = store.store_info().await?;
        let stats = store.domain_stats().await?;
        let coverage = store.embedding_coverage().await?;
        drop(store);
        let active_embedded = coverage.embedded_for(&self.model_id);
        // Annotate each domain with its ownership relative to this instance so an
        // operator sees at a glance which domains this daemon hosts in a shared
        // database and which it serves read-from-database. `hosted_here` is true
        // only for a file domain whose host lock this instance holds.
        let domains: Vec<Value> = stats
            .iter()
            .map(|s| {
                let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
                if let Value::Object(map) = &mut v {
                    let hosted_here = !self.instance_id.is_empty()
                        && s.host_instance_id.as_deref() == Some(self.instance_id.as_str());
                    map.insert("hosted_here".to_string(), json!(hosted_here));
                }
                v
            })
            .collect();
        let registered: Vec<String> = self
            .config
            .read()
            .unwrap()
            .domains
            .keys()
            .cloned()
            .collect();
        let mut activity = self.activity.lock().unwrap().snapshot_json();
        if let Value::Object(map) = &mut activity {
            map.insert(
                "embedding_backlog".to_string(),
                json!(coverage.backlog_for(&self.model_id)),
            );
        }
        let mut result = json!({
            "fts_mode": info.fts_mode,
            "schema_version": info.schema_version,
            "db_path": info.db_path,
            "db_size": info.db_size,
            "instance_id": if self.instance_id.is_empty() { Value::Null } else { json!(self.instance_id) },
            "registered": registered,
            "domains": serde_json::to_value(&domains).unwrap_or(Value::Null),
            "embeddings": {
                "active_model": self.model_id,
                "provider": self.provider().is_some(),
                "embedded_chunks": active_embedded,
                "total_chunks": coverage.total_chunks,
                "hybrid_available": coverage.has_active_embeddings(&self.model_id),
            },
            "activity": activity,
        });
        // Omitted entirely while collaboration is off, so pre-feature output
        // stays byte-stable for an install that never touches GitHub.
        if self.config.read().unwrap().github_enabled()
            && let Value::Object(map) = &mut result
        {
            map.insert("origins".to_string(), self.origins_status_block().await);
        }
        Ok(result)
    }

    /// Chunks awaiting embedding for the active model: the figure `status_report`
    /// exposes as `embedding_backlog`. Reads the cached coverage snapshot, so it
    /// is cheap enough for the daemon's self-heal tick to poll; no per-chunk
    /// scan.
    pub async fn embedding_backlog(&self) -> Result<usize> {
        let coverage = {
            let store = self.store.lock().await;
            store.embedding_coverage().await?
        };
        Ok(coverage.backlog_for(&self.model_id))
    }

    /// Best-effort WAL checkpoint: reclaims disk after a burst of writes (a
    /// bulk embed pass, daemon shutdown) by merging the WAL back into the main
    /// db file and truncating it. The engine already bounds WAL growth on its
    /// own (a passive checkpoint fires past a hardcoded un-backfilled-frame
    /// threshold, see the PRAGMA probe comment on `TursoStore::build`), so
    /// this call is disk hygiene, never growth control - callers must not
    /// depend on it for correctness. Errors are logged and swallowed: never
    /// let a checkpoint block or fail the caller. A no-op on Postgres via the
    /// `Store::checkpoint_wal` trait default.
    pub async fn checkpoint_wal(&self) {
        let store = self.store.lock().await;
        if let Err(e) = store.checkpoint_wal().await {
            tracing::warn!("WAL checkpoint failed: {e}");
        }
    }

    /// The domain-id set this instance should embed, or `None` for "all domains".
    /// Outside collaboration mode it is `None` (embed everything). In
    /// collaboration mode it is the file domains this instance hosts plus every
    /// virtual domain (whose single source of truth is the shared database, so
    /// every instance is jointly responsible for keeping them embedded). An empty
    /// set is returned as `Some([])`, which the store treats as "nothing to do".
    async fn embed_scope(&self, store: &dyn Store) -> Result<Option<Vec<DomainId>>> {
        if self.instance_id.is_empty() {
            return Ok(None);
        }
        let mut ids: Vec<DomainId> = self.hosted.read().unwrap().values().copied().collect();
        let mut virtuals: Vec<String> = self
            .config
            .read()
            .unwrap()
            .domains
            .iter()
            .filter(|(_, e)| e.is_virtual())
            .map(|(n, _)| n.clone())
            .collect();
        for (name, entry) in self.discovered_domains.read().unwrap().iter() {
            if entry.is_virtual() && !self.config.read().unwrap().domains.contains_key(name) {
                virtuals.push(name.clone());
            }
        }
        for name in virtuals {
            let id = store
                .upsert_domain(&name, None, DomainKind::Virtual)
                .await?;
            ids.push(id);
        }
        ids.sort_by_key(|d| d.0);
        ids.dedup_by_key(|d| d.0);
        Ok(Some(ids))
    }

    /// Embed outstanding chunks for the active model in bounded batches, locking
    /// the store only to pull jobs and to store vectors so long embeds do not
    /// block searches. Returns the number of chunks embedded.
    pub async fn embed_pending(&self) -> Result<usize> {
        let Some(provider) = self.provider() else {
            return Ok(0);
        };
        let model = self.model_id.clone();
        // One snapshot of outstanding chunks; the store lock is held only to pull
        // jobs and to write vectors, never across the embed call. In
        // collaboration mode the scan is scoped to the file domains this instance
        // hosts plus all virtual domains, so a non-host does not wastefully
        // re-embed a chunk another instance owns; standalone it embeds everything.
        let mut jobs = {
            let store = self.store.lock().await;
            let scope = self.embed_scope(&*store).await?;
            store
                .chunks_needing_embedding(&model, scope.as_deref())
                .await?
        };
        if jobs.is_empty() {
            return Ok(0);
        }
        // Length-sort so batches pay for their longest member once instead of
        // padding every short chunk out to whatever long one happened to land
        // in the same batch.
        order_jobs_for_batching(&mut jobs);
        let _activity = ActivityState::begin(&self.activity, "embed", None);
        let mut embedded = 0usize;
        for batch in jobs.chunks(EMBED_BATCH) {
            let texts: Vec<String> = batch.iter().map(|j| j.text.clone()).collect();
            let vectors = provider
                .embed(&texts)
                .await
                .map_err(|e| EngineError::Internal(e.to_string()))?;
            if vectors.len() != batch.len() {
                return Err(EngineError::Internal(
                    "embedding provider returned a mismatched vector count".into(),
                ));
            }
            let rows: Vec<crystalline_index::EmbeddingRow> = batch
                .iter()
                .zip(vectors)
                .map(|(job, embedding)| crystalline_index::EmbeddingRow {
                    chunk_id: job.chunk_id,
                    dims: embedding.len(),
                    embedding,
                })
                .collect();
            let store = self.store.lock().await;
            store.store_embeddings(&rows, &model).await?;
            embedded += batch.len();
        }
        Ok(embedded)
    }

    /// Schedules a background embedding pass when a worker is wired,
    /// returning whether it was scheduled; callers run an inline pass when
    /// it was not.
    pub fn request_embed(&self) -> bool {
        match &self.embed_tx {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    // --- configure -------------------------------------------------------------

    /// Show, set or reset an agent-adjustable setting from the
    /// [`crate::settings`] registry. `show` takes only the config's read lock
    /// and is always allowed, even on a read-only instance; `set` and `unset`
    /// refuse with `EngineError::ReadOnly` on a read-only instance (config is
    /// frozen the same way the four content-mutating methods are), otherwise
    /// they validate and apply the change, persist the config file this engine
    /// was started with (or the default path) and update the in-memory config
    /// so a later read (including a concurrent one, once the write lock
    /// releases) sees it.
    pub async fn configure(&self, action: &ConfigureAction) -> Result<Value> {
        match action {
            ConfigureAction::Show => {
                let file = self.file_config.read().unwrap();
                Ok(json!({ "settings": settings::snapshot(&file, &self.overlay) }))
            }
            ConfigureAction::Set { key, value } => {
                if self.read_only {
                    return Err(EngineError::ReadOnly);
                }
                // Take the file-config write lock first to serialize against a
                // concurrent configure call, so two tasks cannot both clone the
                // old file and clobber each other's change. `persist_config` is
                // synchronous (no .await), so holding the guard across it is
                // safe. Lock order is always file_config then config.
                let mut file_guard = self.file_config.write().unwrap();
                let mut file = file_guard.clone();
                settings::apply(&mut file, key, value)?;
                self.persist_config(&file)?;
                // Recompute the effective config from the freshly saved file
                // plus the overlay, so an env-overridden key keeps reading its
                // env value even after the file value changes underneath it.
                let effective = self.overlay.apply(&file);
                let view = self.setting_view_json(&file, key);
                *file_guard = file;
                *self.config.write().unwrap() = effective;
                Ok(view)
            }
            ConfigureAction::Unset { key } => {
                if self.read_only {
                    return Err(EngineError::ReadOnly);
                }
                // Same write-lock-first discipline and lock order as Set above.
                let mut file_guard = self.file_config.write().unwrap();
                let mut file = file_guard.clone();
                settings::unset(&mut file, key)?;
                self.persist_config(&file)?;
                let effective = self.overlay.apply(&file);
                let view = self.setting_view_json(&file, key);
                *file_guard = file;
                *self.config.write().unwrap() = effective;
                Ok(view)
            }
        }
    }

    /// The just-applied setting's snapshot entry, as a JSON value, with a
    /// `note` field attached when [`settings::change_note`] has one (for
    /// example, a startup-effective key reminding the caller that a running
    /// daemon keeps its old value, or an env-overridden key reminding it that
    /// the saved value waits on the variable being removed). `file` is the
    /// freshly saved file config; the snapshot layers the overlay on top, so an
    /// env-overridden key reports its env value with `source: env`. `key` has
    /// already been validated against the registry by `apply`/`unset`, so it is
    /// always found.
    fn setting_view_json(&self, file: &GlobalConfig, key: &str) -> Value {
        settings::snapshot(file, &self.overlay)
            .into_iter()
            .find(|v| v.key == key)
            .map(|v| {
                let mut value = serde_json::to_value(v).unwrap_or(Value::Null);
                if let Some(note) = settings::change_note(key, &self.overlay)
                    && let Value::Object(map) = &mut value
                {
                    map.insert("note".to_string(), Value::String(note));
                }
                value
            })
            .unwrap_or(Value::Null)
    }

    /// Persist a config to the path this engine was started with (its
    /// `--config` override), or the default global config path when none was
    /// given. Never touches unrelated content: the caller always passes the
    /// current, load-modify-save typed config, so the serde round trip keeps
    /// every other key byte-for-byte.
    fn persist_config(&self, config: &GlobalConfig) -> Result<()> {
        let path = match &self.config_path {
            Some(p) => p.clone(),
            None => crystalline_core::config::global_config_path()
                .map_err(|e| EngineError::Internal(e.to_string()))?,
        };
        crystalline_core::config::save_yaml(&path, config).map_err(|e| {
            EngineError::Internal(format!("failed to save config {}: {e}", path.display()))
        })
    }

    // --- provision ---------------------------------------------------------

    /// Whether any registered domain currently declares a `## Provisioning`
    /// section in its MANIFEST, the gate for the `provision` MCP tool's
    /// visibility: a fresh install with no such domain never sees the tool at
    /// all, zero context cost. Wraps
    /// [`crystalline_core::provision::any_domain_declares`] against the live
    /// effective config, read fresh off the config lock on every call rather
    /// than cached - the same cost class as `routing_text`, since a domain's
    /// MANIFEST can gain or lose a `Provisioning` section between calls (a
    /// freshly added domain, or an `update_domain` pull) and the very next
    /// `list_tools` must reflect that.
    pub fn provisioning_declared(&self) -> bool {
        crystalline_core::provision::any_domain_declares(&self.config.read().unwrap())
    }

    /// Apply, inspect or record a decision for domain-declared artifact
    /// provisioning (the skills, commands, agents and MCP servers a domain's
    /// `## Provisioning` section ships into a harness's own config
    /// directory). [`ProvisionAction::Status`] reports every domain's
    /// decision and every installed harness's counts, writing nothing -
    /// always allowed, even on a read-only instance, mirroring
    /// `configure`'s `Show`. [`ProvisionAction::Allow`] and
    /// [`ProvisionAction::Deny`] record one domain's decision (the same
    /// file-config write-lock-first discipline as `configure`'s `Set`, see
    /// [`Engine::configure`]) and then reconcile; [`ProvisionAction::Apply`]
    /// reconciles without changing any decision. All three refuse with
    /// `EngineError::ReadOnly` on a read-only instance.
    ///
    /// The harnesses reconciled into always come from this machine's install
    /// receipt (`crystalline install`'s own memory of which harnesses are
    /// onboarded), never a caller-supplied list: provisioning targets every
    /// harness this machine has actually wired up.
    pub async fn provision(&self, action: &ProvisionAction) -> Result<Value> {
        let install_receipt = crystalline_core::provision::install_receipt_path()
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        let harnesses = crystalline_core::provision::installed_harnesses(&install_receipt);
        let receipt_path = crystalline_core::provision::receipt_path()
            .map_err(|e| EngineError::Internal(e.to_string()))?;

        let env_domains: HashSet<&str> = self
            .overlay
            .env_domains()
            .map(|(name, _)| name.as_str())
            .collect();

        match action {
            ProvisionAction::Status => {
                let config = self.config.read().unwrap().clone();
                let report = crystalline_core::provision::status(
                    &config,
                    &receipt_path,
                    &harnesses,
                    &env_domains,
                )
                .map_err(|e| EngineError::Internal(e.to_string()))?;
                Ok(status_report_json(&report))
            }
            ProvisionAction::Allow { domain } | ProvisionAction::Deny { domain } => {
                if self.read_only {
                    return Err(EngineError::ReadOnly);
                }
                // An env-defined domain's source of truth is its variable: the
                // overlay re-inserts a fresh entry (provision unset) on every
                // effective-config recompute, so a decision written to the
                // file would be silently discarded. Checked before the
                // registered-domain lookup so a shadowed and an env-only name
                // both get the env message, mirroring `origin_add`.
                if let Some(env) = self.overlay.env_domain(domain) {
                    return Err(EngineError::Conflict(format!(
                        "domain '{domain}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                        env.var
                    )));
                }
                let allow = matches!(action, ProvisionAction::Allow { .. });
                // Take the file-config write lock first, the same discipline
                // `configure`'s Set uses: serialize against a concurrent
                // decision, mutate a clone, persist, then swap both configs
                // in. Lock order is always file_config then config.
                {
                    let mut file_guard = self.file_config.write().unwrap();
                    let mut file = file_guard.clone();
                    set_domain_provision_decision(&mut file, domain, allow)?;
                    self.persist_config(&file)?;
                    let effective = self.overlay.apply(&file);
                    *file_guard = file;
                    *self.config.write().unwrap() = effective;
                }
                self.run_provision_apply(&receipt_path, &harnesses)
            }
            ProvisionAction::Apply => {
                if self.read_only {
                    return Err(EngineError::ReadOnly);
                }
                self.run_provision_apply(&receipt_path, &harnesses)
            }
        }
    }

    /// Reconcile every opted-in domain's declared artifacts into `harnesses`
    /// through the real system MCP runner - the shared tail of
    /// `provision`'s `Allow`, `Deny` and `Apply` arms.
    fn run_provision_apply(&self, receipt_path: &Path, harnesses: &[HarnessKind]) -> Result<Value> {
        let config = self.config.read().unwrap().clone();
        let mut mcp = crate::harness_cli::SystemMcpRunner;
        let env_domains: HashSet<&str> = self
            .overlay
            .env_domains()
            .map(|(name, _)| name.as_str())
            .collect();
        let report = crystalline_core::provision::apply(
            &config,
            receipt_path,
            harnesses,
            &mut mcp,
            &env_domains,
        )
        .map_err(|e| EngineError::Internal(e.to_string()))?;
        Ok(apply_report_json(&report))
    }

    // --- domain add (local and virtual) ---------------------------------------

    /// Create or adopt a local file domain and bring it into the index, the
    /// non-GitHub half of `add_domain`. Resolves the on-disk root (an explicit
    /// `folder`, otherwise `<domains_root>/<name>`), creates it, scaffolds a
    /// `MANIFEST.md` when the folder does not already carry one (so a fresh
    /// folder becomes a domain and an existing one is adopted in place, its
    /// files untouched), registers it in the global config and syncs.
    ///
    /// At least one of `name`/`folder` is required. Without `name`, the name is
    /// derived from the folder's basename (auto-suffixed on collision); with an
    /// explicit `name`, a different-folder or virtual clash is refused. Pointing
    /// at a folder already registered adopts it idempotently. Refuses on a
    /// read-only instance; no `github.enabled` gate, so it works on a fresh
    /// install. Returns `{ domain, root, kind, manifest_created, adopted, sync }`.
    pub async fn domain_add_local(
        &self,
        name: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if name.is_none() && folder.is_none() {
            return Err(EngineError::Invalid(
                "provide a domain name, a folder, or both".to_string(),
            ));
        }

        // An explicit folder wins; otherwise a named domain lands under the
        // configured root at `<root>/<name>`.
        let root = match folder {
            Some(f) => crystalline_core::config::expand_tilde(f),
            None => {
                let domains_root = self.config.read().unwrap().domains_root();
                origin::default_domain_folder(&domains_root, name.expect("checked above"))
            }
        };
        std::fs::create_dir_all(&root).map_err(|e| {
            EngineError::Internal(format!("creating domain directory {}: {e}", root.display()))
        })?;
        let canonical = std::fs::canonicalize(&root)
            .map_err(|e| EngineError::Internal(format!("resolving {}: {e}", root.display())))?;

        // Decide the domain name and whether we adopt an existing registration.
        let (domain_name, adopted) = {
            let cfg = self.config.read().unwrap();
            match name {
                Some(n) => {
                    // An env-defined domain of this name is owned by its variable.
                    if let Some(env) = self.overlay.env_domain(n) {
                        return Err(EngineError::Conflict(format!(
                            "domain '{n}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                            env.var
                        )));
                    }
                    match cfg.domains.get(n) {
                        None => (n.to_string(), false),
                        Some(entry) if entry.is_virtual() => {
                            return Err(EngineError::Conflict(format!(
                                "domain '{n}' is a virtual domain; pass a different name"
                            )));
                        }
                        Some(entry) => match canonicalized_file_path(entry) {
                            Some(p) if p == canonical => (n.to_string(), true),
                            _ => {
                                return Err(EngineError::Conflict(format!(
                                    "domain '{n}' is already registered at a different folder; pass a different name or omit the folder to connect it in place"
                                )));
                            }
                        },
                    }
                }
                // No name: adopt an existing registration of this exact folder,
                // else derive a fresh unique name from the folder basename.
                None => match existing_file_domain_at(&canonical, &cfg) {
                    Some(existing) => (existing.to_string(), true),
                    None => (unique_domain_name(&canonical, &cfg), false),
                },
            }
        };

        // Create-or-adopt: scaffold a MANIFEST.md only when the folder lacks one.
        let manifest = canonical.join("MANIFEST.md");
        let manifest_created = if manifest.exists() {
            false
        } else {
            let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();
            std::fs::write(
                &manifest,
                crystalline_core::manifest_template(&domain_name, &today),
            )
            .map_err(|e| EngineError::Internal(format!("writing {}: {e}", manifest.display())))?;
            true
        };

        // Register a genuinely new domain, mirroring `origin_add`'s write-lock-
        // first file-then-effective pattern so a concurrent read never observes a
        // half-applied config and no env value bakes into the saved file. An
        // adopted registration is already in the config.
        if !adopted {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = file_guard.clone();
            file.domains
                .insert(domain_name.clone(), DomainEntry::file(canonical.clone()));
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
        }

        // Tell a running daemon's watcher to watch the new root; an adopted
        // domain is already watched. This engine's own sync runs regardless.
        if !adopted && let Some(tx) = &self.watch_tx {
            let _ = tx.send(WatchEvent::Add(domain_name.clone(), canonical.clone()));
        }

        let sync = self.sync(Some(&domain_name)).await?;
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after creating '{domain_name}' failed: {e}");
        }

        Ok(json!({
            "domain": domain_name,
            "root": canonical.display().to_string(),
            "kind": "file",
            "manifest_created": manifest_created,
            "adopted": adopted,
            "sync": sync,
        }))
    }

    /// Create a virtual (database-backed) domain, the DB half of `add_domain`.
    /// Registers a `DomainEntry::virtual_domain()` in the global config, then
    /// scaffolds a `MANIFEST.md` engram into the database (a no-op when one is
    /// already present). Re-creating an existing virtual domain is idempotent; a
    /// file domain of the same name is refused. No filesystem root, no watcher,
    /// no sync. Refuses on a read-only instance; no `github.enabled` gate.
    /// Returns `{ domain, kind, manifest_created, registered }`.
    pub async fn domain_add_virtual(&self, name: &str) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let is_new = {
            let cfg = self.config.read().unwrap();
            if let Some(env) = self.overlay.env_domain(name) {
                return Err(EngineError::Conflict(format!(
                    "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                    env.var
                )));
            }
            match cfg.domains.get(name) {
                None => true,
                Some(entry) if entry.is_virtual() => false,
                Some(_) => {
                    return Err(EngineError::Conflict(format!(
                        "domain '{name}' is a file domain; pass a different name"
                    )));
                }
            }
        };

        // Register before scaffolding: `scaffold_virtual_manifest` reads the
        // content source, which requires the domain to already be registered.
        if is_new {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = file_guard.clone();
            file.domains
                .insert(name.to_string(), DomainEntry::virtual_domain());
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
        }

        let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();
        let scaffold = self
            .scaffold_virtual_manifest(name, &crystalline_core::manifest_template(name, &today))
            .await?;
        let manifest_created = scaffold
            .get("created")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        Ok(json!({
            "domain": name,
            "kind": "virtual",
            "manifest_created": manifest_created,
            "registered": is_new,
        }))
    }

    // --- origin (GitHub collaboration) ----------------------------------------

    /// Connects a new domain to a GitHub repository: downloads its tracked
    /// subtree, registers it in the global config and brings it into the
    /// index, mirroring what `domain add` does for a local folder.
    ///
    /// `domain` defaults to the repository's own name segment; `folder`
    /// defaults to `~/Documents/Crystalline/<domain>`. `path` is the
    /// subfolder within the repository that is the domain root (absent means
    /// the repository root); `branch` defaults to `main`.
    ///
    /// Refuses with `github.enabled`'s message when collaboration is off,
    /// and with `EngineError::ReadOnly` on a read-only instance (this both
    /// writes content and mutates config, exactly the two things read-only
    /// mode protects). A fresh connect returns `{ domain, root, engrams,
    /// base_commit, adopted, files_added, local_changes }`, so a caller knows
    /// what landed and whether existing local knowledge was adopted. A retry
    /// of the exact same connect - matching repo, subpath, branch and folder -
    /// instead returns `{ domain, root, engrams, base_commit, already_connected:
    /// true }`, so a client that timed out on the first attempt reads the
    /// connected state rather than a conflict.
    pub async fn origin_add(
        &self,
        repo: &str,
        domain: Option<&str>,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Value> {
        self.origin_add_with_progress(repo, domain, path, branch, folder, None)
            .await
    }

    /// [`origin_add`](Self::origin_add) with an optional stage-boundary
    /// progress callback. A real connect reports four stages through it -
    /// downloading, downloaded, indexing, connected - so a client can keep
    /// its request timeout alive during a long download and index; an
    /// already-connected retry is instant and reports none.
    pub async fn origin_add_with_progress(
        &self,
        repo: &str,
        domain: Option<&str>,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
        progress: Option<OriginProgress>,
    ) -> Result<Value> {
        let progress_at = |step: u64, msg: &str| {
            if let Some(p) = &progress {
                p(step, 4, msg);
            }
        };
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }

        let domain_name = match domain {
            Some(d) => d.to_string(),
            None => origin::default_domain_name(repo),
        };
        // A registered name is adoptable when it is an origin-less file
        // domain and the caller does not point somewhere else: the origin
        // attaches to the existing root in place and local knowledge is
        // kept. Anything else stays a conflict.
        let existing_root = match self.domain_entry(&domain_name) {
            Err(_) => None,
            Ok(entry) => {
                // An env-defined domain names the variable that owns it, so
                // the operator knows to unset it rather than pick another
                // name.
                if let Some(env) = self.overlay.env_domain(&domain_name) {
                    return Err(EngineError::Conflict(format!(
                        "domain '{domain_name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                        env.var
                    )));
                }
                if let Some(origin_cfg) = &entry.origin {
                    // A retry of the exact connect that already succeeded answers
                    // with the connected state instead of a conflict, so a client
                    // that timed out waiting for the first response never reads
                    // success as failure. This pre-lock guard keeps the common
                    // retry-after-completion case instant and lock-free; a re-read
                    // under the lock below catches a retry that raced an in-flight
                    // connect (see `origin_add_with_progress`).
                    if Self::origin_matches_request(&entry, origin_cfg, repo, path, branch, folder)
                    {
                        return self.origin_already_connected(&domain_name, &entry).await;
                    }
                    return Err(EngineError::Conflict(format!(
                        "domain '{domain_name}' is already connected to {}; pass a domain name to connect this origin under a different one",
                        origin_cfg.repo
                    )));
                }
                let Some(registered_root) = entry.file_path() else {
                    return Err(EngineError::Conflict(format!(
                        "domain '{domain_name}' is a virtual domain; an origin connects a file domain, so pass a different domain name"
                    )));
                };
                if let Some(f) = folder {
                    let wanted = crystalline_core::config::expand_tilde(f);
                    if wanted != registered_root {
                        return Err(EngineError::Conflict(format!(
                            "domain '{domain_name}' is rooted at {}; omit the folder to connect it in place, or pass a different domain name",
                            registered_root.display()
                        )));
                    }
                }
                Some(registered_root)
            }
        };

        let lock = self.origin_lock(&domain_name);
        let _guard = lock.lock().await;

        // Re-read the config under the lock. A connect that raced ahead of us
        // - a timed-out client's first attempt, still downloading when our
        // retry slipped past the pre-lock guard with no origin on file yet -
        // may have persisted its origin while we queued here. Answer a
        // now-matching origin idempotently instead of downloading the whole
        // repo again, and a now-conflicting one with the same conflict the
        // pre-lock guard raises. The locked helper skips the lock we hold, and
        // no progress stage fires: an idempotent return reports none, exactly
        // like the pre-lock guard's.
        if let Ok(entry) = self.domain_entry(&domain_name)
            && let Some(origin_cfg) = &entry.origin
        {
            if Self::origin_matches_request(&entry, origin_cfg, repo, path, branch, folder) {
                return self
                    .origin_already_connected_locked(&domain_name, &entry)
                    .await;
            }
            return Err(EngineError::Conflict(format!(
                "domain '{domain_name}' is already connected to {}; pass a domain name to connect this origin under a different one",
                origin_cfg.repo
            )));
        }

        let adopts_registered = existing_root.is_some();
        let root = match existing_root {
            Some(r) => r,
            None => match folder {
                Some(f) => crystalline_core::config::expand_tilde(f),
                None => {
                    let domains_root = self.config.read().unwrap().domains_root();
                    origin::default_domain_folder(&domains_root, &domain_name)
                }
            },
        };
        let branch_name = branch.unwrap_or("main").to_string();
        let spec = OriginSpec {
            repo: repo.to_string(),
            subpath: path.map(str::to_string),
            branch: branch_name,
        };
        let state_dir = self.origin_state_dir(&domain_name)?;

        let provider = self.resolve_origin_provider()?;
        progress_at(1, &format!("downloading {repo}"));
        let report = ops::subscribe(provider.as_ref(), &spec, &root, &state_dir)
            .await
            .inspect_err(|e| self.drop_github_credential_on_auth(e))?;
        progress_at(
            2,
            &format!(
                "downloaded {} engrams, registering the domain",
                report.engrams
            ),
        );

        // Register the domain and persist, mirroring `configure`'s file-then-
        // effective write-lock-first pattern so a concurrent read never observes
        // a half-applied config and no env value bakes into the saved file.
        {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = file_guard.clone();
            file.domains.insert(
                domain_name.clone(),
                DomainEntry {
                    kind: CoreDomainKind::File,
                    path: Some(root.clone()),
                    origin: Some(OriginConfig {
                        repo: repo.to_string(),
                        path: path.map(str::to_string),
                        branch: branch.map(str::to_string),
                        poll_secs: None,
                    }),
                    provision: None,
                },
            );
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
        }

        // Tell a running daemon's watcher to start watching the new root; it
        // also runs its own catch-up sync and embed once the watch is armed.
        // This engine's own sync just below runs regardless, so the domain is
        // searchable immediately even outside a daemon (a standalone CLI
        // command, or a race with the watcher's async catch-up); sync is
        // checksum idempotent, so the watcher repeating it moments later is a
        // harmless no-op. An adopted registered domain is already watched.
        if !adopts_registered && let Some(tx) = &self.watch_tx {
            let _ = tx.send(WatchEvent::Add(domain_name.clone(), root.clone()));
        }

        progress_at(3, "indexing for search");
        self.sync(Some(&domain_name)).await?;
        // Embedding a whole freshly connected repo can outlast any client
        // timeout, so a daemon or in-process MCP server runs it on the embed
        // worker; without a worker (standalone one-shot commands, tests) the
        // inline pass keeps the old behavior, and is a no-op anyway whenever
        // no provider is loaded.
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after connecting '{domain_name}' failed: {e}");
        }

        progress_at(4, "connected");
        Ok(json!({
            "domain": domain_name,
            "root": root.display().to_string(),
            "engrams": report.engrams,
            "base_commit": report.base_commit,
            "adopted": report.adopted || adopts_registered,
            "files_added": report.files_written,
            "local_changes": report.local_changes,
        }))
    }

    /// Whether a registered domain's origin matches this connect request
    /// exactly, so a retry answers idempotently instead of re-connecting.
    /// GitHub treats owner/name case insensitively, so the repo compares that
    /// way; the subpath compares exactly and an absent branch means main on
    /// both sides; an omitted folder always matches, a given one must resolve
    /// to the registered root. Shared by the pre-lock guard and the re-read
    /// under the lock so both sites judge a match identically.
    fn origin_matches_request(
        entry: &DomainEntry,
        origin_cfg: &OriginConfig,
        repo: &str,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
    ) -> bool {
        let same_repo = origin_cfg.repo.eq_ignore_ascii_case(repo);
        let same_path = origin_cfg.path.as_deref() == path;
        let same_branch =
            origin_cfg.branch.as_deref().unwrap_or("main") == branch.unwrap_or("main");
        let same_folder = match (folder, entry.file_path()) {
            (None, _) => true,
            (Some(f), Some(r)) => crystalline_core::config::expand_tilde(f) == r,
            (Some(_), None) => false,
        };
        same_repo && same_path && same_branch && same_folder
    }

    /// The response for a connect retry that matches the existing
    /// connection: the same shape `origin_add` returns, marked
    /// `already_connected`, read under the domain's origin lock.
    async fn origin_already_connected(&self, name: &str, entry: &DomainEntry) -> Result<Value> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;
        self.origin_already_connected_locked(name, entry).await
    }

    /// [`origin_already_connected`](Self::origin_already_connected)'s body,
    /// assuming the caller already holds the domain's origin lock. The re-read
    /// inside `origin_add_with_progress` calls this directly: the origin lock
    /// is a non-reentrant tokio mutex, so re-acquiring it there would
    /// deadlock.
    async fn origin_already_connected_locked(
        &self,
        name: &str,
        entry: &DomainEntry,
    ) -> Result<Value> {
        let root = entry.file_path().unwrap_or_default();
        let state_dir = self.origin_state_dir(name)?;
        let base_commit = crystalline_remote::state::OriginState::load(&state_dir)?
            .map(|s| s.base_commit)
            .unwrap_or_default();
        let engrams = {
            let store = self.store.lock().await;
            store
                .domain_stats()
                .await
                .unwrap_or_default()
                .iter()
                .find(|d| d.name == name)
                .map(|d| d.engrams)
                .unwrap_or(0)
        };
        Ok(json!({
            "domain": name,
            "root": root.display().to_string(),
            "engrams": engrams,
            "base_commit": base_commit,
            "already_connected": true,
        }))
    }

    /// Brings one origin-connected domain (or every one, when `domain` is
    /// `None`) up to date with its origin. Errors when a named domain is not
    /// registered or has no origin; one domain failing (offline, revoked)
    /// never aborts the others, each per-domain failure is collected into the
    /// `errors` array instead. Allowed on a read-only instance: a pull is a
    /// derived-truth update like sync, not a user-authored content write.
    pub async fn origin_update(&self, domain: Option<&str>) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        let targets = self.origin_targets(domain)?;

        let mut domains = Vec::new();
        let mut errors = Vec::new();
        for (name, entry) in targets {
            match self.origin_update_one(&name, &entry).await {
                Ok(v) => domains.push(v),
                Err(e) => errors.push(json!({ "domain": name, "error": e.to_string() })),
            }
        }
        Ok(json!({ "domains": domains, "errors": errors }))
    }

    /// Pulls and syncs one domain, under its origin lock. The per-domain body
    /// behind `origin_update`'s aggregate loop.
    async fn origin_update_one(&self, name: &str, entry: &DomainEntry) -> Result<Value> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;

        let (spec, root, state_dir) = self.origin_spec_for(name, entry)?;

        // An env-defined team domain with no origin state yet bootstraps itself
        // on first contact: the zero-config read-only node's first pull is a
        // subscribe, not an update. This is gated on the domain being
        // env-defined so a non-env domain with missing state still fails exactly
        // as before (it was never fully connected). Bootstrapping is a
        // derived-truth pull, so it is allowed on a read-only instance. The
        // env check comes first so ordinary domains skip the state read on
        // every poll tick.
        if self.overlay.env_domain(name).is_some()
            && crystalline_remote::state::OriginState::load(&state_dir)
                .ok()
                .flatten()
                .is_none()
        {
            return self
                .bootstrap_env_origin(name, &spec, &root, &state_dir)
                .await;
        }

        let provider = self.resolve_origin_provider()?;
        let report = ops::pull(provider.as_ref(), &spec, &root, &state_dir)
            .await
            .inspect_err(|e| self.drop_github_credential_on_auth(e))?;

        self.sync(Some(name)).await?;
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after updating '{name}' failed: {e}");
        }

        // `ops::pull` already saved the post-pull state to `state_dir`; reload
        // it fresh so each transition's url and title can be joined in for
        // the caller. A reload failure only degrades the proposal entries to
        // number and status (see `origin::proposal_transitions_json`), it
        // never fails an update that has already landed on disk.
        let state = crystalline_remote::state::OriginState::load(&state_dir)
            .ok()
            .flatten();
        let proposals = origin::proposal_transitions_json(&report.proposals, state.as_ref());

        Ok(origin::pull_report_json(name, &report, proposals))
    }

    /// Bootstraps an env-defined team domain on its first contact with GitHub:
    /// creates the root, runs the same [`ops::subscribe`] `origin_add` uses
    /// (minus the config write, since an env domain is never persisted), then
    /// syncs and best-effort embeds. Called under the domain's origin lock by
    /// [`Engine::origin_update_one`]. The report is shaped like a normal update
    /// (`up_to_date`, `applied`, `merged`, `conflicts`, `proposals`) so
    /// `print_origin_update` and the poller's outcome handling keep working
    /// unchanged, plus `bootstrapped: true` and the subscribe facts (`engrams`,
    /// `base_commit`) a bootstrapped line reads from.
    async fn bootstrap_env_origin(
        &self,
        name: &str,
        spec: &OriginSpec,
        root: &Path,
        state_dir: &Path,
    ) -> Result<Value> {
        let provider = self.resolve_origin_provider()?;
        // notify refuses to watch a missing directory; the daemon pre-creates
        // env-domain roots at startup, but a subscribe run outside that path
        // (an on-demand `origin update`, a poll tick) creates it here too.
        std::fs::create_dir_all(root).map_err(|e| {
            EngineError::Internal(format!(
                "could not create the domain root {}: {e}",
                root.display()
            ))
        })?;
        let report = ops::subscribe(provider.as_ref(), spec, root, state_dir)
            .await
            .inspect_err(|e| self.drop_github_credential_on_auth(e))?;

        // Tell a running daemon's watcher to start watching the freshly
        // bootstrapped root, the same signal `origin_add` sends.
        if let Some(tx) = &self.watch_tx {
            let _ = tx.send(WatchEvent::Add(name.to_string(), root.to_path_buf()));
        }

        self.sync(Some(name)).await?;
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after bootstrapping '{name}' failed: {e}");
        }

        Ok(json!({
            "domain": name,
            "bootstrapped": true,
            "up_to_date": false,
            "applied": [],
            "merged": [],
            "conflicts": [],
            "proposals": [],
            "skipped_large": report.skipped_large,
            "re_baselined": false,
            "engrams": report.engrams,
            "base_commit": report.base_commit,
        }))
    }

    /// Bootstraps every env-defined team domain that carries an origin but
    /// has no local origin state yet, bringing each up through
    /// [`Engine::origin_update_one`] so bootstrapping and a plain background
    /// pull stay exactly one code path. Called once from the daemon's startup
    /// task. A missing GitHub connection is not a failure - the background
    /// poller retries the moment a connection lands - so `NotConnected` only
    /// logs an info line; any other per-domain error is logged and never
    /// aborts startup. When env-origin domains exist while collaboration is
    /// off, one warning tells the operator to turn it on.
    pub async fn bootstrap_env_origins(&self) {
        let targets: Vec<(String, DomainEntry)> = self
            .overlay
            .env_domains()
            .filter(|(_, env)| env.entry.origin.is_some())
            .map(|(name, env)| (name.clone(), env.entry.clone()))
            .collect();
        if targets.is_empty() {
            return;
        }
        if !self.config.read().unwrap().github_enabled() {
            tracing::warn!(
                "env-defined team domains are configured but GitHub collaboration is off; set CRYSTALLINE_GITHUB_ENABLED=true to let them bootstrap"
            );
            return;
        }

        for (name, entry) in targets {
            let Ok(state_dir) = self.origin_state_dir(&name) else {
                continue;
            };
            // Already bootstrapped in an earlier run: nothing to do here, the
            // poller keeps it up to date from now on.
            let has_state = crystalline_remote::state::OriginState::load(&state_dir)
                .ok()
                .flatten()
                .is_some();
            if has_state {
                continue;
            }
            match self.origin_update_one(&name, &entry).await {
                Ok(v) => {
                    tracing::info!(
                        "bootstrapped env-defined team domain '{name}' ({} engram(s) at {})",
                        v["engrams"].as_u64().unwrap_or(0),
                        v["base_commit"].as_str().unwrap_or("")
                    );
                }
                Err(EngineError::Remote(RemoteError::NotConnected)) => {
                    tracing::info!(
                        "env-defined team domain '{name}' is waiting for a GitHub connection; the poller retries automatically"
                    );
                }
                Err(e) => {
                    tracing::warn!("could not bootstrap env-defined team domain '{name}': {e}");
                }
            }
        }
    }

    /// Reports where one origin-connected domain (or every one, when
    /// `domain` is `None`) stands relative to its origin, plus this
    /// machine's GitHub connection. Never hard-fails just because the
    /// machine is offline or has no saved connection: each domain's `behind`
    /// is `None` and the connection block reports `connected: false` rather
    /// than erroring. One domain's genuine failure (corrupt state, a missing
    /// filesystem root) never aborts the others: it is collected into the
    /// `errors` array instead, mirroring `origin_update`. Allowed on a
    /// read-only instance (a pure read).
    pub async fn origin_status(&self, domain: Option<&str>) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        let targets = self.origin_targets(domain)?;
        let connection = self.origin_connection_json().await?;

        let mut domains = Vec::new();
        let mut errors = Vec::new();
        for (name, entry) in targets {
            match self.origin_status_one(&name, &entry).await {
                Ok(v) => domains.push(v),
                Err(e) => errors.push(json!({ "domain": name, "error": e.to_string() })),
            }
        }
        Ok(json!({ "connection": connection, "domains": domains, "errors": errors }))
    }

    /// Reports one domain's status, under its origin lock. The per-domain
    /// body behind `origin_status`'s aggregate loop.
    ///
    /// A live probe is best-effort in two layers: no connection, or a
    /// provider that fails to build, degrades straight to `probe: None`
    /// (unchanged from before). When a provider was resolved but the probe
    /// call itself fails for a transport reason - offline, rate limited, an
    /// expired connection, see [`origin::is_probe_transport_error`] - the
    /// same domain is retried once with no probe at all, so the
    /// offline-capable report still comes back; the probe's own error
    /// message rides along verbatim as `probe_error` instead of aborting
    /// the domain. Any other failure (corrupt local state, and so on) is a
    /// genuine per-domain error, propagated to the caller's `errors` array.
    async fn origin_status_one(&self, name: &str, entry: &DomainEntry) -> Result<Value> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for(name, entry)?;
        // A probe is best-effort: no connection, or a provider that fails to
        // build, must never turn a status call into a hard failure.
        let probe = self.resolve_origin_provider().ok();
        match ops::status(&spec, &root, &state_dir, probe.as_deref()).await {
            Ok(report) => Ok(origin::status_report_json(name, &report, None)),
            Err(e) if probe.is_some() && origin::is_probe_transport_error(&e) => {
                // AuthExpired is one of the transport errors this arm catches
                // (see `origin::is_probe_transport_error`), so a probe that
                // failed because the token was revoked drops the cached
                // credential here too; the retry below runs probe-free, so
                // status still comes back offline.
                self.drop_github_credential_on_auth(&e);
                let report = ops::status(&spec, &root, &state_dir, None).await?;
                Ok(origin::status_report_json(
                    name,
                    &report,
                    Some(e.to_string()),
                ))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Runs one scheduling pass of the background origin poller: checks
    /// whether collaboration is enabled, connected and not paused for a
    /// shared rate limit, then brings every due origin-connected domain up
    /// to date via [`Engine::origin_update_one`], the same per-domain pull
    /// an on-demand `origin_update` runs, under the same per-domain lock, so
    /// a poll tick and a concurrent on-demand update on the same domain
    /// never interleave. This method never talks to GitHub itself; it only
    /// decides which domains are due and delegates the actual pull, so
    /// polling and on-demand updating stay exactly one code path.
    ///
    /// `now` drives every due/not-due decision and `wall_now` is its
    /// wall-clock mirror, recorded alongside every reschedule so
    /// `status_report`'s offline `origins` block can show `next_due` without
    /// ever touching an `Instant` (which carries no epoch and cannot be
    /// serialized). Passing both in, rather than reading `Instant::now()`
    /// and `Utc::now()` here, is what lets a test drive several ticks
    /// deterministically with no real waiting.
    ///
    /// A tick does nothing when collaboration is off (so enabling it later
    /// starts polling on the very next tick, no restart needed), when the
    /// shared rate-limit pause has not yet elapsed or when no GitHub token
    /// is on file (so a `connect` lands and the next tick picks it up
    /// automatically; a debug line notes this at most once an hour). A
    /// domain hitting `RemoteError::RateLimited` pauses every domain until
    /// the reported reset (defaulting an hour out when GitHub reports none)
    /// and ends the tick immediately, since GitHub rate limits are
    /// per-token, not per-repository. Any other per-domain failure (offline,
    /// a revoked token, a corrupt state directory) is recorded quietly and
    /// never stops the tick from moving on to the next due domain.
    pub async fn origin_poll_tick(&self, now: Instant, wall_now: DateTime<Utc>) {
        if !self.config.read().unwrap().github_enabled() {
            return;
        }
        if let Some(until) = self.origin_poller.rate_limited_until() {
            if wall_now < until {
                return;
            }
            self.origin_poller.set_rate_limited_until(None);
        }
        if !self.origin_connection_offline().0 {
            if self.origin_poller.should_log_no_token(now) {
                tracing::debug!(
                    "origin poll: no GitHub connection yet; waiting for connect to resume polling"
                );
            }
            return;
        }
        let Ok(targets) = self.origin_targets(None) else {
            return;
        };
        let github_poll_secs = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.poll_secs);

        for (name, entry) in targets {
            if !self.origin_poller.is_due(&name, now) {
                continue;
            }
            let domain_poll_secs = entry.origin.as_ref().and_then(|o| o.poll_secs);
            let interval_secs = poller::effective_interval_secs(domain_poll_secs, github_poll_secs);
            let tick = self.origin_poller.next_tick();
            let jitter = poller::jittered_interval(interval_secs, &name, tick);
            let jitter_chrono =
                Duration::from_std(jitter).unwrap_or(Duration::seconds(interval_secs as i64));
            self.origin_poller
                .schedule(&name, now + jitter, wall_now + jitter_chrono);

            match self.origin_update_one(&name, &entry).await {
                Ok(v) => {
                    let up_to_date = v["up_to_date"].as_bool().unwrap_or(false);
                    let applied = v["applied"].as_array().map(Vec::len).unwrap_or(0);
                    let conflict_paths: Vec<&str> = v["conflicts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["path"].as_str())
                        .collect();
                    // A share proposal can transition (merged, declined) with
                    // no file in this domain changing at all, so it needs its
                    // own info line even when the pull otherwise reports
                    // `up_to_date`: `PullReport::proposals` (see
                    // `crystalline_remote::ops::settle_up_to_date`) is
                    // refreshed on every pull regardless of whether the
                    // branch itself moved.
                    let proposal_lines: Vec<String> = v["proposals"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|p| {
                            let number = p["number"].as_u64().unwrap_or(0);
                            let status = p["status"].as_str().unwrap_or("?");
                            format!("#{number} {status}")
                        })
                        .collect();
                    if v["bootstrapped"].as_bool().unwrap_or(false) {
                        tracing::info!(
                            "origin poll: bootstrapped '{name}' ({} engram(s))",
                            v["engrams"].as_u64().unwrap_or(0)
                        );
                    } else if !conflict_paths.is_empty() {
                        tracing::info!(
                            "origin poll: '{name}' has new conflict(s): {}",
                            conflict_paths.join(", ")
                        );
                    } else if !proposal_lines.is_empty() {
                        tracing::info!(
                            "origin poll: '{name}' proposal update: {}",
                            proposal_lines.join(", ")
                        );
                    } else if !up_to_date {
                        tracing::info!("origin poll: '{name}' applied {applied} file(s)");
                    } else {
                        tracing::debug!("origin poll: '{name}' is up to date");
                    }
                    let outcome = if up_to_date {
                        poller::DomainPollOutcome::UpToDate
                    } else {
                        poller::DomainPollOutcome::Applied {
                            applied,
                            conflicts: conflict_paths.len(),
                        }
                    };
                    self.origin_poller.record_result(&name, outcome);
                }
                Err(EngineError::Remote(RemoteError::RateLimited { reset })) => {
                    let until = reset.unwrap_or_else(|| wall_now + Duration::hours(1));
                    tracing::warn!(
                        "origin poll: GitHub is rate limiting this machine; pausing every domain until {until}"
                    );
                    self.origin_poller.set_rate_limited_until(Some(until));
                    return;
                }
                Err(e) => {
                    tracing::debug!("origin poll: '{name}' failed: {e}");
                    self.origin_poller
                        .record_result(&name, poller::DomainPollOutcome::Error(e.to_string()));
                }
            }
        }
    }

    /// This machine's GitHub connection for `status_report`'s offline
    /// `origins` block: `(connected, token_store)`. Unlike
    /// `origin_connection_json` (used by the live `origin_status` operation,
    /// which reflects an injected test provider's own identity as always
    /// connected), this never special-cases an injected provider: it is a
    /// plain token-store lookup, exactly the same check the poller itself
    /// makes before spending a tick on any domain, so the two never
    /// disagree about whether this machine is connected.
    fn origin_connection_offline(&self) -> (bool, Option<&'static str>) {
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        let host = origin::token_host(api_url.as_deref());
        match self.github_credential(host.as_deref()) {
            Ok((store, Some(_))) => (true, Some(store.kind())),
            _ => (false, None),
        }
    }

    /// Builds `status_report`'s `origins` block entirely offline: this
    /// machine's GitHub connection, the poller's shared rate-limit pause
    /// and, per origin-connected domain, its repo, branch, proposal and
    /// conflict counts and local change count from a probe-free
    /// `ops::status` call (the same state-only read `origin_status` itself
    /// falls back to when a live probe fails), plus the poller's own
    /// schedule and last result for that domain. Every read here is local:
    /// the token store and each domain's saved origin state, never a GitHub
    /// call, so `status` never blocks on the network even when
    /// collaboration is on.
    async fn origins_status_block(&self) -> Value {
        let (connected, token_store) = self.origin_connection_offline();
        let rate_limit_wait_until = self.origin_poller.rate_limited_until();
        let targets = self.origin_targets(None).unwrap_or_default();

        let mut domains = Vec::new();
        for (name, entry) in targets {
            let Ok((spec, root, state_dir)) = self.origin_spec_for(&name, &entry) else {
                continue;
            };
            let Ok(report) = ops::status(&spec, &root, &state_dir, None).await else {
                continue;
            };
            let next_due = self.origin_poller.next_due_at(&name);
            let last_result = self.origin_poller.last_result(&name);
            domains.push(origin::origin_poll_status_json(
                &name,
                &report,
                next_due,
                last_result.as_ref(),
            ));
        }

        json!({
            "enabled": true,
            "connected": connected,
            "token_store": token_store,
            "rate_limit_wait_until": rate_limit_wait_until,
            "domains": domains,
        })
    }

    /// Proposes one domain's local changes as a pull request against its
    /// origin, under its origin lock.
    ///
    /// Refuses with `github.enabled`'s message when collaboration is off,
    /// and with `EngineError::ReadOnly` on a read-only instance (a share
    /// publishes content, exactly what read-only mode protects). When
    /// `ops::propose` refuses because conflicts are still pending, this
    /// degrades that refusal into a `conflicts_pending` outcome carrying the
    /// actual conflict paths (reloaded from the domain's now-current state,
    /// durable on disk since the inline pull inside `propose` already
    /// persisted them) rather than the bare count `RemoteError` alone
    /// carries, so a caller never needs to make a second round trip to learn
    /// what needs resolving. Nothing local changes on a share, so no sync
    /// runs afterward.
    pub async fn origin_share(
        &self,
        domain: &str,
        title: Option<&str>,
        description: Option<&str>,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let lock = self.origin_lock(domain);
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let provider = self.resolve_origin_provider()?;
        match ops::propose(
            provider.as_ref(),
            &spec,
            &root,
            domain,
            &state_dir,
            title,
            description,
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))
        {
            Ok(outcome) => Ok(origin::propose_outcome_json(&outcome)),
            Err(RemoteError::ConflictsPending { count }) => {
                let conflicts = crystalline_remote::state::OriginState::load(&state_dir)
                    .ok()
                    .flatten()
                    .map(|s| s.conflicts)
                    .unwrap_or_default();
                Ok(json!({
                    "outcome": "conflicts_pending",
                    "count": count,
                    "conflicts": conflicts,
                }))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Discards a declined, or still-open ("never mind"), share proposal for
    /// one domain, under its origin lock, then syncs the domain (and
    /// embeds) since discarding can restore or delete working-tree files.
    ///
    /// Refuses with `github.enabled`'s message when collaboration is off,
    /// and with `EngineError::ReadOnly` on a read-only instance (a discard
    /// writes the working tree).
    pub async fn origin_discard(&self, domain: &str, proposal_number: u64) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let lock = self.origin_lock(domain);
        let _guard = lock.lock().await;
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let report = ops::discard(&root, &state_dir, proposal_number)?;

        self.sync(Some(domain)).await?;
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!(
                "embedding after discarding proposal #{proposal_number} for '{domain}' failed: {e}"
            );
        }

        Ok(json!({
            "restored": report.restored,
            "deleted": report.deleted,
            "skipped_diverged": report.skipped_diverged,
        }))
    }

    /// Resolves one recorded conflict for one domain, under its origin lock,
    /// then syncs the domain (and embeds) since resolving writes the
    /// working tree.
    ///
    /// `keep` is `"mine"` or `"theirs"`; exactly one of `keep` or `content`
    /// must be supplied (see [`origin::resolution_from`]). Refuses with
    /// `github.enabled`'s message when collaboration is off, and with
    /// `EngineError::ReadOnly` on a read-only instance.
    pub async fn origin_resolve(
        &self,
        domain: &str,
        path: &str,
        keep: Option<&str>,
        content: Option<&[u8]>,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let resolution = origin::resolution_from(keep, content)?;
        let lock = self.origin_lock(domain);
        let _guard = lock.lock().await;
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let report = ops::resolve(&root, &state_dir, path, resolution)?;

        self.sync(Some(domain)).await?;
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after resolving a conflict for '{domain}' failed: {e}");
        }

        Ok(json!({
            "resolved": report.resolved,
            "remaining": report.remaining,
        }))
    }

    /// Resolves a single domain's `OriginSpec`, root and state directory for
    /// `origin_share`, `origin_discard` and `origin_resolve`: each a
    /// single-domain operation unlike `origin_update`/`origin_status`'s
    /// optional "every domain" mode. Errors with `UnknownDomain` when
    /// unregistered, and with the same "has no origin" message
    /// `origin_spec_for` raises when registered but not origin-connected.
    fn origin_spec_for_domain(&self, domain: &str) -> Result<(OriginSpec, PathBuf, PathBuf)> {
        let entry = self.domain_entry(domain)?;
        self.origin_spec_for(domain, &entry)
    }

    /// The domains `origin_update`/`origin_status` operate on: the one named
    /// (erroring if it is not registered or has no origin) or every
    /// registered domain with an origin, mirroring `sync_targets`'s
    /// config-then-discovered layering.
    fn origin_targets(&self, domain: Option<&str>) -> Result<Vec<(String, DomainEntry)>> {
        match domain {
            Some(name) => {
                let entry = self.domain_entry(name)?;
                if entry.origin.is_none() {
                    return Err(EngineError::Invalid(format!(
                        "domain '{name}' has no origin; connect it with `crystalline domain add --origin`"
                    )));
                }
                Ok(vec![(name.to_string(), entry)])
            }
            None => {
                let mut out: Vec<(String, DomainEntry)> = Vec::new();
                let config = self.config.read().unwrap();
                for (name, entry) in &config.domains {
                    if entry.origin.is_some() {
                        out.push((name.clone(), entry.clone()));
                    }
                }
                for (name, entry) in self.discovered_domains.read().unwrap().iter() {
                    if config.domains.contains_key(name) {
                        continue;
                    }
                    if entry.origin.is_some() {
                        out.push((name.clone(), entry.clone()));
                    }
                }
                Ok(out)
            }
        }
    }

    /// The `OriginSpec`, domain root and origin state directory for a
    /// registered domain's origin.
    fn origin_spec_for(
        &self,
        name: &str,
        entry: &DomainEntry,
    ) -> Result<(OriginSpec, PathBuf, PathBuf)> {
        let origin_cfg = entry
            .origin
            .as_ref()
            .ok_or_else(|| EngineError::Invalid(format!("domain '{name}' has no origin")))?;
        let root = entry.file_path().ok_or_else(|| {
            EngineError::Invalid(format!(
                "domain '{name}' has no filesystem root to sync an origin into"
            ))
        })?;
        let state_dir = self.origin_state_dir(name)?;
        let spec = OriginSpec {
            repo: origin_cfg.repo.clone(),
            subpath: origin_cfg.path.clone(),
            branch: origin_cfg.branch().to_string(),
        };
        Ok((spec, root, state_dir))
    }

    /// The per-domain lock serializing origin operations for one domain
    /// name, created lazily on first use.
    fn origin_lock(&self, domain: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.origin_locks.lock().unwrap();
        locks
            .entry(domain.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// The base directory per-domain origin state lives under: the test
    /// override, or the real state directory.
    fn origins_base_dir(&self) -> Result<PathBuf> {
        match &self.origins_dir_override {
            Some(p) => Ok(p.clone()),
            None => crystalline_core::config::origins_state_dir()
                .map_err(|e| EngineError::Internal(e.to_string())),
        }
    }

    /// One domain's origin state directory (base snapshot, conflict records,
    /// `state.json`).
    fn origin_state_dir(&self, domain: &str) -> Result<PathBuf> {
        Ok(self.origins_base_dir()?.join(domain))
    }

    /// Resolves the provider an origin operation runs its GitHub calls
    /// through: the injected test provider when one is set, or a fresh
    /// `GitHubProvider` built from the current config and the cached GitHub
    /// token (read from the OS keychain at most once per process, see
    /// [`Engine::github_credential`]). A `connect` earlier this same process
    /// is picked up without a restart - the connect refreshes the cache - and
    /// a machine that has not connected yet is never cached, so a later
    /// connect is seen too. Errors with `RemoteError::NotConnected` when no
    /// token has been saved and no test provider is injected.
    fn resolve_origin_provider(&self) -> Result<Arc<dyn Provider>> {
        if let Some(p) = &self.origin_provider_override {
            return Ok(p.clone());
        }
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        let host = origin::token_host(api_url.as_deref());
        let (_store, token) = self.github_credential(host.as_deref())?;
        let token = token.ok_or(RemoteError::NotConnected)?;
        Ok(Arc::new(GitHubProvider::new(
            api_url,
            Some(token.access_token),
        )))
    }

    /// This machine's GitHub connection, for `origin_status`: `{ connected,
    /// user, token_store }`. With an injected test provider, reflects the
    /// mock's own identity instead of the real token store, so origin tests
    /// never touch the OS keychain or a real credential file. `user` renders
    /// as JSON `null` rather than an empty string for the environment token
    /// store, whose synthesized identity has no login attached (see
    /// `StoredToken::user_display`).
    async fn origin_connection_json(&self) -> Result<Value> {
        if let Some(provider) = &self.origin_provider_override {
            let user = provider.current_user().await.ok();
            return Ok(json!({ "connected": true, "user": user, "token_store": "file" }));
        }
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        let host = origin::token_host(api_url.as_deref());
        let (store, token) = self.github_credential(host.as_deref())?;
        Ok(json!({
            "connected": token.is_some(),
            "user": token.as_ref().and_then(|t| t.user_display()),
            "token_store": store.kind(),
        }))
    }

    /// The GitHub token store for `host` and the token it holds, reading the
    /// OS keychain at most once per process. The environment token wins first
    /// (`CRYSTALLINE_GITHUB_TOKEN`, via `self.overlay`; keyring-free and never
    /// cached, so unsetting it is picked up live); then a cached present-token
    /// for this host; then the resolved store - the test file override (see
    /// [`Engine::with_token_store_dir`], a plain file that never touches the
    /// real OS keychain), or the real `TokenStore::resolve_and_load`, whose
    /// single `get_password` both picks the backend and loads the token. Only
    /// a present token is cached: a `None` stays live so a later `connect`
    /// (here, or from a standalone CLI writing the same keychain item) is seen
    /// on the next call without a restart. Replaces the old per-operation
    /// resolve-then-load double read that turned every origin op into two
    /// keychain touches.
    ///
    /// The environment wins over the test override too, so a poller or connect
    /// test can prove the env token is actually what gets used even when a
    /// token directory is also wired up.
    fn github_credential(&self, host: Option<&str>) -> Result<(TokenStore, Option<StoredToken>)> {
        if let Some(token) = self.overlay.github_token() {
            let store = TokenStore::env(token, host);
            let stored = store.load()?;
            return Ok((store, stored));
        }
        let key = host.unwrap_or("").to_string();
        // The std mutex is held across the keychain read on a cache miss on
        // purpose: the critical section never awaits, and single-flighting the
        // first touch under the lock collapses N concurrent first reads (a
        // daemon resolving several team domains at once) into a single keychain
        // prompt instead of a race of N. Every later call is a cache hit and
        // never reaches the read.
        let mut cache = self.github_tokens.lock().unwrap();
        if let Some(cached) = cache.get(&key) {
            return Ok((cached.store.clone(), Some(cached.token.clone())));
        }
        let (store, token) = match &self.token_store_dir_override {
            Some(dir) => {
                let store = TokenStore::File {
                    path: dir.join("github-token.json"),
                };
                let token = store.load()?;
                (store, token)
            }
            None => {
                let base = self.origins_base_dir()?;
                TokenStore::resolve_and_load(host, &base)?
            }
        };
        if let Some(token) = &token {
            cache.insert(
                key,
                CachedGithub {
                    store: store.clone(),
                    token: token.clone(),
                },
            );
        }
        Ok((store, token))
    }

    /// The plan a connect flow saves its token through: the test file override
    /// or a real `save_resolving`, plus a handle to the token cache to refresh
    /// after the write. `host` is the token host this connect targets, captured
    /// by value so the device-flow task can own the plan across the spawn.
    fn github_save_plan(&self, host: Option<&str>) -> Result<TokenSavePlan> {
        let target = match &self.token_store_dir_override {
            Some(dir) => SaveTarget::File(dir.join("github-token.json")),
            None => SaveTarget::Resolve {
                fallback_dir: self.origins_base_dir()?,
            },
        };
        Ok(TokenSavePlan {
            host: host.map(str::to_string),
            target,
            cache: Arc::clone(&self.github_tokens),
        })
    }

    /// Clears the whole GitHub token cache when `e` is
    /// [`RemoteError::AuthExpired`] - the mapped GitHub 401, see
    /// `crystalline_remote::github` - so a token rotated or revoked out from
    /// under a long-running daemon is dropped and the next `github_credential`
    /// re-reads from the keychain or file, picking up a standalone CLI connect
    /// that wrote a fresh token while the daemon ran. Coarse on purpose:
    /// clearing every host's entry (there is at most one per host) avoids
    /// threading the offending host through every provider-op call site and
    /// costs only one extra keychain read per host on the next touch.
    fn drop_github_credential_on_auth(&self, e: &RemoteError) {
        if matches!(e, RemoteError::AuthExpired) {
            self.github_tokens.lock().unwrap().clear();
        }
    }

    // --- configure: GitHub connect ------------------------------------------

    /// The api url a connect action uses for this one call: `host`
    /// (formatted as a GitHub Enterprise Server api base) when supplied,
    /// otherwise the durable `github.api_url` setting. `host` never persists;
    /// durable Enterprise Server setup is `set github.api_url`.
    fn connect_api_url(&self, host: Option<&str>) -> Option<String> {
        host.map(|h| format!("https://{h}/api/v3")).or_else(|| {
            self.config
                .read()
                .unwrap()
                .github
                .as_ref()
                .and_then(|g| g.api_url.clone())
        })
    }

    /// The OAuth App client id a connect action authenticates as: the
    /// self-hosted override from `github.oauth_client_id` when set, else the
    /// embedded Crystalline client id.
    fn oauth_client_id(&self) -> String {
        self.config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.oauth_client_id.clone())
            .unwrap_or_else(|| crystalline_remote::GITHUB_CLIENT_ID.to_string())
    }

    /// The pending device flow's display view, `{ pending: true, user_code,
    /// verification_url, expires_in_secs }`, or `None` when no flow is
    /// running.
    fn pending_view(&self) -> Option<Value> {
        self.pending_connect.lock().unwrap().as_ref().map(|p| {
            json!({
                "pending": true,
                "user_code": p.user_code,
                "verification_url": p.verification_url,
                "expires_in_secs": p.expires_in_secs,
            })
        })
    }

    /// Takes the pending flow's outcome if it has landed, clearing the slot
    /// so a later connect starts fresh. Returns `None` both when no flow is
    /// pending at all and when one is pending but still waiting on the
    /// user; a caller distinguishes those with [`Engine::pending_view`].
    fn take_finished_pending(&self) -> Option<std::result::Result<String, RemoteError>> {
        let mut guard = self.pending_connect.lock().unwrap();
        let landed = guard
            .as_ref()
            .and_then(|p| p.outcome.lock().unwrap().take());
        if landed.is_some() {
            *guard = None;
        }
        landed
    }

    /// The `github` block of the `configure` tool's snapshot: `{ connected,
    /// user, token_store, pending_connect }`. A flow still waiting on the
    /// user reports `pending_connect`; one that landed since the last call
    /// is reported here exactly once and the slot is cleared - a successful
    /// sign-in folds into `connected`/`user`, while an expired or declined
    /// one surfaces as an error (its message is already actionable) instead
    /// of being silently swallowed.
    async fn configure_connection_block(&self) -> Result<Value> {
        if let Some(outcome) = self.take_finished_pending() {
            return match outcome {
                Ok(_user) => {
                    let mut github = self.origin_connection_json().await?;
                    github["pending_connect"] = Value::Null;
                    Ok(github)
                }
                Err(e) => Err(e.into()),
            };
        }
        if let Some(view) = self.pending_view() {
            return Ok(json!({
                "connected": false,
                "user": Value::Null,
                "token_store": Value::Null,
                "pending_connect": view,
            }));
        }
        let mut github = self.origin_connection_json().await?;
        github["pending_connect"] = Value::Null;
        Ok(github)
    }

    /// Wraps a `github` block with the settings registry snapshot, the full
    /// shape the `configure` tool always returns.
    fn configure_snapshot_with(&self, github: Value) -> Result<Value> {
        let file = self.file_config.read().unwrap();
        Ok(json!({ "settings": settings::snapshot(&file, &self.overlay), "github": github }))
    }

    /// The `configure` tool's plain snapshot: every registry setting plus
    /// the GitHub connection block. Used for a bare call and after applying
    /// `set`/`unset`.
    pub async fn configure_snapshot(&self) -> Result<Value> {
        let github = self.configure_connection_block().await?;
        self.configure_snapshot_with(github)
    }

    /// The `configure` tool's personal-access-token path: validates `token`
    /// against GitHub (or `host`, for this call only), saves it and reports
    /// the connection. Drops any unrelated pending device flow, since a PAT
    /// connect settles identity immediately and a later-landing background
    /// flow must never overwrite that with a stale report. Refuses up front,
    /// before validating anything against GitHub, when
    /// `CRYSTALLINE_GITHUB_TOKEN` is set: this machine's identity is already
    /// fixed by the environment. The response's `github_enabled` and `note`
    /// state enablement explicitly, straight from the live effective config,
    /// so an agent narrates it from data rather than inferring it from tool
    /// wording (connecting and enabling are independent of each other).
    pub async fn connect_with_token(&self, token: &str, host: Option<&str>) -> Result<Value> {
        if self.overlay.github_token().is_some() {
            return Err(EngineError::EnvTokenConnect);
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let api_url = self.connect_api_url(host);
        let user = self
            .connect_auth
            .validate_token(api_url.as_deref(), token)
            .await?;
        let token_host = origin::token_host(api_url.as_deref());
        let plan = self.github_save_plan(token_host.as_deref())?;
        plan.save(&StoredToken {
            access_token: token.to_string(),
            host: token_host.unwrap_or_else(|| "github.com".to_string()),
            user: user.clone(),
            created_at: chrono::Utc::now(),
        })?;
        *self.pending_connect.lock().unwrap() = None;

        let mut github = self.origin_connection_json().await?;
        github["pending_connect"] = Value::Null;
        let enabled = self.config.read().unwrap().github_enabled();
        github["github_enabled"] = json!(enabled);
        github["note"] = json!(connect_enablement_note(enabled, false));
        self.configure_snapshot_with(github)
    }

    /// The `configure` tool's device-flow path: starts a new sign-in, or
    /// reports the one already running (or just finished), so a second
    /// connect call never starts a second flow. A fresh start spawns a
    /// background task that runs the flow to completion, validates the
    /// token and saves it, stashing the outcome in the pending slot for a
    /// later `configure` call to report and clear (see
    /// [`Engine::configure_connection_block`]). Returns immediately either
    /// way: the caller sees `pending_connect` in the same call that starts
    /// the flow, never blocking on the user confirming the code. Refuses up
    /// front, before starting anything, when `CRYSTALLINE_GITHUB_TOKEN` is
    /// set: this machine's identity is already fixed by the environment.
    pub async fn start_device_connect(&self, host: Option<&str>) -> Result<Value> {
        if self.overlay.github_token().is_some() {
            return Err(EngineError::EnvTokenConnect);
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if self.pending_connect.lock().unwrap().is_some() {
            let github = self.configure_connection_block().await?;
            return self.configure_snapshot_with(github);
        }

        let api_url = self.connect_api_url(host);
        let auth_base = crystalline_remote::github::auth::auth_base(api_url.as_deref());
        let client_id = self.oauth_client_id();
        let start = self
            .connect_auth
            .start_device_flow(&auth_base, &client_id)
            .await?;

        let outcome_slot: Arc<std::sync::Mutex<Option<std::result::Result<String, RemoteError>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let pending = PendingConnect {
            user_code: start.user_code.clone(),
            verification_url: start.verification_url.clone(),
            expires_in_secs: start.expires_in_secs,
            outcome: outcome_slot.clone(),
        };
        let view = json!({
            "pending": true,
            "user_code": pending.user_code,
            "verification_url": pending.verification_url,
            "expires_in_secs": pending.expires_in_secs,
        });
        *self.pending_connect.lock().unwrap() = Some(pending);

        let auth = self.connect_auth.clone();
        let token_host = origin::token_host(api_url.as_deref());
        let plan = self.github_save_plan(token_host.as_deref())?;
        tokio::spawn(async move {
            let result: std::result::Result<String, RemoteError> = async {
                let access_token = auth.run_device_flow(&auth_base, &client_id, &start).await?;
                let user = auth
                    .validate_token(api_url.as_deref(), &access_token)
                    .await?;
                plan.save(&StoredToken {
                    access_token,
                    host: token_host
                        .clone()
                        .unwrap_or_else(|| "github.com".to_string()),
                    user: user.clone(),
                    created_at: chrono::Utc::now(),
                })?;
                Ok(user)
            }
            .await;
            *outcome_slot.lock().unwrap() = Some(result);
        });

        let enabled = self.config.read().unwrap().github_enabled();
        self.configure_snapshot_with(json!({
            "connected": false,
            "user": Value::Null,
            "token_store": Value::Null,
            "pending_connect": view,
            "github_enabled": enabled,
            "note": connect_enablement_note(enabled, true),
        }))
    }
}

/// The one-line status paired with `github_enabled` in a fresh connect
/// response (see [`Engine::connect_with_token`] and
/// [`Engine::start_device_connect`]), so an agent narrates enablement from
/// the response data instead of inferring it from tool wording; connecting
/// and enabling `github.enabled` are independent of each other and either
/// order works. `pending` distinguishes a device flow that just started and
/// is still waiting on the user to confirm the code from a personal access
/// token connect that already landed.
fn connect_enablement_note(enabled: bool, pending: bool) -> &'static str {
    match (enabled, pending) {
        (true, true) => {
            "GitHub collaboration is enabled; once the code is confirmed team domains are ready to add."
        }
        (true, false) => "GitHub collaboration is enabled; team domains are ready to add.",
        (false, true) => {
            "Connecting works with github.enabled off; set it to true with configure when you want team domains."
        }
        (false, false) => {
            "Connected with github.enabled off; set it to true with configure when you want team domains."
        }
    }
}

/// One in-flight GitHub device-flow sign-in, held by
/// [`Engine::pending_connect`] so a second `configure` connect call while
/// one is running reports the same code instead of starting another. The
/// background task started by [`Engine::start_device_connect`] writes its
/// result into `outcome` once, for the next `configure` call (any call, not
/// just a connect) to observe and clear.
struct PendingConnect {
    /// The short code the user types in at `verification_url`.
    user_code: String,
    /// Where the user confirms the code.
    verification_url: String,
    /// How many seconds from when the flow started it stops being valid.
    expires_in_secs: u64,
    /// `None` while still waiting on the user; set once by the background
    /// task that runs the flow to completion, to either the signed-in login
    /// or the error that ended the flow (expired, declined, offline).
    outcome: Arc<std::sync::Mutex<Option<std::result::Result<String, RemoteError>>>>,
}

/// How a connect flow persists a freshly issued token: where it writes and the
/// cache it refreshes afterwards, bundled so both the inline
/// [`Engine::connect_with_token`] path and the spawned
/// [`Engine::start_device_connect`] task save through exactly one code path.
/// Built by [`Engine::github_save_plan`]; the device-flow task owns its plan by
/// value (an `Arc` handle to the cache plus an owned host and target),
/// mirroring how the pending outcome slot is moved into that task.
struct TokenSavePlan {
    /// The token host this connect targets, `None` for GitHub.com. Owned so
    /// the plan survives the move into the device-flow task.
    host: Option<String>,
    /// Where the write lands.
    target: SaveTarget,
    /// The engine's token cache, refreshed after a successful write so the
    /// next `github_credential` serves the new identity with no keychain read.
    cache: Arc<std::sync::Mutex<HashMap<String, CachedGithub>>>,
}

/// Where a [`TokenSavePlan`] writes: a fixed file under a test override, or a
/// real `save_resolving` that writes through the keychain and lands in a file
/// only when the keychain write itself fails.
enum SaveTarget {
    /// The test token-directory override's fixed file path.
    File(PathBuf),
    /// Production: `save_resolving` under this origins state directory.
    Resolve {
        /// The origins state directory the file fallback lives under.
        fallback_dir: PathBuf,
    },
}

impl TokenSavePlan {
    /// Writes `token` once (through the override file or `save_resolving`) then
    /// refreshes this host's cache entry, so the very next `github_credential`
    /// serves the new identity without another keychain read. A connect is
    /// therefore one keychain write and zero reads.
    fn save(&self, token: &StoredToken) -> std::result::Result<(), RemoteError> {
        let store = match &self.target {
            SaveTarget::File(path) => {
                let store = TokenStore::File { path: path.clone() };
                store.save(token)?;
                store
            }
            SaveTarget::Resolve { fallback_dir } => {
                TokenStore::save_resolving(self.host.as_deref(), fallback_dir, token)?
            }
        };
        let key = self.host.clone().unwrap_or_default();
        self.cache.lock().unwrap().insert(
            key,
            CachedGithub {
                store,
                token: token.clone(),
            },
        );
        Ok(())
    }
}

/// The requested settings action for [`Engine::configure`], mirroring the
/// ctl `configure` command's `action` field. The MCP `configure` tool also
/// drives `Set`/`Unset` through this same method, once per key, for its
/// richer `set`/`unset` maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigureAction {
    /// Show every registry setting's effective value.
    Show,
    /// Set `key` to the string `value`, validating type and bounds.
    Set {
        /// The dotted setting key.
        key: String,
        /// The value to parse and apply.
        value: String,
    },
    /// Reset `key` to its default.
    Unset {
        /// The dotted setting key.
        key: String,
    },
}

impl From<settings::SettingsError> for EngineError {
    fn from(e: settings::SettingsError) -> Self {
        EngineError::Invalid(e.to_string())
    }
}

/// The requested provisioning action for [`Engine::provision`], mirroring
/// the ctl `provision` command's `action` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionAction {
    /// Report every domain's decision and every installed harness's counts,
    /// writing nothing.
    Status,
    /// Opt `domain` in (`provision: true`), then reconcile.
    Allow {
        /// The domain to opt in.
        domain: String,
    },
    /// Opt `domain` out (`provision: false`), then reconcile - this removes
    /// any artifacts it previously shipped.
    Deny {
        /// The domain to opt out.
        domain: String,
    },
    /// Reconcile every already opted-in domain's artifacts, without
    /// changing any decision.
    Apply,
}

/// Record `name`'s provisioning decision (`provision: true` for `allow`,
/// `provision: false` otherwise) directly on `file`. The one seam
/// [`Engine::provision`]'s daemon path and `client::provision`'s static
/// fallback both mutate a config through, so the two can never diverge on
/// what counts as "unregistered" or "virtual". Errors with
/// [`EngineError::UnknownDomain`] naming every domain `file` does carry when
/// `name` is not one of them, and with [`EngineError::Invalid`] when `name`
/// is a virtual domain - it has no filesystem root to ship artifacts from,
/// so no decision is recorded.
pub(crate) fn set_domain_provision_decision(
    file: &mut GlobalConfig,
    name: &str,
    allow: bool,
) -> Result<()> {
    let Some(entry) = file.domains.get(name) else {
        return Err(EngineError::UnknownDomain {
            domain: name.to_string(),
            registered: file.domains.keys().cloned().collect(),
        });
    };
    if entry.is_virtual() {
        return Err(EngineError::Invalid(format!(
            "domain '{name}' is virtual; virtual domains have no files to provision, so no decision was recorded"
        )));
    }
    file.domains.get_mut(name).unwrap().provision = Some(allow);
    Ok(())
}

/// Serialize an [`crystalline_core::provision::ApplyReport`] into the JSON
/// shape both `Engine::provision`'s daemon path and `client::provision`'s
/// static fallback return, since neither the report nor its nested types
/// derive `Serialize` (the format crate keeps that derive off types whose
/// JSON shape a caller-facing envelope, not a Rust API, should own).
pub(crate) fn apply_report_json(report: &crystalline_core::provision::ApplyReport) -> Value {
    let harnesses: Vec<Value> = report
        .harnesses
        .iter()
        .map(|(harness, actions)| {
            json!({
                "harness": harness.id(),
                "actions": actions.iter().map(artifact_action_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "harnesses": harnesses,
        "notices": report.notices,
        "pending": report.pending.iter().map(pending_domain_json).collect::<Vec<_>>(),
    })
}

/// Serialize a [`crystalline_core::provision::StatusReport`] into JSON, the
/// read-only sibling of [`apply_report_json`].
pub(crate) fn status_report_json(report: &crystalline_core::provision::StatusReport) -> Value {
    json!({
        "domains": report.domains.iter().map(domain_status_json).collect::<Vec<_>>(),
        "harnesses": report.harnesses.iter().map(harness_status_json).collect::<Vec<_>>(),
        "pending": report.pending.iter().map(pending_domain_json).collect::<Vec<_>>(),
        "virtual_with_decision": report.virtual_with_decision,
    })
}

fn artifact_action_json(action: &crystalline_core::provision::ArtifactAction) -> Value {
    json!({ "target": action.target, "status": action_status_id(action.status) })
}

/// A stable snake_case id for one [`crystalline_core::provision::ActionStatus`]
/// variant, the wire and CLI-rendering spelling for what a reconcile did to
/// one artifact.
fn action_status_id(status: crystalline_core::provision::ActionStatus) -> &'static str {
    use crystalline_core::provision::ActionStatus::*;
    match status {
        Installed => "installed",
        Adopted => "adopted",
        ForeignKept => "foreign_kept",
        Updated => "updated",
        UpdatedBackup => "updated_backup",
        Removed => "removed",
        RetiredBackup => "retired_backup",
        McpAdded => "mcp_added",
        McpUpdated => "mcp_updated",
        McpRemoved => "mcp_removed",
        McpSkipped => "mcp_skipped",
        McpFailed => "mcp_failed",
        McpDeferred => "mcp_deferred",
    }
}

fn pending_domain_json(pending: &crystalline_core::provision::PendingDomain) -> Value {
    json!({ "domain": pending.domain, "counts": pending.counts })
}

fn domain_status_json(status: &crystalline_core::provision::DomainStatus) -> Value {
    json!({
        "domain": status.domain,
        "is_virtual": status.is_virtual,
        "decision": decision_id(status.decision),
        "declares": status.declares,
        "counts": status.counts,
        "parse_problems": status.parse_problems,
    })
}

/// A stable snake_case id for one [`crystalline_core::provision::Decision`]
/// variant.
fn decision_id(decision: crystalline_core::provision::Decision) -> &'static str {
    use crystalline_core::provision::Decision::*;
    match decision {
        Allowed => "allowed",
        Denied => "denied",
        Undecided => "undecided",
    }
}

fn harness_status_json(status: &crystalline_core::provision::HarnessStatus) -> Value {
    json!({
        "harness": status.harness.id(),
        "installed_files": status.installed_files,
        "installed_mcps": status.installed_mcps,
        "drift": status.drift,
        "edited": status.edited,
        "orphaned": status.orphaned,
        "missing": status.missing,
    })
}

/// Build an engine that opens the store directly for a one-shot standalone CLI
/// command. Builds the embedding provider only when the command may need it.
/// Takes a [`LoadedConfig`] so the environment overlay reaches a standalone
/// command exactly as it reaches the daemon.
pub async fn open_standalone(
    loaded: LoadedConfig,
    db: &Path,
    want_embeddings: bool,
) -> anyhow::Result<Engine> {
    let LoadedConfig {
        path,
        file,
        effective,
        overlay,
    } = loaded;
    // The factory resolves the backend from the effective `database`, creates
    // the parent directory for a Turso file and unsizes the concrete store into
    // a `dyn Store`. `db` is the resolved `--db` override for the Turso arm.
    let store = crystalline_index::open_store(&effective.database(), Some(db), false).await?;
    // A standalone data command has no `--read-only` flag of its own, so the
    // mode comes purely from the effective `service.read_only` (config or
    // environment); a read-only config refuses CLI writes here the same way the
    // daemon refuses them over the socket. The resolved `path` is threaded
    // through so a domain registered mid-command persists to, and re-reads from,
    // the same file even when it came from `CRYSTALLINE_CONFIG`.
    let read_only = effective.read_only();
    let engine = Engine::new(store, file, None, Some(path))
        .with_read_only(read_only)
        .with_env_overlay(overlay);
    // Build the provider (which may download the model) only when the index
    // already holds embeddings for the active model, so a text or filter search
    // never triggers a surprise download. With no embeddings, search falls back
    // to text without a provider anyway.
    if want_embeddings {
        let has_embeddings = {
            let store = engine.store.lock().await;
            store
                .embedding_coverage()
                .await
                .map(|c| c.has_active_embeddings(&engine.model_id))
                .unwrap_or(false)
        };
        let snapshot = engine.config.read().unwrap().clone();
        if has_embeddings && let Some(provider) = build_provider(&snapshot).await {
            engine.set_provider(provider);
        }
    }
    Ok(engine)
}

/// Build the configured embedding provider, tolerating failure (the daemon logs
/// and continues text-only). Returns `None` when no provider could be built.
pub async fn build_provider(config: &GlobalConfig) -> Option<Arc<dyn EmbeddingProvider>> {
    let ecfg =
        config
            .embeddings
            .clone()
            .unwrap_or_else(|| crystalline_core::config::EmbeddingsConfig {
                provider: "local".to_string(),
                model: crystalline_index::embed::DEFAULT_MODEL_ID.to_string(),
                endpoint: None,
                api_key_env: None,
            });
    match provider_from_config(&ecfg).await {
        Ok(p) => Some(Arc::from(p)),
        Err(e) => {
            tracing::warn!("embedding provider unavailable, continuing text-only: {e}");
            None
        }
    }
}

/// Runs embedding passes on demand: one pass per burst of requests, the
/// burst coalesced so queued signals never stack redundant passes. Ends
/// when every sender is gone, which only happens alongside the engine
/// itself going away.
pub async fn run_embed_worker(
    engine: Arc<Engine>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    while rx.recv().await.is_some() {
        while rx.try_recv().is_ok() {}
        match engine.embed_pending().await {
            Ok(0) => {}
            Ok(_) => {
                // The engine passive-checkpoints on its own past a hardcoded
                // un-backfilled-frame threshold, so this is disk reclamation
                // of the post-bulk-embed high-water mark, not growth control.
                engine.checkpoint_wal().await;
            }
            Err(e) => tracing::warn!("background embed failed: {e}"),
        }
    }
}

// --- free helpers ------------------------------------------------------------

fn parse_mode(s: Option<&str>) -> Result<SearchMode> {
    Ok(match s.unwrap_or("hybrid") {
        "hybrid" => SearchMode::Hybrid,
        "text" => SearchMode::Text,
        "semantic" => SearchMode::Semantic,
        "title" => SearchMode::Title,
        "permalink" => SearchMode::Permalink,
        other => {
            return Err(EngineError::Invalid(format!(
                "unknown search_type '{other}'; expected hybrid, text, semantic, title or permalink"
            )));
        }
    })
}

fn mode_str(m: SearchMode) -> &'static str {
    match m {
        SearchMode::Hybrid => "hybrid",
        SearchMode::Text => "text",
        SearchMode::Semantic => "semantic",
        SearchMode::Title => "title",
        SearchMode::Permalink => "permalink",
    }
}

/// The refusal message for a named sync of a file domain hosted by another live
/// instance: names the host and its last heartbeat and points at `--take-over`.
fn host_refusal(name: &str, host: &DomainHost) -> String {
    format!(
        "domain '{name}' is hosted by instance {} (last heartbeat {}); this instance serves it read-from-database only. Pass --take-over to migrate hosting here.",
        host.instance_id, host.heartbeat_at
    )
}

fn section_err(e: crystalline_core::emit::EditError) -> EngineError {
    match e {
        crystalline_core::emit::EditError::SectionNotFound { path } => {
            EngineError::NotFound(format!("no section found for heading path: {path}"))
        }
    }
}

fn routing_bullets(root: &Path) -> Vec<String> {
    let manifest = root.join("MANIFEST.md");
    let Ok(source) = std::fs::read_to_string(&manifest) else {
        return Vec::new();
    };
    let Ok(engram) = parse_engram(&source) else {
        return Vec::new();
    };
    Manifest::from_engram(&engram, &source)
        .routing_bullets()
        .to_vec()
}

fn read_engram_file(root: &Path, rel: &str) -> Option<Engram> {
    let abs = join_rel(root, rel);
    let source = std::fs::read_to_string(abs).ok()?;
    parse_engram(&source).ok()
}

/// A registered file domain's root, canonicalized. Falls back to the expanded
/// (non-canonical) path when it no longer resolves, so a domain whose folder
/// moved away still compares by its last-known path instead of silently
/// dropping out of the comparison. A virtual domain has no path, so `None`.
fn canonicalized_file_path(entry: &DomainEntry) -> Option<PathBuf> {
    let path = entry.file_path()?;
    Some(std::fs::canonicalize(&path).unwrap_or(path))
}

/// The name of the file domain already rooted at `canonical`, if any: the
/// idempotency hook so re-creating the same folder adopts its existing
/// registration rather than adding a second domain over the same files.
fn existing_file_domain_at<'a>(canonical: &Path, cfg: &'a GlobalConfig) -> Option<&'a str> {
    cfg.domains.iter().find_map(|(name, entry)| {
        (canonicalized_file_path(entry).as_deref() == Some(canonical)).then_some(name.as_str())
    })
}

/// Whether `name` is registered to a path other than `canonical`. A virtual
/// domain already using `name` counts as taken, since it has no path to
/// compare. Drives [`unique_domain_name`]'s collision search.
fn name_taken_by_other(name: &str, canonical: &Path, cfg: &GlobalConfig) -> bool {
    match cfg.domains.get(name) {
        None => false,
        Some(entry) => canonicalized_file_path(entry).as_deref() != Some(canonical),
    }
}

/// Derive a domain name from a folder's basename using the same slug rules as a
/// permalink, falling back to `domain` for a basename that slugifies to nothing
/// (a root path, or one made only of punctuation). Appends `-2`, `-3`... when
/// the name is already registered to a different path, so a derived name never
/// silently collides with an unrelated domain.
fn unique_domain_name(canonical: &Path, cfg: &GlobalConfig) -> String {
    let basename = canonical
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| canonical.display().to_string());
    let base = slugify(&basename);
    let base = if base.is_empty() {
        "domain".to_string()
    } else {
        base
    };
    if !name_taken_by_other(&base, canonical, cfg) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !name_taken_by_other(&candidate, canonical, cfg) {
            return candidate;
        }
        n += 1;
    }
}

/// Every `.md` file under `root` as `(forward-slashed relative path, absolute
/// path)`, skipping dot-directories and dot-files. Mirrors the sync engine's
/// walk so `domain import` sees the same files a file-domain sync would.
fn walk_markdown(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let walker = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(&e.file_name().to_string_lossy()));
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy();
        if is_hidden(&fname) || !fname.to_lowercase().ends_with(".md") {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        out.push((rel, entry.path().to_path_buf()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') && name != "." && name != ".."
}

/// Whether a directory exists and contains at least one entry.
fn dir_is_nonempty(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
}

/// Join a forward-slashed domain-relative path onto a root, per-segment so it is
/// correct on every platform.
fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(seg);
    }
    p
}

/// Normalize a destination into a forward-slashed `.md` path.
fn normalize_md(dest: &str) -> String {
    let trimmed = dest.trim_start_matches("./").trim_matches('/');
    let joined = trimmed
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if joined.is_empty() {
        String::new()
    } else if joined.to_lowercase().ends_with(".md") {
        joined
    } else {
        format!("{joined}.md")
    }
}

fn write_file(abs: &Path, contents: &str) -> Result<()> {
    write_bytes(abs, contents.as_bytes())
}

fn write_bytes(abs: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|source| EngineError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    // Write to a sibling temp then rename so the watcher never sees a partial file.
    let tmp = abs.with_extension(format!("md.tmp.{}", std::process::id()));
    std::fs::write(&tmp, contents).map_err(|source| EngineError::Io {
        path: tmp.display().to_string(),
        source,
    })?;
    std::fs::rename(&tmp, abs).map_err(|source| EngineError::Io {
        path: abs.display().to_string(),
        source,
    })?;
    Ok(())
}

/// A synthesized file stamp for a virtual write: the current epoch seconds, the
/// content byte length and its SHA-256. The sha doubles as the CAS token, so a
/// virtual engram gets the same `(mtime, size, sha256)` shape a file write would
/// without ever touching a filesystem.
fn virtual_stamp(content: &str) -> FileStamp {
    FileStamp {
        mtime: chrono::Utc::now().timestamp(),
        size: content.len() as u64,
        sha256: sha256_hex(content.as_bytes()),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    crystalline_index::hex_lower(&hasher.finalize())
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_offset() -> DateTime<FixedOffset> {
    chrono::Utc::now().fixed_offset()
}

/// The ISO date `spec` before today, for `timeframe` windows like `7d`, `24h`,
/// `2w`, `3m`, `1y`. Falls back to seven days on a parse failure.
fn timeframe_cutoff(spec: &str) -> Option<String> {
    let spec = spec.trim();
    let (num, unit) = spec.split_at(spec.find(|c: char| c.is_alphabetic()).unwrap_or(spec.len()));
    let n: i64 = num.trim().parse().unwrap_or(7);
    let days = match unit.trim() {
        "h" => (n + 23) / 24,
        "d" | "" => n,
        "w" => n * 7,
        "m" => n * 30,
        "y" => n * 365,
        _ => 7,
    };
    let cutoff = chrono::Utc::now().date_naive() - Duration::days(days.max(0));
    Some(cutoff.format("%Y-%m-%d").to_string())
}

/// Build engram markdown with auto-filled frontmatter via the core emitter.
/// Metadata date fields are validated against the temporal write contract: a
/// valid ISO date lands in its typed frontmatter position, while a sentinel or
/// null bound is dropped because open-ended validity is expressed by absence.
#[allow(clippy::too_many_arguments)]
fn build_markdown(
    engram_type: &str,
    title: &str,
    permalink: &str,
    tags: &[String],
    status: &str,
    recorded_at: &str,
    timestamp: &str,
    metadata: Option<&Value>,
    body: &str,
) -> Result<String> {
    let mut fm = Frontmatter {
        engram_type: engram_type.to_string(),
        title: title.to_string(),
        permalink: Some(permalink.to_string()),
        tags: tags.to_vec(),
        status: Some(status.to_string()),
        ..Frontmatter::default()
    };
    fm.recorded_at = chrono::NaiveDate::parse_from_str(recorded_at, "%Y-%m-%d").ok();
    fm.timestamp = DateTime::parse_from_rfc3339(timestamp).ok();
    // Models routinely double-encode nested tool arguments, so an object
    // arriving as a JSON string is accepted by parsing it first.
    let decoded;
    let metadata = match metadata {
        Some(Value::String(raw)) => {
            decoded = serde_json::from_str::<Value>(raw)
                .map_err(|_| EngineError::Invalid("metadata must be an object".into()))?;
            Some(&decoded)
        }
        other => other,
    };
    if let Some(Value::Object(map)) = metadata {
        for (k, v) in map {
            if is_reserved_key(k) {
                continue;
            }
            fm.extra.insert(k.clone(), json_to_yaml(v));
        }
    } else if let Some(other) = metadata
        && !other.is_null()
    {
        return Err(EngineError::Invalid("metadata must be an object".into()));
    }

    crystalline_core::temporal::normalize_temporal_fields(&mut fm)
        .map_err(|e| EngineError::Invalid(e.to_string()))?;

    let engram = Engram {
        frontmatter: fm,
        body: format!("\n{}\n", body.trim_matches('\n')),
        observations: Vec::new(),
        relations: Vec::new(),
        links: Vec::new(),
        headings: Vec::new(),
    };
    Ok(crystalline_core::emit_engram(&engram))
}

/// Frontmatter keys the write tool owns; a caller cannot override them through
/// `metadata`.
fn is_reserved_key(key: &str) -> bool {
    matches!(
        key,
        "type" | "title" | "permalink" | "tags" | "status" | "recorded_at" | "timestamp"
    )
}

fn json_to_yaml(v: &Value) -> YamlValue {
    match v {
        Value::Null => YamlValue::Null,
        Value::Bool(b) => YamlValue::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                YamlValue::Int(i)
            } else {
                YamlValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => YamlValue::String(s.clone()),
        Value::Array(a) => YamlValue::Sequence(a.iter().map(json_to_yaml).collect()),
        Value::Object(o) => YamlValue::Mapping(
            o.iter()
                .map(|(k, v)| (k.clone(), json_to_yaml(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod activity_tests {
    use super::*;

    #[test]
    fn activity_guard_registers_and_clears_on_drop() {
        let state = Arc::new(std::sync::Mutex::new(ActivityState::default()));
        let guard = ActivityState::begin(&state, "sync", Some("payments"));
        let snap = state.lock().unwrap().snapshot_json();
        assert_eq!(snap["now"][0]["kind"], "sync");
        assert_eq!(snap["now"][0]["domain"], "payments");
        assert!(snap["last"].is_null());

        drop(guard);
        let snap = state.lock().unwrap().snapshot_json();
        assert_eq!(snap["now"], serde_json::json!([]));
        assert_eq!(snap["last"]["kind"], "sync");
        assert_eq!(snap["last"]["domain"], "payments");
    }

    #[test]
    fn overlapping_activity_guards_clear_independently() {
        let state = Arc::new(std::sync::Mutex::new(ActivityState::default()));
        let sync = ActivityState::begin(&state, "sync", None);
        let embed = ActivityState::begin(&state, "embed", None);

        drop(sync);
        let snap = state.lock().unwrap().snapshot_json();
        assert_eq!(snap["now"].as_array().unwrap().len(), 1);
        assert_eq!(snap["now"][0]["kind"], "embed");
        assert_eq!(snap["last"]["kind"], "sync");
        drop(embed);
    }
}
