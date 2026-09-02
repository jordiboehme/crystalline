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

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use crystalline_core::config::{
    DomainConfig, DomainEntry, DomainKind as CoreDomainKind, GlobalConfig, OriginConfig,
    ResponseFormat, ShareIdentityMode, VerifyConfig,
};
use crystalline_core::emit::{
    append_body, insert_after_section, insert_before_section, prepend_body,
    remove_frontmatter_field, replace_section, set_evolve_ack, set_frontmatter_field,
    set_frontmatter_number, set_stale_after, set_verified, touch_generated,
};
use crystalline_core::schema::{self, Schema};
use crystalline_core::{
    CrystallineUrl, EVOLVE_ACK_KEY, Engram, EvolveAck, Frontmatter, HarnessKind, LinkTarget,
    Manifest, YamlValue, is_lower_hyphen, parse_engram, parse_engram_lossless, slugify,
};
use crystalline_index::{
    AckCounts, AckEntry, AttachmentRow, ChunkParams, DEFAULT_RETIRED_WEIGHT,
    DEFAULT_SALIENCE_WEIGHT, DomainHost, DomainId, DomainKind, EMBED_PAGE_SIZE, EdgeKind,
    EmbeddingProvider, EngramDescriptor, EngramFacts, EngramId, EngramRecord, Family, FileStamp,
    Finding, GraphNode, GraphSlice, HostClaim, InboundQuery, RULES, RecentFilter, SearchMode,
    SearchQuery, ShareFacts, Store, SweepInput, SweepOptions, SweepReport, SyncReport, apply_scan,
    chunk_engram, configured_model_id, detect, order_jobs_for_batching, parse_metadata_filters,
    provider_from_config, rank, retired_factor, rule_info, salience_prior, scan_domain, scan_paths,
};
use crystalline_remote::ops;
use crystalline_remote::{
    GitHubProvider, OriginSpec, Provider, RemoteError, StoredToken, TokenIdentity, TokenStore,
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

/// How many seed ids one [`Store::neighbors`] call takes during a consolidation
/// sweep. The backends inline the seed list into an SQL `IN (...)`, so a whole
/// domain in one call would build a statement proportional to its size. The
/// slices are merged afterwards, which is what keeps the resolved degrees
/// whole-index correct rather than per-chunk.
const NEIGHBOR_CHUNK: usize = 5_000;

/// The most nodes one [`Engine::graph_neighborhood`] response carries. A graph
/// view is read by eye, and past a couple of hundred nodes it is a hairball
/// rather than a picture; the ceiling also keeps a hand-written `max_nodes` from
/// asking for a whole index in one payload. A cut slice says so through its
/// `truncated` flag rather than pretending to be whole.
const MAX_GRAPH_NODES: usize = 150;

/// The most engrams one level of [`Engine::browse_domain`] carries.
///
/// The tree is a navigation aid, not the listing: a sidebar exists to get a
/// reader into a folder, and the folder listing - paged, filterable, and
/// server-side - is what shows a folder holding thousands of engrams. A level
/// past this cap is cut rather than loaded, and says so through `truncated`
/// beside the `total` the level really holds, so a client can send its reader
/// to the listing instead of drawing a tree nobody can read.
///
/// The number is generous on purpose: a folder anyone still navigates by tree
/// is well under it, so in practice the cap is the ceiling that keeps one
/// window-focus refetch from loading a whole domain, not a limit a real folder
/// runs into.
pub const TREE_LEVEL_CAP: usize = 500;

/// The largest page a search or a listing hands back.
///
/// The page size is client-controlled and the filter-only path projects whole
/// bodies through a sorter that turso bounds by exactly this number, so an
/// unclamped `limit` lets any reader ask the database to hold a hundred
/// thousand engram bodies at once (the 2026-08-11 query-spill audit, whose
/// sharpest finding this closes: the bound on that sorter must not be the
/// caller's to choose). A hundred rows is more than a page anyone reads and far
/// less than a page anyone can weaponize; a client that wants more pages
/// through them, which is what the envelope's `total` is for.
///
/// Clamped rather than refused, like every other bound on this surface: a hand
/// written URL asking for too much gets the largest page there is, not a 4xx.
///
/// The same number as [`MAX_INBOUND_LIMIT`] and for the same reason, kept as
/// its own constant because the two bound different queries and either could
/// move without the other: this one bounds a sorter holding bodies, that one
/// bounds how much of the reference index a popover materializes.
const MAX_PAGE_LIMIT: usize = 100;

/// The deepest level [`Engine::browse_domain`] walks.
///
/// The depth cut is pushed into SQL as a pattern that grows one term per level,
/// and `depth` arrives from a request, so this is where an absurd number stops
/// before it builds an absurd pattern. No domain nests folders anywhere near
/// this deep, which is what makes the clamp invisible to a real tree.
const TREE_MAX_DEPTH: usize = 64;

/// The default `evolve_engrams` page size. Small on purpose: the queue is meant
/// to be worked top-down and agreed item by item, not read in bulk.
const EVOLVE_DEFAULT_LIMIT: usize = 10;

/// The largest `evolve_engrams` page size.
const EVOLVE_MAX_LIMIT: usize = 100;

/// The largest `inbound_references` page size.
///
/// A ceiling rather than a suggestion, because the bound on how much of an
/// index one request may materialize must not be the caller's to choose: an
/// engram a few thousand engrams point at is exactly the case this endpoint
/// exists for, and `?limit=<enormous>` would turn the endpoint that makes that
/// engram cheap into the one way to load all of it at once. A hundred is well
/// past any popover page and small enough that the widest answer is still one
/// screenful of rows.
const MAX_INBOUND_LIMIT: usize = 100;

/// The largest domain [`Engine::delete_preview`] enumerates sole-referent
/// attachments on.
///
/// The enumeration is a full-domain read, and the cost is worth naming exactly:
/// [`Engine::sole_referent_attachments`] lists every engram in the domain and
/// loads the text of each one that could hold a reference, so asking "what does
/// this delete orphan" on a domain of fifty thousand engrams reads fifty
/// thousand engrams - to build a question, before anything is deleted. The
/// scan stops early only when every candidate is already accounted for, which a
/// domain that shares none of them never reaches.
///
/// So the bound caps **when** the enumeration runs, never what the delete does.
/// Past it the question says the attachments were not enumerated instead of
/// naming them, and [`Engine::delete_engram`] behaves exactly as it always has:
/// it removes the markdown and the rows and leaves every file alone, on a
/// domain of five hundred engrams and on a domain of fifty thousand alike.
///
/// Five hundred is the same shape of number as [`TREE_LEVEL_CAP`] and picked
/// the same way: past any archive a person curates by hand, and small enough
/// that the read behind the question stays a fraction of a second.
pub const MAX_PREVIEW_SCAN_ENGRAMS: usize = 500;

/// The fixed instruction every `evolve_engrams` response carries. It states the
/// authority the queue does and does not have, so an agent working it never
/// treats detection as permission to rewrite the archive.
pub const EVOLVE_GUIDANCE: &str = "This queue changes nothing by itself. Present it and agree what to work before any write. \
     Items marked mechanical complete intent the archive already records - fix those directly and summarize once. \
     Items marked judgment change what the archive claims - read the engram, propose and wait for a yes, one at a time. \
     A lifecycle finding never knows whether a change is a correction or a replacement; read and decide with the edit-versus-supersede test. \
     Act only on the evidence stated: this sweep detects by dates, links and graph shape, never by meaning, so it cannot confirm a contradiction. \
     Re-run the same scope when done.";

/// The frontmatter keys `edit_engram`'s `set_frontmatter` operation may write:
/// the lifecycle surface an agent tends while keeping knowledge honest. Every
/// other key is refused there, because identity (`permalink`, `title`, `type`),
/// classification (`tags`), the record of when knowledge was captured
/// (`recorded_at`) and the write provenance (`generated`) are owned by the
/// tools that maintain them and a blind assignment would corrupt an address, a
/// history or the index.
pub const SETTABLE_FRONTMATTER_KEYS: &[&str] = &[
    "status",
    "valid_from",
    "valid_to",
    "stale_after",
    "source_date",
    "salience",
    "verified",
    "evolve_ack",
];

/// [`SETTABLE_FRONTMATTER_KEYS`] rendered for an error message.
fn settable_keys() -> String {
    SETTABLE_FRONTMATTER_KEYS.join(", ")
}

/// The OKF actor recorded as `generated.by` when nothing else identifies the
/// writer: no `identity.actor` setting and no client identity from the MCP
/// handshake. Follows the spec's agent form, `name/version`.
pub const DEFAULT_ACTOR: &str = "crystalline/mcp";

/// The OKF actor a CLI-driven write records when `identity.actor` is unset.
/// The CLI is an automated job from the knowledge's point of view, so it takes
/// the spec's `process:name` form.
pub const CLI_ACTOR: &str = "process:crystalline-cli";

/// Normalize a client-supplied identity into an OKF actor token: whitespace
/// runs collapse to a single hyphen, control characters and the flow-mapping
/// punctuation that would need quoting are dropped and the result is capped, so
/// a client that calls itself "Some Client (beta)" still yields a clean
/// `generated.by`.
fn sanitize_actor(raw: &str) -> String {
    const MAX_CHARS: usize = 120;
    let mut out = String::with_capacity(raw.len());
    let mut kept = 0usize;
    let mut pending_gap = false;
    for c in raw.trim().chars() {
        if kept >= MAX_CHARS {
            break;
        }
        if c.is_whitespace() {
            pending_gap = !out.is_empty();
            continue;
        }
        if c.is_control() || matches!(c, '{' | '}' | '[' | ']' | ',' | '"' | '\'' | '\\') {
            continue;
        }
        if pending_gap {
            out.push('-');
            kept += 1;
            pending_gap = false;
        }
        out.push(c);
        kept += 1;
    }
    out.trim_matches('-').to_string()
}

/// The default host-lock heartbeat interval, seconds. Overridable via
/// `CRYSTALLINE_HEARTBEAT_SECS` (used to drive fast multi-instance verification).
const DEFAULT_HEARTBEAT_SECS: i64 = 30;
/// The default host-lock stale threshold, seconds (three missed heartbeats). A
/// lock whose last heartbeat is older than this is takeable by another instance.
/// Overridable via `CRYSTALLINE_STALE_SECS`.
const DEFAULT_STALE_SECS: i64 = 90;

/// The probability of following an edge rather than teleporting back to a
/// seed during context ranking; the standard PageRank damping factor.
const CONTEXT_DAMPING: f64 = 0.85;
/// Power-iteration cap for context ranking. Context slices are tens of
/// nodes, far past convergence at this count.
const CONTEXT_MAX_ITERATIONS: usize = 50;
/// Early-exit threshold on the L1 delta between iterations. Data-dependent
/// only, so ranking stays deterministic.
const CONTEXT_TOLERANCE: f64 = 1e-10;

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
    /// A device-flow sign-in was started while another identity's was still in
    /// flight. There is exactly one flow slot per engine and it is tagged with
    /// the credential it will store into (see `Engine::begin_device_flow`), so
    /// two sign-ins cannot complete into each other's slot; the second caller
    /// is told to wait rather than silently joining somebody else's flow.
    #[error(
        "another sign-in is in progress on this instance: wait for it to finish, then start yours again"
    )]
    ConnectInProgress,
    /// A filesystem error.
    #[error("io error at {path}: {source}{}", crystalline_core::config::io_hint_suffix(.path, .source))]
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

/// The one sentence a refused compare-and-swap speaks, wherever the comparison
/// happened. It opens with the store's own `stale edit` wording (see
/// `IndexError::StaleEdit`) because that phrase is the seam: the database
/// enforces the swap for virtual domains and [`Engine::save_engram`] enforces
/// it by hand for file domains, and the HTTP layer classifies both as the same
/// conflict by looking for it. Keep the prefix stable.
fn stale_edit_message(expected: &str, found: &str) -> String {
    format!(
        "stale edit: engram changed since it was read \
         (expected {expected}, found {found}); re-read and retry"
    )
}

/// Renders one side of a conflict for [`Engine::origin_conflict_detail`]: an
/// absent side is `null`, a UTF-8 one is a JSON string, and a side that
/// exists but is not UTF-8 is `null` with `note` set to say which side was
/// omitted and why. A later non-UTF-8 side overwrites an earlier note, so the
/// note names whichever side was last found unreadable; a caller reading it
/// learns that at least one side is binary, and the null tells it which.
fn utf8_side(bytes: Option<Vec<u8>>, name: &str, note: &mut Option<String>) -> Value {
    match bytes {
        None => Value::Null,
        Some(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Value::String(text),
            Err(_) => {
                *note = Some(format!("the {name} side is not UTF-8 and is omitted"));
                Value::Null
            }
        },
    }
}

#[cfg(test)]
mod utf8_side_tests {
    use super::*;

    #[test]
    fn an_absent_side_is_null_without_a_note() {
        let mut note = None;
        assert_eq!(utf8_side(None, "local", &mut note), Value::Null);
        assert!(note.is_none());
    }

    #[test]
    fn a_utf8_side_is_the_text_itself() {
        let mut note = None;
        let v = utf8_side(Some(b"line one\n".to_vec()), "base", &mut note);
        assert_eq!(v, Value::String("line one\n".to_string()));
        assert!(note.is_none());
    }

    #[test]
    fn a_non_utf8_side_is_null_and_names_itself_in_the_note() {
        let mut note = None;
        // A lone 0x80 continuation byte is never valid UTF-8.
        let v = utf8_side(Some(vec![0x80, 0x00, 0xff]), "upstream", &mut note);
        assert_eq!(v, Value::Null);
        let note = note.expect("a binary side sets the note");
        assert!(note.contains("upstream"), "{note}");
        assert!(note.contains("not UTF-8"), "{note}");
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
                EngineError::Conflict(stale_edit_message(&expected, &found))
            }
            other => EngineError::Internal(other.to_string()),
        }
    }
}

// The whole module is macos-gated, not just the test: on other platforms a
// gated-out lone test would leave `use super::*` dangling and fail clippy.
#[cfg(all(test, target_os = "macos"))]
mod error_tests {
    use super::*;

    #[test]
    fn io_display_carries_the_privacy_hint_for_eperm_under_documents() {
        let path = crystalline_core::config::expand_tilde("~/Documents/x")
            .to_string_lossy()
            .to_string();
        let e = EngineError::Io {
            path,
            source: std::io::Error::from_raw_os_error(1),
        };
        assert!(e.to_string().contains("Files and Folders"), "{e}");
    }
}

/// The result type used across the engine.
pub type Result<T> = std::result::Result<T, EngineError>;

/// One engram's exact file text and identity, as [`Engine::engram_text`]
/// returns it. The `checksum` is the same CAS token a save takes back.
#[derive(Debug, Clone)]
pub struct EngramText {
    /// The owning domain name.
    pub domain: String,
    /// The engram permalink.
    pub permalink: String,
    /// The domain-relative file path, forward-slashed, with the `.md` suffix.
    pub path: String,
    /// The engram's full markdown, byte for byte as stored.
    pub content: String,
    /// The content checksum: the CAS token of the next save.
    pub checksum: String,
}

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
    // The effective `skills.serve` value, snapshotted while this engine is
    // built and never re-read. See `Engine::skills_serve` for why it is frozen
    // and `Engine::with_env_overlay` for why the snapshot is taken twice.
    skills_serve: crystalline_core::config::SkillsServe,
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
    // Per-file lock serializing the checksum-guarded verbs against each other
    // for one file on disk, keyed by absolute path. See `Engine::write_lock`
    // for what it protects and why a compare-then-write without it is a race
    // two browser tabs can reach. Created lazily, one `tokio::sync::Mutex` per
    // file ever written through those verbs.
    write_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    // A fixed provider used by every origin operation instead of the
    // production per-operation `GitHubProvider` build, for tests: an engine
    // built this way never reads config or the token store to decide who to
    // talk to, and `origin_status`'s connection block reflects the injected
    // provider's own identity rather than a real, untestable OS credential
    // store. Production code never sets this.
    origin_provider_override: Option<Arc<dyn Provider>>,
    // The GitHub login the injected provider stands in as, for tests that need
    // a share to record an author (`Proposal::author_login`): a mock has no
    // credential behind it, so the login it would have carried is supplied
    // beside it. `None` - the default, and what every test that does not care
    // leaves it as - reads exactly as an instance whose credential names
    // nobody. Production code never sets this either.
    origin_provider_override_login: Option<String>,
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
    // token it holds, keyed by credential identity and token host (see
    // `credential_cache_key`: the instance credential and every personal one
    // get their own slot per host). The point is that one machine reads its
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
    // Every open `subscriptions/listen` stream that accepted the tools
    // category, so a `configure` call that moves the tool list can announce it
    // to whoever asked to hear about it. It lives here rather than on the MCP
    // handler because over streamable HTTP rmcp builds a fresh handler per
    // request and the engine is the only thing the subscriber and the flipper
    // share; see `crate::subscribers`.
    list_subscribers: Arc<crate::subscribers::ListSubscribers>,
}

/// One identity's cached GitHub credential for one host: the resolved store
/// and the token it held at the single keychain read this process ever does for
/// that pair. The token is non-optional - only a present-token outcome is ever
/// cached, so an entry existing in [`Engine::github_tokens`] means a token
/// exists - and the type carries no `Debug` impl, so a cached secret cannot
/// reach a log line or panic message through the engine's own `Debug`.
struct CachedGithub {
    store: TokenStore,
    token: StoredToken,
}

/// Who a write verb acts as, resolved by each surface before it calls the
/// engine. Read verbs never carry one: pulls, polls and probes stay on the one
/// instance credential whatever `github.share_identity` says, so a person with
/// no GitHub connection of their own still sees everything the instance sees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareActor {
    /// The machine owner: the CLI, control-socket clients and stdio MCP.
    Owner,
    /// An authenticated account: Fluid and the REST API.
    Account(String),
    /// An agent over HTTP MCP, a transport with no user auth of its own:
    /// resolved through the `github.agent_identity` setting.
    HttpAgent,
}

/// Which credential a SHARE PREVIEW may compute on when the acting identity has
/// no personal one of its own. A preview writes nothing to the forge - it pulls,
/// detects local changes and names the layer a share would target - so the two
/// answers differ only in what a caller wants a missing connection to mean.
///
/// This is a preview-only choice. The share itself always resolves the acting
/// identity's own credential and refuses without it, in every mode and on every
/// surface, which is what makes serving the plan a read rather than a loophole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewCredential {
    /// The share's own, or the share's own refusal. A caller about to ASK
    /// whether to go ahead needs the answer the confirmed call would give, so a
    /// question is never put about a share this instance would then refuse -
    /// the MCP confirmation round (spec section 3, propose_preview's
    /// write-class refusal probes ride along).
    ActingIdentity,
    /// The instance credential where the acting identity holds no personal
    /// token: the read-scope plan of spec section 6, which is what lets a
    /// browser show what a share would carry BEFORE the person connects. A
    /// personal token is a read superset of the instance one - any collaborator
    /// reads what the instance reads - so nothing here is served that the
    /// connected answer would have hidden, and nothing is written either way.
    ReadScopeFallback,
}

/// The fixed identity name the machine owner's personal credential is stored
/// under - the CLI and stdio MCP have no account to be, so they share one local
/// name rather than inventing one per machine. `crystalline connect github
/// --personal` with no `--as` writes exactly this slot.
pub const OWNER_IDENTITY_NAME: &str = "owner";

/// The refusal a write verb answers with in personal mode when the acting
/// identity has connected no GitHub account of its own (spec section 6, and the
/// locked decision behind it: no silent fallback to the instance token, ever).
/// Surface-neutral on purpose - it names both ways back - because the engine
/// serves Fluid, the REST API, MCP and the CLI from this one string; a surface
/// that can say something sharper says it in its own layer.
const PERSONAL_TOKEN_MISSING: &str = "This instance shares with personal GitHub identities. Connect yours in Fluid (profile > GitHub identity) or run 'crystalline connect github --personal', then share again.";

/// The refusal an HTTP-MCP write gets when this instance shares personally and
/// no agent identity is configured. HTTP MCP carries no user auth, so there is
/// nobody to resolve a credential for until an admin names the account those
/// shares run as - which is what this message teaches, rather than reporting a
/// missing token for an identity the caller never chose.
const AGENT_IDENTITY_UNSET: &str = "This instance shares with personal GitHub identities and no agent identity is configured: set github.agent_identity to the account whose GitHub connection agent shares should use, or share from Fluid or the CLI.";

/// The [`TokenIdentity`] one account's personal credential lives under, or the
/// teaching refusal for a name that cannot address a credential at all.
///
/// The auth store allows any non-whitespace name, while a credential is
/// addressed by a strict `[a-z0-9._-]` allowlist
/// ([`crystalline_remote::valid_identity_name`], which is what stops a name
/// from choosing where a token file lands). The gap is small and real, so it is
/// caught HERE - when someone connects an identity - and said in words that
/// name the fix, rather than months later as the token store's generic refusal
/// on that person's first share.
///
/// The rejected name is quoted back through `escape_debug`: it failed the
/// allowlist, so unlike everywhere else this crate interpolates an identity
/// name, it may carry a control byte or a terminal escape, and this message is
/// rendered in a browser, a terminal and a log line alike. A name that is
/// merely outside the allowlist (`ann+lee`) prints exactly as it was typed.
fn personal_identity(account: &str) -> Result<TokenIdentity> {
    if !crystalline_remote::valid_identity_name(account) {
        return Err(EngineError::Invalid(format!(
            "your account name '{}' cannot hold a GitHub identity - account names for sharing use lowercase letters, digits, dots, hyphens and underscores; ask an admin to recreate the account",
            account.escape_debug()
        )));
    }
    Ok(TokenIdentity::Personal(account.to_string()))
}

/// The [`Engine::github_tokens`] cache key for one credential on one host.
///
/// A unit separator joins the parts because an identity name can never contain
/// one: [`crystalline_remote::valid_identity_name`] is an allowlist of
/// `[a-z0-9._-]`, so no name can impersonate another identity or swallow the
/// host boundary. The instance credential keeps a slot of its own, which is
/// what stops a personal write from ever being served the machine's token out
/// of the cache.
fn credential_cache_key(identity: &TokenIdentity, host: Option<&str>) -> String {
    let host = host.unwrap_or("");
    match identity {
        TokenIdentity::Instance => format!("i\u{1f}{host}"),
        TokenIdentity::Personal(name) => format!("p\u{1f}{name}\u{1f}{host}"),
    }
}

/// Whether a refusal is the one a share preview may compute past: personal mode
/// with no credential on file for the acting identity.
///
/// Matched on the frozen text rather than on a variant of its own, because that
/// text IS the distinguishing fact - [`PERSONAL_TOKEN_MISSING`] is produced at
/// exactly one place ([`Engine::resolve_share_credential`]) and every other
/// refusal a preview can meet (an unset agent identity, a name no credential can
/// be addressed under, no instance connection at all, an unreadable store) has
/// to keep standing for both kinds of caller.
fn is_personal_token_missing(e: &EngineError) -> bool {
    matches!(e, EngineError::Remote(RemoteError::Refused(text)) if text == PERSONAL_TOKEN_MISSING)
}

/// Turns a write failure on a PERSONAL credential into the teaching error that
/// failure actually needs (spec section 8). `login` is `None` for an
/// instance-credential write, which keeps today's texts untouched.
///
/// Two failures are personal mode's own, and both are unreadable in their raw
/// form: a 403 means the account authenticated fine and simply cannot push to
/// this repository (stacks are same-repo and forks are unsupported, so
/// collaborator access is a hard requirement, not a suggestion), and an expired
/// token means THIS person's connection lapsed, not the instance's - so the
/// instruction is to reconnect their own identity rather than to run the
/// machine-wide connect. Every other error passes through: an offline machine
/// or a rate limit is the same event whoever was acting.
fn enrich_write_error(e: RemoteError, login: Option<&str>, repo: &str) -> RemoteError {
    let Some(login) = login else {
        return e;
    };
    match e {
        RemoteError::Api { status: 403, .. } => RemoteError::Refused(format!(
            "your GitHub account @{login} needs write access to {repo} - ask a maintainer to add you as a collaborator."
        )),
        RemoteError::AuthExpired => RemoteError::Refused(format!(
            "the GitHub connection for @{login} has expired or was revoked - reconnect your GitHub identity (Fluid profile, or 'crystalline connect github --personal')."
        )),
        other => other,
    }
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
        let skills_serve = config.skills_serve();
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
            skills_serve,
            instance_id: String::new(),
            label: String::new(),
            hosted: std::sync::RwLock::new(HashMap::new()),
            heartbeat_secs: env_secs("CRYSTALLINE_HEARTBEAT_SECS", DEFAULT_HEARTBEAT_SECS),
            stale_secs: env_secs("CRYSTALLINE_STALE_SECS", DEFAULT_STALE_SECS),
            origin_locks: std::sync::Mutex::new(HashMap::new()),
            write_locks: std::sync::Mutex::new(HashMap::new()),
            origin_provider_override: None,
            origin_provider_override_login: None,
            origins_dir_override: None,
            connect_auth: Arc::new(RealConnectAuth),
            pending_connect: std::sync::Mutex::new(None),
            token_store_dir_override: None,
            github_tokens: Arc::default(),
            origin_poller: poller::OriginPollerState::default(),
            routing_virtual: std::sync::RwLock::new(BTreeMap::new()),
            activity: Arc::default(),
            list_subscribers: Arc::default(),
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
    ///
    /// **The `skills.serve` snapshot is retaken here, and that is load
    /// bearing.** The overlay arrives through this builder *after*
    /// [`Engine::new`] has run (the daemon and `build_embedded` both spell
    /// `Engine::new(store, loaded.file, ...).with_env_overlay(loaded.overlay)`,
    /// passing the **file** config to the constructor), so a snapshot taken
    /// only in the constructor would miss `CRYSTALLINE_SKILLS_SERVE` and serve
    /// the wrong answer to exactly the deployments that set it. Both builders
    /// take `self` by value and the engine is then shared behind an `Arc`, so
    /// the value is still frozen for the engine's lifetime.
    pub fn with_env_overlay(mut self, overlay: EnvOverlay) -> Engine {
        let effective = overlay.apply(&self.file_config.read().unwrap());
        self.skills_serve = effective.skills_serve();
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

    /// The GitHub login the injected provider acts as, for tests that need a
    /// share to record an author (see
    /// [`crystalline_remote::state::Proposal::author_login`]). Test-only, and
    /// only meaningful beside [`Engine::with_origin_provider`]: a mock has no
    /// credential for [`Engine::resolve_share_provider`] to read a login off,
    /// so the login it would have carried is supplied here. Leaving it unset
    /// keeps the injected provider's original behaviour - a write that names
    /// nobody.
    pub fn with_origin_provider_login(mut self, login: impl Into<String>) -> Engine {
        self.origin_provider_override_login = Some(login.into());
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

    /// Whose GitHub identity a share on this instance runs as, read live from
    /// the effective config: `instance` (the default, one credential does
    /// everything) or `personal` (the acting identity's own).
    ///
    /// Public because it is a fact about the instance that surfaces branch on
    /// before they ever reach a credential - the REST share routes pick their
    /// role gate with it (an admin-only instance credential, versus a personal
    /// one that carries its own accountability), and Fluid renders a different
    /// share dialog.
    pub fn share_identity_mode(&self) -> ShareIdentityMode {
        self.config.read().unwrap().github_share_identity()
    }

    /// The open `subscriptions/listen` streams a moved tool list is announced
    /// on. Shared rather than per-handler on purpose - see
    /// [`crate::subscribers`] for why the streamable-HTTP transport forces the
    /// registry down to the engine.
    pub fn list_subscribers(&self) -> &Arc<crate::subscribers::ListSubscribers> {
        &self.list_subscribers
    }

    /// How the shipped agent skills are served over MCP: the value this engine
    /// was **built** with, not the live setting.
    ///
    /// Deliberately unlike [`Engine::github_enabled`] beside it. This one
    /// shapes three MCP list endpoints (the `skills` tool, the five
    /// `skill://` resources and the two prompts), and MCP 2026-07-28's
    /// SEP-2567 says a list "MUST NOT vary per-connection or as a side effect
    /// of other requests on the connection". Read live, a `configure set
    /// skills.serve` moved all three lists on the very connection that made
    /// the call. So the effective value is snapshotted while the engine is
    /// built and never re-read, which is the only layer where stdio, HTTP,
    /// embedded and the degraded stub all agree - over HTTP rmcp rebuilds the
    /// server per request from this same shared engine, so freezing anything
    /// higher up would have left that transport moving.
    ///
    /// `configure` still writes the setting; it applies at the next daemon
    /// start, which is what `startup_effective: true` (`settings.rs`) tells
    /// the user through `change_note`. That flag is the label on this
    /// behaviour, never the mechanism: its only consumer is that note.
    pub fn skills_serve(&self) -> crystalline_core::config::SkillsServe {
        self.skills_serve
    }

    /// How the MCP server encodes list-shaped tool results, from the
    /// effective `service.response_format`. Read per response, so a runtime
    /// configure switch applies from the next tool call on.
    pub fn response_format(&self) -> ResponseFormat {
        self.config.read().unwrap().response_format()
    }

    /// The OKF actor to record as `generated.by` for a write, resolved fresh
    /// per call so a runtime `configure` of `identity.actor` applies from the
    /// next write on.
    ///
    /// Resolution order: the `identity.actor` setting when set, then the
    /// caller-supplied identity (`clientname/version` for an MCP client,
    /// `process:crystalline-cli` for a CLI-driven write), then
    /// [`DEFAULT_ACTOR`].
    pub fn actor(&self, client: Option<&str>) -> String {
        if let Some(configured) = self.config.read().unwrap().identity_actor() {
            return configured.to_string();
        }
        client
            .map(sanitize_actor)
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| DEFAULT_ACTOR.to_string())
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

    /// Fail unless `name` is a registered domain, with the same
    /// [`EngineError::UnknownDomain`] naming the domains that do exist that
    /// every other verb produces.
    ///
    /// This is the check [`Engine::browse_domain`] opens with, exposed for a
    /// caller whose own verb does not resolve a domain but whose surface still
    /// has to: the REST engram listing addresses a domain in the path, where a
    /// name nobody registered is a missing resource rather than a filter that
    /// selected nothing. Search itself deliberately does not resolve its
    /// `domains` filter - an unmatched name there is simply a narrower filter -
    /// and that stays as it is.
    pub fn require_domain(&self, name: &str) -> Result<()> {
        self.domain_entry(name)?;
        Ok(())
    }

    /// Whether `name` is a team domain: a registered domain that carries a
    /// GitHub origin. [`EngineError::UnknownDomain`] when nobody registered
    /// it, which is a caller's missing resource rather than a false answer.
    ///
    /// The distinction a surface needs before offering anything origin-shaped:
    /// [`Engine::origin_status`] and [`Engine::origin_update`] both refuse a
    /// domain with no origin, and their refusal is one message for a request
    /// that could never have worked. Asking first lets a caller answer in its
    /// own terms - there is no sync status here, or there is nothing to sync -
    /// without parsing an error string.
    pub fn domain_has_origin(&self, name: &str) -> Result<bool> {
        Ok(self.domain_entry(name)?.origin.is_some())
    }

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
            // A MANIFEST carries the domain's `## Tag Aliases` declarations, so a
            // single-engram write, edit, scaffold or file reindex of it refreshes
            // the derived alias rows in the same transaction - the table never
            // needs a full sync to catch up.
            if rel == "MANIFEST.md" {
                store
                    .replace_tag_aliases(domain_id, &crystalline_core::tag_alias_pairs(text))
                    .await?;
            }
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
        self.write_engram_as(p, None).await
    }

    /// [`Engine::write_engram`] with the writer's identity: `client` is the
    /// caller's own idea of who is writing (an MCP client's
    /// `clientname/version` from the initialize handshake, or the CLI's process
    /// actor), which [`Engine::actor`] resolves against the `identity.actor`
    /// setting before it lands in the engram's `generated.by`.
    pub async fn write_engram_as(&self, p: &WriteParams, client: Option<&str>) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let actor = self.actor(client);
        let source = self.content_source(&p.domain)?;
        let engram_type = p
            .engram_type
            .clone()
            .unwrap_or_else(|| "engram".to_string());
        let status = p.status.clone().unwrap_or_else(|| "stable".to_string());
        let tags = p.tags.clone();

        let folder = p.folder.clone().unwrap_or_default();
        let title_slug = slugify(&p.title);
        if title_slug.is_empty() {
            return Err(EngineError::Invalid(
                "title does not slugify to a permalink; provide a title with letters or digits"
                    .into(),
            ));
        }
        let folder = normalize_rel(&folder);
        let rel = if folder.is_empty() {
            format!("{title_slug}.md")
        } else {
            format!("{folder}/{title_slug}.md")
        };
        // Screened before either reserved check, and before `join_rel` ever
        // sees it: `join_rel` pushes segment by segment, `..` included, so an
        // unscreened folder both places the file outside the domain root and
        // hides the destination from a textual reserved check - `a/../assets`
        // is neither `assets` nor `assets/...` as a string, yet it lands
        // exactly there on disk.
        if !is_within_domain(&rel) {
            return Err(EngineError::Invalid(escapes_root_error(&rel)));
        }
        if crystalline_core::is_reserved_path(&rel) {
            return Err(EngineError::Invalid(reserved_name_error(&rel)));
        }
        // The other reserved shape: the folder attachments live in. Checked on
        // the joined path, so a `folder` of `assets`, `/assets/`, `Assets` or
        // `assets/deep` is refused whichever spelling arrived.
        if is_assets_reserved(&rel) {
            return Err(EngineError::Invalid(assets_reserved_error(&rel)));
        }
        let permalink = slugify(&rel);

        // The whole existence-check-then-write, for a file domain, under that
        // file's lock: the check and the write it authorizes must be one step,
        // or two creates of one title both find the permalink free, both write,
        // and the second answers "created" over the first's body instead of the
        // conflict that says the name was taken. Taken before the store lock,
        // like every other holder. See `Engine::write_lock`.
        let file_lock = match &source {
            ContentSource::File { root } => Some(self.write_lock(&join_rel(root, &rel))),
            ContentSource::Virtual => None,
        };
        let _guard = match &file_lock {
            Some(lock) => Some(lock.lock().await),
            None => None,
        };

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
            &actor,
            now,
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
        // The new engram belongs in its folder's generated index.
        self.refresh_index_files(&p.domain).await;

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

    /// Save an engram's complete markdown text verbatim, guarded by the
    /// checksum of the version the caller read.
    ///
    /// The full-document counterpart of [`Engine::edit_engram`], for the HTTP
    /// PUT: the client edited the whole file, so the whole file is what lands.
    /// Nothing is rebuilt and `generated` is not touched - a save of what was
    /// read must be byte-identical, which is the editor's fidelity contract,
    /// and the text already carries whatever provenance its author put there.
    ///
    /// `expected_checksum` is enforced on BOTH storage kinds. File domains get
    /// the comparison here (read, hash, compare, write), virtual domains get
    /// it in the store's compare-and-swap (`upsert_engram_checked`); both
    /// failure paths speak the store's own "stale edit" language so the HTTP
    /// layer classifies them as one conflict.
    ///
    /// The `permalink` in the receipt is the one the engram answers to *after*
    /// the write, which is not always the one it was addressed by: writing the
    /// document verbatim means an author may have edited the `permalink` line
    /// in the frontmatter, and the index takes the permalink from the file. A
    /// caller that saved a rename is told where its engram went.
    pub async fn save_engram(&self, p: &SaveParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // A document that is not an engram would poison the index on reindex,
        // so it is refused before anything is written. This is the one hard
        // gate, and it is deliberately narrow: the text must parse (clean
        // UTF-8, frontmatter that is a YAML mapping) and must carry frontmatter
        // that actually says something, because a save that drops it silently
        // strips the engram's type, title, permalink, tags and status at once,
        // leaving the index to fall back to the path slug. An empty block is
        // that same strip wearing delimiters, so it is refused the same way.
        // Everything a document can get wrong while still being an engram - a
        // missing tag, a permalink that is not a slug, an inverted validity
        // window - is the validation endpoint's business to report, not this
        // path's to refuse: an engram that already carries such a flaw must
        // stay editable, since fixing it here is what the editor is for.
        let parsed =
            parse_engram_lossless(&p.content).map_err(|e| EngineError::Invalid(e.to_string()))?;
        if !parsed.has_frontmatter || parsed.raw_frontmatter.trim().is_empty() {
            return Err(EngineError::Invalid(
                "the document carries no frontmatter, so it is not an engram; \
                 keep the --- delimited frontmatter block, and the type, title, \
                 permalink and tags in it, at the top of the file"
                    .into(),
            ));
        }
        let (desc, source) = self.resolve(&p.identifier, Some(&p.domain)).await?;
        // A reserved name never resolves to an engram today (sync skips both),
        // so this is defence in depth rather than a reachable branch: the
        // generated `index.md` is derived from its folder and would be
        // overwritten on the next refresh, and `log.md` is reserved beside it.
        // Checked on the resolved path, which is the authority on what would
        // actually be written.
        if crystalline_core::is_reserved_path(&desc.path) {
            return Err(EngineError::Invalid(reserved_name_error(&desc.path)));
        }
        // Defence in depth for the same reason: the walk never indexes
        // anything under `assets/`, so a resolved engram cannot sit there
        // today, and a row left over from before the prefix was reserved must
        // not become a way to write into the attachment folder.
        if is_assets_reserved(&desc.path) {
            return Err(EngineError::Invalid(assets_reserved_error(&desc.path)));
        }

        match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &desc.path);
                // Held across the comparison and the write. See
                // `Engine::write_lock`.
                let lock = self.write_lock(&abs);
                let _guard = lock.lock().await;
                let current = match std::fs::read_to_string(&abs) {
                    Ok(text) => text,
                    // Indexed but absent from this machine's disk: the file was
                    // removed behind the index, or this instance is not the
                    // domain's host and only ever saw the database rows. Either
                    // way the engram the caller asked to save is not here to
                    // save, which is a miss rather than a server fault - the
                    // same reading `save_manifest` takes of its own file.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(EngineError::NotFound(format!(
                            "engram '{}' in domain '{}' has no file at {}",
                            desc.permalink,
                            desc.domain,
                            abs.display()
                        )));
                    }
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                };
                let found = sha256_hex(current.as_bytes());
                if found != p.expected_checksum {
                    return Err(EngineError::Conflict(stale_edit_message(
                        &p.expected_checksum,
                        &found,
                    )));
                }
                write_file(&abs, &p.content)?;
                let store = self.store.lock().await;
                self.reindex_file(&*store, desc.domain_id, root, &desc.path)
                    .await?;
            }
            ContentSource::Virtual => {
                let stamp = virtual_stamp(&p.content);
                let store = self.store.lock().await;
                self.index_markdown(
                    &*store,
                    desc.domain_id,
                    &desc.path,
                    &p.content,
                    stamp,
                    Some(&p.expected_checksum),
                    true,
                )
                .await?;
            }
        }

        // Where the engram now answers. Read back after the reindex rather than
        // echoed from the resolution that preceded it: the index takes an
        // engram's permalink from its frontmatter, so an author who edited that
        // line has just moved the address, and a receipt naming the old one
        // would send its caller to a permalink nothing resolves. The saved
        // content is the truth here, so the truth is what is asked. Asked
        // tolerantly, though: the save is committed by this line, so neither a
        // missing row nor a failing lookup may turn it into a reported error -
        // see [`receipt_permalink`], which answers both with the resolved name.
        let permalink = {
            let store = self.store.lock().await;
            let found = store
                .list_engrams(&desc.domain, Some(&desc.path), None)
                .await
                .map_err(EngineError::from)
                .map(|rows| {
                    rows.into_iter()
                        .find(|found| found.path == desc.path)
                        .map(|found| found.permalink)
                });
            receipt_permalink(found, desc.permalink.clone())
        };

        // A save can rewrite the MANIFEST engram of a virtual domain or the
        // titles a folder index lists, same as an edit.
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }
        self.refresh_index_files(&desc.domain).await;

        Ok(json!({
            "domain": desc.domain,
            "permalink": permalink,
            "path": desc.path,
            "checksum": sha256_hex(p.content.as_bytes()),
        }))
    }

    /// Write an engram file back into existence with this exact content, then
    /// reindex it: the resolution path for "externally deleted while a collab
    /// session held unsaved work". [`Engine::save_engram`] refuses a missing
    /// file by design (a save of something that is not there is a miss, not a
    /// create), so a room whose author keeps their text needs this verb.
    ///
    /// Same parse gate as a save and the same receipt shape, addressed by
    /// PATH rather than by identifier: the engram is gone from the index, so
    /// there is nothing left to resolve. No CAS token either, for the same
    /// reason - there is no stored version to compare against.
    pub async fn restore_engram(&self, domain: &str, path: &str, content: &str) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let parsed =
            parse_engram_lossless(content).map_err(|e| EngineError::Invalid(e.to_string()))?;
        if !parsed.has_frontmatter || parsed.raw_frontmatter.trim().is_empty() {
            return Err(EngineError::Invalid(
                "the document carries no frontmatter, so it is not an engram; \
                 keep the --- delimited frontmatter block, and the type, title, \
                 permalink and tags in it, at the top of the file"
                    .into(),
            ));
        }
        // Normalized and screened before the two reserved checks read it, the
        // same order the create and move paths use. A stored path is already in
        // this shape, so nothing a caller sends today changes.
        let normalized = normalize_rel(path);
        let path = normalized.as_str();
        if !is_within_domain(path) {
            return Err(EngineError::Invalid(escapes_root_error(path)));
        }
        if crystalline_core::is_reserved_path(path) {
            return Err(EngineError::Invalid(reserved_name_error(path)));
        }
        if is_assets_reserved(path) {
            return Err(EngineError::Invalid(assets_reserved_error(path)));
        }
        let (domain_id, source) = self.domain_source(domain).await?;
        match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, path);
                // Held across the write and the reindex, like every other
                // file write. See `Engine::write_lock`.
                let lock = self.write_lock(&abs);
                let _guard = lock.lock().await;
                write_file(&abs, content)?;
                let store = self.store.lock().await;
                self.reindex_file(&*store, domain_id, root, path).await?;
            }
            ContentSource::Virtual => {
                let stamp = virtual_stamp(content);
                let store = self.store.lock().await;
                self.index_markdown(&*store, domain_id, path, content, stamp, None, true)
                    .await?;
            }
        }
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }
        self.refresh_index_files(domain).await;

        // Read back after the reindex, exactly as a save does: the index takes
        // the permalink from the restored frontmatter, which need not match
        // the path slug. Tolerantly, for the same reason a save asks
        // tolerantly: the restore is committed by this line, so a missing row
        // and a failing lookup alike fall back to the path-derived name rather
        // than reporting a done write as failed - see [`receipt_permalink`].
        let permalink = {
            let store = self.store.lock().await;
            let found = store
                .list_engrams(domain, Some(path), None)
                .await
                .map_err(EngineError::from)
                .map(|rows| {
                    rows.into_iter()
                        .find(|found| found.path == path)
                        .map(|found| found.permalink)
                });
            receipt_permalink(found, path.trim_end_matches(".md").to_string())
        };
        Ok(json!({
            "domain": domain,
            "permalink": permalink,
            "path": path,
            "checksum": sha256_hex(content.as_bytes()),
        }))
    }

    /// A registered domain's row id and content source, upserting the row the
    /// way a create does. The domain-addressed half of what
    /// [`Engine::resolve`] does for an identifier, for a write path whose
    /// engram is not in the index to resolve.
    async fn domain_source(&self, domain: &str) -> Result<(DomainId, ContentSource)> {
        let source = self.content_source(domain)?;
        let store = self.store.lock().await;
        let domain_id = match &source {
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
        Ok((domain_id, source))
    }

    // --- attachments ---------------------------------------------------------
    //
    // The byte seam every attachment surface goes through: the REST file
    // routes, the archive and the MCP resource reads. Above it nothing knows
    // which kind of domain it is addressing; below it a file domain keeps plain
    // files under its root (so a git team domain carries them like any other
    // tracked file) and a virtual domain keeps the bytes in the index beside
    // the row. The metadata row is identical either way, and it is written
    // here rather than left to the next walker pass, so a surface that just
    // uploaded a file can list it immediately.

    /// Every attachment a domain carries, metadata only, ordered by path.
    ///
    /// Bytes are never loaded: a listing of a domain full of slide decks costs
    /// one query.
    pub async fn attachment_list(&self, domain: &str) -> Result<Vec<AttachmentRow>> {
        let (domain_id, _) = self.domain_source(domain).await?;
        let store = self.store.lock().await;
        Ok(store.list_attachments(domain_id).await?)
    }

    /// One attachment's bytes and its metadata row.
    ///
    /// A file domain reads the file under its root; a virtual domain reads the
    /// stored blob. Either way an absent attachment is
    /// [`EngineError::NotFound`], the same miss an absent engram reports.
    ///
    /// The file arm heals the row it serves. A file can arrive behind the index
    /// (a `git pull`, an editor, a domain whose first sync has not run) and can
    /// change behind it the same way, so when the recorded row does not match
    /// the file's own size and modification instant the bytes just read are
    /// hashed and the row is refreshed through the same upsert the walker uses.
    /// That keeps the sha a caller caches on describing exactly what it
    /// received. The match itself is the walker's stat prefilter, so the common
    /// case costs no hashing at all.
    pub async fn attachment_read(
        &self,
        domain: &str,
        path: &str,
    ) -> Result<(Vec<u8>, AttachmentRow)> {
        validate_attachment_path(path)?;
        let (domain_id, source) = self.domain_source(domain).await?;
        match &source {
            ContentSource::File { root } => {
                let abs = contained_asset_path(root, path)?;
                // The stat comes first so an over-cap file is refused without
                // ever being read: the ceiling is enforced by the walker (which
                // skips such a file, so it has no row) and by the write, and a
                // read that hashed one anyway would both spend the memory and
                // mint a row the next full scan deletes again.
                let meta = match std::fs::metadata(&abs) {
                    Ok(meta) => meta,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(EngineError::NotFound(missing_attachment(domain, path)));
                    }
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                };
                if meta.len() > crystalline_core::MAX_ATTACHMENT_BYTES {
                    return Err(EngineError::Invalid(over_cap_error(path, meta.len())));
                }
                let bytes = match std::fs::read(&abs) {
                    Ok(bytes) => bytes,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(EngineError::NotFound(missing_attachment(domain, path)));
                    }
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                };
                // The bytes that were actually read decide, like the walker's
                // post-read check: a file that grew past the ceiling between
                // the stat and the read is caught here rather than served.
                if bytes.len() as u64 > crystalline_core::MAX_ATTACHMENT_BYTES {
                    return Err(EngineError::Invalid(over_cap_error(
                        path,
                        bytes.len() as u64,
                    )));
                }
                let modified = asset_modified(&abs);
                let store = self.store.lock().await;
                if let Some(row) = store.get_attachment(domain_id, path).await?
                    && row.size == bytes.len() as u64
                    && row.modified == modified
                {
                    return Ok((bytes, row));
                }
                let row = attachment_row(path, &bytes, modified)?;
                store.upsert_attachment(domain_id, &row).await?;
                Ok((bytes, row))
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let row = store
                    .get_attachment(domain_id, path)
                    .await?
                    .ok_or_else(|| EngineError::NotFound(missing_attachment(domain, path)))?;
                let bytes = store
                    .read_attachment_blob(domain_id, path)
                    .await?
                    .ok_or_else(|| EngineError::NotFound(missing_attachment(domain, path)))?;
                Ok((bytes, row))
            }
        }
    }

    /// Create or replace one attachment, returning the row that now describes
    /// it.
    ///
    /// Every gate runs before a byte is stored: the path rules and the
    /// extension allowlist ([`crystalline_core::validate_asset_path`]) and the
    /// size ceiling. A file domain then writes the bytes atomically under its
    /// root, with the joined path proven to stay inside it, and takes the same
    /// per-file lock every other file write takes so a concurrent replace
    /// cannot leave the row describing the loser's bytes. A virtual domain
    /// writes the row and then the blob, in that order, since the store keeps a
    /// blob without a row an error rather than an orphan; a blob write that
    /// fails takes the row back out with it, so a failure leaves the domain
    /// exactly as it found it rather than listing a path with no bytes.
    ///
    /// Both kinds mark the domain pending in the maintenance state afterwards:
    /// a human just added something the agent has not read yet, which is
    /// exactly what a consolidation sweep is for.
    pub async fn attachment_write(
        &self,
        domain: &str,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<AttachmentRow> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        validate_attachment_path(path)?;
        if bytes.len() as u64 > crystalline_core::MAX_ATTACHMENT_BYTES {
            return Err(EngineError::Invalid(over_cap_error(
                path,
                bytes.len() as u64,
            )));
        }
        let (domain_id, source) = self.domain_source(domain).await?;
        let row = match &source {
            ContentSource::File { root } => {
                let abs = contained_asset_path(root, path)?;
                // Held across the write and the row upsert, the file lock
                // before the store lock like every other writer here. See
                // `Engine::write_lock`.
                let lock = self.write_lock(&abs);
                let _guard = lock.lock().await;
                write_bytes(&abs, &bytes)?;
                // The modification instant is read back off the file rather
                // than taken from the clock, so it is the same value the sync
                // walker's stat prefilter compares against and an upload costs
                // no re-hash on the next scan.
                let row = attachment_row(path, &bytes, asset_modified(&abs))?;
                let store = self.store.lock().await;
                store.upsert_attachment(domain_id, &row).await?;
                row
            }
            ContentSource::Virtual => {
                let row = attachment_row(path, &bytes, Utc::now().to_rfc3339())?;
                let store = self.store.lock().await;
                // What was there before this write, so a failure can put it
                // back rather than approximate it.
                let previous = store.get_attachment(domain_id, path).await?;
                store.upsert_attachment(domain_id, &row).await?;
                // A failed blob write must leave the domain exactly as it found
                // it, which is two different things depending on what was
                // there. A replace: the upsert above moved the row's metadata
                // only - the blob belongs to the row and still holds the older
                // bytes - so restoring the recorded row restores the whole
                // attachment, and deleting instead would destroy an attachment
                // this write never got to replace. A create: nothing was there,
                // so the row this write inserted goes with it, or a listing
                // would advertise bytes that were never stored. Best effort
                // either way (the next write, or a scan of the file domain's
                // twin, reconciles), and the original error is what the caller
                // sees. A file domain needs none of this: the temp file is
                // renamed into place only on success, so a failed write leaves
                // the old file untouched.
                if let Err(e) = store.write_attachment_blob(domain_id, path, &bytes).await {
                    let undone = match &previous {
                        Some(prior) => store.upsert_attachment(domain_id, prior).await.err(),
                        None => store.delete_attachment(domain_id, path).await.err(),
                    };
                    if let Some(failed) = undone {
                        tracing::warn!(
                            "attachment '{path}' in '{domain}' could not be rolled back after a failed blob write: {failed}"
                        );
                    }
                    return Err(e.into());
                }
                row
            }
        };
        crate::maintenance::record_pending(domain);
        Ok(row)
    }

    /// Remove one attachment: the file or the blob, and the row.
    ///
    /// [`EngineError::NotFound`] when neither was there, so a caller can answer
    /// a miss. A file that is gone while its row stands (or the reverse, after
    /// a hand-edited domain) still counts as a delete: whichever half existed
    /// is removed and the pair ends up consistent.
    pub async fn attachment_delete(&self, domain: &str, path: &str) -> Result<()> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        validate_attachment_path(path)?;
        let (domain_id, source) = self.domain_source(domain).await?;
        let mut file_removed = false;
        if let ContentSource::File { root } = &source {
            let abs = contained_asset_path(root, path)?;
            let lock = self.write_lock(&abs);
            let _guard = lock.lock().await;
            match std::fs::remove_file(&abs) {
                Ok(()) => file_removed = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(EngineError::Io {
                        path: abs.display().to_string(),
                        source,
                    });
                }
            }
        }
        let row_removed = {
            let store = self.store.lock().await;
            store.delete_attachment(domain_id, path).await?
        };
        if !file_removed && !row_removed {
            return Err(EngineError::NotFound(missing_attachment(domain, path)));
        }
        crate::maintenance::record_pending(domain);
        Ok(())
    }

    // --- attachments a cross-domain move carries ------------------------------
    //
    // An `assets/` reference is domain-root relative, so it survives a rename
    // inside its own domain untouched and means nothing at all in another
    // domain. A cross-domain move therefore has to bring the files with the
    // engram, which is three questions asked before a byte moves: which
    // attachments does the moving engram actually use, does anything else in
    // the source still need them (copy) or not (move), and is the name they
    // arrive under free at the destination.

    /// The most `-N` suffixes a colliding attachment name is offered before
    /// the move leaves it where it is. A destination holding ninety-nine
    /// different files under one name is a domain with a problem an automatic
    /// rename would only deepen.
    const MAX_ASSET_SUFFIX: usize = 99;

    /// What the cross-domain move owes each attachment the moving engram uses:
    /// where it lands, whether the bytes are already there, and whether the
    /// source copy stays behind.
    ///
    /// Settled before anything is written, because the destination names it
    /// picks are what the engram's body references and `analyzes` claim are
    /// rewritten to, and that rewrite has to travel in the same write that
    /// lands the engram at its destination.
    ///
    /// Every miss is quiet rather than fatal: a reference to a file that is not
    /// there is already a dangling reference, and a move is not the verb that
    /// should refuse over one. Quiet is not silent, though - each miss is
    /// returned beside the plan as a sentence the move receipt carries, so the
    /// caller learns which attachment stayed behind without having to read the
    /// daemon's trace.
    async fn plan_attachment_carry(
        &self,
        src: &EngramDescriptor,
        dest_domain: &str,
        content: &str,
    ) -> (Vec<AttachmentCarry>, Vec<String>) {
        let candidates = referenced_asset_paths(content);
        if candidates.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let mut warnings: Vec<String> = Vec::new();

        // The bytes are read and dropped here: what the plan needs is the
        // sha256, and the read is what makes the row's sha describe the file
        // that is actually on disk. Reading them again in the carry itself
        // costs one more read of one attachment and keeps the peak at a single
        // attachment rather than at everything the engram references.
        let mut present: Vec<(String, String)> = Vec::new();
        for path in candidates {
            match self.attachment_read(&src.domain, &path).await {
                Ok((_, row)) => present.push((path, row.sha256)),
                Err(e) => {
                    // Loud enough to answer "why did my screenshot not
                    // travel": the move went through, but something the engram
                    // points at did not come with it. The trace keeps the store
                    // error, which is an operator's detail; the receipt gets
                    // the same sentence without it.
                    let warning = attachment_missing_warning(&path, &src.permalink, &src.domain);
                    tracing::warn!("{warning} ({e})");
                    warnings.push(warning);
                }
            }
        }
        if present.is_empty() {
            return (Vec::new(), warnings);
        }

        let paths: Vec<String> = present.iter().map(|(path, _)| path.clone()).collect();
        let shared = self.shared_asset_paths(src, &paths).await;
        let mut claimed: HashSet<String> = HashSet::new();
        let mut plan = Vec::new();
        for (from, sha) in present {
            let Some((to, reuse)) = self
                .free_asset_destination(dest_domain, &from, &sha, &claimed)
                .await
            else {
                // The file is whole and still in the source domain, which is
                // the part that matters; what the caller cannot see without
                // being told is that the reference travelling with the engram
                // now points at whatever the destination happens to hold under
                // that name.
                warnings.push(attachment_not_carried_warning(&from, dest_domain));
                continue;
            };
            claimed.insert(to.clone());
            plan.push(AttachmentCarry {
                shared: shared.contains(&from),
                from,
                to,
                reuse,
            });
        }
        (plan, warnings)
    }

    /// The attachment paths another engram in the source domain still
    /// references or claims, with every failure resolved the safe way by
    /// [`resolve_shared`].
    ///
    /// Counted across live and retired engrams alike, the way the
    /// consolidation sweep counts referents: a deprecated engram showing a
    /// screenshot needs the file exactly as much as a current one does, so its
    /// reference is what turns a move into a copy.
    async fn shared_asset_paths(
        &self,
        src: &EngramDescriptor,
        candidates: &[String],
    ) -> HashSet<String> {
        resolve_shared(
            self.count_shared_asset_paths(src, candidates).await,
            candidates,
        )
    }

    /// The counting itself: which candidates another engram in `src`'s domain
    /// still references or claims, or the first failure that stopped the count
    /// from answering.
    ///
    /// Engrams are read one at a time rather than in one batch, because a
    /// domain can hold multi-megabyte engrams and this runs on an ordinary
    /// move; the screen keeps the parse to the engrams that could possibly
    /// match, and the scan stops as soon as every candidate is accounted for.
    ///
    /// The screen tests the path *below* the reserved folder (`shot.png` for
    /// `assets/shot.png`) rather than the whole path, which makes it strictly
    /// wider than the decision it protects: a body reference must spell the
    /// folder as `assets/` to be a reference at all, and a claim is folded to
    /// that spelling when it is read, so an engram claiming `Assets/shot.png`
    /// (the same folder on APFS and NTFS) is screened in and then decided
    /// exactly. A narrower screen would let a live claim lose its file.
    ///
    /// Nothing here answers "not referenced" on a failure. A store error is
    /// returned, and text that will not parse marks every candidate the screen
    /// matched in it as referenced: the engram plainly mentions the path and
    /// the only reading that cannot delete something in use is that it uses
    /// it.
    async fn count_shared_asset_paths(
        &self,
        src: &EngramDescriptor,
        candidates: &[String],
    ) -> Result<HashSet<String>> {
        let mut shared: HashSet<String> = HashSet::new();
        let source = self.content_source(&src.domain)?;
        let others = {
            let store = self.store.lock().await;
            store.list_engrams(&src.domain, None, None).await?
        };
        for other in others {
            if shared.len() == candidates.len() {
                break;
            }
            if other.path == src.path {
                continue;
            }
            let Some(text) = self
                .peer_engram_text(&source, src.domain_id, &other.path)
                .await?
            else {
                // Whatever the listing knew about, its text is not there any
                // more, and text that is gone references nothing. Only a real
                // read failure counts as not knowing, and that is an `Err`.
                continue;
            };
            let screened: Vec<&String> = candidates
                .iter()
                .filter(|path| text.contains(asset_tail(path)))
                .collect();
            if screened.is_empty() {
                continue;
            }
            let Ok(engram) = parse_engram(&text) else {
                for candidate in screened {
                    shared.insert(candidate.clone());
                }
                continue;
            };
            let refs = crystalline_core::find_asset_refs(&engram.body);
            let claim = asset_claim(&engram.frontmatter);
            for candidate in screened {
                if refs.contains(candidate) || claim.as_deref() == Some(candidate.as_str()) {
                    shared.insert(candidate.clone());
                }
            }
        }
        Ok(shared)
    }

    /// One peer engram's whole text, frontmatter included, for the referent
    /// count.
    ///
    /// Deliberately not [`Store::engram_content`] alone. The index keeps only
    /// the *body* for a file domain (`EngramRecord::from_engram` sets
    /// `content` to `engram.body`; the virtual write path is the one that
    /// stores the whole source), and an `analyzes` claim lives in the
    /// frontmatter - so counting off the index alone would never see a claim
    /// in a file domain and would delete a claimed attachment out from under
    /// the engram that claimed it. A file domain is therefore read from its
    /// files, which is where its frontmatter actually is, and a virtual domain
    /// from the database, which is where its whole engram actually is.
    ///
    /// That is one file read per engram in the domain, on a cross-domain move
    /// that carries attachments and on the confirmation preview of a delete
    /// whose engram references one ([`Engine::sole_referent_attachments`]). It
    /// is the price of counting claims at all, both verbs are rare, and an
    /// engram that references no attachment pays none of it.
    ///
    /// `None` when the text is genuinely absent (a row the index still lists
    /// for a file that is gone): text that is not there references nothing. A
    /// read that fails for any other reason is an `Err`, which
    /// [`resolve_shared`] turns into "still referenced".
    async fn peer_engram_text(
        &self,
        source: &ContentSource,
        domain_id: DomainId,
        path: &str,
    ) -> Result<Option<String>> {
        match source {
            ContentSource::File { root } => {
                let abs = join_rel(root, path);
                match std::fs::read_to_string(&abs) {
                    Ok(text) => Ok(Some(text)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(source) => Err(EngineError::Io {
                        path: abs.display().to_string(),
                        source,
                    }),
                }
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                Ok(store.engram_content(domain_id, path).await?)
            }
        }
    }

    /// The path an attachment takes at the destination, and whether the
    /// destination already holds exactly these bytes there.
    ///
    /// Its own name when nothing holds it; its own name with nothing to write
    /// when the file already there has the same sha256 (same name, same bytes,
    /// same file); `name-2.ext`, `name-3.ext` and so on when something
    /// different holds it, since a move must never overwrite a file the
    /// destination domain already had. Every suffixed name is put back through
    /// [`crystalline_core::validate_asset_path`], so the name the engram's
    /// references are rewritten to is always a name a write will accept.
    ///
    /// `None` when no free, valid name could be settled or the destination
    /// could not be inspected. That is the fallback that keeps everything
    /// referenced: the attachment is simply not carried, so nothing is written
    /// at the destination, nothing is deleted at the source and the moving
    /// engram's references keep the spelling they had. The whole file stays in
    /// the source domain under the name that still addresses it there, and the
    /// destination is left with a plain dangling reference for the sweep to
    /// report - the same outcome an already-missing attachment produces, and
    /// the one thing that cannot happen is a reference rewritten to a name
    /// nothing will ever accept.
    async fn free_asset_destination(
        &self,
        dest_domain: &str,
        path: &str,
        sha: &str,
        claimed: &HashSet<String>,
    ) -> Option<(String, bool)> {
        for attempt in 1..=Self::MAX_ASSET_SUFFIX {
            let candidate = if attempt == 1 {
                path.to_string()
            } else {
                // A stem that cannot be shortened into a valid name at all
                // cannot be shortened into a valid longer-suffixed one either,
                // so this ends the search rather than skipping an attempt.
                suffixed_asset_path(path, attempt)?
            };
            // A name another attachment in this same move already took is
            // occupied even though nothing is written there yet.
            if claimed.contains(&candidate) {
                continue;
            }
            match self.attachment_read(dest_domain, &candidate).await {
                Ok((_, row)) if row.sha256 == sha => return Some((candidate, true)),
                Ok(_) => {}
                Err(EngineError::NotFound(_)) => return Some((candidate, false)),
                Err(e) => {
                    // The attachment stays whole where it is, so this is a
                    // note for whoever is reading the trace rather than
                    // something the user lost.
                    tracing::debug!(
                        "'{candidate}' in '{dest_domain}' could not be inspected ({e}); '{path}' stays where it is"
                    );
                    return None;
                }
            }
        }
        tracing::warn!(
            "'{path}' collides with {} different files in '{dest_domain}'; it stays where it is",
            Self::MAX_ASSET_SUFFIX
        );
        None
    }

    /// Carry out the planned attachment moves and copies.
    ///
    /// Runs after the engram itself has landed and never turns a failure into
    /// a failed move: the engram is already where it was asked to be, and
    /// every failure here leaves the source copy in place, so the worst
    /// outcome is a reference the destination cannot resolve yet - which the
    /// consolidation sweep reports as a dangling attachment rather than
    /// something a move should have refused over. Both domains are marked
    /// pending by the writes and deletes themselves.
    async fn carry_attachments(
        &self,
        src_domain: &str,
        dest_domain: &str,
        plan: &[AttachmentCarry],
    ) {
        for carry in plan {
            if !carry.reuse {
                let bytes = match self.attachment_read(src_domain, &carry.from).await {
                    Ok((bytes, _)) => bytes,
                    Err(e) => {
                        tracing::warn!(
                            "attachment '{}' could not be read out of '{src_domain}' for the move: {e}",
                            carry.from
                        );
                        continue;
                    }
                };
                if let Err(e) = self.attachment_write(dest_domain, &carry.to, bytes).await {
                    tracing::warn!(
                        "attachment '{}' could not be written into '{dest_domain}' as '{}': {e}",
                        carry.from,
                        carry.to
                    );
                    continue;
                }
            }
            // Only now, with the bytes proven to be at the destination, does
            // the source copy go - and only when nothing there still uses it.
            if !carry.shared
                && let Err(e) = self.attachment_delete(src_domain, &carry.from).await
            {
                tracing::warn!(
                    "attachment '{}' was carried into '{dest_domain}' but could not be removed from '{src_domain}': {e}",
                    carry.from
                );
            }
        }
    }

    /// The retirement statuses [`Engine::retire_engram`] accepts. Any other
    /// status is this verb's business to refuse, not a global rule: the
    /// ordinary save and edit paths accept any status string.
    const RETIREMENT_STATUSES: [&str; 3] = ["deprecated", "superseded", "archived"];

    /// Guided retirement: set a retirement `status`, optionally close out
    /// `valid_to`, and, for `superseded`, wire the supersede pair as body
    /// relations so verify's T005 and the evolve sweep see a reciprocal link
    /// rather than a dangling one.
    pub async fn retire_engram(&self, p: &RetireParams) -> Result<Value> {
        self.retire_engram_as(p, None).await
    }

    /// [`Engine::retire_engram`] with the retiring identity, resolved by
    /// [`Engine::actor`] and stamped into both engrams' `generated` block.
    ///
    /// Everything is validated and resolved before anything is written: the
    /// status is checked against [`Self::RETIREMENT_STATUSES`], the
    /// successor rule (required for `superseded`, refused otherwise) is
    /// enforced, `valid_to` is parsed and, when a successor is named, it is
    /// resolved in the same domain so a missing successor is `NotFound`
    /// before the target is touched. The target is then written first, and
    /// only then the successor's reciprocal `- supersedes [[..]]` line
    /// (appended only when not already present, so a repeat call is
    /// idempotent). A failure on the successor write leaves the target
    /// retired with a one-sided pair; nothing here rolls that back, since the
    /// evolve sweep already flags a `superseded_by` with no matching
    /// `supersedes` as its own finding.
    pub async fn retire_engram_as(&self, p: &RetireParams, client: Option<&str>) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if !Self::RETIREMENT_STATUSES.contains(&p.status.as_str()) {
            return Err(EngineError::Invalid(format!(
                "retire_engram accepts status deprecated, superseded or archived, got '{}'; \
                 use edit_engram's set_frontmatter operation for any other status",
                p.status
            )));
        }
        match (p.status.as_str(), p.successor.is_some()) {
            ("superseded", false) => {
                return Err(EngineError::Invalid(
                    "status superseded needs a successor to wire the supersede pair, \
                     or verify rule T005 flags the result as a dangling retirement"
                        .into(),
                ));
            }
            (other, true) if other != "superseded" => {
                return Err(EngineError::Invalid(format!(
                    "successor is only accepted when status is superseded, not '{other}'"
                )));
            }
            _ => {}
        }
        let valid_to = p
            .valid_to
            .as_deref()
            .map(|raw| {
                NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
                    EngineError::Invalid(format!(
                        "valid_to must be a plain ISO date (YYYY-MM-DD), got '{raw}'"
                    ))
                })
            })
            .transpose()?;

        let actor = self.actor(client);
        let (desc, source) = self.resolve(&p.identifier, Some(&p.domain)).await?;

        // Resolved before the target is touched: a missing successor must
        // never leave the target half-retired.
        let successor = match &p.successor {
            Some(identifier) => Some(self.resolve(identifier, Some(&p.domain)).await?),
            None => None,
        };
        // A successor that resolves to the target itself would append a
        // supersedes-self relation: no deadlock (the target's lock is
        // released before the successor's is taken), just a nonsense pair
        // that verify would then have to make sense of. Refused before
        // anything is written rather than left to produce that pair.
        if let Some((succ_desc, _)) = &successor
            && succ_desc.id == desc.id
        {
            return Err(EngineError::Invalid(format!(
                "successor '{}' resolves to the same engram being retired; \
                 a retirement needs a different engram to supersede it",
                p.successor.as_deref().unwrap_or_default()
            )));
        }
        let successor_title = successor.as_ref().map(|(d, _)| d.title.clone());

        // -- target: status, optional valid_to, optional superseded_by line --
        match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &desc.path);
                // Held across the read, the retirement edit and the write, for
                // the reason `edit_engram_as` gives: this is a read-modify-write
                // with nothing to refuse a concurrent change on, so serializing
                // is what stops one from being dropped. See `Engine::write_lock`.
                let lock = self.write_lock(&abs);
                let _guard = lock.lock().await;
                let current = std::fs::read_to_string(&abs).map_err(|source| EngineError::Io {
                    path: abs.display().to_string(),
                    source,
                })?;
                let edited = Self::build_retirement_edit(
                    &current,
                    &p.status,
                    valid_to,
                    successor_title.as_deref(),
                    &actor,
                );
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
                let edited = Self::build_retirement_edit(
                    &current,
                    &p.status,
                    valid_to,
                    successor_title.as_deref(),
                    &actor,
                );
                let edited = Self::enforce_temporal(edited)?;
                let stamp = virtual_stamp(&edited);
                let store = self.store.lock().await;
                self.index_markdown(
                    &*store,
                    desc.domain_id,
                    &desc.path,
                    &edited,
                    stamp,
                    None,
                    true,
                )
                .await?;
            }
        }
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }
        self.refresh_index_files(&desc.domain).await;

        // -- successor: reciprocal supersedes line, appended once --
        if let Some((succ_desc, succ_source)) = &successor {
            let line = format!("- supersedes [[{}]]", desc.title);
            match succ_source {
                ContentSource::File { root } => {
                    let abs = join_rel(root, &succ_desc.path);
                    // The successor's own file, under its own lock: appending
                    // the reciprocal line is another read-modify-write. Taken
                    // after the target's has been released, never with it, so
                    // two retirements naming each other cannot deadlock.
                    let lock = self.write_lock(&abs);
                    let _guard = lock.lock().await;
                    let current =
                        std::fs::read_to_string(&abs).map_err(|source| EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        })?;
                    if !current.contains(&line) {
                        let edited =
                            touch_generated(&append_body(&current, &line), &actor, now_offset());
                        write_file(&abs, &edited)?;
                        let store = self.store.lock().await;
                        self.reindex_file(&*store, succ_desc.domain_id, root, &succ_desc.path)
                            .await?;
                    }
                }
                ContentSource::Virtual => {
                    let current = {
                        let store = self.store.lock().await;
                        store
                            .engram_content(succ_desc.domain_id, &succ_desc.path)
                            .await?
                            .ok_or_else(|| {
                                EngineError::NotFound(format!(
                                    "no content stored for '{}' in domain '{}'",
                                    succ_desc.permalink, succ_desc.domain
                                ))
                            })?
                    };
                    if !current.contains(&line) {
                        let edited =
                            touch_generated(&append_body(&current, &line), &actor, now_offset());
                        let stamp = virtual_stamp(&edited);
                        let store = self.store.lock().await;
                        self.index_markdown(
                            &*store,
                            succ_desc.domain_id,
                            &succ_desc.path,
                            &edited,
                            stamp,
                            None,
                            true,
                        )
                        .await?;
                    }
                }
            }
            if matches!(succ_source, ContentSource::Virtual) {
                self.refresh_routing_cache().await;
            }
            self.refresh_index_files(&succ_desc.domain).await;
        }

        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "status": p.status,
            "successor": successor.map(|(d, _)| d.permalink),
        }))
    }

    /// Build the target engram's retirement edit: set `status`, set
    /// `valid_to` when given, append the `superseded_by` relation when a
    /// successor title is given and the line is not already there, then
    /// stamp `generated` provenance. Shared by the file and virtual arms of
    /// [`Engine::retire_engram_as`]. The `contains` guard, checked against
    /// `current` rather than the frontmatter-edited text (the two never
    /// disagree on body content), matches the successor side's guard so a
    /// retry after a timeout, say, retires idempotently instead of
    /// duplicating the relation.
    fn build_retirement_edit(
        current: &str,
        status: &str,
        valid_to: Option<NaiveDate>,
        successor_title: Option<&str>,
        actor: &str,
    ) -> String {
        let mut edited = set_frontmatter_field(current, "status", status);
        if let Some(date) = valid_to {
            edited =
                set_frontmatter_field(&edited, "valid_to", &date.format("%Y-%m-%d").to_string());
        }
        if let Some(title) = successor_title {
            let line = format!("- superseded_by [[{title}]]");
            if !current.contains(&line) {
                edited = append_body(&edited, &line);
            }
        }
        touch_generated(&edited, actor, now_offset())
    }

    // --- read ----------------------------------------------------------------

    /// One engram's exact file text and identity: what the collab session
    /// layer loads at open and probes with on its idle external-change check.
    /// Deliberately thin - [`Engine::read_engram`] resolves references and
    /// builds hints this caller never reads.
    pub async fn engram_text(&self, domain: &str, identifier: &str) -> Result<EngramText> {
        let (desc, source) = self.resolve(identifier, Some(domain)).await?;
        let content = self.load_content(&source, &desc).await?;
        let checksum = sha256_hex(content.as_bytes());
        Ok(EngramText {
            domain: desc.domain,
            permalink: desc.permalink,
            path: desc.path,
            content,
            checksum,
        })
    }

    /// The exact text a domain holds at a domain-relative PATH right now, or
    /// `None` when nothing is there.
    ///
    /// Path-addressed on purpose, and the counterpart of
    /// [`Engine::restore_engram`]: a collab room whose engram vanished from
    /// the index has no identifier left to resolve, and before it puts its own
    /// text back it has to know whether somebody else's bytes are sitting at
    /// that path (an external rename, or a delete followed by a recreate).
    /// The `permalink` reported back is what the index answers for this path,
    /// which an external rename may just have moved; it falls back to the path
    /// slug when no row holds the path any more.
    pub async fn engram_text_at_path(
        &self,
        domain: &str,
        path: &str,
    ) -> Result<Option<EngramText>> {
        let source = self.content_source(domain)?;
        let content = match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, path);
                match std::fs::read_to_string(&abs) {
                    Ok(text) => text,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                }
            }
            ContentSource::Virtual => {
                let (domain_id, _) = self.domain_source(domain).await?;
                let store = self.store.lock().await;
                match store.engram_content(domain_id, path).await? {
                    Some(text) => text,
                    None => return Ok(None),
                }
            }
        };
        let permalink = {
            let store = self.store.lock().await;
            store
                .list_engrams(domain, Some(path), None)
                .await?
                .into_iter()
                .find(|found| found.path == path)
                .map(|found| found.permalink)
                .unwrap_or_else(|| path.trim_end_matches(".md").to_string())
        };
        Ok(Some(EngramText {
            domain: domain.to_string(),
            permalink,
            path: path.to_string(),
            checksum: sha256_hex(content.as_bytes()),
            content,
        }))
    }

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

        // Enrich the response with reference resolution: which outbound links
        // land, and who points back in. The descriptor carries the ids, so this
        // works for file, virtual and non-host reads alike.
        let (outbound, inbound) = {
            let store = self.store.lock().await;
            let outbound = store.outbound_refs(desc.id).await?;
            let inbound = store
                .inbound_refs(desc.id, desc.domain_id, &desc.permalink, &desc.title)
                .await?;
            (outbound, inbound)
        };

        // A parsed reference resolves when a matching indexed row (same source
        // line, kind and target) is resolved. An unmatched parsed entry, which a
        // just-edited or non-host read can produce, is reported as unresolved.
        let resolves = |kind: EdgeKind, line: usize, target: &LinkTarget| -> bool {
            outbound.iter().any(|o| {
                o.kind == kind
                    && o.line == line
                    && o.to_target == target.target
                    && o.to_domain == target.domain
                    && o.resolved
            })
        };

        #[derive(serde::Serialize)]
        struct RelationOut<'a> {
            line: usize,
            rel_type: &'a str,
            target: &'a LinkTarget,
            resolved: bool,
        }
        #[derive(serde::Serialize)]
        struct LinkOut<'a> {
            line: usize,
            target: &'a LinkTarget,
            resolved: bool,
        }

        let relations: Vec<RelationOut> = engram
            .relations
            .iter()
            .map(|r| RelationOut {
                line: r.line,
                rel_type: &r.rel_type,
                target: &r.target,
                resolved: resolves(EdgeKind::Relation, r.line, &r.target),
            })
            .collect();
        let links: Vec<LinkOut> = engram
            .links
            .iter()
            .map(|l| LinkOut {
                line: l.line,
                target: &l.target,
                resolved: resolves(EdgeKind::Link, l.line, &l.target),
            })
            .collect();
        let resolved_outbound = relations.iter().filter(|r| r.resolved).count()
            + links.iter().filter(|l| l.resolved).count();

        let url = format!("crystalline://{}/{}", desc.domain, desc.permalink);
        let mut value = json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "title": desc.title,
            "type": desc.engram_type,
            "status": desc.status,
            "path": desc.path,
            "url": url,
            "content": content,
            "checksum": checksum,
            "frontmatter": engram.frontmatter,
            "observations": engram.observations,
            "relations": relations,
            "links": links,
        });
        let obj = value
            .as_object_mut()
            .expect("read_engram response is a JSON object");

        // Inbound summary: how many references point here, with a small capped
        // sample so a heavily linked engram never bloats the response. Omitted
        // entirely when nothing points here.
        if !inbound.is_empty() {
            let refs: Vec<Value> = inbound
                .iter()
                .take(5)
                .map(|r| {
                    json!({
                        "domain": r.src_domain,
                        "path": r.src_path,
                        "kind": match r.kind {
                            EdgeKind::Relation => "relation",
                            EdgeKind::Link => "link",
                        },
                    })
                })
                .collect();
            obj.insert(
                "inbound".to_string(),
                json!({ "count": inbound.len(), "refs": refs }),
            );
        }

        // A build_context hint, emitted only when there is a neighbourhood to
        // explore: something points here, or something here resolves outward.
        if !inbound.is_empty() || resolved_outbound > 0 {
            obj.insert(
                "related".to_string(),
                json!(format!(
                    "build_context anchor {url} to explore linked knowledge"
                )),
            );
        }

        Ok(value)
    }

    /// One page of what points at an engram, with the per-relation summary of
    /// all of it: the browsing view of the inbound block [`Engine::read_engram`]
    /// samples.
    ///
    /// The read payload's `inbound` stays what it is - an exact count and five
    /// references, cheap enough to ride every read. This is for the case that
    /// count implies but cannot serve: hundreds or thousands of engrams pointing
    /// at one, where the answer is a map to browse rather than a list to print.
    /// Both are the same rows, so the counts agree.
    ///
    /// `q` matches the referencing engram's title or path, case-insensitively,
    /// and `rel` narrows to one relation type (`links_to` for prose wikilinks).
    /// `total` is exact under both; `types` ignores both, because a summary that
    /// shrank as it was used would be a map redrawing itself while it is read.
    ///
    /// `limit` is clamped to [`MAX_INBOUND_LIMIT`] and a page past the end is an
    /// empty page carrying the true total, never the first page's rows.
    ///
    /// An engram nobody wrote is [`EngineError::NotFound`], the same resolution
    /// every other read of one identifier opens with.
    pub async fn inbound_references(
        &self,
        p: &ReadParams,
        q: Option<&str>,
        rel: Option<&str>,
        page: Option<usize>,
        limit: Option<usize>,
    ) -> Result<Value> {
        let (desc, _) = self.resolve(&p.identifier, p.domain.as_deref()).await?;
        // Clamped rather than refused, the way the listing clamps its own: a
        // hand-written page number below one is answered with the first page,
        // and a page size past [`MAX_INBOUND_LIMIT`] is answered with that
        // many. The envelope reports the clamped values, so a caller is told
        // what it was actually given rather than having its own number read
        // back at it.
        let page = page.unwrap_or(1).max(1);
        let limit = limit.unwrap_or(10).clamp(1, MAX_INBOUND_LIMIT);
        let found = {
            let store = self.store.lock().await;
            store
                .inbound_page(&InboundQuery {
                    engram_id: desc.id,
                    domain_id: desc.domain_id,
                    permalink: &desc.permalink,
                    title: &desc.title,
                    q,
                    rel,
                    page,
                    limit,
                })
                .await?
        };
        let types: Vec<Value> = found
            .types
            .iter()
            .map(|t| json!({ "rel": t.name, "count": t.count }))
            .collect();
        let hits: Vec<Value> = found
            .hits
            .iter()
            .map(|h| {
                json!({
                    "domain": h.domain,
                    "permalink": h.permalink,
                    "title": h.title,
                    "path": h.path,
                    "status": h.status,
                    "rel": h.rel,
                })
            })
            .collect();
        Ok(json!({
            "total": found.total,
            "page": page,
            "limit": limit,
            "count": hits.len(),
            "types": types,
            "hits": hits,
        }))
    }

    // --- edit ----------------------------------------------------------------

    /// Apply a surgical edit to an engram, then reindex it. A file domain edits
    /// the file on disk and reindexes it; a virtual domain reads the current
    /// content from the database, applies the same edit and writes it back under
    /// a compare-and-swap guard so a stale edit is refused rather than silently
    /// clobbering a concurrent change (see `expected_checksum`).
    pub async fn edit_engram(&self, p: &EditParams) -> Result<Value> {
        self.edit_engram_as(p, None).await
    }

    /// [`Engine::edit_engram`] with the editor's identity, resolved by
    /// [`Engine::actor`] and written into the engram's `generated` block. An
    /// engram that still carries the legacy `timestamp` key migrates to
    /// `generated` here, on its next edit.
    pub async fn edit_engram_as(&self, p: &EditParams, client: Option<&str>) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let actor = self.actor(client);
        let (desc, source) = self.resolve(&p.identifier, Some(&p.domain)).await?;
        // An `evolve_ack` assignment is the one set_frontmatter key whose value
        // the server completes rather than takes: the scope comes from running
        // detection over this engram's domain, which needs the store and so
        // cannot happen inside the pure text edit below. Computed before the
        // write lock is taken, so a sweep never runs while a file is held.
        let ack = self.ack_draft(p, &desc, &actor).await?;

        self.apply_source_edit(
            &desc,
            &source,
            p.expected_checksum.as_deref(),
            &actor,
            |current| self.apply_edit(current, p, &desc.permalink, &actor, ack.as_ref()),
        )
        .await?;

        let mut response = json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "path": desc.path,
            "operation": p.operation,
        });
        match &ack {
            Some(AckDraft::Record(entry)) => response["evolve_ack"] = ack_json(entry),
            Some(AckDraft::Remove(rule)) => response["evolve_ack_removed"] = json!(rule),
            None => {}
        }
        Ok(response)
    }

    /// Read an engram's source, hand it to `apply`, and write the result back:
    /// the shared body of every edit that rewrites content in place.
    ///
    /// Kind-agnostic and lock-correct, which is why it is one function rather
    /// than repeated per caller. For a file domain the write lock is held
    /// across the read, the compare, the edit and the write; without an
    /// `expected_checksum` that serialization is the whole guarantee, since two
    /// unguarded edits must each apply to what the other wrote rather than
    /// silently dropping it. For a virtual domain the store's own compare and
    /// swap plays that part, with the checksum of what was just read standing
    /// in when the caller presents none.
    async fn apply_source_edit<F>(
        &self,
        desc: &EngramDescriptor,
        source: &ContentSource,
        expected_checksum: Option<&str>,
        actor: &str,
        apply: F,
    ) -> Result<()>
    where
        F: FnOnce(&str) -> Result<String>,
    {
        match source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &desc.path);
                let lock = self.write_lock(&abs);
                let _guard = lock.lock().await;
                let current = std::fs::read_to_string(&abs).map_err(|source| EngineError::Io {
                    path: abs.display().to_string(),
                    source,
                })?;
                // The CAS token, when the caller presents one: compared inside
                // the lock, against the bytes just read, exactly as save_engram
                // compares.
                if let Some(expected) = expected_checksum {
                    let found = sha256_hex(current.as_bytes());
                    if found != expected {
                        return Err(EngineError::Conflict(stale_edit_message(expected, &found)));
                    }
                }
                let edited = apply(&current)?;
                let edited = touch_generated(&edited, actor, now_offset());
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
                let expected = expected_checksum
                    .map(str::to_string)
                    .unwrap_or_else(|| sha256_hex(current.as_bytes()));
                let edited = apply(&current)?;
                let edited = touch_generated(&edited, actor, now_offset());
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
        // An edit can change the title or the description the folder's
        // generated index lists this engram under.
        self.refresh_index_files(&desc.domain).await;
        Ok(())
    }

    /// Apply one edit operation to an engram's markdown, returning the edited
    /// text. Content-agnostic: the same logic serves file and virtual edits.
    /// `actor` is the resolved editor identity, which `set_frontmatter` stamps
    /// into a verification when the caller names no other one.
    fn apply_edit(
        &self,
        source: &str,
        p: &EditParams,
        permalink: &str,
        actor: &str,
        ack: Option<&AckDraft>,
    ) -> Result<String> {
        Ok(match p.operation.as_str() {
            "append" => append_body(source, self.require_content(p)?),
            "prepend" => prepend_body(source, self.require_content(p)?),
            "find_replace" => {
                let content = self.require_content(p)?;
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
                source.replace(find, content)
            }
            "replace_section" => {
                let content = self.require_content(p)?;
                let section = self.require_section(p)?;
                replace_section(source, section, content, p.include_subsections)
                    .map_err(section_err)?
            }
            "insert_before_section" => {
                let content = self.require_content(p)?;
                let section = self.require_section(p)?;
                insert_before_section(source, section, content).map_err(section_err)?
            }
            "insert_after_section" => {
                let content = self.require_content(p)?;
                let section = self.require_section(p)?;
                insert_after_section(source, section, content).map_err(section_err)?
            }
            "set_frontmatter" => Self::apply_set_frontmatter(source, p, permalink, actor, ack)?,
            other => {
                return Err(EngineError::Invalid(format!(
                    "unknown edit operation '{other}'; expected append, prepend, find_replace, replace_section, insert_before_section, insert_after_section or set_frontmatter"
                )));
            }
        })
    }

    /// Assign or clear one lifecycle frontmatter field, the `set_frontmatter`
    /// operation. Restricted to [`SETTABLE_FRONTMATTER_KEYS`]: identity,
    /// provenance and index keys are owned by the tools that maintain them, so
    /// rewriting one here is refused rather than silently corrupting the
    /// engram's address or its write history.
    ///
    /// An absent or empty value clears the field, except on `status`, which is
    /// required, and on `verified`, which stamps a verification instead.
    fn apply_set_frontmatter(
        source: &str,
        p: &EditParams,
        permalink: &str,
        actor: &str,
        ack: Option<&AckDraft>,
    ) -> Result<String> {
        let key = p
            .key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                EngineError::Invalid(format!(
                    "set_frontmatter requires key, one of {}",
                    settable_keys()
                ))
            })?;
        let value = p.value.as_deref().map(str::trim).filter(|v| !v.is_empty());

        match key {
            "status" => {
                let status = value.ok_or_else(|| {
                    EngineError::Invalid(
                        "status cannot be removed: every engram needs one (verify rule T001). Set a retirement status such as deprecated or superseded instead".into(),
                    )
                })?;
                Ok(set_frontmatter_field(source, "status", status))
            }
            "valid_from" | "valid_to" | "stale_after" | "source_date" => {
                let Some(raw) = value else {
                    // Clearing a bound is how absence - always valid, valid
                    // forever, no review due - is restored. The legacy
                    // `review_after` spelling goes with `stale_after` so the
                    // bound is really gone whichever spelling the file used.
                    let out = remove_frontmatter_field(source, key);
                    return Ok(if key == "stale_after" {
                        remove_frontmatter_field(&out, "review_after")
                    } else {
                        out
                    });
                };
                // Validate through the write contract itself rather than a
                // second parser, so a timestamp, an int or a sentinel bound is
                // answered here exactly as write_engram answers it.
                let mut probe = Frontmatter::default();
                probe
                    .extra
                    .insert(key.to_string(), YamlValue::String(raw.to_string()));
                let dropped = crystalline_core::temporal::normalize_temporal_fields(&mut probe)
                    .map_err(|e| EngineError::Invalid(e.to_string()))?;
                if !dropped.is_empty() {
                    // A sentinel bound: absence is how open-ended validity is
                    // expressed, so the field is cleared rather than written.
                    return Ok(remove_frontmatter_field(source, key));
                }
                let date = match key {
                    "valid_from" => probe.valid_from,
                    "valid_to" => probe.valid_to,
                    "source_date" => probe.source_date,
                    _ => probe.stale_after,
                }
                .expect("a normalized date field is promoted into its typed slot");
                Ok(if key == "stale_after" {
                    // Migrates a legacy `review_after` line in place.
                    set_stale_after(source, date)
                } else {
                    set_frontmatter_field(source, key, &date.format("%Y-%m-%d").to_string())
                })
            }
            "salience" => {
                let Some(raw) = value else {
                    return Ok(remove_frontmatter_field(source, "salience"));
                };
                let n: f64 = raw.parse().map_err(|_| {
                    EngineError::Invalid(format!(
                        "salience must be a number from 0 to 10, got '{raw}'"
                    ))
                })?;
                if !n.is_finite() || !(0.0..=10.0).contains(&n) {
                    return Err(EngineError::Invalid(format!(
                        "salience must be a number from 0 to 10, got {raw}"
                    )));
                }
                Ok(set_frontmatter_number(source, "salience", n))
            }
            "verified" => {
                // A verification is a record of a check that happened, so it is
                // never cleared here: an omitted value names the caller as the
                // verifier instead, which is the common "I re-checked this and
                // it still holds" case.
                let by = value
                    .map(sanitize_actor)
                    .filter(|a| !a.is_empty())
                    .unwrap_or_else(|| actor.to_string());
                let entry = crystalline_core::Verified {
                    by,
                    at: Some(now_offset()),
                };
                // Keep other actors' verifications and replace this actor's, so
                // the trust record stays a history without growing a line on
                // every sweep.
                let mut entries = parse_engram(source)
                    .map(|e| e.frontmatter.verified)
                    .unwrap_or_default();
                entries.retain(|e| e.by != entry.by);
                entries.push(entry);
                Ok(set_verified(source, &entries))
            }
            EVOLVE_ACK_KEY => {
                // The draft was completed before the lock (see
                // `Engine::ack_draft`), because a record's scope is the sweep's
                // verdict about this engram and no text edit can know it.
                let draft = ack.ok_or_else(|| EngineError::Invalid(ack_value_message()))?;
                let entry = match draft {
                    AckDraft::Record(entry) => entry,
                    // A removal reads the entries the file holds right now,
                    // under the lock, so a concurrent acknowledgment is either
                    // fully there or not there at all when it filters.
                    AckDraft::Remove(rule) => {
                        if !has_ack(source, rule) {
                            return Err(EngineError::Invalid(format!(
                                "no acknowledgment for {rule} on '{permalink}'; nothing to remove"
                            )));
                        }
                        return guarded_ack_write(
                            without_ack(source, rule),
                            "removal",
                            &p.identifier,
                        );
                    }
                };
                // An engram whose frontmatter no longer parses would have its
                // existing entries read as none, and this would write a second
                // `evolve_ack` key beside the one already there - compounding a
                // break instead of reporting it.
                parse_engram(source).map_err(|e| {
                    EngineError::Invalid(format!(
                        "cannot acknowledge a finding on an engram that does not parse ({e}); repair the frontmatter first"
                    ))
                })?;
                // Defense in depth: whatever the note carried, the bytes about
                // to be persisted have to be readable.
                guarded_ack_write(
                    set_evolve_ack(source, &merged_acks(source, entry.clone())),
                    "acknowledgment",
                    &p.identifier,
                )
            }
            other => Err(EngineError::Invalid(format!(
                "set_frontmatter cannot set '{other}'; the settable keys are {}",
                settable_keys()
            ))),
        }
    }

    fn require_content<'a>(&self, p: &'a EditParams) -> Result<&'a str> {
        p.content.as_deref().ok_or_else(|| {
            EngineError::Invalid(format!("operation '{}' requires content", p.operation))
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
    /// field or a `verified` entry left malformed and surgically drop sentinel
    /// or null bounds,
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
        crystalline_core::temporal::normalize_verified(&mut fm)
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
        // As on the create path: `normalize_md` drops empty segments but keeps
        // `..`, so containment is decided here, before either reserved check
        // reads the destination as text and before `join_rel` builds a path
        // from it.
        if !is_within_domain(&dest_rel) {
            return Err(EngineError::Invalid(escapes_root_error(&dest_rel)));
        }
        if crystalline_core::is_reserved_path(&dest_rel) {
            return Err(EngineError::Invalid(reserved_name_error(&dest_rel)));
        }
        if is_assets_reserved(&dest_rel) {
            return Err(EngineError::Invalid(assets_reserved_error(&dest_rel)));
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

        // What the move carries besides the engram: the attachments it
        // references or claims. Filled in the cross-domain branch and acted on
        // once every store lock that branch takes has been released. Beside it,
        // the attachments the plan could not carry, which ride out in the
        // receipt so a caller who never sees the daemon's trace still learns
        // what stayed behind.
        let mut carried: Vec<AttachmentCarry> = Vec::new();
        let mut attachment_warnings: Vec<String> = Vec::new();

        if cross {
            // Read the source content, index it into the destination source,
            // then remove the source. Stricter than the read path on purpose: a
            // move re-emits the file at the destination, and a file domain's
            // stored `content` column holds the body only, so falling back to
            // it would strip the frontmatter off the engram that lands. Failing
            // loudly is the only honest option; the store is the source of
            // truth for a virtual domain, so only that kind reads from it.
            let mut content = match &src_source {
                ContentSource::File { root } => {
                    let abs = join_rel(root, &src.path);
                    std::fs::read_to_string(&abs).map_err(|e| {
                        EngineError::NotFound(format!(
                            "the source file for '{}' at {} is unreadable ({e}); resync '{}' and retry the move",
                            src.permalink,
                            abs.display(),
                            p.domain
                        ))
                    })?
                }
                ContentSource::Virtual => self.load_content(&src_source, &src).await?,
            };
            // Resolved before the write, since an attachment that has to be
            // renamed at the destination changes the very text being written:
            // the engram lands already pointing at the name its file took.
            (carried, attachment_warnings) = self
                .plan_attachment_carry(&src, &dest_domain, &content)
                .await;
            let renames: BTreeMap<String, String> = carried
                .iter()
                .filter(|carry| carry.to != carry.from)
                .map(|carry| (carry.from.clone(), carry.to.clone()))
                .collect();
            if !renames.is_empty() {
                content = rewrite_carried_refs(&content, &renames);
            }
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

        // The attachments follow the engram, now that the engram itself has
        // landed and the branch above has released its store lock. A
        // same-domain move carries nothing: an `assets/` reference is
        // domain-root relative, so a rename inside one domain leaves every one
        // of them valid as written.
        if !carried.is_empty() {
            self.carry_attachments(&src.domain, &dest_domain, &carried)
                .await;
        }

        // Rewrite inbound bare links from other domains to the prefixed form.
        // The linking engrams were not authored by whoever asked for the move,
        // so their refreshed `generated.by` records Crystalline itself (or the
        // configured `identity.actor`), not the moving client.
        let actor = self.actor(None);
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
                    let replaced =
                        touch_generated(&text.replace(&needle, &prefixed), &actor, now_offset());
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
                    let replaced =
                        touch_generated(&text.replace(&needle, &prefixed), &actor, now_offset());
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
        // A move empties one folder and fills another, so both ends need their
        // index files back in step; a same-domain move refreshes once.
        self.refresh_index_files(&p.domain).await;
        if cross {
            self.refresh_index_files(&dest_domain).await;
        }

        // The address the engram answers to at its destination, asked rather
        // than assumed, exactly as `save_engram` asks: a permalink that was
        // derived from the path follows the move (the store's rename says so
        // in as many words), so a receipt repeating the one it went in with
        // would name a permalink that no longer resolves on the very calls
        // that changed it. The move is committed by this line, so the asking is
        // tolerant: a missing row and a failing lookup alike fall back to the
        // name it went in with rather than failing a done move - see
        // [`receipt_permalink`].
        let dest_permalink = {
            let store = self.store.lock().await;
            let found = store
                .list_engrams(&dest_domain, Some(&dest_rel), None)
                .await
                .map_err(EngineError::from)
                .map(|rows| {
                    rows.into_iter()
                        .find(|found| found.path == dest_rel)
                        .map(|found| found.permalink)
                });
            receipt_permalink(found, src.permalink.clone())
        };

        Ok(json!({
            "from": { "domain": p.domain, "permalink": src.permalink, "path": src.path },
            "to": { "domain": dest_domain, "permalink": dest_permalink, "path": dest_rel },
            "cross_domain": cross,
            "links_rewritten": rewritten,
            "attachment_warnings": attachment_warnings,
        }))
    }

    /// Rename a tag to `new`, or (with `merge`) fold it into an existing `new`,
    /// across every engram that carries it, optionally scoped to one domain.
    /// Each affected file is rewritten string-surgically by
    /// [`crystalline_core::retag`]: only the tag tokens change, every other byte
    /// (including the `generated` provenance block) is preserved, so a hygiene
    /// rename never
    /// reflows a file or looks like a fresh edit. Files are the source of truth,
    /// so each rewrite writes the file (or the virtual row) then reindexes it,
    /// which is where the index picks up the new tag identity.
    ///
    /// The two verbs differ only in a precheck: a `rename` refuses when `new`
    /// already exists (that would silently merge, so it points at `tags merge`),
    /// and a `merge` refuses when `new` does not exist yet. `dry_run` reports the
    /// affected engrams without writing anything.
    ///
    /// A non-dry-run `merge` with `record_alias` also records the fold as a tag
    /// alias: it appends `- old -> new` to each affected domain's MANIFEST
    /// `## Tag Aliases` section (creating the section when absent), so a later
    /// search for the old name still finds its engrams through the alias. The
    /// recording is idempotent and permissive: when the pair is already present
    /// the append no-ops and the domain still counts as recorded (a re-merge of a
    /// tag that reappeared must work, never be refused), and a domain with no
    /// MANIFEST lands in `alias_skipped` rather than erroring. When `old` is
    /// already aliased to a different canonical, first-wins parsing keeps the
    /// existing mapping, so a fresh bullet would be inert: the MANIFEST is left
    /// untouched and the domain is surfaced in `alias_conflict` rather than a
    /// false `alias_recorded`. The merge's tag rewrites still proceed either way;
    /// only the recording is skipped. A `rename` records nothing.
    pub async fn retag(
        &self,
        old: &str,
        new: &str,
        domain: Option<&str>,
        merge: bool,
        dry_run: bool,
        record_alias: bool,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let old_f = old.trim().to_lowercase();
        let new_f = new.trim().to_lowercase();
        // Only the target name must be a canonical lowercase-with-hyphens tag:
        // the whole point of a rename or merge is to move a non-canonical `old`
        // (an underscore or separator variant that the cluster detection flags)
        // onto a clean name, so `old` only has to be a non-empty folded tag.
        if old_f.is_empty() {
            return Err(EngineError::Invalid(
                "the tag to rename or merge is empty".into(),
            ));
        }
        if !is_lower_hyphen(&new_f) {
            return Err(EngineError::Invalid(format!(
                "target tag '{new_f}' is not a lowercase-with-hyphens tag"
            )));
        }
        if old_f == new_f {
            return Err(EngineError::Invalid(
                "the old and new tag are the same".into(),
            ));
        }

        // Precheck against the vocabulary in scope: a rename must not collide
        // with an existing tag, a merge must land on one.
        let new_exists = {
            let store = self.store.lock().await;
            let vocab = store.vocabulary(domain).await?;
            vocab.tags.iter().any(|t| t.name == new_f)
        };
        let scope = match domain {
            Some(d) => format!(" in domain '{d}'"),
            None => String::new(),
        };
        if merge {
            if !new_exists {
                return Err(EngineError::NotFound(format!(
                    "cannot merge into '{new_f}': no engram carries it{scope}"
                )));
            }
        } else if new_exists {
            return Err(EngineError::Conflict(format!(
                "tag '{new_f}' already exists{scope}; use `crystalline tags merge` to combine them"
            )));
        }

        // The engrams carrying the old tag, ordered by domain then path.
        let targets = {
            let store = self.store.lock().await;
            store.engrams_with_tag(&old_f, domain).await?
        };
        let listed: Vec<Value> = targets
            .iter()
            .map(|d| json!({ "domain": d.domain, "permalink": d.permalink, "path": d.path }))
            .collect();

        if dry_run {
            return Ok(json!({
                "old": old_f, "new": new_f, "merge": merge, "dry_run": true,
                "engrams": listed, "rewritten": targets.len(),
            }));
        }

        // Rewrite each engram, mirroring move_engram's file-vs-virtual branches.
        let mut rewritten = 0usize;
        for desc in &targets {
            match self.read_source(&desc.domain) {
                ContentSource::File { root } => {
                    let abs = join_rel(&root, &desc.path);
                    let Ok(text) = std::fs::read_to_string(&abs) else {
                        continue;
                    };
                    let Some((edited, _)) = crystalline_core::retag(&text, &old_f, &new_f) else {
                        continue;
                    };
                    write_file(&abs, &edited)?;
                    let store = self.store.lock().await;
                    self.reindex_file(&*store, desc.domain_id, &root, &desc.path)
                        .await?;
                    rewritten += 1;
                }
                ContentSource::Virtual => {
                    let current = {
                        let store = self.store.lock().await;
                        store.engram_content(desc.domain_id, &desc.path).await?
                    };
                    let Some(text) = current else { continue };
                    let Some((edited, _)) = crystalline_core::retag(&text, &old_f, &new_f) else {
                        continue;
                    };
                    let stamp = virtual_stamp(&edited);
                    let store = self.store.lock().await;
                    self.index_markdown(
                        &*store,
                        desc.domain_id,
                        &desc.path,
                        &edited,
                        stamp,
                        None,
                        true,
                    )
                    .await?;
                    rewritten += 1;
                }
            }
        }

        let mut response = json!({
            "old": old_f, "new": new_f, "merge": merge, "dry_run": false,
            "engrams": listed, "rewritten": rewritten,
        });

        // Record the fold as a tag alias in each affected domain's MANIFEST, once
        // per distinct domain in first-seen order. Only a merge records, and only
        // when the caller did not opt out.
        if merge && record_alias {
            let mut domains: Vec<(String, DomainId)> = Vec::new();
            for desc in &targets {
                if !domains.iter().any(|(name, _)| *name == desc.domain) {
                    domains.push((desc.domain.clone(), desc.domain_id));
                }
            }

            let mut alias_recorded: Vec<String> = Vec::new();
            let mut alias_skipped: Vec<String> = Vec::new();
            let mut alias_conflict: Vec<String> = Vec::new();
            let mut virtual_manifest_changed = false;
            for (name, domain_id) in &domains {
                match self.read_source(name) {
                    ContentSource::File { root } => {
                        let abs = join_rel(&root, "MANIFEST.md");
                        let Ok(text) = std::fs::read_to_string(&abs) else {
                            alias_skipped.push(name.clone());
                            continue;
                        };
                        match decide_alias_record(&text, &old_f, &new_f) {
                            AliasRecord::Recorded(edited) => {
                                write_file(&abs, &edited)?;
                                let store = self.store.lock().await;
                                self.reindex_file(&*store, *domain_id, &root, "MANIFEST.md")
                                    .await?;
                                alias_recorded.push(name.clone());
                            }
                            AliasRecord::AlreadyPresent => alias_recorded.push(name.clone()),
                            AliasRecord::Conflict => alias_conflict.push(name.clone()),
                        }
                    }
                    ContentSource::Virtual => {
                        let current = {
                            let store = self.store.lock().await;
                            store.engram_content(*domain_id, "MANIFEST.md").await?
                        };
                        let Some(text) = current else {
                            alias_skipped.push(name.clone());
                            continue;
                        };
                        match decide_alias_record(&text, &old_f, &new_f) {
                            AliasRecord::Recorded(edited) => {
                                let stamp = virtual_stamp(&edited);
                                let store = self.store.lock().await;
                                self.index_markdown(
                                    &*store,
                                    *domain_id,
                                    "MANIFEST.md",
                                    &edited,
                                    stamp,
                                    None,
                                    true,
                                )
                                .await?;
                                virtual_manifest_changed = true;
                                alias_recorded.push(name.clone());
                            }
                            AliasRecord::AlreadyPresent => alias_recorded.push(name.clone()),
                            AliasRecord::Conflict => alias_conflict.push(name.clone()),
                        }
                    }
                }
            }

            // A rewritten virtual MANIFEST may have changed its routing bullets,
            // so refresh the cache once, after every store lock is released.
            if virtual_manifest_changed {
                self.refresh_routing_cache().await;
            }

            if let Value::Object(map) = &mut response {
                map.insert("alias_recorded".to_string(), json!(alias_recorded));
                map.insert("alias_skipped".to_string(), json!(alias_skipped));
                map.insert("alias_conflict".to_string(), json!(alias_conflict));
            }
        }

        Ok(response)
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
    ///
    /// An `assets/` identifier deletes that attachment instead - the row plus
    /// the file or the blob - which is what completes an orphaned-attachment
    /// finding without a second write verb existing. The two are one verb
    /// because they are one act from the caller's side ("remove this thing from
    /// the domain"), and the identifier says which thing without ambiguity: an
    /// engram can never live under the reserved `assets/` folder.
    pub async fn delete_engram(&self, p: &DeleteParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if let Some(path) = attachment_identifier(&p.identifier) {
            // Refused rather than ignored: `expected_checksum` is a promise
            // about markdown a caller read, and an attachment's bytes are not
            // that. Accepting it silently would let a caller believe a delete
            // was guarded when nothing compared anything.
            if p.expected_checksum.is_some() {
                return Err(EngineError::Invalid(format!(
                    "expected_checksum guards an engram edit and has no meaning for the attachment '{path}'; delete it without one"
                )));
            }
            self.attachment_delete(&p.domain, &path).await?;
            return Ok(json!({
                "domain": p.domain,
                "path": path,
                "attachment": true,
                "deleted": true,
            }));
        }
        let (desc, source) = self.resolve(&p.identifier, Some(&p.domain)).await?;
        // Held across the comparison and the removal, so a guarded delete
        // cannot check a file that a concurrent save then rewrites underneath
        // it. See `Engine::write_lock`.
        let file_lock = match &source {
            ContentSource::File { root } => Some(self.write_lock(&join_rel(root, &desc.path))),
            ContentSource::Virtual => None,
        };
        let _guard = match &file_lock {
            Some(lock) => Some(lock.lock().await),
            None => None,
        };
        if let Some(expected) = &p.expected_checksum {
            let current = self.load_content(&source, &desc).await?;
            let found = sha256_hex(current.as_bytes());
            if &found != expected {
                return Err(EngineError::Conflict(stale_edit_message(expected, &found)));
            }
        }
        if let ContentSource::File { root } = &source {
            let abs = join_rel(root, &desc.path);
            std::fs::remove_file(&abs).map_err(|source| EngineError::Io {
                path: abs.display().to_string(),
                source,
            })?;
        }
        let store = self.store.lock().await;
        store.delete_engram(desc.domain_id, &desc.path).await?;
        // Deleting a MANIFEST removes its `## Tag Aliases` declarations, so clear
        // the domain's derived alias rows: the content is already gone, so the
        // refresh folds to no pairs and replaces the rows with nothing.
        if desc.path == "MANIFEST.md" {
            crystalline_index::refresh_tag_aliases(&*store, desc.domain_id).await?;
        }
        drop(store);

        // Deleting a virtual domain's MANIFEST engram empties its routing
        // bullets, so refresh the cache once the store lock is released.
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }
        // The deleted engram must leave its folder's generated index, and an
        // emptied folder loses the index file altogether.
        self.refresh_index_files(&desc.domain).await;

        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "path": desc.path,
            "deleted": true,
        }))
    }

    /// What [`Engine::delete_engram`] would remove, without removing any of it.
    ///
    /// Written for the confirmation round the MCP layer opens on a peer that
    /// can put a question to its user: the question has to name what dies, and
    /// naming it means resolving the identifier first. Every refusal the delete
    /// itself would raise on the way to the file - an unknown domain, an
    /// identifier that resolves to nothing or to two things, a read-only
    /// server, an `expected_checksum` on an attachment - is raised here too, so
    /// a call that cannot succeed fails before a human is asked to approve it
    /// rather than after.
    ///
    /// Two shapes, one per branch of the delete. An `assets/` identifier
    /// previews `{domain, path, size, attachment: true}`; anything else
    /// previews `{domain, permalink, title, path, attachments}`, where
    /// `attachments` is the stored attachments **only this engram references**.
    /// Those files are not deleted with it - `delete_engram` removes the
    /// markdown and its rows and nothing else - so what the list says is which
    /// attachments the delete leaves with no referent at all.
    ///
    /// `attachments` is `null` rather than a list on a domain past
    /// [`MAX_PREVIEW_SCAN_ENGRAMS`], where enumerating them would read the
    /// whole domain to build one sentence. The delete is the same delete
    /// either way; only the question is worded differently, and `null` says
    /// nobody looked where `[]` says somebody looked and found none.
    ///
    /// The `expected_checksum` comparison is deliberately not repeated here:
    /// the file can change between the two rounds, so the guard is worth
    /// nothing unless it runs in the round that actually deletes, which is
    /// where it already runs.
    ///
    /// One narrow divergence the other way, stated so it is not discovered:
    /// the engram branch loads the engram's content unconditionally, to see
    /// what it references, where the delete loads it only when a checksum is
    /// being compared. A row whose content cannot be loaded therefore fails
    /// the preview while the plain delete of it would succeed - reachable on a
    /// virtual domain holding a row with no stored content. It is a miss the
    /// caller can act on rather than a silent one, and the alternative is
    /// answering "attachments: none" for an engram nobody could read.
    pub async fn delete_preview(&self, p: &DeleteParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if let Some(path) = attachment_identifier(&p.identifier) {
            if p.expected_checksum.is_some() {
                return Err(EngineError::Invalid(format!(
                    "expected_checksum guards an engram edit and has no meaning for the attachment '{path}'; delete it without one"
                )));
            }
            let size = self.attachment_delete_size(&p.domain, &path).await?;
            return Ok(json!({
                "domain": p.domain,
                "path": path,
                "size": size,
                "attachment": true,
            }));
        }
        let (desc, source) = self.resolve(&p.identifier, Some(&p.domain)).await?;
        let content = self.load_content(&source, &desc).await?;
        let attachments = self.previewable_attachments(&desc, &content).await;
        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "title": desc.title,
            "path": desc.path,
            "attachments": attachments,
        }))
    }

    /// [`Engine::sole_referent_attachments`] under
    /// [`MAX_PREVIEW_SCAN_ENGRAMS`], and [`None`] above it.
    ///
    /// The two answers are different facts and the shape says which is which:
    /// an array is "these are the attachments the delete orphans", empty
    /// included, and `null` is "nobody looked". The caller that renders the
    /// question reads the difference and words the clause accordingly; nothing
    /// here changes what the delete removes.
    async fn previewable_attachments(
        &self,
        desc: &EngramDescriptor,
        content: &str,
    ) -> Option<Vec<String>> {
        if !self.within_preview_scan_bound(&desc.domain).await {
            return None;
        }
        Some(self.sole_referent_attachments(desc, content).await)
    }

    /// Whether `domain` is small enough for the preview to enumerate, one
    /// metadata query and no bodies.
    ///
    /// **A count that cannot be taken answers `true`.** The bound exists to
    /// keep a question cheap, not to give round one a new way to fail, and a
    /// preview that is stricter than the delete it previews is the one thing
    /// this whole surface must never be. It costs nothing in practice either:
    /// the listing this failed on is the same listing the enumeration opens
    /// with, so it fails there immediately and resolves the safe way, which
    /// names no attachments at all.
    async fn within_preview_scan_bound(&self, domain: &str) -> bool {
        let counted = {
            let store = self.store.lock().await;
            store.list_engrams(domain, None, None).await
        };
        match counted {
            Ok(rows) => count_within_preview_bound(rows.len()),
            Err(e) => {
                tracing::warn!(
                    "the engrams of '{domain}' could not be counted ({e}); the delete preview enumerates attachments as it would on a small domain"
                );
                true
            }
        }
    }

    /// How many bytes [`Engine::attachment_delete`] would remove, or
    /// [`EngineError::NotFound`] when it would remove nothing.
    ///
    /// **Deliberately not [`Engine::attachment_read`], and the difference is a
    /// bug rather than a preference.** That read refuses a file over
    /// [`crystalline_core::MAX_ATTACHMENT_BYTES`] and, on a virtual domain,
    /// insists on both a row and a blob. The delete does neither: it reads no
    /// bytes and succeeds when either half is there. A preview built on the
    /// read would therefore fail round one - and so refuse the delete outright
    /// for a peer that gets asked - on exactly the files this verb is the
    /// escape hatch for: the stray oversized file the walker skipped and so
    /// never gave a row, and the half-present pair a hand-edited domain leaves
    /// behind. **A preview must never be stricter than the act it previews.**
    ///
    /// So the size is looked up rather than measured: the file's own metadata
    /// where there is a file, the recorded row where the file is already gone
    /// and only the row stands. No bytes are read and nothing is written -
    /// unlike the read, which heals the row it serves, so round one no longer
    /// mutates the derived layer at all.
    async fn attachment_delete_size(&self, domain: &str, path: &str) -> Result<u64> {
        validate_attachment_path(path)?;
        let (domain_id, source) = self.domain_source(domain).await?;
        let row = {
            let store = self.store.lock().await;
            store.get_attachment(domain_id, path).await?
        };
        // Whichever half the delete would find, in the order that gives the
        // truest number: a row can be stale about a file that is right there,
        // and a file cannot be stale about itself.
        if let ContentSource::File { root } = &source {
            let abs = contained_asset_path(root, path)?;
            match std::fs::metadata(&abs) {
                Ok(meta) => return Ok(meta.len()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(EngineError::Io {
                        path: abs.display().to_string(),
                        source,
                    });
                }
            }
        }
        row.map(|row| row.size)
            .ok_or_else(|| EngineError::NotFound(missing_attachment(domain, path)))
    }

    /// The stored attachments `src` refers to that nothing else in its domain
    /// refers to.
    ///
    /// Counted by the same pair the cross-domain move counts with
    /// ([`referenced_asset_paths`] and [`Engine::shared_asset_paths`]), so
    /// "only this engram uses it" means one thing across the crate, including
    /// the part where a count that fails resolves to shared and therefore
    /// names nothing.
    ///
    /// Screened against the domain's attachment rows at the end, one metadata
    /// query and no bytes: a reference to a file the domain does not hold is a
    /// dangling reference the sweep already reports, and a delete does not
    /// orphan something that was never there.
    async fn sole_referent_attachments(
        &self,
        src: &EngramDescriptor,
        content: &str,
    ) -> Vec<String> {
        let candidates = referenced_asset_paths(content);
        if candidates.is_empty() {
            return Vec::new();
        }
        let shared = self.shared_asset_paths(src, &candidates).await;
        let mut sole: Vec<String> = candidates
            .into_iter()
            .filter(|path| !shared.contains(path))
            .collect();
        if sole.is_empty() {
            return sole;
        }
        let stored: HashSet<String> = match self.attachment_list(&src.domain).await {
            Ok(rows) => rows.into_iter().map(|row| row.path).collect(),
            Err(e) => {
                // The screen is a refinement, not the decision. When it cannot
                // run, the unscreened list is still every path this engram is
                // the last referent of, which is the honest answer minus one
                // filter.
                tracing::warn!(
                    "the attachments of '{}' could not be listed ({e}); the delete preview names every path the engram is the last referent of",
                    src.domain
                );
                return sole;
            }
        };
        sole.retain(|path| stored.contains(path));
        sole
    }

    // --- search --------------------------------------------------------------

    /// Search across domains, embedding the query when the mode needs it.
    pub async fn search_engrams(&self, p: &SearchParams) -> Result<Value> {
        self.search_engrams_under(p, None).await
    }

    /// [`Engine::search_engrams`] narrowed to one domain-relative folder, which
    /// is what a folder view pages from: the same filter-only search, with the
    /// folder pushed into SQL beside the other filters, so `total` stays exact
    /// and paging is unchanged.
    ///
    /// The folder is segment-safe - see [`folder_prefix`] - and `None` or an
    /// empty value searches the whole scope, which is what every caller that
    /// never names a folder keeps getting.
    ///
    /// The `total` in the envelope counts the folder recursively: every engram
    /// under it at any depth, since a folder listing promises the folder. The
    /// tree's own `total` counts one level and is deliberately smaller; see
    /// [`Engine::browse_domain`]. It is a separate verb rather than a
    /// field on [`SearchParams`] because that struct is the MCP search tool's
    /// argument schema, and a folder filter is a browsing affordance of this
    /// API rather than a knob worth spending an agent's context on.
    pub async fn search_engrams_under(
        &self,
        p: &SearchParams,
        folder: Option<&str>,
    ) -> Result<Value> {
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
            path_prefix: folder.and_then(folder_prefix),
            limit: p.limit.unwrap_or(10).clamp(1, MAX_PAGE_LIMIT),
            page: p.page.unwrap_or(1).max(1),
            ..SearchQuery::default()
        };
        {
            let config = self.config.read().unwrap();
            query.salience_weight = config.salience_weight();
            query.retired_weight = config.retired_weight();
        }
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

        // Rank the full slice before any filtering so a domain-filtered node
        // still conducts mass as a bridge; the domain filter applies only at
        // output selection, preserving the current presentation.
        let mass = context_rank(&slice, &seed_ids);
        let (weight, retired_weight) = {
            let config = self.config.read().unwrap();
            (
                config.salience_weight().unwrap_or(DEFAULT_SALIENCE_WEIGHT),
                config.retired_weight().unwrap_or(DEFAULT_RETIRED_WEIGHT),
            )
        };

        // Output pass: keep seeds in slice (ascending-id) order, then rank the
        // related nodes by spread mass lifted by the salience prior, highest
        // first with an ascending-id tiebreak, capped at max_related.
        let mut seed_nodes: Vec<&GraphNode> = Vec::new();
        let mut related: Vec<(f64, &GraphNode)> = Vec::new();
        for node in &slice.nodes {
            if let Some(filter) = &domain_filter
                && !filter.contains(&node.domain)
            {
                continue;
            }
            if seed_ids.contains(&node.id.0) {
                seed_nodes.push(node);
            } else {
                let score = mass.get(&node.id.0).copied().unwrap_or(0.0)
                    * (1.0 + salience_prior(node.salience, weight))
                    * retired_factor(&node.status, retired_weight);
                related.push((score, node));
            }
        }
        related.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.1.id.0.cmp(&b.1.id.0))
        });
        related.truncate(max_related);

        let mut kept: HashSet<i64> = HashSet::new();
        let mut nodes = Vec::new();
        for node in seed_nodes
            .into_iter()
            .chain(related.into_iter().map(|(_, node)| node))
        {
            let is_seed = seed_ids.contains(&node.id.0);
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

    /// The nodes and typed edges around an anchor, for a graph view.
    ///
    /// The same traversal [`Engine::build_context`] runs, answered in the flat
    /// shape a graph renderer wants: every node carries what a client labels and
    /// styles it with (`id`, `domain`, `permalink`, `title`, `status`, `type`),
    /// every edge keeps its direction and `rel_type`, and `truncated` says
    /// whether the cap cut anything. `id` is the index's own engram id, opaque
    /// to a client and stable only within one response: it is what the edges
    /// join on, never an address. `crystalline://domain/permalink` is the
    /// address.
    ///
    /// `depth` is clamped to one or two hops and `max_nodes` to at least one and
    /// at most [`MAX_GRAPH_NODES`], so a hand-written URL can ask neither for
    /// nothing nor for the whole index in one payload.
    ///
    /// Retired engrams come back like any other, with their status, because the
    /// graph is the shape of what is written rather than of what still holds:
    /// hiding a superseded node would break the chain that explains what replaced
    /// it. A client fades them; this does not drop them from an uncapped answer.
    ///
    /// When the cap bites, the anchors are kept first, then the rest are kept by
    /// the same spread mass and salience lift `build_context` ranks with - except
    /// that retired non-anchor nodes yield to live ones first, since a fading
    /// node is the one the budget can least afford to keep over live knowledge.
    /// `hidden` counts every node the cap cut, retired or not.
    pub async fn graph_neighborhood(
        &self,
        anchor: &str,
        depth: u8,
        max_nodes: usize,
    ) -> Result<Value> {
        let url = CrystallineUrl::parse(anchor).ok_or_else(|| {
            EngineError::Invalid(format!("anchor '{anchor}' is not a crystalline:// URL"))
        })?;
        let depth = depth.clamp(1, 2);
        let max_nodes = max_nodes.clamp(1, MAX_GRAPH_NODES);

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
        drop(store);
        if seeds.is_empty() {
            return Err(EngineError::NotFound(format!(
                "anchor '{anchor}' matched no engrams"
            )));
        }

        let seed_ids: HashSet<i64> = seeds.iter().map(|d| d.id.0).collect();
        let ids: Vec<EngramId> = seeds.iter().map(|d| d.id).collect();
        let slice = self.sweep_neighbors(&ids, depth).await?;

        let mass = context_rank(&slice, &seed_ids);
        let weight = {
            let config = self.config.read().unwrap();
            config.salience_weight().unwrap_or(DEFAULT_SALIENCE_WEIGHT)
        };
        let mut anchors: Vec<&GraphNode> = Vec::new();
        let mut related: Vec<(f64, &GraphNode)> = Vec::new();
        for node in &slice.nodes {
            if seed_ids.contains(&node.id.0) {
                anchors.push(node);
            } else {
                let score = mass.get(&node.id.0).copied().unwrap_or(0.0)
                    * (1.0 + salience_prior(node.salience, weight));
                related.push((score, node));
            }
        }
        related.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.1.id.0.cmp(&b.1.id.0))
        });
        // Retired knowledge yields first when the cap bites. A stable partition
        // after the score sort: under the cap the kept SET is identical (order
        // within the payload is not part of the contract), over it the live
        // neighborhood survives and the hidden count below reports the cut.
        related.sort_by_key(|(_, node)| crystalline_index::is_retired_status(&node.status));

        let total = anchors.len() + related.len();
        let mut kept: HashSet<i64> = HashSet::new();
        let mut nodes = Vec::new();
        for node in anchors
            .into_iter()
            .chain(related.into_iter().map(|(_, node)| node))
            .take(max_nodes)
        {
            kept.insert(node.id.0);
            nodes.push(json!({
                "id": node.id.0,
                "domain": node.domain,
                "permalink": node.permalink,
                "title": node.title,
                "status": node.status,
                "type": node.engram_type,
            }));
        }
        // An edge is only meaningful when both of its ends survived the cap; one
        // that lost an end would render as an arrow into nothing. The relation
        // and the prose link between one pair are one edge here, because the
        // payload states the type rather than the origin: an engram that both
        // declares `- links_to [[X]]` and writes the wikilink in its prose is
        // one line on the picture, not two drawn over each other.
        let mut drawn: HashSet<(i64, i64, &str)> = HashSet::new();
        let edges: Vec<Value> = slice
            .edges
            .iter()
            .filter(|e| kept.contains(&e.from.0) && kept.contains(&e.to.0))
            .filter(|e| drawn.insert((e.from.0, e.to.0, e.rel_type.as_str())))
            .map(|e| {
                json!({
                    "from": e.from.0,
                    "to": e.to.0,
                    "rel_type": e.rel_type,
                })
            })
            .collect();

        Ok(json!({
            "nodes": nodes,
            "edges": edges,
            "truncated": total > nodes.len(),
            "hidden": total - nodes.len(),
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
    ///
    /// With `include_routing` the response also carries a top-level `behavior`
    /// array: the same rules the onboarding block renders, from
    /// [`crystalline_core::behavior_bullets`]. Remote clients never show the
    /// model the initialize instructions, so this one call is their whole
    /// onboarding - the routing lines and the rules that govern them together.
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
        if p.include_routing {
            return Ok(json!({
                "behavior": crystalline_core::behavior_bullets(self.read_only()),
                "domains": out,
            }));
        }
        Ok(json!({ "domains": out }))
    }

    /// One domain's MANIFEST markdown, read through the same source its routing
    /// bullets are read through: a file domain's `MANIFEST.md` on disk, a
    /// virtual domain's MANIFEST engram in the database.
    ///
    /// The source, not a reduction of it: a client that renders or edits a
    /// manifest needs the frontmatter and every section, not the routing
    /// bullets [`Engine::list_domains`] already extracts. An unregistered domain
    /// errors with the registered set named, like every other verb; a domain
    /// that carries no MANIFEST yet is a `NotFound`, since a manifest is what
    /// routes an agent to a domain at all rather than an optional extra.
    pub async fn manifest_markdown(&self, domain: &str) -> Result<String> {
        match self.content_source(domain)? {
            ContentSource::File { root } => {
                let path = root.join("MANIFEST.md");
                match std::fs::read_to_string(&path) {
                    Ok(source) => Ok(source),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        Err(EngineError::NotFound(format!(
                            "domain '{domain}' has no MANIFEST.md at {}",
                            path.display()
                        )))
                    }
                    Err(source) => Err(EngineError::Io {
                        path: path.display().to_string(),
                        source,
                    }),
                }
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let content = match store.find_engram(domain, "manifest").await? {
                    Some(d) => store.engram_content(d.domain_id, &d.path).await?,
                    None => None,
                };
                content.ok_or_else(|| {
                    EngineError::NotFound(format!("domain '{domain}' has no MANIFEST engram yet"))
                })
            }
        }
    }

    /// Save a domain's MANIFEST markdown verbatim, guarded by the checksum of
    /// the version the caller read - the manifest counterpart of
    /// [`Engine::save_engram`], through the same `expected_checksum` seam and
    /// the same "stale edit" wording on both domain kinds.
    ///
    /// `refresh_routing_cache` runs unconditionally afterwards, on both file
    /// and virtual domains, even though the cache it fills
    /// (`Engine::routing_virtual`) only ever holds virtual-domain bullets: a
    /// file domain's bullets are read straight off `MANIFEST.md` on disk by
    /// `routing_text` at request time, so a file-domain save has nothing in
    /// the cache to refresh. Calling it unconditionally keeps this call site
    /// correct without the caller needing to know which kind answered.
    pub async fn save_manifest(
        &self,
        domain: &str,
        markdown: &str,
        expected_checksum: &str,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // Same hard gate as `save_engram`: a MANIFEST with no frontmatter (or
        // an empty block) is not a manifest at all - it carries the domain's
        // routing bullets and Tag Aliases, so losing the frontmatter here
        // silently strips those too. `parse_engram` alone would not catch
        // this, since an empty frontmatter span parses to
        // `Frontmatter::default()` rather than an error.
        let parsed =
            parse_engram_lossless(markdown).map_err(|e| EngineError::Invalid(e.to_string()))?;
        if !parsed.has_frontmatter || parsed.raw_frontmatter.trim().is_empty() {
            return Err(EngineError::Invalid(
                "the document carries no frontmatter, so it is not a MANIFEST; \
                 keep the --- delimited frontmatter block at the top of the file"
                    .into(),
            ));
        }

        match self.content_source(domain)? {
            ContentSource::File { root } => {
                let path = root.join("MANIFEST.md");
                // The same compare-then-write section `save_engram` holds, for
                // the same reason: two saves of one MANIFEST must not both find
                // their token fresh. See `Engine::write_lock`.
                let lock = self.write_lock(&path);
                let _guard = lock.lock().await;
                let current = match std::fs::read_to_string(&path) {
                    Ok(source) => source,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(EngineError::NotFound(format!(
                            "domain '{domain}' has no MANIFEST.md at {}",
                            path.display()
                        )));
                    }
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: path.display().to_string(),
                            source,
                        });
                    }
                };
                let found = sha256_hex(current.as_bytes());
                if found != expected_checksum {
                    return Err(EngineError::Conflict(stale_edit_message(
                        expected_checksum,
                        &found,
                    )));
                }
                write_file(&path, markdown)?;
                let store = self.store.lock().await;
                let domain_id = store
                    .upsert_domain(domain, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?;
                self.reindex_file(&*store, domain_id, &root, "MANIFEST.md")
                    .await?;
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let desc = store
                    .find_engram(domain, "manifest")
                    .await?
                    .ok_or_else(|| {
                        EngineError::NotFound(format!(
                            "domain '{domain}' has no MANIFEST engram yet"
                        ))
                    })?;
                let stamp = virtual_stamp(markdown);
                self.index_markdown(
                    &*store,
                    desc.domain_id,
                    &desc.path,
                    markdown,
                    stamp,
                    Some(expected_checksum),
                    true,
                )
                .await?;
            }
        }

        self.refresh_routing_cache().await;

        Ok(json!({
            "domain": domain,
            "checksum": sha256_hex(markdown.as_bytes()),
        }))
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

    // --- generated index files -----------------------------------------------

    /// Regenerate a file domain's OKF `index.md` files, so the knowledge on
    /// disk keeps navigating statically after a mutation or a sync.
    ///
    /// Silently does nothing for a virtual domain (no files to navigate), for a
    /// read-only engine (the curating side owns the index files) and while the
    /// `index.files` setting is off. The pass itself is idempotent: a
    /// regeneration that renders the same bytes writes nothing, so an unchanged
    /// index file keeps its mtime and the watcher stays quiet. Failures are
    /// logged, never propagated: a generated navigation file is a convenience,
    /// and losing it must never fail the write, move, delete or sync that
    /// triggered the pass.
    async fn refresh_index_files(&self, domain: &str) {
        if self.read_only || !self.config.read().unwrap().index_files() {
            return;
        }
        let ContentSource::File { root } = self.read_source(domain) else {
            return;
        };
        let name = domain.to_string();
        // A full walk plus one read per engram is blocking IO, so it runs on the
        // blocking pool rather than on a runtime worker.
        let joined = tokio::task::spawn_blocking(move || crate::index_files::refresh(&root)).await;
        match joined {
            Ok(report) if report.written > 0 || report.removed > 0 => {
                tracing::debug!(
                    "refreshed index files of '{name}': {} written, {} removed",
                    report.written,
                    report.removed
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("index refresh of '{name}' did not run: {e}"),
        }
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
    ///
    /// One level at a time and bounded: at most [`TREE_LEVEL_CAP`] engrams come
    /// back, with `total` saying how many the level holds and `truncated`
    /// saying whether the two differ. `folders` is never cut - it is derived
    /// from the paths themselves rather than from the rows that survived the
    /// cap, so a truncated level still names every folder a reader can descend
    /// into.
    ///
    /// `total` counts the level, not the folder: it moves with `depth` and
    /// leaves out everything nested deeper, so a folder of ten engrams holding a
    /// subfolder of a thousand reports ten here. The paged listing scoped to the
    /// same folder ([`Engine::search_engrams_under`]) counts recursively and
    /// reports the larger number. Neither is the other's approximation: a level
    /// states a fact about the rows it drew, a folder listing promises the
    /// folder, and a client that means to say "N engrams in this folder" takes
    /// the number from the listing.
    ///
    /// A `glob` narrows the rows this level returned, so on a truncated level
    /// it selects within the cap rather than across the whole folder. The tree
    /// is a navigation aid; a folder too big to draw is what the paged listing
    /// is for.
    pub async fn browse_domain(&self, p: &BrowseParams) -> Result<Value> {
        // A domain-exists check, not a filesystem-root requirement, so a virtual
        // domain browses.
        self.domain_entry(&p.domain)?;
        let raw = p.path.clone().unwrap_or_else(|| "/".to_string());
        let prefix = folder_prefix(&raw);
        let depth = p.depth.unwrap_or(1).clamp(1, TREE_MAX_DEPTH);
        let matcher = match &p.glob {
            Some(g) => Some(
                globset::Glob::new(g)
                    .map_err(|e| EngineError::Invalid(format!("invalid glob '{g}': {e}")))?
                    .compile_matcher(),
            ),
            None => None,
        };

        // Three bounded queries in the store rather than one listing of the
        // whole domain filtered here: the prefix and the depth cut are pushed
        // into SQL in every case, the root included, so a client that refetches
        // its tree can never pull tens of thousands of rows across per request.
        let store = self.store.lock().await;
        let level = store
            .browse_level(&p.domain, prefix.as_deref(), depth, TREE_LEVEL_CAP)
            .await?;
        drop(store);

        // Whether the level was cut is a fact about the rows, decided before the
        // glob narrows them: a glob that matches two of five hundred rows has
        // not un-truncated the level, and `total` stays the level's own count so
        // a client can offer the listing instead.
        let truncated = level.total > level.engrams.len();
        let entries: Vec<Value> = level
            .engrams
            .iter()
            .filter(|d| matcher.as_ref().is_none_or(|m| m.is_match(&d.path)))
            .map(|d| {
                // `status` rides along with the rest of the descriptor, which
                // already carries it: a browse row is what a navigation tree is
                // drawn from, and whether an engram is retired is the one thing
                // such a tree has to say about a row it is not otherwise
                // describing. Leaving it out meant every client browsing a
                // domain had to fetch the listing again to learn it.
                json!({
                    "permalink": d.permalink,
                    "title": d.title,
                    "type": d.engram_type,
                    "status": d.status,
                    "path": d.path,
                })
            })
            .collect();

        Ok(json!({
            "domain": p.domain,
            "path": raw,
            "folders": level.folders,
            "engrams": entries,
            "truncated": truncated,
            "total": level.total,
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
        // When drift is requested, target engrams are grouped by their selected
        // schema so `schema::diff` runs once per schema over its own group.
        let mut drift_groups: Vec<(Schema, Vec<Engram>)> = Vec::new();
        for d in &targets {
            let Some(engram) = self.load_engram(&source, d.domain_id, &d.path).await else {
                continue;
            };
            checked += 1;
            let selected = schema::select_schema(&engram, &schemas);
            if let Some(schema) = &selected {
                for issue in schema::validate(&engram, schema) {
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
            if p.drift
                && let Some(schema) = selected
            {
                match drift_groups.iter_mut().find(|(s, _)| *s == schema) {
                    Some((_, group)) => group.push(engram),
                    None => drift_groups.push((schema, vec![engram])),
                }
            }
        }

        let mut response = json!({
            "domain": p.domain,
            "checked": checked,
            "schemas": schemas.len(),
            "issue_count": issues.len(),
            "issues": issues,
        });
        if p.drift {
            let drift: Vec<Value> = drift_groups
                .iter()
                .map(|(schema, engrams)| {
                    let d = schema::diff(schema, engrams);
                    json!({
                        "schema": schema.entity,
                        "undeclared_observations": d.undeclared_observations,
                        "undeclared_relations": d.undeclared_relations,
                        "unused_observations": d.unused_observations,
                        "unused_relations": d.unused_relations,
                    })
                })
                .collect();
            response["drift"] = Value::Array(drift);
        }
        Ok(response)
    }

    // --- evolve --------------------------------------------------------------

    /// Run the consolidation sweep over a scope and return one page of its
    /// ranked queue, recording that the sweep ran.
    ///
    /// The thin half of the seam: [`Engine::evolve_detect`] does the work and
    /// this adds the one side effect, stamping the sweep into the maintenance
    /// state so the Stop hook stops nudging about domains this sweep just
    /// looked at - the swept scope for a scoped call, the whole backlog for an
    /// unscoped one. Detection is shared and pure, so a surface that
    /// only wants to show the queue (the REST queue view) calls `evolve_detect`
    /// and never counts as a run; an agent that actually works the queue comes
    /// through here.
    ///
    /// The recording is best effort by design - see [`crate::maintenance`] -
    /// and the response is returned exactly as detection built it.
    pub async fn evolve_engrams(&self, p: &EvolveParams) -> Result<Value> {
        let value = self.evolve_detect(p).await?;
        // The swept scope is read back out of the response rather than
        // re-derived from the parameters: an unscoped call defaults to every
        // registered domain, and only the response knows which those were.
        let swept: Vec<String> = value["scope"]["domains"]
            .as_array()
            .map(|domains| {
                domains
                    .iter()
                    .filter_map(|d| d.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        // A sweep with no scope of its own looked at every registered domain,
        // so it settles the whole backlog rather than subtracting the names it
        // saw. That is what heals the state file: a domain a human wrote to and
        // then unregistered can never appear in a swept scope again, and
        // subtracting would leave it pending for ever with the Stop hook naming
        // a ghost nothing can act on. A scoped call keeps the exact opposite
        // property, settling its own domains and leaving the rest of the
        // backlog standing at its original age.
        if p.domains.is_empty() {
            crate::maintenance::record_run_unscoped();
        } else {
            crate::maintenance::record_run(&swept);
        }
        Ok(value)
    }

    /// The detection half of the consolidation sweep: one page of the ranked
    /// queue over a scope, with no side effect of any kind.
    ///
    /// Read-only end to end: it resolves the scope, assembles the facts every
    /// detector reads, runs [`crystalline_index::detect`] once per domain and
    /// shapes the merged result. Nothing is written and nothing is remembered,
    /// so "what is left" is re-derived by calling again with the same scope.
    ///
    /// Six details of the assembly are load-bearing, each guarding a class of
    /// silently wrong finding:
    ///
    /// - the resolved degrees are counted over the **merged** graph slices, so
    ///   chunking the `neighbors` seed list never turns a linked engram into a
    ///   `V104` orphan. The merge dedupes edges on the same key the backends
    ///   use, because an edge whose ends land in two different chunks comes
    ///   back from both calls;
    /// - `stale_on` and `verified_on` come from the [`Frontmatter`] accessors,
    ///   never the raw keys, so the legacy `review_after` and `last_verified`
    ///   spellings fold in exactly as they do for search and verify;
    /// - the token budget is resolved the way verify's `Q002` resolves it - a
    ///   per-file override, then the domain default, then 2500 - so `V105` and
    ///   `Q002` never disagree about what oversized means;
    /// - `status` and `engram_type` arrive lowercased, because the status sets
    ///   the rules test against are exact matches;
    /// - `known_domains` is every registered domain, so `V102` can tell an
    ///   unregistered target domain apart from a target that does not exist,
    ///   and the graph is taken at depth 1 so cross-domain targets carry a
    ///   status for `V101` to read;
    /// - the attachment facts (`analyzes`, `analyzed_hash`, `asset_refs`) are
    ///   read off the **parsed engram** [`Engine::load_engram`] returns - the
    ///   file for a file domain, the stored source for a virtual one - and
    ///   never off the index's `content` column, which for a file domain holds
    ///   the body alone. A claim lives in the frontmatter, so counting it off
    ///   the index would make every file domain look as if it claimed nothing
    ///   and would report claimed attachments as orphans. This is the same
    ///   split [`Engine::peer_engram_text`] makes for the move's referent
    ///   count, and the two agree on what a reference is: an `assets/` link in
    ///   the body or the `analyzes` key, compared as exact paths.
    pub async fn evolve_detect(&self, p: &EvolveParams) -> Result<Value> {
        let today = match p.today.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
                EngineError::Invalid(format!("today '{s}' is not an ISO date (YYYY-MM-DD)"))
            })?,
            None => Utc::now().date_naive(),
        };
        let families = parse_families(&p.families)?;
        let rules = parse_rules(&p.rules)?;

        // Every registered domain, both as the default scope and as `V102`'s
        // idea of which `[[domain:Target]]` prefixes name a real domain.
        let mut known_domains = self.known_domain_names();
        known_domains.sort();
        known_domains.dedup();

        let mut scope: Vec<String> = Vec::new();
        if p.domains.is_empty() {
            scope = known_domains.clone();
        } else {
            for name in &p.domains {
                // The same resolution every other tool uses, so an unknown name
                // errors identically and a domain registered after startup is
                // still found.
                self.domain_entry(name)?;
                if !scope.contains(name) {
                    scope.push(name.clone());
                }
            }
        }

        let mut findings: Vec<Finding> = Vec::new();
        let mut truncations: Vec<String> = Vec::new();
        let mut engrams_scanned = 0usize;
        let mut unparsed = 0usize;
        let mut acknowledged = AckCounts::default();

        // One domain at a time: `SweepInput` is domain-scoped (two rules are
        // domain-relative) and processing them in turn bounds the memory an
        // unscoped sweep needs to whatever the largest domain costs.
        for name in &scope {
            let Some(swept) = self
                .sweep_domain(name, today, &known_domains, p.include_acknowledged)
                .await?
            else {
                continue;
            };
            engrams_scanned += swept.report.engrams_scanned;
            unparsed += swept.unparsed;
            // A cap that fired is domain-local, so the merged list names the
            // domain it fired in.
            truncations.extend(
                swept
                    .report
                    .truncations
                    .iter()
                    .map(|t| format!("{name} - {t}")),
            );
            // Counted before the family and rule filters below, because what an
            // acknowledgment suppressed is a fact about the domain rather than
            // about the slice of it this call asked for.
            acknowledged.total += swept.report.acknowledged.total;
            acknowledged.temporal += swept.report.acknowledged.temporal;
            acknowledged.structure += swept.report.acknowledged.structure;
            acknowledged.redundancy += swept.report.acknowledged.redundancy;
            findings.extend(swept.report.findings);
        }

        findings.retain(|f| {
            (families.is_empty() || families.contains(&f.family))
                && (rules.is_empty() || rules.contains(&f.rule))
                && p.min_priority.is_none_or(|min| f.priority >= min)
        });
        // Re-ranked after the merge: each domain's report is ranked on its own,
        // and the sort is total and deterministic, so consecutive pages of an
        // unscoped sweep stay coherent.
        rank(&mut findings);

        let total = findings.len();
        let limit = p
            .limit
            .unwrap_or(EVOLVE_DEFAULT_LIMIT)
            .clamp(1, EVOLVE_MAX_LIMIT);
        let page = p.page.unwrap_or(1).max(1);
        let offset = (page - 1).saturating_mul(limit);
        let shown: &[Finding] = match findings.get(offset..) {
            Some(rest) => &rest[..rest.len().min(limit)],
            None => &[],
        };

        // Family counts are over the whole filtered result, not the page, so a
        // reader on page 1 sees the shape of everything waiting.
        let family_counts: Vec<Value> = Family::ALL
            .iter()
            .filter_map(|family| {
                let count = findings.iter().filter(|f| f.family == *family).count();
                (count > 0).then(|| json!({ "family": family.as_str(), "findings": count }))
            })
            .collect();

        // Every row flat with scalar-only cells, so the queue renders as one
        // tabular block. `n` is the rank across the whole result, not within the
        // page, so an item keeps its number as the reader pages.
        let queue: Vec<Value> = shown
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let mut row = json!({
                    "n": offset + i + 1,
                    "priority": f.priority,
                    "rule": f.rule,
                    "class": f.class.as_str(),
                    "domain": f.domain,
                    "permalink": f.permalink,
                    "title": f.title,
                    "line": f.line,
                    "finding": f.finding,
                    "evidence": f.evidence,
                    "fix": f.fix,
                });
                // The acknowledgment columns ride along only when they say
                // something, so an ordinary queue row stays the flat shape every
                // renderer already knows.
                if f.acknowledged {
                    row["acknowledged"] = Value::Bool(true);
                }
                if f.ack_stale {
                    row["ack_stale"] = Value::Bool(true);
                }
                // The scope the acknowledgment was **given for**, which on a
                // stale row is deliberately not what the finding fires on now:
                // the row's own evidence and fix columns say that, and the pair
                // is what shows a reader why the acknowledgment stopped
                // matching. Named beside `ack_note` rather than plain `scope`,
                // which at the top level already names the swept domains.
                if let Some(scope) = f.ack_scope.as_deref().filter(|s| !s.is_empty()) {
                    row["ack_scope"] = Value::String(scope.to_string());
                }
                if let Some(note) = &f.ack_note {
                    row["ack_note"] = Value::String(note.clone());
                }
                row
            })
            .collect();

        // The prose instruction rides a per-rule legend rather than a column, so
        // a page of ten findings from one rule carries it once. Only the rules
        // on this page appear. The catalog's short `summary` rides beside it:
        // a renderer wants a few words for a heading and the instruction for
        // the body, and deriving one from the other is not a client's job.
        let actions: Vec<Value> = RULES
            .iter()
            .filter(|info| shown.iter().any(|f| f.rule == info.id))
            .map(|info| {
                json!({
                    "rule": info.id,
                    "summary": info.summary,
                    "instruction": info.instruction,
                })
            })
            .collect();

        Ok(json!({
            "scope": {
                "domains": scope,
                "families": families.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
                "rules": rules,
                "min_priority": p.min_priority,
                "today": today.to_string(),
            },
            "engrams_scanned": engrams_scanned,
            "unparsed": unparsed,
            "total": total,
            "page": page,
            "limit": limit,
            "count": queue.len(),
            "families": family_counts,
            "acknowledged": {
                "total": acknowledged.total,
                "by_family": {
                    "temporal": acknowledged.temporal,
                    "structure": acknowledged.structure,
                    "redundancy": acknowledged.redundancy,
                },
            },
            "queue": queue,
            "actions": actions,
            "guidance": EVOLVE_GUIDANCE,
            "truncations": truncations,
        }))
    }

    // --- acknowledgments -----------------------------------------------------

    /// What an `evolve_ack` assignment asks for, or `None` when this edit is
    /// not one. Pure and public, so a surface can put the act to a user before
    /// it happens without the engine having to guess at the wording.
    ///
    /// The value is the rule id optionally followed by a note, split at the
    /// first whitespace: `V101` or `V101 lineage citation, keep`. The rule has
    /// to be one the catalog knows, because an acknowledgment of a rule that
    /// does not exist can never suppress anything and silently storing it would
    /// read as work done. `remove V101` takes an entry back instead of
    /// recording one (see [`parse_ack_value`]).
    pub fn ack_intent(p: &EditParams) -> Result<Option<AckIntent>> {
        if p.operation != "set_frontmatter"
            || p.key.as_deref().map(str::trim) != Some(EVOLVE_ACK_KEY)
        {
            return Ok(None);
        }
        let raw = p.value.as_deref().map(str::trim).unwrap_or_default();
        parse_ack_value(raw).map(Some)
    }

    /// What an `evolve_ack` confirmation round names: `{domain, permalink}` for
    /// the engram the assignment would land on.
    ///
    /// Shaped after [`Engine::delete_preview`] and there for the same reason: a
    /// question is only worth putting to a user about a call that can run.
    /// Read-only is checked first and the identifier is resolved next, so a
    /// server that never writes, a domain nobody registered and an identifier
    /// nobody has each fail in round one - rather than collecting a yes and
    /// reporting the miss in round two, against a name the user already
    /// approved.
    ///
    /// It resolves and nothing else. The `expected_checksum` comparison and the
    /// "is that acknowledgment even there" test both read what the file holds,
    /// the file can change between the rounds, and both already run in the
    /// round that writes; repeating them here would buy a guarantee that does
    /// not survive the gap.
    pub async fn ack_preview(&self, p: &EditParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let (desc, _) = self.resolve(&p.identifier, Some(&p.domain)).await?;
        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
        }))
    }

    /// The acknowledgment an `evolve_ack` assignment is asking for, completed
    /// with the scope only the sweep can supply, or `None` when this edit is
    /// not one.
    ///
    /// The scope is the firing finding's, and its absence is meaningful: a rule
    /// that is not currently firing for this engram is acknowledged scope-less,
    /// which matches whatever it finds later. That is the honest reading of
    /// "acknowledge this before it appears".
    ///
    /// A removal needs none of that: it names an entry that is already on the
    /// engram, so it travels to the text edit as the rule id alone and the
    /// filtering happens there, under the lock, against what the file holds.
    async fn ack_draft(
        &self,
        p: &EditParams,
        desc: &EngramDescriptor,
        actor: &str,
    ) -> Result<Option<AckDraft>> {
        Ok(match Self::ack_intent(p)? {
            None => None,
            Some(AckIntent::Remove { rule }) => Some(AckDraft::Remove(rule)),
            Some(AckIntent::Record { rule, note }) => Some(AckDraft::Record(EvolveAck {
                scope: self
                    .firing_scope(&desc.domain, &desc.permalink, &rule)
                    .await?,
                rule,
                note,
                by: actor.to_string(),
                at: Some(now_offset()),
            })),
        })
    }

    /// What `rule` is currently firing on `permalink` for, as the scope an
    /// acknowledgment is matched against later. `None` when the rule is not
    /// firing, or when it fires with an empty scope because its identity is
    /// just the engram and the rule.
    ///
    /// Detection runs with the suppressed findings included, so
    /// re-acknowledging a finding an older entry already silences still sees
    /// the evidence it fires on and records the current scope rather than
    /// dropping to a scope-less entry.
    async fn firing_scope(
        &self,
        domain: &str,
        permalink: &str,
        rule: &str,
    ) -> Result<Option<String>> {
        let mut known_domains = self.known_domain_names();
        known_domains.sort();
        known_domains.dedup();
        let today = Utc::now().date_naive();
        let Some(swept) = self
            .sweep_domain(domain, today, &known_domains, true)
            .await?
        else {
            return Ok(None);
        };
        Ok(swept
            .report
            .findings
            .into_iter()
            .find(|f| f.rule == rule && f.permalink == permalink)
            .map(|f| f.scope)
            .filter(|scope| !scope.is_empty()))
    }

    /// Acknowledge a finding: record on the engram that this rule's finding was
    /// read and ruled intentional, so future sweeps count it rather than
    /// raising it. The Fluid half of the same act
    /// [`Engine::edit_engram_as`]'s `set_frontmatter` performs for an agent,
    /// through the one edit path, with the scope computed the one way.
    ///
    /// The rule is screened here as well as inside the edit, because this
    /// surface carries the rule and the note as separate fields and joins them
    /// into one value: the screen is what keeps a rule field that happens to
    /// read `remove` an unknown-rule refusal rather than a value the parser
    /// would take for a removal.
    ///
    /// **The screen runs before resolution by design, so an unknown rule
    /// answers before a missing engram does**: a request that gets both halves
    /// wrong is refused for the rule (a 422 on the REST surface) rather than
    /// for the permalink (a 404). The rule is the half the caller can fix
    /// without another lookup - the catalog is right there in the message - and
    /// an acknowledgment of a rule nobody has could not be recorded even on an
    /// engram that does exist.
    pub async fn acknowledge_finding_as(
        &self,
        domain: &str,
        identifier: &str,
        rule: &str,
        note: Option<&str>,
        client: Option<&str>,
    ) -> Result<Value> {
        let screened = rule.trim().to_ascii_uppercase();
        if rule_info(&screened).is_none() {
            return Err(EngineError::Invalid(unknown_rule_message(&screened)));
        }
        let value = match note.map(str::trim).filter(|n| !n.is_empty()) {
            Some(note) => format!("{} {note}", rule.trim()),
            None => rule.trim().to_string(),
        };
        let params = EditParams {
            identifier: identifier.to_string(),
            domain: domain.to_string(),
            operation: "set_frontmatter".to_string(),
            key: Some(EVOLVE_ACK_KEY.to_string()),
            value: Some(value),
            ..EditParams::default()
        };
        let result = self.edit_engram_as(&params, client).await?;
        Ok(result.get("evolve_ack").cloned().unwrap_or(Value::Null))
    }

    /// Withdraw an acknowledgment, leaving the engram's other entries alone.
    /// `false` when the engram carries none for that rule, which the surface
    /// answers as a 404 rather than pretending a removal happened.
    ///
    /// Fluid's half of the take-back an agent asks for with the `remove
    /// <rule-id>` value form; both filter through [`without_ack`], and they
    /// differ only in how an entry that is not there is reported.
    pub async fn unacknowledge_finding_as(
        &self,
        domain: &str,
        identifier: &str,
        rule: &str,
        client: Option<&str>,
    ) -> Result<bool> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let rule = rule.trim().to_ascii_uppercase();
        if rule_info(&rule).is_none() {
            return Err(EngineError::Invalid(unknown_rule_message(&rule)));
        }
        let actor = self.actor(client);
        let (desc, source) = self.resolve(identifier, Some(domain)).await?;
        // Checked before the write so an engram carrying no such entry answers
        // "nothing to withdraw" without a rewrite, a reindex or a touched
        // generated block.
        let current = self.load_source(&source, &desc).await?;
        if !has_ack(&current, &rule) {
            return Ok(false);
        }
        self.apply_source_edit(&desc, &source, None, &actor, |current| {
            Ok(without_ack(current, &rule))
        })
        .await?;
        Ok(true)
    }

    /// An engram's markdown as its domain holds it, whichever kind that is.
    async fn load_source(&self, source: &ContentSource, desc: &EngramDescriptor) -> Result<String> {
        match source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &desc.path);
                std::fs::read_to_string(&abs).map_err(|source| EngineError::Io {
                    path: abs.display().to_string(),
                    source,
                })
            }
            ContentSource::Virtual => {
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
        }
    }

    /// One domain's assembled facts, detected: the sweep's whole per-domain
    /// half, shared by [`Engine::evolve_detect`] and the acknowledgment write
    /// path, which needs the same verdict about one engram before it can record
    /// what a finding was acknowledged for.
    ///
    /// `Ok(None)` for a domain with no engrams: no domain row to query against
    /// and nothing to detect. An empty domain is quiet, not an error.
    async fn sweep_domain(
        &self,
        name: &str,
        today: NaiveDate,
        known_domains: &[String],
        include_acknowledged: bool,
    ) -> Result<Option<DomainSweep>> {
        let mut unparsed = 0usize;
        let source = self.content_source(name)?;
        let store = self.store.lock().await;
        let descs = store.list_engrams(name, None, None).await?;
        drop(store);
        // No engrams means no domain row to query against and nothing to
        // detect. An empty domain is quiet, not an error.
        let Some(domain_id) = descs.first().map(|d| d.domain_id) else {
            return Ok(None);
        };

        let graph = self.sweep_graph(&descs).await?;
        let mut inbound: HashMap<i64, usize> = HashMap::new();
        let mut outbound: HashMap<i64, usize> = HashMap::new();
        for edge in &graph.edges {
            *outbound.entry(edge.from.0).or_default() += 1;
            *inbound.entry(edge.to.0).or_default() += 1;
        }

        let store = self.store.lock().await;
        let unresolved = store.unresolved_refs(domain_id).await?;
        let vocab = store.vocabulary(Some(name)).await?;
        // Metadata only, one query: the attachment rules compare paths,
        // sizes and hashes and never read a byte of any file.
        let attachments = store.list_attachments(domain_id).await?;
        drop(store);

        let verify_config = domain_verify_config(&source);
        let mut facts: Vec<EngramFacts> = Vec::with_capacity(descs.len());
        for d in &descs {
            // Files-are-truth for a file domain, the stored content for a
            // virtual one. An engram that no longer parses is counted and
            // skipped rather than failing the whole sweep, since one broken
            // file must not hide every finding behind it.
            let Some(engram) = self.load_engram(&source, d.domain_id, &d.path).await else {
                unparsed += 1;
                continue;
            };
            let fm = &engram.frontmatter;
            let status = match fm
                .status
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(s) => s.to_ascii_lowercase(),
                None => d.status.trim().to_ascii_lowercase(),
            };
            let title = if fm.title.trim().is_empty() {
                d.title.clone()
            } else {
                fm.title.clone()
            };
            let tokens = engram.body.chars().count() / 4;
            facts.push(EngramFacts {
                id: d.id,
                domain: d.domain.clone(),
                permalink: d.permalink.clone(),
                title,
                path: d.path.clone(),
                status,
                engram_type: fm.engram_type.trim().to_ascii_lowercase(),
                tags: fm.tags.clone(),
                salience: yaml_number(fm.extra.get("salience")),
                recorded_at: fm.recorded_at,
                valid_from: fm.valid_from,
                valid_to: fm.valid_to,
                stale_on: fm.stale_on(),
                verified_on: fm.latest_verified().map(|v| v.at.date_naive()),
                tokens,
                token_budget: resolve_token_budget(verify_config.as_ref(), &d.path),
                inbound: inbound.get(&d.id.0).copied().unwrap_or(0),
                outbound: outbound.get(&d.id.0).copied().unwrap_or(0),
                generated_by: fm.generated.as_ref().map(|g| g.by.clone()),
                analyzes: asset_claim(fm),
                analyzed_hash: fm
                    .extra
                    .get("analyzed_hash")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|h| !h.is_empty())
                    .map(str::to_string),
                asset_refs: crystalline_core::find_asset_refs(&engram.body),
                acks: ack_entries(fm),
                body: engram.body,
            });
        }

        let input = SweepInput {
            domain: name.to_string(),
            today,
            engrams: facts,
            graph,
            unresolved,
            tags: vocab.tags,
            tag_aliases: vocab.aliases,
            known_domains: known_domains.to_vec(),
            attachments,
            share: self.share_facts(name).await,
            include_acknowledged,
            options: SweepOptions::default(),
        };
        let report = detect(&input);
        Ok(Some(DomainSweep { report, unparsed }))
    }

    /// What a domain owes its team origin, for `V009`, or `None` for a domain
    /// with no origin, no recorded origin state or no readable working tree.
    ///
    /// Offline by construction: [`crate::origin::unshared_work`] walks the tree
    /// against the base snapshot and never probes the forge, so a sweep costs
    /// the same whether the machine is connected or on a train. The rule reads
    /// substantive changes only, which is the filter that helper applies.
    ///
    /// Under the domain's [`Engine::origin_lock`], like every other origin
    /// read: the tree and `state.json` are compared against each other, and a
    /// share or a pull rewrites both. Unlocked, a sweep landing mid-share reads
    /// one of them from before the write and the other from after, and answers
    /// a count for a delta that never existed.
    async fn share_facts(&self, name: &str) -> Option<ShareFacts> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;
        let (_spec, root, state_dir) = self.origin_spec_for_domain(name).ok()?;
        let work = origin::unshared_work(&root, &state_dir)?;
        Some(ShareFacts {
            unshared: work.count(),
            oldest_change: work.oldest_change_date(),
        })
    }

    /// The resolved graph around a whole domain, at depth 1 so every
    /// cross-domain target carries a status.
    async fn sweep_graph(&self, descs: &[EngramDescriptor]) -> Result<GraphSlice> {
        let ids: Vec<EngramId> = descs.iter().map(|d| d.id).collect();
        self.sweep_neighbors(&ids, 1).await
    }

    /// [`Store::neighbors`] over a seed list of any size, merged into one slice.
    ///
    /// The seed list is chunked because both backends inline it into an SQL
    /// `IN (...)`, so a whole domain in one call would build a statement
    /// proportional to its size. Merging is exact rather than approximate at
    /// every depth the callers use: the traversal collects an edge whenever one
    /// of its ends lies within `depth - 1` hops of a seed, and a node whenever it
    /// lies within `depth` hops, and hop distance from the whole seed set is the
    /// smallest hop distance from any one chunk. The union over the chunks is
    /// therefore exactly what one unchunked call would have returned.
    ///
    /// Nodes and edges are deduped on the merge: a node reached from two chunks,
    /// and an edge whose two ends sit in different chunks, both come back more
    /// than once, and a double-counted edge would inflate the degrees the
    /// consolidation ranking and the orphan rule read. The merged nodes are
    /// sorted by id, so a chunked sweep answers in the same ascending order a
    /// single-chunk one does and every caller's ordering holds either way.
    async fn sweep_neighbors(&self, ids: &[EngramId], depth: u8) -> Result<GraphSlice> {
        let mut graph = GraphSlice::default();
        let mut seen_nodes: HashSet<i64> = HashSet::new();
        let mut seen_edges: HashSet<(i64, i64, String, u8)> = HashSet::new();
        for chunk in ids.chunks(NEIGHBOR_CHUNK) {
            let store = self.store.lock().await;
            let slice = store.neighbors(chunk, depth).await?;
            drop(store);
            for node in slice.nodes {
                if seen_nodes.insert(node.id.0) {
                    graph.nodes.push(node);
                }
            }
            for edge in slice.edges {
                let kind = match edge.kind {
                    EdgeKind::Relation => 0u8,
                    EdgeKind::Link => 1u8,
                };
                if seen_edges.insert((edge.from.0, edge.to.0, edge.rel_type.clone(), kind)) {
                    graph.edges.push(edge);
                }
            }
        }
        graph.nodes.sort_by_key(|n| n.id.0);
        Ok(graph)
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

    // --- vocabulary ----------------------------------------------------------

    /// List the tags, observation categories, relation types and engram `type`
    /// and `status` values already in use, each with a usage count, for one
    /// domain or across every domain. An unknown domain reports empty lists
    /// rather than erroring, matching the store contract, so an agent can probe a
    /// fresh domain safely. `domain` echoes the request, `null` for an all-domain
    /// sweep.
    pub async fn vocabulary(&self, p: &VocabularyParams) -> Result<Value> {
        let store = self.store.lock().await;
        let vocab = store.vocabulary(p.domain.as_deref()).await?;
        drop(store);
        // Every count list is present unconditionally, empty when nothing is in
        // use, so a client reads a list rather than testing for a missing key.
        // Only the two advisory keys below (clusters, aliases) are omitted when
        // they have nothing to say.
        let mut out = json!({
            "domain": p.domain,
            "tags": vocab.tags,
            "categories": vocab.categories,
            "relation_types": vocab.relation_types,
            "types": vocab.types,
            "statuses": vocab.statuses,
        });
        // Near-duplicate tag clusters, omitted entirely when there are none so a
        // clean vocabulary stays quiet. They point at tags to consolidate with
        // `crystalline tags merge`. Declared aliases are folded out first, so a
        // cluster an alias already explains is never reported.
        let clusters = crystalline_index::tag_clusters_with_aliases(&vocab.tags, &vocab.aliases);
        if !clusters.is_empty()
            && let Value::Object(map) = &mut out
        {
            map.insert(
                "clusters".to_string(),
                serde_json::to_value(&clusters).unwrap_or(Value::Null),
            );
        }
        // The tag aliases in effect, omitted when there are none. They tell an
        // agent which spellings fold onto which canonical tag.
        if !vocab.aliases.is_empty()
            && let Value::Object(map) = &mut out
        {
            map.insert(
                "aliases".to_string(),
                serde_json::to_value(&vocab.aliases).unwrap_or(Value::Null),
            );
        }
        Ok(out)
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

    /// Import in-memory engram files - an unpacked archive - into a domain of
    /// either storage kind, classifying every entry as `create`, `overwrite`,
    /// `skip`, `invalid` or `ignored`.
    ///
    /// One verb backs both the preview and the commit: `dry_run` runs the whole
    /// classification and writes nothing, so what an operator is shown is what
    /// the same call would then do. Each kind is written through its own normal
    /// road rather than a shortcut: a file domain gets the exact incoming bytes
    /// on disk and is indexed by one targeted [`Engine::sync_paths`] after the
    /// loop, the same pass the watcher runs for an external edit, and a virtual
    /// domain is indexed directly because the row is its only source of truth.
    /// Two files must never claim one permalink, so a permalink already held at
    /// a different path is refused under both policies - `overwrite` decides
    /// what happens at the SAME path, nothing more.
    pub async fn import_domain_files(
        &self,
        domain: &str,
        files: &[(String, String)],
        overwrite: bool,
        dry_run: bool,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // Delta 2 vs `import_domain`: a file domain is served too, so the source
        // decides how a write lands rather than being refused outright.
        let source = self.content_source(domain)?;
        let store = self.store.lock().await;
        let domain_id = match &source {
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
        let existing = store.all_engram_contents(domain_id).await?;
        drop(store);

        // The snapshot is taken once, before the loop, and then kept live as the
        // batch is classified: an entry accepted here claims its path and its
        // permalink for the rest of the batch, so two files of one archive
        // claiming one permalink are resolved deterministically in input order
        // (first wins, second skips) instead of both passing a stale snapshot.
        let mut path_perms: HashMap<String, String> = HashMap::new();
        let mut perm_paths: HashMap<String, String> = HashMap::new();
        for e in &existing {
            path_perms.insert(e.path.clone(), e.permalink.clone());
            perm_paths.insert(e.permalink.clone(), e.path.clone());
        }

        let mut created = 0usize;
        let mut overwritten = 0usize;
        let mut skipped = 0usize;
        let mut invalid = 0usize;
        let mut ignored = 0usize;
        // Delta 6: every entry gets a row, whatever became of it.
        let mut entries: Vec<Value> = Vec::new();
        let mut changed_paths: Vec<String> = Vec::new();

        // Delta 1: the files arrive in memory, already unpacked by the caller,
        // so there is no folder to walk and no source directory to validate.
        for (path, text) in files {
            // Delta 4: a MANIFEST at any depth is ignored - defense in depth,
            // the REST layer screens these before the engine ever sees them.
            // Matched case-insensitively because the filesystem underneath is:
            // on APFS or NTFS a `manifest.md` entry lands on the domain's real
            // MANIFEST.md, so an exact-string screen would let a third-party
            // archive replace the one file a domain cannot regenerate.
            if Path::new(path)
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("MANIFEST.md"))
            {
                entries.push(json!({
                    "path": path,
                    "permalink": Value::Null,
                    "action": "ignored",
                    "reason": "a MANIFEST belongs to the domain, which keeps its own",
                }));
                ignored += 1;
                continue;
            }
            // Only markdown is an engram. Anything else would land in the folder
            // as junk a sync never walks, or as a row no reader can parse, so it
            // is reported rather than written.
            if !path.to_lowercase().ends_with(".md") {
                entries.push(json!({
                    "path": path,
                    "permalink": Value::Null,
                    "action": "ignored",
                    "reason": "not a markdown engram file",
                }));
                ignored += 1;
                continue;
            }
            // The OKF reserved names are never imported: `index.md` is rebuilt
            // from the folder it sits in, and `log.md` is reserved without ever
            // being generated at all, so an import can only damage it.
            //
            // Matched case-insensitively, and deliberately stricter than
            // `crystalline_core::is_reserved_path` (whose exact match is a
            // documented rule about what Crystalline generates and exports).
            // Import faces the filesystem instead, and that filesystem is
            // case-insensitive on APFS and NTFS: a `Log.md` entry renames onto
            // the existing `log.md`, replacing its bytes while the on-disk name
            // stays lowercase. Nothing regenerates a log, so that loss is
            // permanent - the same argument that makes the MANIFEST screen
            // above case-insensitive.
            if Path::new(path).file_name().is_some_and(|name| {
                name.eq_ignore_ascii_case(crystalline_core::INDEX_FILE)
                    || name.eq_ignore_ascii_case(crystalline_core::LOG_FILE)
            }) {
                entries.push(json!({
                    "path": path,
                    "permalink": Value::Null,
                    "action": "ignored",
                    "reason": "a reserved OKF index or log is never imported",
                }));
                ignored += 1;
                continue;
            }
            // An archive is untrusted input and `join_rel` joins segment by
            // segment, `..` included, so containment is decided before any path
            // is built: an entry can never address a byte outside the domain.
            if !is_contained_rel(path) {
                entries.push(json!({
                    "path": path,
                    "permalink": Value::Null,
                    "action": "invalid",
                    "reason": "path escapes the domain root",
                }));
                invalid += 1;
                continue;
            }
            // Delta 5: unparseable content is a first-class `invalid` entry
            // carrying the parse error, not a warning on the side.
            let engram = match parse_engram(text) {
                Ok(e) => e,
                Err(e) => {
                    entries.push(json!({
                        "path": path,
                        "permalink": Value::Null,
                        "action": "invalid",
                        "reason": e.to_string(),
                    }));
                    invalid += 1;
                    continue;
                }
            };
            // `parse_engram` is deliberately permissive - a file with no
            // frontmatter at all parses into an engram with empty fields - so
            // the required OKF keys are checked here. Without them there is
            // nothing to import: the permalink would be invented from the file
            // name and the engram would carry no type.
            if engram.frontmatter.engram_type.trim().is_empty()
                || engram.frontmatter.title.trim().is_empty()
            {
                entries.push(json!({
                    "path": path,
                    "permalink": Value::Null,
                    "action": "invalid",
                    "reason": "not an engram: the frontmatter needs a type and a title",
                }));
                invalid += 1;
                continue;
            }
            let record = EngramRecord::from_engram(&engram, path, virtual_stamp(text));

            // Delta 3: a permalink held at a different path is refused under
            // BOTH policies - `overwrite` is a same-path decision only.
            if let Some(held_at) = perm_paths.get(&record.permalink)
                && held_at != path
            {
                entries.push(json!({
                    "path": path,
                    "permalink": record.permalink,
                    "action": "skip",
                    "reason": format!(
                        "permalink '{}' already exists at another path",
                        record.permalink
                    ),
                }));
                skipped += 1;
                continue;
            }
            // A file domain's truth is the file on disk, so an entry that never
            // made it into the index still counts as existing there.
            let exists = path_perms.contains_key(path)
                || match &source {
                    ContentSource::File { root } => join_rel(root, path).exists(),
                    ContentSource::Virtual => false,
                };
            if exists && !overwrite {
                entries.push(json!({
                    "path": path,
                    "permalink": record.permalink,
                    "action": "skip",
                    "reason": format!("'{path}' already exists"),
                }));
                skipped += 1;
                continue;
            }

            if !dry_run {
                let outcome = match &source {
                    // Delta 2: the exact incoming bytes go to disk and the index
                    // follows afterwards through the targeted sync, so an import
                    // takes the same road as any external write.
                    ContentSource::File { root } => {
                        write_file(&join_rel(root, path), text).map(|()| {
                            changed_paths.push(path.clone());
                        })
                    }
                    // A virtual domain has no file: the row is the document, so
                    // the full markdown is indexed directly.
                    ContentSource::Virtual => {
                        let stamp = virtual_stamp(text);
                        let store = self.store.lock().await;
                        self.index_markdown(&*store, domain_id, path, text, stamp, None, true)
                            .await
                            .map(|_| ())
                    }
                };
                if let Err(e) = outcome {
                    entries.push(json!({
                        "path": path,
                        "permalink": record.permalink,
                        "action": "skip",
                        "reason": e.to_string(),
                    }));
                    skipped += 1;
                    continue;
                }
            }

            if exists {
                overwritten += 1;
            } else {
                created += 1;
            }
            entries.push(json!({
                "path": path,
                "permalink": record.permalink,
                "action": if exists { "overwrite" } else { "create" },
                "reason": Value::Null,
            }));
            // Claim both for the rest of the batch. An overwrite that changes an
            // engram's permalink releases the one that path used to hold, so a
            // later entry is judged against what the batch will really leave
            // behind - in a dry run too, where the preview must match the commit.
            if let Some(prev) = path_perms.insert(path.clone(), record.permalink.clone())
                && prev != record.permalink
            {
                perm_paths.remove(&prev);
            }
            perm_paths.insert(record.permalink, path.clone());
        }

        // Delta 2, after the loop and only once: one targeted sync for the whole
        // batch, the pass the watcher runs for a small debounced set of paths,
        // rather than a full rescan or a per-file reindex. A dry run collected
        // no path here, so it never reaches the sync either.
        if !changed_paths.is_empty() {
            self.sync_paths(domain, changed_paths).await?;
        }

        Ok(json!({
            "domain": domain,
            "dry_run": dry_run,
            "files": entries,
            "created": created,
            "overwritten": overwritten,
            "skipped": skipped,
            "invalid": invalid,
            "ignored": ignored,
        }))
    }

    /// Every file of a domain as `(domain-relative path, bytes)`, MANIFEST and
    /// attachments included: the portable view an archive download is built
    /// from, byte for byte as the domain holds it.
    ///
    /// Bytes rather than text, and that is the whole reason for the type: an
    /// attachment is a PNG or a slide deck, and a collection of `String` could
    /// only carry a domain's markdown. Markdown entries are the same bytes they
    /// always were - UTF-8 is validated where a body is parsed, not here, since
    /// nothing on this path parses anything.
    ///
    /// Each storage kind is read from its own source of truth, which is why
    /// this is not simply `export_domain`'s read half. A file domain's truth is
    /// the markdown on disk, walked exactly the way a sync walks it: the index
    /// keeps only the body there, with the frontmatter shredded into columns,
    /// so reading the store would hand back headerless engrams and a MANIFEST
    /// that never indexed would go missing entirely. A virtual domain has no
    /// disk at all - the row IS the file, and it carries the full text.
    ///
    /// Attachments come last, and through the seam rather than off either
    /// source directly ([`Engine::attachment_list`] then
    /// [`Engine::attachment_read`]), so both kinds hand over the same bytes
    /// under the same `assets/` paths - which is what lets an export of one
    /// kind be imported as the other. An attachment whose row stands but whose
    /// bytes cannot be read (a file deleted behind the index) is logged and
    /// skipped, like an unreadable markdown file: a backup missing one file
    /// beats no backup.
    pub async fn domain_files(&self, domain: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let entry = self.domain_entry(domain)?;
        let mut files = match self.source_of(&entry) {
            ContentSource::File { root } => {
                let mut files = Vec::new();
                for (rel, abs) in walk_markdown(&root) {
                    // The OKF reserved names are excluded, as everywhere else:
                    // `index.md` is generated from the folder it sits in and
                    // regenerates itself wherever this archive is restored,
                    // and writing either name back is refused by design.
                    if crystalline_core::is_reserved_path(&rel) {
                        continue;
                    }
                    match std::fs::read(&abs) {
                        Ok(bytes) => files.push((rel, bytes)),
                        // One unreadable file must not deny the operator the
                        // rest of the backup: it is skipped and logged rather
                        // than failing the whole archive.
                        Err(e) => {
                            tracing::warn!("archive of '{domain}' skipped '{rel}': {e}");
                        }
                    }
                }
                files
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let domain_id = store
                    .upsert_domain(domain, None, DomainKind::Virtual)
                    .await?;
                let all = store.all_engram_contents(domain_id).await?;
                drop(store);
                all.into_iter()
                    .map(|e| (e.path, e.content.into_bytes()))
                    .collect()
            }
        };
        for row in self.attachment_list(domain).await? {
            match self.attachment_read(domain, &row.path).await {
                Ok((bytes, _)) => files.push((row.path, bytes)),
                Err(e) => {
                    tracing::warn!("archive of '{domain}' skipped '{}': {e}", row.path);
                }
            }
        }
        Ok(files)
    }

    /// Export every file of a domain (file or virtual) to `dest` as a normal
    /// filesystem engram folder. Refuses to write into a non-empty directory
    /// unless `force`; `dry_run` reports without writing.
    ///
    /// The read half is [`Engine::domain_files`], the same one the archive
    /// download uses, so an export is a copy of the domain rather than a
    /// re-serialization of the index: a file domain hands over its exact disk
    /// bytes (frontmatter included, MANIFEST included), a virtual domain the
    /// full text of every row, both hand over their attachments under
    /// `assets/`, and the OKF reserved names are excluded from both. Reading
    /// the store directly instead - the shape this verb had - wrote
    /// frontmatter-less markdown for file domains, since their index rows keep
    /// only the body, and silently dropped MANIFEST.md.
    ///
    /// Report shape follows from that source: `domain_files` carries
    /// `(path, bytes)` and no permalink column, so each row reports its
    /// path and byte count instead of the former path/permalink pair. Parsing
    /// every body back just to re-derive a permalink would re-introduce the
    /// re-serialization this verb exists to avoid, and no caller reads the
    /// field: the two callers (`ctl` and the daemonless CLI) print the report
    /// as-is.
    pub async fn export_domain(
        &self,
        domain: &str,
        dest: &Path,
        force: bool,
        dry_run: bool,
    ) -> Result<Value> {
        let all = self.domain_files(domain).await?;

        if !dry_run && dir_is_nonempty(dest) && !force {
            return Err(EngineError::Conflict(format!(
                "destination '{}' is not empty; pass force to overwrite",
                dest.display()
            )));
        }

        let mut written = 0usize;
        let mut files: Vec<Value> = Vec::new();
        for (path, content) in &all {
            files.push(json!({ "path": path, "bytes": content.len() }));
            if dry_run {
                continue;
            }
            let abs = join_rel(dest, path);
            write_bytes(&abs, content)?;
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
        let mut failed = Vec::new();
        // Two short store-lock windows per domain with the scan in between, so the
        // walk-and-hash pass of a large domain no longer blocks every concurrent
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
            let scan = match scan_domain(name, root, snapshot, &self.chunk_params).await {
                Ok(scan) => scan,
                Err(e) if only.is_none() => {
                    // One denied domain must not block the rest of the
                    // sweep; its error is carried in the result and the
                    // daemon log, and the next sync retries it.
                    tracing::warn!("sync of '{name}' skipped: {e}");
                    failed.push(json!({ "domain": name, "error": e.to_string() }));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let report = {
                let store = self.store.lock().await;
                apply_scan(&*store, domain, scan)
                    .await
                    .map_err(|e| EngineError::Internal(format!("sync of '{name}' failed: {e}")))?
            };
            // Files changed under us, so the generated index files follow.
            if changed_anything(&report) {
                self.refresh_index_files(name).await;
            }
            reports.push(report);
        }
        Ok(json!({
            "reports": serde_json::to_value(&reports).unwrap_or(Value::Null),
            "skipped": skipped,
            "failed": failed,
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
        // An out-of-band edit the watcher caught changes what the folder's
        // generated index should say.
        if changed_anything(&report) {
            self.refresh_index_files(name).await;
        }
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
        // same shape as `sync_take_over`, so a large domain's walk-and-hash pass
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
            let scan = scan_domain(&name, &root, snapshot, &self.chunk_params).await?;
            let report = {
                let store = self.store.lock().await;
                apply_scan(&*store, domain, scan).await.map_err(|e| {
                    EngineError::Internal(format!("reindex of '{name}' failed: {e}"))
                })?
            };
            if changed_anything(&report) {
                self.refresh_index_files(&name).await;
            }
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
        self.embed_pending_with_page(EMBED_PAGE_SIZE).await
    }

    /// [`Self::embed_pending`] with an explicit backlog page size. Production
    /// callers take [`EMBED_PAGE_SIZE`] through the wrapper; the parameter lets
    /// a test drive several pages over a small corpus.
    ///
    /// A batch the provider rejects is logged and skipped, not fatal: its chunks
    /// keep no embedding and stay in the backlog, visible in `status`, for a
    /// later pass, so one poisoned batch cannot starve the whole queue. Only
    /// store errors abort the pass.
    pub async fn embed_pending_with_page(&self, page_size: usize) -> Result<usize> {
        let Some(provider) = self.provider() else {
            return Ok(0);
        };
        let model = self.model_id.clone();
        let page_size = page_size.max(1);
        // In collaboration mode the scan is scoped to the file domains this
        // instance hosts plus all virtual domains, so a non-host does not
        // wastefully re-embed a chunk another instance owns; standalone it
        // embeds everything. The scope holds for the whole pass.
        let scope = {
            let store = self.store.lock().await;
            self.embed_scope(&*store).await?
        };
        // The backlog is walked one keyset page at a time so a large first index
        // never holds every chunk's text at once. The store lock is held only to
        // pull a page and to write vectors, never across the embed call.
        let mut embedded = 0usize;
        let mut cursor: Option<(i64, i64)> = None;
        let mut activity: Option<ActivityGuard> = None;
        loop {
            let mut jobs = {
                let store = self.store.lock().await;
                store
                    .chunks_needing_embedding(&model, scope.as_deref(), page_size, cursor)
                    .await?
            };
            if jobs.is_empty() {
                break;
            }
            // A short page is the last one. The cursor is taken from the store's
            // ordering, before the length sort reorders the page.
            let last_page = jobs.len() < page_size;
            cursor = jobs.last().map(|j| (j.engram_id, j.seq));
            // Length-sort so batches pay for their longest member once instead
            // of padding every short chunk out to whatever long one happened to
            // land in the same batch.
            order_jobs_for_batching(&mut jobs);
            if activity.is_none() {
                activity = Some(ActivityState::begin(&self.activity, "embed", None));
            }
            for batch in jobs.chunks(EMBED_BATCH) {
                let texts: Vec<String> = batch.iter().map(|j| j.text.clone()).collect();
                // A batch the provider cannot handle is logged and skipped: its
                // chunks keep no embedding, so they stay in the backlog for a
                // later pass instead of starving every batch behind them.
                let vectors = match provider.embed(&texts).await {
                    Ok(v) if v.len() == batch.len() => v,
                    Ok(v) => {
                        tracing::warn!(
                            chunks = ?batch.iter().map(|j| j.chunk_id).collect::<Vec<_>>(),
                            "skipping an embed batch: the provider returned {} vectors for {} inputs",
                            v.len(),
                            batch.len()
                        );
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(
                            chunks = ?batch.iter().map(|j| j.chunk_id).collect::<Vec<_>>(),
                            "skipping an embed batch the provider rejected: {e}"
                        );
                        continue;
                    }
                };
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
            if last_page {
                break;
            }
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
    ///
    /// A change that moves the MCP tool list is announced here, by
    /// [`Engine::announce_a_moved_tool_list`], because this method is the one
    /// seam all three settings callers share.
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
                let github_before = self.github_enabled();
                // The inner block scopes the lock guards, so they are released
                // before the announcement below awaits.
                let view = {
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
                    view
                };
                self.announce_a_moved_tool_list(github_before).await;
                Ok(view)
            }
            ConfigureAction::Unset { key } => {
                if self.read_only {
                    return Err(EngineError::ReadOnly);
                }
                let github_before = self.github_enabled();
                let view = {
                    // Same write-lock-first discipline and lock order as Set above.
                    let mut file_guard = self.file_config.write().unwrap();
                    let mut file = file_guard.clone();
                    settings::unset(&mut file, key)?;
                    self.persist_config(&file)?;
                    let effective = self.overlay.apply(&file);
                    let view = self.setting_view_json(&file, key);
                    *file_guard = file;
                    *self.config.write().unwrap() = effective;
                    view
                };
                self.announce_a_moved_tool_list(github_before).await;
                Ok(view)
            }
        }
    }

    /// Announce a moved `tools/list` on every open subscription stream, when
    /// the write that just landed changed what `github.enabled` effectively
    /// reads.
    ///
    /// `github.enabled` gates the listing of the five GitHub collaboration
    /// tools (`crate::mcp`'s `hidden_collab_tool`), so a settings write that
    /// flips it is the one thing on this server that moves a tool list - and
    /// the one that owes an announcement. It lives on the engine rather than
    /// in the MCP handler because three callers write that setting and all
    /// three move every connected peer's list: the `configure` MCP tool,
    /// `crystalline config set` over the control socket (`crate::control`) and
    /// Fluid's Connect button through the REST API
    /// (`crate::rest::github_settings`'s `ensure_enabled`). All three go
    /// through [`Engine::configure`], so putting it here is what makes the
    /// notification unconditional on the route taken.
    ///
    /// The setting is read either side of the write rather than parsed out of
    /// the request: a key can be set to the value it already had, unset back
    /// to the default, or overridden by the environment, and only the
    /// effective setting decides what the next `tools/list` returns.
    ///
    /// It reaches subscribers only. MCP 2026-07-28 removed the unsolicited
    /// channel outright, so a legacy peer - which cannot subscribe at all - is
    /// told nothing and re-reads the list at its own discretion. See
    /// [`crate::subscribers`].
    async fn announce_a_moved_tool_list(&self, github_before: bool) {
        if self.github_enabled() != github_before {
            self.list_subscribers().notify_tool_list_changed().await;
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
    /// section in its MANIFEST, the gate on the `provision` MCP tool's
    /// mutating actions: with no such domain, `allow`, `deny` and `apply`
    /// refuse rather than report a reconcile that touched nothing. It used to
    /// gate the tool's visibility instead, which MCP 2026-07-28 forbids
    /// (SEP-2567: a tool list must not vary as a side effect of other requests
    /// on the connection, and `add_domain` and `update_domain` can create a
    /// declaration mid-session). Wraps
    /// [`crystalline_core::provision::any_domain_declares`] against the live
    /// effective config, read fresh off the config lock on every call rather
    /// than cached - the same cost class as `routing_text`, since a domain's
    /// MANIFEST can gain or lose a `Provisioning` section between calls (a
    /// freshly added domain, or an `update_domain` pull) and the very next
    /// `provision` call must reflect that.
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

    /// Unregister a domain: the config entry goes, the watcher and discovery
    /// forget it and its index rows are cleared so search stops serving it.
    /// Files are never touched - for a file domain they stay on disk exactly
    /// as they are (re-adding the folder re-adopts them); a virtual domain's
    /// rows ARE its truth, so callers should export first and their
    /// confirmation copy must say the knowledge is gone. "Index rows cleared"
    /// means the engram rows only: the store's domain row itself is left in
    /// place (`Store::clear_domain` keeps it by design), so a later re-add of
    /// the same name adopts the same row rather than minting a new one.
    ///
    /// Known race: the config write (name freed) is persisted and both config
    /// locks release before the tail runs `forget_domain` and `clear_domain`.
    /// No lock in this engine currently serializes `domain_remove` against a
    /// concurrent `domain_add_local`/`domain_add_virtual` for the same name
    /// (the daemon spawns each connection independently, and the admin verbs
    /// only hold `file_config`/`config` for their brief mutate-and-persist
    /// step, not the whole call - `origin_lock` exists but serializes only
    /// the origin verbs against each other, not these). A same-name add
    /// racing into that window has its fresh watcher registration dropped and
    /// its freshly-indexed rows wiped by this call's tail, since both resolve
    /// the same `DomainId` by name. Closing this needs the add verbs to take
    /// the same per-name lock this verb would need to hold across its own
    /// tail, which is a cross-verb change out of scope here; a caller that
    /// cannot tolerate the window should serialize admin mutations for a
    /// given name at its own layer - which the REST surface does, in
    /// `RestState::domain_admin`: one mutex held across the whole of a create
    /// and the whole of an unregister.
    pub async fn domain_remove(&self, name: &str) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }

        // Same file-lock persist dance as `domain_add_local`: file_config
        // write lock, clone, mutate, persist_config, overlay.apply, write
        // both locks - same order. The miss is classified before persisting
        // anything. Scoped in a block, mirroring the sibling verbs, so the
        // guard is released well before the awaits that follow.
        let removed = {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = file_guard.clone();
            let removed = match file.domains.shift_remove(name) {
                Some(entry) => entry,
                None => {
                    // A miss in the file config may be an env-defined domain:
                    // those are immune to `domain_remove` (the variable is
                    // their source of truth), mirroring `cmd::domain_remove`.
                    if let Some(env) = self.overlay.env_domain(name) {
                        return Err(EngineError::Conflict(format!(
                            "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                            env.var
                        )));
                    }
                    return Err(EngineError::UnknownDomain {
                        domain: name.to_string(),
                        registered: self.known_domain_names(),
                    });
                }
            };
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
            removed
        };
        // Record whether the entry was a file domain before removing it.
        let files_kept = !removed.is_virtual();

        // Tell the runtime: drop it from the discovered overlay and stop
        // watching its root.
        self.forget_domain(name);

        // Index rows: resolve the DomainId the way `reindex(full)` does
        // before it calls `clear_domain` and clear them. The domain row
        // stays either way (idempotent upsert); only the engram rows matter.
        let kind = if removed.is_virtual() {
            DomainKind::Virtual
        } else {
            DomainKind::File
        };
        let path = removed.file_path();
        let path_str = path.as_ref().map(|p| p.to_string_lossy());
        let store = self.store.lock().await;
        let domain_id = store.upsert_domain(name, path_str.as_deref(), kind).await?;
        store.clear_domain(domain_id).await?;
        drop(store);

        self.refresh_routing_cache().await;

        Ok(json!({
            "domain": name,
            "unregistered": true,
            "files_kept": files_kept,
            "index_cleared": true,
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
        // Every still-open proposal rides along in full, review feedback
        // included: an update is where a pull refreshes it, so this is the
        // channel an agent reads reviewer comments from without a second call.
        let open_proposals: Vec<Value> = state
            .as_ref()
            .map(|s| {
                s.proposals
                    .iter()
                    .filter(|p| p.status == crystalline_remote::state::ProposalStatus::Open)
                    .map(|p| serde_json::to_value(p).expect("a proposal serializes"))
                    .collect()
            })
            .unwrap_or_default();
        let mut v = origin::pull_report_json(name, &report, proposals);
        v["open_proposals"] = Value::Array(open_proposals);
        Ok(v)
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
        let connection = self.origin_status_connection().await?;

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
    ///
    /// One thing this read is not allowed to do is settle an owed stack link.
    /// [`ops::status`] can pay that debt off with `create_stack`/`extend_stack`,
    /// which are forge WRITES, and the credential it would spend is the probe's
    /// own: the instance one, since a status carries no actor. In instance mode
    /// that is the credential every write goes out on anyway and the settlement
    /// runs exactly as it always has; in personal mode it would be the one
    /// instance-credential write the wave promises never happens, so permission
    /// is withheld and the debt stays recorded until the next share, amend or
    /// withdrawal pays it off on the acting identity's own credential.
    async fn origin_status_one(&self, name: &str, entry: &DomainEntry) -> Result<Value> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for(name, entry)?;
        // A probe is best-effort: no connection, or a provider that fails to
        // build, must never turn a status call into a hard failure.
        let probe = self.resolve_origin_provider().ok();
        let settle_owed_link = {
            let config = self.config.read().unwrap();
            config.github_stacks() && config.github_share_identity() == ShareIdentityMode::Instance
        };
        match ops::status(&spec, &root, &state_dir, probe.as_deref(), settle_owed_link).await {
            Ok(report) => Ok(origin::status_report_json(name, &report, None)),
            Err(e) if probe.is_some() && origin::is_probe_transport_error(&e) => {
                // AuthExpired is one of the transport errors this arm catches
                // (see `origin::is_probe_transport_error`), so a probe that
                // failed because the token was revoked drops the cached
                // credential here too; the retry below runs probe-free, so
                // status still comes back offline.
                self.drop_github_credential_on_auth(&e);
                let report = ops::status(&spec, &root, &state_dir, None, settle_owed_link).await?;
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
            // No provider, so nothing here could settle an owed stack link
            // anyway; withholding the permission says so at the call rather
            // than leaving it to be inferred from the `None` beside it.
            let Ok(report) = ops::status(&spec, &root, &state_dir, None, false).await else {
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
    /// what needs resolving. The share itself never touches local files, but
    /// the pull it opens with does, so it ends with the same sync and embed
    /// tail `origin_update_one` runs (see
    /// `Engine::index_what_the_share_pull_applied`).
    ///
    /// `proposal` names an open layer to amend instead of letting the share
    /// pick its own target; `None` is the ordinary call.
    ///
    /// `files` narrows what the share carries to those domain-relative paths,
    /// the generated listings of their folders riding along; `None` is the
    /// whole unshared delta. A path that is not among the domain's unshared
    /// changes refuses the share by name (see
    /// `crystalline_remote::ops::ShareOptions::files`).
    ///
    /// `actor` is who the share runs as, which decides the credential the
    /// forge writes go out on (see [`Engine::resolve_share_provider`]); it is
    /// inert while `github.share_identity` is `instance`, the default.
    ///
    /// The login that credential was connected as is recorded on the proposal
    /// this share creates or rewrites, in both modes
    /// (`crystalline_remote::state::Proposal::author_login`), so a chain whose
    /// layers belong to different people can say so.
    pub async fn origin_share(
        &self,
        domain: &str,
        title: Option<&str>,
        description: Option<&str>,
        proposal: Option<u64>,
        files: Option<&[String]>,
        actor: ShareActor,
    ) -> Result<Value> {
        let stacks_allowed = {
            let config = self.config.read().unwrap();
            if !config.github_enabled() {
                return Err(RemoteError::NotEnabled.into());
            }
            config.github_stacks()
        };
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let (provider, login) = self.resolve_share_provider(&actor)?;
        let acting = self.personal_write_login(login.as_deref());
        match ops::propose(
            provider.as_ref(),
            &spec,
            &root,
            domain,
            &state_dir,
            ops::ShareOptions {
                title,
                description,
                proposal,
                stacks_allowed,
                // Who the proposal record names, in either identity mode: the
                // credential this share actually went out on. `None` only when
                // that credential carries no login (the environment token).
                author_login: login.as_deref(),
                files,
            },
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))
        {
            Ok(outcome) => {
                self.index_what_the_share_pull_applied(domain, "sharing")
                    .await;
                Ok(origin::propose_outcome_json(&outcome))
            }
            Err(RemoteError::ConflictsPending { count }) => {
                // The conflicts refusal is the loudest case for syncing: the
                // pull ran, applied everything that merged cleanly and only
                // then refused, so this shape carries the most unindexed work
                // of any share outcome.
                self.index_what_the_share_pull_applied(domain, "sharing")
                    .await;
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
            Err(e) => Err(enrich_write_error(e, acting.as_deref(), &spec.repo).into()),
        }
    }

    /// Indexes whatever the pull inside a share or a preview wrote to the
    /// working tree.
    ///
    /// Both `ops::propose` and `ops::propose_preview` pull first - freshness is
    /// part of proposing honestly - and a pull applies upstream files. Without
    /// this, those files sit on disk unsearchable until the poller's next tick
    /// happens to sync them, which is `origin_update_one`'s bug with a
    /// different call in front of it. Run unconditionally rather than only when
    /// something looks applied: the sync is incremental, so a pull that changed
    /// nothing costs a cheap no-op scan, and there is no cheaper signal here
    /// that is also correct (a preview reports the share's plan, not the pull's
    /// effect).
    ///
    /// Best effort in the same sense `origin_update_one`'s embed tail is: a
    /// failure is logged, never turned into a failed share whose proposal is
    /// already open on the forge.
    async fn index_what_the_share_pull_applied(&self, domain: &str, verb: &str) {
        if let Err(e) = self.sync(Some(domain)).await {
            tracing::warn!("syncing '{domain}' after {verb} failed: {e}");
            return;
        }
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after {verb} '{domain}' failed: {e}");
        }
    }

    /// Previews what a share of one domain would do, under its origin lock,
    /// without making a single provider write.
    ///
    /// The same three gates `origin_share` applies apply here: a preview runs
    /// the real pull first (freshness is part of previewing honestly), so it
    /// writes the working tree and is refused on a read-only instance exactly
    /// as a share is - and, for the same reason, it ends with the same sync and
    /// embed tail (see `Engine::index_what_the_share_pull_applied`).
    ///
    /// It carries the share's own credential resolution too, `actor` and all:
    /// a preview that resolved a different identity than the share would could
    /// promise a plan this instance then refuses to perform.
    ///
    /// `credential` is the one place that resolution bends, and only for the
    /// personal-token refusal (see [`PreviewCredential`]): a caller that asks
    /// for [`PreviewCredential::ReadScopeFallback`] gets the plan computed on
    /// the instance credential when the acting identity has connected none of
    /// its own, which is the browser's case - the checkbox list a person picks
    /// files in is fed by this call, so refusing it would make connecting a
    /// hoop in front of an unknown. Every other refusal stands for both
    /// callers, the acting login is `None` on that path (nothing personal was
    /// resolved to name), and the share itself still refuses.
    pub async fn origin_share_preview(
        &self,
        domain: &str,
        title: Option<&str>,
        proposal: Option<u64>,
        files: Option<&[String]>,
        actor: ShareActor,
        credential: PreviewCredential,
    ) -> Result<Value> {
        let stacks_allowed = {
            let config = self.config.read().unwrap();
            if !config.github_enabled() {
                return Err(RemoteError::NotEnabled.into());
            }
            config.github_stacks()
        };
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let (provider, login) = match self.resolve_share_provider(&actor) {
            Ok(resolved) => resolved,
            Err(e)
                if credential == PreviewCredential::ReadScopeFallback
                    && is_personal_token_missing(&e) =>
            {
                (self.resolve_origin_provider()?, None)
            }
            Err(e) => return Err(e),
        };
        let acting = self.personal_write_login(login.as_deref());
        let plan = ops::propose_preview(
            provider.as_ref(),
            &spec,
            &root,
            domain,
            &state_dir,
            ops::ShareOptions {
                title,
                description: None,
                proposal,
                stacks_allowed,
                // Carried for the same reason the provider is: a preview
                // resolves exactly what the share would. It records nothing.
                author_login: login.as_deref(),
                files,
            },
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))
        .map_err(|e| enrich_write_error(e, acting.as_deref(), &spec.repo))?;
        self.index_what_the_share_pull_applied(domain, "previewing a share")
            .await;
        Ok(origin::share_plan_json(&plan, &root))
    }

    /// Previews which proposal a withdrawal would take out, without touching
    /// the forge at all.
    ///
    /// A pure local read: the offline status path ([`ops::status`] with no
    /// probe) reports this domain's open and declined proposals off origin
    /// state, and [`origin::withdraw_plan_json`] resolves the target out of
    /// that exactly as [`ops::withdraw`] would, refusing with the same
    /// teaching errors when no single target can be named. Nothing is written
    /// and no provider call is made, which is what lets an eliciting client
    /// ask its user before a pull request is closed.
    ///
    /// It still carries the withdrawal's own gates, all of them and in the
    /// same order - collaboration off, read-only, an unregistered domain, and
    /// a provider this instance cannot build - rather than only the read's, so
    /// a user is never asked to confirm a withdrawal this instance would
    /// refuse to perform. The provider is resolved and dropped: an instance
    /// with no credential on file has to fail in round one, where the failure
    /// is still the answer to the call, rather than after the user has said
    /// yes to a question.
    ///
    /// The provider it resolves and drops is the withdrawal's own, `actor`
    /// included, so an instance that shares personally refuses here - before
    /// the question - when the acting identity has no connection of its own.
    pub async fn origin_withdraw_preview(
        &self,
        domain: &str,
        proposal: Option<u64>,
        revert: bool,
        actor: ShareActor,
    ) -> Result<Value> {
        let stacks_allowed = {
            let config = self.config.read().unwrap();
            if !config.github_enabled() {
                return Err(RemoteError::NotEnabled.into());
            }
            config.github_stacks()
        };
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let (_provider, _login) = self.resolve_share_provider(&actor)?;
        // Probe-free, so no forge call of any kind: the settlement permission
        // is withheld for the same reason the provider was dropped.
        let report = ops::status(&spec, &root, &state_dir, None, false).await?;
        Ok(origin::withdraw_plan_json(
            &report,
            proposal,
            revert,
            stacks_allowed,
        )?)
    }

    /// Withdraws a share proposal for one domain: closes its pull request on
    /// the forge, best-effort deletes its branch, optionally restores the
    /// shared files (`revert`) and records it as withdrawn. Under the
    /// domain's origin lock; syncs and embeds afterward only when files
    /// moved. Refuses when collaboration is off and on a read-only instance.
    ///
    /// `actor` decides the credential the close and the branch delete go out
    /// on, exactly as it does for a share.
    pub async fn origin_withdraw(
        &self,
        domain: &str,
        proposal: Option<u64>,
        revert: bool,
        actor: ShareActor,
    ) -> Result<Value> {
        let stacks_allowed = {
            let config = self.config.read().unwrap();
            if !config.github_enabled() {
                return Err(RemoteError::NotEnabled.into());
            }
            config.github_stacks()
        };
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let (provider, login) = self.resolve_share_provider(&actor)?;
        let acting = self.personal_write_login(login.as_deref());
        let report = ops::withdraw(
            provider.as_ref(),
            &spec,
            &root,
            &state_dir,
            proposal,
            revert,
            stacks_allowed,
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))
        .map_err(|e| enrich_write_error(e, acting.as_deref(), &spec.repo))?;

        if !report.restored.is_empty() || !report.deleted.is_empty() {
            self.sync(Some(domain)).await?;
            if !self.request_embed()
                && let Err(e) = self.embed_pending().await
            {
                tracing::warn!(
                    "embedding after withdrawing proposal #{} for '{domain}' failed: {e}",
                    report.number
                );
            }
        }
        Ok(origin::withdraw_report_json(&report))
    }

    /// One conflict's full detail: both recorded sides plus the current local
    /// content, addressed by id or by path (exactly one must be given; if
    /// both arrive the id wins and the path is ignored, never mixed, so an id
    /// lookup can never be answered by some other conflict that happens to
    /// match the path). Neither is `EngineError::Invalid` rather than a
    /// misleading not-found. Sides are returned as UTF-8 strings; a side that
    /// exists but is not UTF-8 comes back null with `note` saying so. A pure
    /// read: no gate beyond the domain being registered with an origin, no
    /// lock needed.
    pub async fn origin_conflict_detail(
        &self,
        domain: &str,
        id: Option<&str>,
        path: Option<&str>,
    ) -> Result<Value> {
        if id.is_none() && path.is_none() {
            return Err(EngineError::Invalid(
                "origin_conflict_detail needs an id or a path".to_string(),
            ));
        }
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let state = crystalline_remote::state::OriginState::load(&state_dir)?.ok_or_else(|| {
            EngineError::Invalid(format!("domain '{domain}' has no origin state"))
        })?;
        let conflict = state
            .conflicts
            .iter()
            .find(|c| match (id, path) {
                (Some(id), _) => c.id == id,
                (None, Some(path)) => c.path == path,
                (None, None) => false,
            })
            .cloned()
            .ok_or_else(|| {
                EngineError::NotFound(format!(
                    "no open conflict {} for '{domain}'",
                    id.or(path).unwrap_or("(none named)")
                ))
            })?;
        let (base, upstream) =
            crystalline_remote::state::read_conflict_files(&state_dir, &conflict.id)?;
        let local_path = root.join(&conflict.path);
        let local = match std::fs::read(&local_path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(EngineError::Io {
                    path: local_path.display().to_string(),
                    source: e,
                });
            }
        };
        let mut note: Option<String> = None;
        let base_v = utf8_side(base, "base", &mut note);
        let local_v = utf8_side(local, "local", &mut note);
        let upstream_v = utf8_side(upstream, "upstream", &mut note);
        Ok(json!({
            "id": conflict.id,
            "path": conflict.path,
            "kind": conflict.kind,
            "detected_at": conflict.detected_at,
            "base": base_v,
            "local": local_v,
            "upstream": upstream_v,
            "note": note,
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
    ///
    /// `_actor` completes the write-verb signature every surface passes an
    /// actor to, and is deliberately unused: resolving writes this machine's
    /// working tree and its origin state and makes no provider call at all, so
    /// there is no credential to resolve and nothing for an identity to change.
    /// The resolved content reaches the forge later, on the next share, under
    /// whoever performs that.
    pub async fn origin_resolve(
        &self,
        domain: &str,
        path: &str,
        keep: Option<&str>,
        content: Option<&[u8]>,
        _actor: ShareActor,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let resolution = origin::resolution_from(keep, content)?;
        let lock = self.origin_lock_registered(domain)?;
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
    /// `origin_share`, `origin_withdraw` and `origin_resolve`: each a
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

    /// [`Engine::origin_lock`] for the single-domain operations whose next step
    /// requires a registered domain: the name is checked first, so a lock entry
    /// is never created for a name that is about to fail anyway. The error is
    /// exactly the `UnknownDomain` [`Engine::origin_spec_for_domain`] would
    /// raise a line later, so a registered name behaves identically and an
    /// unregistered one answers the same, only without leaving an entry behind.
    fn origin_lock_registered(&self, domain: &str) -> Result<Arc<tokio::sync::Mutex<()>>> {
        self.domain_entry(domain)?;
        Ok(self.origin_lock(domain))
    }

    /// The per-domain lock serializing origin operations for one domain
    /// name, created lazily on first use. Callers that already hold a
    /// `DomainEntry`, and `origin_add` (whose domain may not be registered
    /// yet), use this directly; every other single-domain caller goes through
    /// [`Engine::origin_lock_registered`].
    fn origin_lock(&self, domain: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.origin_locks.lock().unwrap();
        locks
            .entry(domain.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// The per-file lock every content write holds across its whole
    /// read-decide-write, created lazily on first use and keyed by the file's
    /// canonical path.
    ///
    /// What it closes is a time-of-check-to-time-of-use race, not a partial
    /// write. Each of the file-domain writes looks at the world and then acts
    /// on what it saw, and between those two steps another writer fits:
    ///
    /// - [`Engine::save_engram`], [`Engine::save_manifest`] and
    ///   [`Engine::delete_engram`] read the file, hash it and compare that
    ///   against the caller's `expected_checksum`. Unlocked, two saves of one
    ///   engram arriving together (two browser tabs, or one tab whose autosave
    ///   overlaps a manual save) both read the same text, both find their token
    ///   fresh and both write. One author's version then wins on disk while the
    ///   other is told the save succeeded, which is precisely the outcome
    ///   `If-Match` exists to prevent.
    /// - [`Engine::write_engram_as`] checks that the permalink is free and then
    ///   creates the file. Two creates of one title would both find it free and
    ///   both write, and the second would answer 201 over the first's body
    ///   rather than the 409 that says it was already taken.
    /// - [`Engine::edit_engram_as`] and [`Engine::retire_engram_as`] serialize
    ///   their read-modify-write under the lock: each reads the file, applies
    ///   its operation to that text and writes the result. Two edits, or an
    ///   agent's edit racing a browser's save, would each compute from a
    ///   version the other has already replaced, and the last write would
    ///   silently drop the other's change; under the lock the second one reads
    ///   the first's result and applies to that instead. `edit_engram_as`
    ///   additionally compares an `expected_checksum` there when one is
    ///   supplied (see [`crate::params::EditParams`]), refusing a stale edit
    ///   rather than merely serializing it. A retirement takes its successor's
    ///   lock too, for the reciprocal line it appends there, but never at the
    ///   same time as its target's.
    ///
    /// Held across the whole sequence, the second caller sees the first's bytes
    /// and either refuses or builds on them.
    ///
    /// Keyed by the file's own identity rather than by `domain/permalink`, so
    /// two domains registered over one root, or over two spellings of one path,
    /// still serialize on the file itself: the key is
    /// [`canonicalize`](std::fs::canonicalize)d where the filesystem can
    /// resolve it, which covers symlinks and `..` segments, and falls back to
    /// the path as given for a file that does not exist yet - a create and a
    /// save of one engram therefore share a key only once the file is there,
    /// which is exactly when both are reading it. Taken before the store lock,
    /// always, so the two never invert; virtual domains take neither, since
    /// their compare-and-swap happens inside a single database statement.
    ///
    /// The map is never pruned, like [`Engine::origin_locks`]: an entry is a
    /// path string and an `Arc`, and the set of files a process ever writes is
    /// bounded by the installation.
    ///
    /// **In-process only.** Two Crystalline processes over one domain root are
    /// not held apart by this; the host-lock machinery governs that.
    fn write_lock(&self, abs: &Path) -> Arc<tokio::sync::Mutex<()>> {
        // Computed before the map-wide guard is taken: `canonicalize` is a
        // blocking stat, and every other file's lookup would otherwise queue
        // behind it while it resolves this one's path.
        let key = lock_key(abs);
        let mut locks = self.write_locks.lock().unwrap();
        locks
            .entry(key)
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

    /// Resolves the provider a WRITE verb runs its GitHub calls through, plus
    /// the login it acts as (the acting `StoredToken.user`, for a proposal's
    /// recorded author).
    ///
    /// The read side is [`Engine::resolve_origin_provider`] and never moves.
    /// This one splits by `github.share_identity`, read LIVE on every call
    /// rather than snapshotted at start: the setting is `startup_effective:
    /// false`, so a mode flipped through `configure` is honoured by the very
    /// next share with no restart.
    ///
    /// - `instance` (the default): byte for byte what the read side does, the
    ///   one instance credential, whoever the actor is.
    /// - `personal`: the actor's own credential -
    ///   [`ShareActor::Owner`] the fixed `owner` slot, [`ShareActor::Account`]
    ///   that account's, [`ShareActor::HttpAgent`] the account
    ///   `github.agent_identity` names. No personal token on file refuses with
    ///   a teaching text; there is no fallback to the instance credential, by
    ///   design (spec section 6).
    ///
    /// The test provider override short-circuits BOTH modes: an injected mock
    /// has no credential behind it to read a login off, so the login it acts as
    /// is whatever [`Engine::with_origin_provider_login`] supplied beside it -
    /// `None` unless a test asked for one, which is the same answer as a
    /// credential that names nobody.
    fn resolve_share_provider(
        &self,
        actor: &ShareActor,
    ) -> Result<(Arc<dyn Provider>, Option<String>)> {
        if let Some(p) = &self.origin_provider_override {
            return Ok((p.clone(), self.origin_provider_override_login.clone()));
        }
        let (api_url, token) = self.resolve_share_credential(actor)?;
        let login = token.user_display().map(str::to_string);
        Ok((
            Arc::new(GitHubProvider::new(api_url, Some(token.access_token))),
            login,
        ))
    }

    /// The credential half of [`Engine::resolve_share_provider`]: the api url
    /// and the token a write goes out on, before an HTTP client exists.
    ///
    /// Split out because this is where every decision lives - the mode, the
    /// actor mapping, the two refusals - while building the client is
    /// mechanical, and because a `reqwest` client build loads the platform
    /// trust store, which is slow enough to be worth keeping out of the tests
    /// that exercise this matrix.
    fn resolve_share_credential(
        &self,
        actor: &ShareActor,
    ) -> Result<(Option<String>, StoredToken)> {
        let (api_url, mode, agent_identity) = {
            let config = self.config.read().unwrap();
            (
                config.github.as_ref().and_then(|g| g.api_url.clone()),
                config.github_share_identity(),
                config.github_agent_identity().map(str::to_string),
            )
        };
        let identity = match mode {
            ShareIdentityMode::Instance => TokenIdentity::Instance,
            ShareIdentityMode::Personal => {
                TokenIdentity::Personal(self.acting_identity_name(actor, agent_identity)?)
            }
        };
        let host = origin::token_host(api_url.as_deref());
        let (_store, token) = self.github_credential_for(&identity, host.as_deref())?;
        let token = match token {
            Some(token) => token,
            // The two absences are different failures and read differently: an
            // instance with no credential at all is simply not connected, while
            // an instance that shares personally and holds no token for THIS
            // identity is connected and still refusing, which is the case that
            // needs teaching.
            None if identity == TokenIdentity::Instance => {
                return Err(RemoteError::NotConnected.into());
            }
            None => return Err(RemoteError::Refused(PERSONAL_TOKEN_MISSING.to_string()).into()),
        };
        Ok((api_url, token))
    }

    /// The login a write failure is enriched in the name of: the acting login
    /// when this instance shares personally, `None` otherwise.
    ///
    /// Instance-token failures keep today's texts (spec section 8), so the
    /// teaching in [`enrich_write_error`] must not fire for them - and the mode
    /// is read live here for the same reason it is read live in
    /// [`Engine::resolve_share_provider`], one call after it, so the two agree
    /// about which credential the call in flight actually used.
    fn personal_write_login(&self, login: Option<&str>) -> Option<String> {
        match self.config.read().unwrap().github_share_identity() {
            ShareIdentityMode::Personal => login.map(str::to_string),
            ShareIdentityMode::Instance => None,
        }
    }

    /// The personal identity name a write runs under, in personal mode.
    ///
    /// The machine owner has no account to be, so it gets the one fixed local
    /// name; an account is itself; an HTTP-MCP agent is whoever
    /// `github.agent_identity` names, or a refusal that says which setting to
    /// write.
    fn acting_identity_name(
        &self,
        actor: &ShareActor,
        agent_identity: Option<String>,
    ) -> Result<String> {
        let name = match actor {
            ShareActor::Owner => OWNER_IDENTITY_NAME.to_string(),
            ShareActor::Account(name) => name.clone(),
            ShareActor::HttpAgent => agent_identity.ok_or_else(|| {
                EngineError::Remote(RemoteError::Refused(AGENT_IDENTITY_UNSET.to_string()))
            })?,
        };
        // Normalization belongs to the layer that mints these names - the auth
        // store (`crate::rest::auth_store`) trims and lowercases an account name
        // before it is ever stored, and the settings layer holds
        // `github.agent_identity` to the same shape - so the engine asserts the
        // invariant rather than quietly re-normalizing and hiding a surface that
        // stopped honouring it. The token store refuses a malformed name anyway;
        // this is the earlier, louder signal in a debug build.
        debug_assert_eq!(
            name,
            name.trim().to_lowercase(),
            "account names reach the engine already trimmed and lowercased"
        );
        Ok(name)
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

    /// How many of one team domain's unshared substantive changes `account`
    /// last wrote, by the changed file's own `generated.by` line.
    ///
    /// `None` for a domain with no origin state to compare against, which is
    /// the same answer a domain with no origin gets: nothing is known to be
    /// unshared, so nothing is known to be anybody's. Never an error - this
    /// enriches a report, and a report survives an unreadable working tree.
    ///
    /// Last-writer provenance, never authorship: it says which actor wrote the
    /// revision on disk, not who the knowledge belongs to.
    ///
    /// Under the domain's [`Engine::origin_lock`], for the reason
    /// [`Engine::share_facts`] gives: a status read that races a share would
    /// otherwise compare a half-written pair and attribute a delta nobody made.
    pub async fn owned_local_changes(&self, domain: &str, account: &str) -> Option<u64> {
        let lock = self.origin_lock(domain);
        let _guard = lock.lock().await;
        let (_spec, root, state_dir) = self.origin_spec_for_domain(domain).ok()?;
        let work = origin::unshared_work(&root, &state_dir)?;
        Some(work.owned_by(&root, &format!("human:{account}")))
    }

    /// [`Engine::origin_connection_json`] plus the two facts a caller needs to
    /// know WHICH credential a share of theirs would go out on: the mode
    /// (`share_identity`, always) and, in personal mode, the machine owner's
    /// own connection (`owner_identity`, absent in instance mode because there
    /// is no personal slot in play).
    ///
    /// Only `origin_status` is enriched, not [`Engine::origin_connection_json`]
    /// itself: the settings screen and the `configure` snapshot report the
    /// instance connection, and the personal identities they care about are the
    /// SESSION's, served by `/me/github-identity`.
    ///
    /// Two slots, because two callers resolve two different credentials.
    /// `owner_identity` is the machine OWNER's, which is what a CLI or
    /// stdio-MCP share resolves ([`Engine::acting_identity_name`]).
    /// `agent_identity` is the one an HTTP-MCP peer's share runs as
    /// (`github.agent_identity`), and it is reported on exactly the same terms:
    /// personal mode only, and only when that setting names an account at all,
    /// because an unset agent slot is not a connection somebody has failed to
    /// make - it is a deployment that has no HTTP agent sharing on it. Both
    /// carry `{ account, connected, user }`, and neither is a claim about who
    /// the reader is: a caller reads the slot it would share on.
    ///
    /// A credential that cannot be resolved reports as not connected rather
    /// than failing the whole status read: this is a report, and every other
    /// line of it survives an unreadable credential store.
    ///
    /// The cost, stated rather than hidden: only a PRESENT token is cached
    /// ([`Engine::github_credential_for`]), so an instance in personal mode
    /// whose owner has connected nothing pays one credential-store read per
    /// `origin status` - deliberate, since it is also what lets a standalone
    /// `crystalline connect github --personal` be seen without a restart.
    async fn origin_status_connection(&self) -> Result<Value> {
        let mut connection = self.origin_connection_json().await?;
        let (mode, agent) = {
            let config = self.config.read().unwrap();
            (
                config.github_share_identity(),
                config.github_agent_identity().map(str::to_string),
            )
        };
        connection["share_identity"] = json!(mode.as_str());
        if mode == ShareIdentityMode::Personal {
            connection["owner_identity"] =
                self.personal_slot_json(OWNER_IDENTITY_NAME, &connection);
            // Only where an HTTP agent has a slot at all: an absent
            // `github.agent_identity` means no share ever runs as one, and a
            // slot reported for it would read as a connection somebody forgot
            // to make.
            if let Some(agent) = agent.as_deref() {
                connection["agent_identity"] = self.personal_slot_json(agent, &connection);
            }
        }
        Ok(connection)
    }

    /// One personal identity slot for the status connection block:
    /// `{ account, connected, user }`, read from the credential store for
    /// `account`.
    ///
    /// A credential that cannot be resolved reports as not connected rather
    /// than failing the whole status read.
    ///
    /// With an injected test provider the store is never touched at all -
    /// reading it would reach the machine's real keychain from a test - and the
    /// login is deliberately null rather than the mock's: that login belongs to
    /// the injected INSTANCE provider, and reporting it here would invent a
    /// personal connection nobody made, the one thing this must never do, since
    /// the whole point of the slot is to say whether a share can go out at all.
    fn personal_slot_json(&self, account: &str, connection: &Value) -> Value {
        let (connected, user) = if self.origin_provider_override.is_some() {
            (
                connection["connected"].as_bool().unwrap_or(false),
                Value::Null,
            )
        } else {
            let identity = TokenIdentity::Personal(account.to_string());
            let host = self.github_token_host();
            let token = self
                .github_credential_for(&identity, host.as_deref())
                .ok()
                .and_then(|(_store, token)| token);
            (
                token.is_some(),
                json!(token.as_ref().and_then(|t| t.user_display())),
            )
        };
        json!({
            "account": account,
            "connected": connected,
            "user": user,
        })
    }

    /// The INSTANCE GitHub token store for `host` and the token it holds -
    /// [`Engine::github_credential_for`] with the instance identity - reading
    /// the OS keychain at most once per process. Every read verb and every
    /// connection-status surface goes through here; only a write in personal
    /// mode addresses another identity. The environment token wins first
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
        self.github_credential_for(&TokenIdentity::Instance, host)
    }

    /// [`Engine::github_credential`] for any identity: the instance credential
    /// or one person's personal one, cached per identity and host so two
    /// identities can never be served the same client.
    ///
    /// The environment token is instance-only and stays that way. One process
    /// serves everybody who reaches it, so a single `CRYSTALLINE_GITHUB_TOKEN`
    /// cannot mean alice's token for one request and bob's for the next; a
    /// personal identity resolves through the keyring or the file store, even on
    /// a machine that sets the variable.
    fn github_credential_for(
        &self,
        identity: &TokenIdentity,
        host: Option<&str>,
    ) -> Result<(TokenStore, Option<StoredToken>)> {
        if *identity == TokenIdentity::Instance
            && let Some(token) = self.overlay.github_token()
        {
            let store = TokenStore::env(token, host);
            let stored = store.load()?;
            return Ok((store, stored));
        }
        let key = credential_cache_key(identity, host);
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
                let store = TokenStore::file_fallback_for(identity, dir)?;
                let token = store.load()?;
                (store, token)
            }
            None => {
                let base = self.origins_base_dir()?;
                TokenStore::resolve_and_load_for(identity, host, &base)?
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

    /// The plan a connect flow saves the INSTANCE credential through.
    fn github_save_plan(&self, host: Option<&str>) -> Result<TokenSavePlan> {
        self.github_save_plan_for(&TokenIdentity::Instance, host)
    }

    /// The plan a connect flow saves its token through: the test file override
    /// or a real `save_resolving`, plus a handle to the token cache to refresh
    /// after the write. `host` is the token host this connect targets, captured
    /// by value so the device-flow task can own the plan across the spawn.
    ///
    /// `identity` decides both halves of where the token lands - the keyring
    /// account (or the fallback file name) and the cache slot the write
    /// refreshes - so a person's connect can never overwrite the machine's
    /// credential, nor leave the machine's client cached under a name it no
    /// longer belongs to.
    fn github_save_plan_for(
        &self,
        identity: &TokenIdentity,
        host: Option<&str>,
    ) -> Result<TokenSavePlan> {
        let target = match &self.token_store_dir_override {
            // The same derivation `github_credential_for` reads back through,
            // rather than a second spelling of the file name here.
            Some(dir) => SaveTarget::File(
                TokenStore::file_fallback_for(identity, dir).map_err(EngineError::Remote)?,
            ),
            None => SaveTarget::Resolve {
                fallback_dir: self.origins_base_dir()?,
            },
        };
        Ok(TokenSavePlan {
            identity: identity.clone(),
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
    /// clearing every entry (up to one per identity per host now) avoids
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

    /// The pending INSTANCE device flow's display view, `{ pending: true,
    /// user_code, verification_url, expires_in_secs }`, or `None` when no
    /// instance flow is running. A person's sign-in is invisible here: the two
    /// are different credentials and neither surface may report the other's
    /// code.
    fn pending_view(&self) -> Option<Value> {
        self.pending_view_for(&TokenIdentity::Instance)
    }

    /// [`Engine::pending_view`] for one identity.
    fn pending_view_for(&self, identity: &TokenIdentity) -> Option<Value> {
        self.pending_connect
            .lock()
            .unwrap()
            .as_ref()
            .filter(|p| p.identity == *identity)
            .map(|p| {
                json!({
                    "pending": true,
                    "user_code": p.user_code,
                    "verification_url": p.verification_url,
                    "expires_in_secs": p.expires_in_secs,
                    "next_steps": p.next_steps,
                })
            })
    }

    /// Takes the pending INSTANCE flow's outcome if it has landed, clearing
    /// the slot so a later connect starts fresh. Returns `None` both when no
    /// instance flow is pending at all and when one is pending but still
    /// waiting on the user; a caller distinguishes those with
    /// [`Engine::pending_view`].
    fn take_finished_pending(&self) -> Option<std::result::Result<String, RemoteError>> {
        self.take_finished_pending_for(&TokenIdentity::Instance)
    }

    /// [`Engine::take_finished_pending`] for one identity: a landed outcome is
    /// reported to - and cleared by - whoever the flow belonged to, never by
    /// the surface that happens to read first.
    fn take_finished_pending_for(
        &self,
        identity: &TokenIdentity,
    ) -> Option<std::result::Result<String, RemoteError>> {
        let mut guard = self.pending_connect.lock().unwrap();
        let landed = guard
            .as_ref()
            .filter(|p| p.identity == *identity)
            .and_then(|p| p.outcome.lock().unwrap().take());
        if landed.is_some() {
            *guard = None;
        }
        landed
    }

    /// The stored guidance for `identity`'s pending flow, read without
    /// taking anything. Called BEFORE [`Engine::take_finished_pending_for`]
    /// on the same identity: that call clears the slot the guidance lives
    /// on, so a caller that wants both the outcome and the guidance it
    /// landed with has to read this one first.
    fn pending_next_steps_for(&self, identity: &TokenIdentity) -> Option<String> {
        self.pending_connect
            .lock()
            .unwrap()
            .as_ref()
            .filter(|p| p.identity == *identity)
            .map(|p| p.next_steps.clone())
    }

    /// Drops a pending flow belonging to `identity`, leaving another
    /// identity's alone. What a connect that settles the same credential by
    /// another route (a pasted token) and a disconnect both do: the flow in
    /// flight is about to be answered by a stale background task, and only for
    /// this one credential.
    fn clear_pending_for(&self, identity: &TokenIdentity) {
        let mut guard = self.pending_connect.lock().unwrap();
        if guard.as_ref().is_some_and(|p| p.identity == *identity) {
            *guard = None;
        }
    }

    /// The `github` block of the `configure` tool's snapshot: `{ connected,
    /// user, token_store, pending_connect }`. A flow still waiting on the
    /// user reports `pending_connect`; one that landed since the last call
    /// is reported here exactly once and the slot is cleared - a successful
    /// sign-in folds into `connected`/`user`, while an expired or declined
    /// one reports `connected: false` with `error` and `next_steps` (the
    /// guidance the flow started with, see [`Engine::pending_next_steps_for`])
    /// telling the caller to connect again and click Authorize this time,
    /// rather than surfacing a bare error a model has nothing to act on.
    async fn configure_connection_block(&self) -> Result<Value> {
        let landed_guidance = self.pending_next_steps_for(&TokenIdentity::Instance);
        if let Some(outcome) = self.take_finished_pending() {
            return match outcome {
                Ok(_user) => {
                    let mut github = self.origin_connection_json().await?;
                    github["pending_connect"] = Value::Null;
                    Ok(github)
                }
                Err(e) => Ok(json!({
                    "connected": false,
                    "user": Value::Null,
                    "token_store": Value::Null,
                    "pending_connect": Value::Null,
                    "error": e.to_string(),
                    "next_steps": retry_guidance(&e, landed_guidance.as_deref().unwrap_or_default()),
                })),
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

    /// The token-store host this connect targets: `github.api_url`'s bare
    /// Enterprise Server host, or `None` for GitHub.com. The same derivation
    /// [`Engine::origin_connection_json`] uses, so status, readiness and
    /// disconnect can never look at a different credential slot than
    /// team-domain operations do - on a GitHub Enterprise instance the GHES
    /// token is the one read (and deleted), never an empty github.com slot.
    fn github_token_host(&self) -> Option<String> {
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        origin::token_host(api_url.as_deref())
    }

    /// The connection as a settings surface polls it. Mirrors
    /// configure_connection_block's lifecycle handling (the MCP view) so the
    /// two surfaces can never disagree about a pending or finished flow: a
    /// finished failure is reported exactly once via the outcome slot, and a
    /// finished success is simply visible as connected (the token was saved).
    pub async fn github_connection(&self) -> Result<GithubConnection> {
        let error = match self.take_finished_pending() {
            Some(Err(e)) => Some(e.to_string()),
            _ => None,
        };
        let host = self.github_token_host();
        let (store, token) = self.github_credential(host.as_deref())?;
        let pending = self.pending_view().map(|v| GithubPending {
            user_code: v["user_code"].as_str().unwrap_or_default().to_string(),
            verification_url: v["verification_url"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            expires_in_secs: v["expires_in_secs"].as_u64().unwrap_or_default(),
        });
        Ok(GithubConnection {
            enabled: self.github_enabled(),
            connected: token.is_some(),
            user: token
                .as_ref()
                .and_then(|t| t.user_display())
                .map(str::to_string),
            token_store: token.is_some().then(|| store.kind().to_string()),
            pending,
            error,
        })
    }

    /// Whether team-domain registration can succeed right now.
    pub async fn github_ready(&self) -> bool {
        if !self.github_enabled() {
            return false;
        }
        let host = self.github_token_host();
        matches!(self.github_credential(host.as_deref()), Ok((_, Some(_))))
    }

    /// Forget the stored credential: delete it where it lives, drop its cache
    /// entries and cancel any pending device flow. Refuses under an
    /// environment token (only the environment can retire it) and on a
    /// read-only instance. github.enabled is untouched: turning the feature
    /// off stays a configure concern.
    ///
    /// Only the INSTANCE credential's cache entries go - every host's, since
    /// the delete above may have been for one host while a GHES entry for
    /// another is stale for the same reason. Personal slots stay: they are
    /// different credentials that this call did not delete, and clearing them
    /// would cost every connected person a keychain read (a prompt, on a real
    /// machine) to recover something that never changed. Same rule as
    /// [`Engine::disconnect_github_identity`], read from the other side.
    pub async fn github_disconnect(&self) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let host = self.github_token_host();
        let (store, token) = self.github_credential(host.as_deref())?;
        if matches!(store, TokenStore::Env { .. }) {
            return Err(EngineError::EnvTokenConnect);
        }
        let kind = store.kind();
        if token.is_some() {
            store.delete().map_err(EngineError::Remote)?;
        }
        // Keyed by prefix rather than by the one host this call resolved: the
        // cache holds one entry per identity per host, and the instance's are
        // exactly the ones this delete invalidates.
        let instance_prefix = credential_cache_key(&TokenIdentity::Instance, None);
        self.github_tokens
            .lock()
            .unwrap()
            .retain(|key, _| !key.starts_with(&instance_prefix));
        // Only the machine's own sign-in: a person's device flow is a
        // different credential and is left to run.
        self.clear_pending_for(&TokenIdentity::Instance);
        Ok(json!({ "connected": false, "token_store": kind }))
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
    ///
    /// With `github.enabled` off the connection block is `{ github_enabled:
    /// false, note }` and nothing else: no `connected`, no `user`, no
    /// `token_store`, no `pending_connect`. Absent rather than false on
    /// purpose - a `connected: false` would be a claim about a credential
    /// this call deliberately did not read, and reading it is the thing the
    /// gate exists to prevent (on a real machine that read is an OS keychain
    /// touch, for a feature that is switched off). What a disabled instance
    /// reports is the feature's state and how to turn it on, which is the
    /// only actionable thing at that moment: `configure` stays visible with
    /// GitHub off precisely so it can be enabled.
    ///
    /// The gate sits ABOVE [`Engine::configure_connection_block`], whose
    /// first act is draining a landed device-flow outcome. That placement is
    /// deliberate: gating below the drain would leave only two options, both
    /// wrong - report the landed outcome (connection facts on a disabled
    /// instance) or drain and swallow it (destroying the one thing a
    /// report-once contract cannot survive). Above the drain the outcome
    /// stays in the slot and is still reported exactly once, by the next
    /// connect call or by the settings surface through
    /// [`Engine::github_connection`], which drains for itself. The only
    /// change is that a bare `configure` stops being one of the surfaces
    /// that report it while the feature is off.
    ///
    /// The connect paths are NOT gated: [`Engine::connect_with_token`] and
    /// [`Engine::start_device_connect`] build their own block and go through
    /// [`Engine::configure_snapshot_with`], so connecting with
    /// `github.enabled` off still reports the connection and says so in its
    /// note. Connecting and enabling are independent and either order works.
    pub async fn configure_snapshot(&self) -> Result<Value> {
        if !self.github_enabled() {
            return self.configure_snapshot_with(json!({
                "github_enabled": false,
                "note": "GitHub is switched off on this instance; set github.enabled true with configure to connect or read the connection.",
            }));
        }
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
        self.clear_pending_for(&TokenIdentity::Instance);

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
        let Some(view) = self
            .begin_device_flow(&TokenIdentity::Instance, host)
            .await?
        else {
            let github = self.configure_connection_block().await?;
            return self.configure_snapshot_with(github);
        };

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

    /// Starts a device-flow sign-in that will store its token as `identity`,
    /// spawning the background task that runs it to completion, validates the
    /// token, saves it and stashes the outcome in the pending slot.
    ///
    /// Answers `Some(view)` with the code to show when a flow was started, and
    /// `None` when this same identity already has one running - the double
    /// click a caller reports the outstanding code for rather than stranding a
    /// second one.
    ///
    /// There is exactly ONE flow slot per engine and it is tagged, so a
    /// sign-in started while a DIFFERENT identity's is still in flight is
    /// refused with [`EngineError::ConnectInProgress`] instead of joining it.
    /// The one exception is a flow whose outcome has already landed: that
    /// sign-in is finished rather than in progress - its token, if any, is
    /// saved and every status reads the store, so the only thing dropped is an
    /// unread error line for a flow nobody came back to look at - and the slot
    /// is taken over.
    async fn begin_device_flow(
        &self,
        identity: &TokenIdentity,
        host: Option<&str>,
    ) -> Result<Option<Value>> {
        {
            let mut guard = self.pending_connect.lock().unwrap();
            match guard.as_ref() {
                Some(p) if p.identity == *identity => return Ok(None),
                Some(p) if p.outcome.lock().unwrap().is_some() => *guard = None,
                Some(_) => return Err(EngineError::ConnectInProgress),
                None => {}
            }
        }

        let api_url = self.connect_api_url(host);
        let auth_base = crystalline_remote::github::auth::auth_base(api_url.as_deref());
        let client_id = self.oauth_client_id();
        let start = self
            .connect_auth
            .start_device_flow(&auth_base, &client_id)
            .await?;

        let next_steps = crystalline_remote::github::auth::confirmation_guidance(&auth_base);
        let outcome_slot: Arc<std::sync::Mutex<Option<std::result::Result<String, RemoteError>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let pending = PendingConnect {
            identity: identity.clone(),
            user_code: start.user_code.clone(),
            verification_url: start.verification_url.clone(),
            expires_in_secs: start.expires_in_secs,
            next_steps: next_steps.clone(),
            outcome: outcome_slot.clone(),
        };
        let view = json!({
            "pending": true,
            "user_code": pending.user_code,
            "verification_url": pending.verification_url,
            "expires_in_secs": pending.expires_in_secs,
            "next_steps": pending.next_steps,
        });
        *self.pending_connect.lock().unwrap() = Some(pending);

        let auth = self.connect_auth.clone();
        let token_host = origin::token_host(api_url.as_deref());
        let plan = self.github_save_plan_for(identity, token_host.as_deref())?;
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
        Ok(Some(view))
    }

    // --- one account's own GitHub identity ----------------------------------

    /// One account's personal GitHub connection, for the profile card that
    /// manages it: whether a token is on file, the login it was connected as,
    /// since when, where it lives, and the device flow's poll.
    ///
    /// The account name is the identity anchor (spec section 4), so a name a
    /// credential cannot be addressed by is refused here, in words that name
    /// the fix, rather than at the person's first share (see the
    /// `personal_identity` gate every verb on this surface goes through).
    ///
    /// A pure read, like the instance status it mirrors: it is served on a
    /// read-only instance, and it doubles as the device flow's poll, reporting
    /// a failed flow's reason on exactly one read.
    pub async fn github_identity_status(&self, account: &str) -> Result<GithubIdentity> {
        let identity = personal_identity(account)?;
        let error = match self.take_finished_pending_for(&identity) {
            Some(Err(e)) => Some(e.to_string()),
            _ => None,
        };
        let host = self.github_token_host();
        let (store, token) = self.github_credential_for(&identity, host.as_deref())?;
        let pending = self.pending_view_for(&identity).map(|v| GithubPending {
            user_code: v["user_code"].as_str().unwrap_or_default().to_string(),
            verification_url: v["verification_url"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            expires_in_secs: v["expires_in_secs"].as_u64().unwrap_or_default(),
        });
        Ok(GithubIdentity {
            account: account.to_string(),
            connected: token.is_some(),
            login: token
                .as_ref()
                .and_then(|t| t.user_display())
                .map(str::to_string),
            connected_at: token.as_ref().map(|t| t.created_at),
            token_store: token.is_some().then(|| store.kind().to_string()),
            pending,
            error,
        })
    }

    /// Connect one account's GitHub identity with a personal access token,
    /// validated against GitHub before it is stored so the login on file is
    /// the one the token actually belongs to.
    ///
    /// `CRYSTALLINE_GITHUB_TOKEN` is deliberately NOT a bar here, unlike on
    /// the instance connect: that variable fixes the MACHINE's identity (the
    /// environment store is instance-only, by construction in
    /// `crystalline_remote::token`), and an instance whose machine credential
    /// comes from the environment is exactly the kind that shares personally.
    pub async fn connect_github_identity_token(
        &self,
        account: &str,
        token: &str,
    ) -> Result<GithubIdentity> {
        let identity = personal_identity(account)?;
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let api_url = self.connect_api_url(None);
        let user = self
            .connect_auth
            .validate_token(api_url.as_deref(), token)
            .await?;
        let token_host = origin::token_host(api_url.as_deref());
        let plan = self.github_save_plan_for(&identity, token_host.as_deref())?;
        plan.save(&StoredToken {
            access_token: token.to_string(),
            host: token_host.unwrap_or_else(|| "github.com".to_string()),
            user,
            created_at: chrono::Utc::now(),
        })?;
        // A pasted token settles this identity now, so a device flow of this
        // person's still in flight must not land on top of it later.
        self.clear_pending_for(&identity);
        self.github_identity_status(account).await
    }

    /// Start a device-code sign-in for one account's GitHub identity. Returns
    /// immediately with the status carrying the code to confirm; the flow runs
    /// in the background and its outcome is read from
    /// [`Engine::github_identity_status`].
    ///
    /// One sign-in at a time across the whole engine: a second account's
    /// connect while this one runs is [`EngineError::ConnectInProgress`], and
    /// the same account asking again reports the code already outstanding.
    pub async fn start_github_identity_device_flow(&self, account: &str) -> Result<GithubIdentity> {
        let identity = personal_identity(account)?;
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.begin_device_flow(&identity, None).await?;
        self.github_identity_status(account).await
    }

    /// Forget one account's GitHub identity: delete the credential where it
    /// lives, drop its cache slot and cancel a device flow of its own.
    /// Idempotent - disconnecting an identity that holds nothing succeeds.
    ///
    /// Only this identity's cache entry is evicted, not the whole cache: every
    /// other credential this process resolved is still valid, and re-reading
    /// them would cost a keychain prompt each for nothing.
    pub async fn disconnect_github_identity(&self, account: &str) -> Result<GithubIdentity> {
        let identity = personal_identity(account)?;
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let host = self.github_token_host();
        let (store, token) = self.github_credential_for(&identity, host.as_deref())?;
        if token.is_some() {
            store.delete().map_err(EngineError::Remote)?;
        }
        self.github_tokens
            .lock()
            .unwrap()
            .remove(&credential_cache_key(&identity, host.as_deref()));
        self.clear_pending_for(&identity);
        self.github_identity_status(account).await
    }

    /// Drop every cached slot for one credential, without touching the
    /// credential itself. `account` is `None` for this machine's own.
    ///
    /// This exists for the credential store's OTHER writer. `crystalline
    /// connect github --disconnect` runs in the CLI process and deletes the
    /// token where it lives, which a running daemon cannot notice: its cache
    /// is a process-lifetime map ([`Engine::github_tokens`]), so it would go
    /// on sharing with a token this machine no longer has until it was
    /// restarted. A credential is forgotten because somebody wanted it to stop
    /// working, so "until the next restart" is the wrong answer; the CLI tells
    /// the daemon over the control socket and this is what it reaches.
    ///
    /// Every host is dropped, not the one host this process would resolve: the
    /// cache holds a slot per identity per host, and the delete the CLI just
    /// performed is not scoped to one either. The prefix is exact - the key
    /// puts a unit separator after the name, and
    /// [`crystalline_remote::valid_identity_name`] keeps that byte out of
    /// names - so `alice` can never evict `alice2`.
    ///
    /// Read-only is not consulted: forgetting a cached secret is not a
    /// mutation this instance serves, and refusing it would leave the token in
    /// memory precisely where the instance can still write with it.
    ///
    /// The pending device-flow record for the same identity is dropped too,
    /// exactly as [`Engine::disconnect_github_identity`] drops it. That frees
    /// the one-flow-at-a-time slot and forgets the flow's outcome; it does not
    /// stop the spawned exchange itself, which on a later completion still
    /// saves its token and re-fills the cache - a residue both disconnect
    /// paths share (see plans/backlog.md), narrow because that flow was
    /// user-initiated moments earlier.
    pub fn forget_cached_credential(&self, account: Option<&str>) -> Result<()> {
        let identity = match account {
            None => TokenIdentity::Instance,
            Some(name) => personal_identity(name)?,
        };
        let prefix = credential_cache_key(&identity, None);
        self.github_tokens
            .lock()
            .unwrap()
            .retain(|key, _| !key.starts_with(&prefix));
        self.clear_pending_for(&identity);
        Ok(())
    }
}

/// Rank a context slice by personalized PageRank so the neighbors on the
/// shortest, best-connected paths back to the seeds surface first.
///
/// The random walk teleports to the seeds (uniform over the seeds present in
/// the slice), so mass concentrates near them and decays with graph distance.
/// Power iteration over a dense, ascending-id index keeps the result
/// deterministic: every accumulation runs over `Vec`s in that fixed order,
/// never over a `HashMap`.
///
/// The adjacency is symmetric - every [`GraphEdge`] contributes both
/// directions - so a pair joined by more than one edge (a relation plus a
/// wikilink, or reciprocal relations) is counted once per edge and conducts
/// proportionally more mass. That double counting is deliberate and is the
/// seam where per-edge-kind weighting would slot in later.
fn context_rank(slice: &GraphSlice, seed_ids: &HashSet<i64>) -> HashMap<i64, f64> {
    // Dense, ascending-id index: id -> position in the fixed-order Vecs.
    let mut ids: Vec<i64> = slice.nodes.iter().map(|n| n.id.0).collect();
    ids.sort_unstable();
    let n = ids.len();
    if n == 0 {
        return HashMap::new();
    }
    let index: HashMap<i64, usize> = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();

    // Symmetric adjacency; skip an edge whose endpoint is not in the node map
    // (defensive, the store returns only in-slice edges).
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &slice.edges {
        if let (Some(&a), Some(&b)) = (index.get(&e.from.0), index.get(&e.to.0)) {
            adj[a].push(b);
            adj[b].push(a);
        }
    }
    // Pin accumulation order: the edge-collection SQL has no ORDER BY, so
    // `slice.edges` order is not guaranteed identical across backends or
    // calls. Sorting each adjacency list makes the inbound-sum order (and so
    // the ranking) bit-identical regardless of edge collection order.
    // Multi-edge duplicates (deliberate weight) are preserved; only their
    // order becomes canonical.
    for list in &mut adj {
        list.sort_unstable();
    }

    // Teleport vector: uniform over the seeds present in the slice; fall back
    // to uniform over every node when no seed made it in (defensive, so the
    // iteration still converges).
    let mut teleport = vec![0.0f64; n];
    let seed_positions: Vec<usize> = ids
        .iter()
        .enumerate()
        .filter(|(_, id)| seed_ids.contains(id))
        .map(|(i, _)| i)
        .collect();
    if seed_positions.is_empty() {
        let uniform = 1.0 / n as f64;
        for t in teleport.iter_mut() {
            *t = uniform;
        }
    } else {
        let share = 1.0 / seed_positions.len() as f64;
        for &i in &seed_positions {
            teleport[i] = share;
        }
    }

    // Power iteration from the teleport distribution.
    let mut rank = teleport.clone();
    let mut next = vec![0.0f64; n];
    for _ in 0..CONTEXT_MAX_ITERATIONS {
        // Mass stranded on zero-degree nodes (only an isolated seed can be one,
        // since every non-seed entered the slice over an edge) is redistributed
        // over the teleport vector so no mass leaks out of the system.
        let dangling: f64 = (0..n).filter(|&i| adj[i].is_empty()).map(|i| rank[i]).sum();
        for i in 0..n {
            let inbound: f64 = adj[i].iter().map(|&j| rank[j] / adj[j].len() as f64).sum();
            next[i] = (1.0 - CONTEXT_DAMPING) * teleport[i]
                + CONTEXT_DAMPING * inbound
                + CONTEXT_DAMPING * dangling * teleport[i];
        }
        let delta: f64 = (0..n).map(|i| (next[i] - rank[i]).abs()).sum();
        rank.copy_from_slice(&next);
        if delta < CONTEXT_TOLERANCE {
            break;
        }
    }

    ids.into_iter().zip(rank).collect()
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

/// The GitHub connection as a settings screen needs it: never any token
/// material, only where the credential lives and who it authenticates.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GithubConnection {
    /// The github.enabled switch (team tools and polling).
    pub enabled: bool,
    pub connected: bool,
    /// The account login, when connected.
    pub user: Option<String>,
    /// "keyring" | "file" | "environment", when connected.
    pub token_store: Option<String>,
    /// A device flow waiting for the browser side.
    pub pending: Option<GithubPending>,
    /// The once-reported failure of the last device flow (expired, denied);
    /// present on exactly one status read, then cleared.
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GithubPending {
    pub user_code: String,
    pub verification_url: String,
    pub expires_in_secs: u64,
}

/// One account's own GitHub identity, as the profile card that manages it
/// renders and polls it. Carries no token material - only whose identity it is,
/// whether one is on file, the login it authenticated as, since when and where
/// it lives.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GithubIdentity {
    /// The crystalline account this identity belongs to.
    pub account: String,
    /// Whether a personal credential is on file for that account.
    pub connected: bool,
    /// The GitHub login it authenticated as, when connected.
    pub login: Option<String>,
    /// When the credential was stored, for the card's "connected since".
    pub connected_at: Option<chrono::DateTime<chrono::Utc>>,
    /// "keyring" | "file", when connected. Never "environment": the
    /// environment supplies the machine's credential and never a personal one.
    pub token_store: Option<String>,
    /// This account's device flow waiting for the browser side.
    pub pending: Option<GithubPending>,
    /// The once-reported failure of this account's last device flow.
    pub error: Option<String>,
}

/// What to tell the caller after a device flow lands as a failure: retry
/// wording that names the reason distinctly for an expired code versus a
/// declined one where the outcome can tell them apart, falling back to a
/// generic reason otherwise, followed by `landed_guidance` (the same
/// confirmation guidance the flow started with, so the authorized-apps url
/// and the Authorize reminder are never phrased twice).
fn retry_guidance(e: &RemoteError, landed_guidance: &str) -> String {
    let reason = match e {
        RemoteError::AuthExpired => "the code expired before it was authorized",
        RemoteError::Api { status: 403, .. } => "the sign-in was declined on GitHub",
        _ => "the sign-in did not complete",
    };
    format!(
        "{reason}. Call configure with connect \"github\" again to start a new sign-in, and \
         this time click Authorize on the page after the code. {landed_guidance}"
    )
}

/// One in-flight GitHub device-flow sign-in, held by
/// [`Engine::pending_connect`] so a second `configure` connect call while
/// one is running reports the same code instead of starting another. The
/// background task started by [`Engine::start_device_connect`] writes its
/// result into `outcome` once, for the next `configure` call (any call, not
/// just a connect) to observe and clear.
struct PendingConnect {
    /// Whose credential this flow will store into when it lands. The slot is
    /// engine-wide and there is exactly one, so without this tag an instance
    /// sign-in and a person's sign-in could report - and clear - each other's
    /// outcome. Every read of the slot is filtered by it (see
    /// [`Engine::pending_view_for`] and
    /// [`Engine::take_finished_pending_for`]), and a connect for a different
    /// identity while one is running is refused rather than joined (see
    /// [`Engine::begin_device_flow`]).
    identity: TokenIdentity,
    /// The short code the user types in at `verification_url`.
    user_code: String,
    /// Where the user confirms the code.
    verification_url: String,
    /// How many seconds from when the flow started it stops being valid.
    expires_in_secs: u64,
    /// What to do after the code is entered and how to tell whether it
    /// landed, computed once at flow start from that flow's own auth base
    /// (see [`crystalline_remote::github::auth::confirmation_guidance`]) so
    /// a GHES sign-in and a github.com one each carry their own applications
    /// url. Read back by every surface that reports this pending flow.
    next_steps: String,
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
    /// Whose credential this write is: the machine's, or one person's. Decides
    /// the store the token lands in and the cache slot refreshed after it.
    identity: TokenIdentity,
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
    /// The test token-directory override's file store for this identity.
    File(TokenStore),
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
            SaveTarget::File(store) => {
                store.save(token)?;
                store.clone()
            }
            SaveTarget::Resolve { fallback_dir } => TokenStore::save_resolving_for(
                &self.identity,
                self.host.as_deref(),
                fallback_dir,
                token,
            )?,
        };
        // This identity's own slot, never a fixed one: a personal connect that
        // refreshed the instance entry would both strand the stale personal
        // client (the very next share would use the token just replaced) and
        // hand the machine's reads somebody's personal credential.
        let key = credential_cache_key(&self.identity, self.host.as_deref());
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

/// Parse the requested detector families, erroring on an unknown value with the
/// valid set named so a caller recovers in one step.
fn parse_families(requested: &[String]) -> Result<Vec<Family>> {
    let mut out: Vec<Family> = Vec::new();
    for raw in requested {
        let family = Family::parse(raw).ok_or_else(|| {
            let valid: Vec<&str> = Family::ALL.iter().map(|f| f.as_str()).collect();
            EngineError::Invalid(format!(
                "unknown family '{raw}'; valid families: {}",
                valid.join(", ")
            ))
        })?;
        if !out.contains(&family) {
            out.push(family);
        }
    }
    Ok(out)
}

/// Parse the requested rule ids into their catalog spellings, erroring on an
/// unknown id with the whole catalog named. The reserved `V3xx` range is not in
/// the catalog, so asking for it errors here rather than returning silence.
fn parse_rules(requested: &[String]) -> Result<Vec<&'static str>> {
    let mut out: Vec<&'static str> = Vec::new();
    for raw in requested {
        let key = raw.trim().to_ascii_uppercase();
        let info = rule_info(&key).ok_or_else(|| {
            let valid: Vec<&str> = RULES.iter().map(|r| r.id).collect();
            EngineError::Invalid(format!(
                "unknown rule '{raw}'; valid rules: {}",
                valid.join(", ")
            ))
        })?;
        if !out.contains(&info.id) {
            out.push(info.id);
        }
    }
    Ok(out)
}

/// A domain's verify overrides, read from the `.crystalline.yaml` at its root.
/// A virtual domain has no root and therefore no overrides, so its engrams take
/// the default budget.
fn domain_verify_config(source: &ContentSource) -> Option<VerifyConfig> {
    let ContentSource::File { root } = source else {
        return None;
    };
    let path = root.join(".crystalline.yaml");
    if !path.is_file() {
        return None;
    }
    crystalline_core::config::load_yaml::<DomainConfig>(&path)
        .ok()
        .and_then(|c| c.verify)
}

/// The approximate token budget for one engram, resolved exactly the way
/// verify's `Q002` resolves it: a per-file override, then the domain default,
/// then [`crystalline_index::sweep::DEFAULT_TOKEN_BUDGET`]. A budget of `0`
/// disables the size rule for that engram. `rel` is the domain-relative,
/// forward-slashed path the override map is keyed by.
fn resolve_token_budget(verify: Option<&VerifyConfig>, rel: &str) -> usize {
    if let Some(v) = verify {
        if let Some(&b) = v.token_budgets.get(rel) {
            return b;
        }
        if let Some(b) = v.token_budget {
            return b;
        }
    }
    crystalline_index::sweep::DEFAULT_TOKEN_BUDGET
}

/// A frontmatter value read as a number, for the `salience` ranking input. An
/// integer, a float and a numeric string all count; anything else is absent, the
/// same neutral reading the index gives a non-numeric salience.
fn yaml_number(value: Option<&YamlValue>) -> Option<f64> {
    match value? {
        YamlValue::Int(i) => Some(*i as f64),
        YamlValue::Float(f) => Some(*f),
        YamlValue::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
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

/// How recording an `old -> new` tag alias in a MANIFEST source resolves, once
/// per affected domain. Both the file and virtual recording branches route
/// through [`decide_alias_record`] so they can never diverge on the decision.
enum AliasRecord {
    /// The source gained the mapping; write this edited text back.
    Recorded(String),
    /// The exact folded pair was already declared, so nothing is written but the
    /// mapping is in effect and the domain still counts as recorded.
    AlreadyPresent,
    /// `old` is already aliased to a different canonical, which first-wins
    /// parsing keeps, so an appended bullet would be inert: nothing is written
    /// and the domain is surfaced as a conflict rather than a false success.
    Conflict,
}

/// Decide how recording `old_f -> new_f` in a MANIFEST `source` should be
/// handled, touching no store. `old_f` and `new_f` are already folded. A `Some`
/// append that first-wins parsing would not honor (a different mapping for
/// `old_f` already wins) is reported as a [`AliasRecord::Conflict`].
fn decide_alias_record(source: &str, old_f: &str, new_f: &str) -> AliasRecord {
    match crystalline_core::append_tag_alias(source, old_f, new_f) {
        None => AliasRecord::AlreadyPresent,
        Some(edited) => {
            let effective = crystalline_core::tag_alias_pairs(&edited)
                .into_iter()
                .any(|(alias, canonical)| alias == old_f && canonical == new_f);
            if effective {
                AliasRecord::Recorded(edited)
            } else {
                AliasRecord::Conflict
            }
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
/// The [`Engine::write_locks`] key for a file: its canonical path wherever the
/// filesystem can resolve one, so two spellings of one file - a symlinked
/// domain root, a `..` segment, a case difference the filesystem folds - share
/// a lock instead of each getting their own.
///
/// A file that does not exist yet cannot be canonicalized, and a create is
/// exactly that case, so the parent folder is resolved instead and the filename
/// joined back on. When even the parent is missing (a create that will also
/// make the folder) the path as given is the key: still stable, and still the
/// same string for two creates racing on one target, since both derive it from
/// the same registered root. The same fallback ladder
/// [`canonicalized_file_path`] uses for a domain root, one level deeper.
fn lock_key(abs: &Path) -> String {
    if let Ok(canonical) = std::fs::canonicalize(abs) {
        return canonical.to_string_lossy().into_owned();
    }
    if let (Some(parent), Some(name)) = (abs.parent(), abs.file_name())
        && let Ok(canonical) = std::fs::canonicalize(parent)
    {
        return canonical.join(name).to_string_lossy().into_owned();
    }
    abs.to_string_lossy().into_owned()
}

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

/// Whether a sync or reindex pass moved anything on disk, the gate on
/// regenerating the domain's index files. A pass that classified every file as
/// unchanged leaves the listing exactly as it is, so it must not pay for a
/// second walk of the domain.
fn changed_anything(report: &SyncReport) -> bool {
    report.added > 0 || report.updated > 0 || report.deleted > 0 || report.moved > 0
}

/// The refusal for a path whose filename is one of the OKF reserved names.
/// Actionable: it says which name is reserved, why, and what to do instead.
fn reserved_name_error(rel: &str) -> String {
    format!(
        "'{rel}' would use the reserved filename {} or {}: OKF keeps both for the generated directory index and log, so they are never engrams. Choose another title or destination filename.",
        crystalline_core::INDEX_FILE,
        crystalline_core::LOG_FILE
    )
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

/// Whether a domain-relative path stays inside the domain: no empty, `.` or
/// `..` segment, and not absolute. [`join_rel`] pushes what it is given segment
/// by segment and would happily push a `..`, so every destination is screened
/// here before any path is built.
///
/// This is the containment rule alone. It deliberately says nothing about the
/// characters a segment may hold, because an engram file is whatever a person
/// named it: the sync walk indexes `notes/plan: v2.md` like any other markdown
/// file, so a save, a move or a restore addressing that engram has to keep
/// working. [`is_contained_rel`] adds the character rules on top, for the paths
/// that arrive from outside.
fn is_within_domain(rel: &str) -> bool {
    !rel.is_empty()
        && !Path::new(rel).is_absolute()
        && rel
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// [`is_within_domain`] plus the character rules for untrusted input: an
/// archive entry and an attachment path, neither of which a person typed as a
/// filename here. A backslash or a colon inside a segment is refused because
/// both are separators or drive and stream markers on Windows, where a name
/// that looks contained on one platform escapes on another.
fn is_contained_rel(rel: &str) -> bool {
    is_within_domain(rel) && rel.split('/').all(|seg| !seg.contains(['\\', ':']))
}

/// A domain-relative folder as the store's path prefix: the trailing slash is
/// what makes the match a folder rather than a string, so `notes` selects
/// `notes/deep/y.md` and never the sibling `notes-misc/z.md`.
///
/// The root - an empty value, `/` or `./` - is `None`, meaning the whole
/// domain. Written once and shared by the tree and the folder-scoped listing,
/// so the two can never disagree about what a folder is.
fn folder_prefix(raw: &str) -> Option<String> {
    let trimmed = raw.trim_start_matches("./").trim_matches('/');
    (!trimmed.is_empty()).then(|| format!("{trimmed}/"))
}

/// Normalize a destination into a forward-slashed `.md` path. A `.` segment is
/// dropped with the empty ones (it resolves to nothing); a `..` segment
/// survives, so the containment screen at the call site refuses it rather than
/// this quietly resolving a destination nobody asked for.
fn normalize_md(dest: &str) -> String {
    let trimmed = dest.trim_start_matches("./").trim_matches('/');
    let joined = trimmed
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
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

/// Distinguishes one write's temp file from another's within this process. See
/// [`write_bytes`].
static TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn write_bytes(abs: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|source| EngineError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    // Write to a sibling temp then rename so the watcher never sees a partial
    // file. The name carries a process-lifetime counter as well as the pid:
    // the pid alone gives every writer in this process the same temp path, so
    // two writes to one file racing inside one daemon would interleave their
    // bytes there and rename the blend into place. Per-file locking keeps the
    // guarded verbs off each other, but the counter is what makes the temp
    // file private to a single write whichever path produced it.
    let seq = TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // The suffix is appended to the whole filename rather than replacing its
    // extension, so an attachment's temp file keeps naming the file it belongs
    // to (`shot.png.tmp.<pid>.<seq>`) instead of claiming an extension it never
    // had. For a `.md` engram the two spellings produce the same name.
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = abs.with_file_name(format!("{name}.tmp.{}.{seq}", std::process::id()));
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

/// Screen an attachment path before it reaches a filesystem or the store:
/// [`crystalline_core::validate_asset_path`]'s rules - the reserved prefix, the
/// segment rules, the character rules, the length ceiling and the extension
/// allowlist - reported as a malformed request.
fn validate_attachment_path(path: &str) -> Result<()> {
    crystalline_core::validate_asset_path(path)
        .map_err(|e| EngineError::Invalid(format!("attachment path '{path}': {e}")))
}

/// The absolute path an attachment occupies under a file domain's root, proven
/// to stay inside it.
///
/// Two proofs, because they catch different things.
/// [`is_contained_rel`] refuses a relative path that could climb out
/// (`..`, an absolute path, a Windows separator or drive marker) before any
/// path is built, which is the one that matters for untrusted input.
/// Canonicalization then catches what a string check cannot see: an `assets`
/// folder, or a folder inside it, that is a symlink pointing somewhere else
/// entirely. It is taken on the deepest ancestor that actually exists, since
/// the file itself usually does not yet.
fn contained_asset_path(root: &Path, rel: &str) -> Result<PathBuf> {
    if !is_contained_rel(rel) {
        return Err(EngineError::Invalid(format!(
            "attachment path '{rel}' escapes the domain root"
        )));
    }
    let abs = join_rel(root, rel);
    // A root that cannot be resolved does not exist yet, so there is no
    // symlink in place to escape through and the segment-by-segment join
    // stands on its own.
    let Ok(canonical_root) = std::fs::canonicalize(root) else {
        return Ok(abs);
    };
    let mut probe = abs.clone();
    while probe != *root {
        if let Ok(resolved) = std::fs::canonicalize(&probe) {
            if !resolved.starts_with(&canonical_root) {
                return Err(EngineError::Invalid(format!(
                    "attachment path '{rel}' resolves outside the domain root"
                )));
            }
            break;
        }
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => break,
        }
    }
    Ok(abs)
}

/// The metadata row describing these bytes at this path. The mime comes from
/// the extension and never from a caller, which is why this cannot be built
/// before [`validate_attachment_path`] has accepted the path.
fn attachment_row(path: &str, bytes: &[u8], modified: String) -> Result<AttachmentRow> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let mime = crystalline_core::attachment_mime(name).ok_or_else(|| {
        EngineError::Invalid(format!(
            "attachment path '{path}': an attachment must carry an allowlisted file extension"
        ))
    })?;
    Ok(AttachmentRow {
        path: path.to_string(),
        sha256: sha256_hex(bytes),
        mime: mime.to_string(),
        size: bytes.len() as u64,
        modified,
    })
}

/// One attachment a cross-domain move takes along with its engram. Built by
/// [`Engine::plan_attachment_carry`] and acted on by
/// [`Engine::carry_attachments`].
#[derive(Debug)]
struct AttachmentCarry {
    /// The path it has in the source domain.
    from: String,
    /// The path it takes at the destination: the same one, unless something
    /// different already sits there.
    to: String,
    /// Whether the destination already holds exactly these bytes under `to`,
    /// so there is nothing to write there.
    reuse: bool,
    /// Whether another engram in the source domain still references or claims
    /// it, so the source copy stays behind.
    shared: bool,
}

/// The receipt sentence for an attachment the source domain does not hold, so
/// the move takes nothing along for the reference that names it.
///
/// The wire text lives here alone: the trace line the planner also writes
/// formats this same sentence and adds the store error beside it, which is an
/// operator's detail rather than something a caller's receipt should carry.
fn attachment_missing_warning(path: &str, permalink: &str, domain: &str) -> String {
    format!(
        "attachment '{path}' referenced by '{permalink}' is not in '{domain}'; the move carries nothing for it"
    )
}

/// The receipt sentence for an attachment that stays in the source domain
/// because no free name for it could be settled at the destination.
fn attachment_not_carried_warning(path: &str, dest_domain: &str) -> String {
    format!(
        "attachment '{path}' could not be carried to '{dest_domain}'; its reference at the destination may resolve to a different same-name file"
    )
}

/// The counting's verdict, resolved so that not knowing can only point the
/// safe way.
///
/// A failure to count means "we do not know whether anything else in the
/// domain still uses these files", and the only reading of not knowing that
/// cannot destroy something is that something does: an unknown resolves to
/// shared, which copies the attachment and leaves the source copy where it is.
/// The opposite default would let a store hiccup authorize a delete, and a
/// delete is the one step of a move that cannot be taken back.
fn resolve_shared(counted: Result<HashSet<String>>, candidates: &[String]) -> HashSet<String> {
    match counted {
        Ok(shared) => shared,
        Err(e) => {
            tracing::warn!(
                "the referents of the attachments being moved could not be counted ({e}); each one is copied rather than moved, so nothing is removed from the source"
            );
            candidates.iter().cloned().collect()
        }
    }
}

/// The address a write's receipt names, resolved so that decorating a receipt
/// can never fail the write it describes.
///
/// Every caller asks the index for the permalink of something it has just
/// written, after the write and its reindex are committed. That read-back is
/// worth doing (the index takes an engram's permalink from its frontmatter, so
/// an author or a rename may have moved the address), but it is the last step
/// of an operation that is already done: a store error here would report a
/// committed write as failed and invite a caller to retry something that
/// already happened. So a failure is logged at debug and answered the same way
/// a missing row is, with the name the caller went in with.
fn receipt_permalink(found: Result<Option<String>>, fallback: String) -> String {
    match found {
        Ok(Some(permalink)) => permalink,
        Ok(None) => fallback,
        Err(e) => {
            tracing::debug!(
                "the address of the engram just written could not be read back ({e}); the receipt names '{fallback}', the address it went in with"
            );
            fallback
        }
    }
}

/// Whether a domain holding `engrams` engrams is inside
/// [`MAX_PREVIEW_SCAN_ENGRAMS`].
///
/// A line of arithmetic with its own name so the boundary is pinned by a test
/// rather than by a reading: exactly the bound still enumerates, one past it
/// does not.
fn count_within_preview_bound(engrams: usize) -> bool {
    engrams <= MAX_PREVIEW_SCAN_ENGRAMS
}

/// Every attachment one engram's own text points at: the `assets/` references
/// in its body plus the one an `analyzes` claim in its frontmatter names,
/// deduplicated and in reference order.
///
/// The one enumeration of "what does this engram use", shared by the
/// cross-domain move (which has to carry them) and by
/// [`Engine::delete_preview`] (which has to name the ones the delete leaves
/// behind). Text that will not parse is read as a body on its own rather than
/// dropped, because a reference in unparseable text is still a reference.
fn referenced_asset_paths(content: &str) -> Vec<String> {
    let parsed = parse_engram(content).ok();
    let body = parsed
        .as_ref()
        .map_or(content, |engram| engram.body.as_str());
    let mut paths = crystalline_core::find_asset_refs(body);
    if let Some(claim) = parsed.as_ref().and_then(|e| asset_claim(&e.frontmatter))
        && !paths.contains(&claim)
    {
        paths.push(claim);
    }
    paths
}

/// The part of an attachment path below the reserved folder (`notes/shot.png`
/// for `assets/notes/shot.png`), which is the part every spelling of a
/// reference to it shares.
fn asset_tail(path: &str) -> &str {
    path.strip_prefix(crystalline_core::ASSETS_PREFIX)
        .unwrap_or(path)
}

/// The `assets/` path an engram's `analyzes` claim names, or `None` when it
/// claims nothing under the folder.
///
/// `analyzes` is ordinary custom frontmatter (the agent's act of claiming an
/// attachment it read), so the value is whatever was written there: a leading
/// `./` is stripped and the folder segment is folded to its canonical
/// spelling, and anything that does not address the reserved folder at all is
/// not a claim.
/// One domain's sweep: its report and how many of its engrams no longer parse.
struct DomainSweep {
    /// The ranked findings for that domain, acknowledgments already applied.
    report: SweepReport,
    /// Engrams that failed to parse and were skipped rather than failing the
    /// run.
    unparsed: usize,
}

/// What an `evolve_ack` assignment asks for, read out of its value and nothing
/// else. [`Engine::ack_intent`] is how a surface gets one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckIntent {
    /// Record `rule`'s finding as intentional, with the caller's note.
    Record {
        /// The rule id, uppercased.
        rule: String,
        /// Why the finding is intentional, folded to one line.
        note: Option<String>,
    },
    /// Take `rule`'s acknowledgment back, so the finding resurfaces.
    Remove {
        /// The rule id, uppercased.
        rule: String,
    },
}

/// An [`AckIntent`] the server has completed, on its way to the text edit: the
/// record carries the scope only a sweep could supply, the removal carries the
/// rule alone because the entry it names is already in the file.
enum AckDraft {
    Record(EvolveAck),
    Remove(String),
}

/// Split an `evolve_ack` value into what it asks for: a record, or a removal.
///
/// The record form is the rule id up to the first whitespace and everything
/// after it as the note. The id is uppercased, so `v101 keep` records the same
/// acknowledgment `V101 keep` does.
///
/// The removal form is the whole first token being exactly `remove`, followed
/// by one rule id and nothing else. Case-sensitive and whole-token on purpose:
/// the record form puts the rule first, so `V101 remove this later` is a note
/// that happens to start with the word and stays a record. Trailing text is
/// refused rather than dropped, because the most likely thing after the rule is
/// a note the caller believed was being stored.
fn parse_ack_value(raw: &str) -> Result<AckIntent> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(EngineError::Invalid(ack_value_message()));
    }
    let (head, rest) = match raw.split_once(char::is_whitespace) {
        Some((head, rest)) => (head, rest.trim()),
        None => (raw, ""),
    };
    if head == REMOVE_VERB {
        let mut tokens = rest.split_whitespace();
        let Some(rule) = tokens.next() else {
            return Err(EngineError::Invalid(removal_form_message(
                "name the rule to take back",
            )));
        };
        if tokens.next().is_some() {
            return Err(EngineError::Invalid(removal_form_message(
                "drop the extra text",
            )));
        }
        let rule = rule.to_ascii_uppercase();
        if rule_info(&rule).is_none() {
            return Err(EngineError::Invalid(unknown_rule_message(&rule)));
        }
        return Ok(AckIntent::Remove { rule });
    }
    let rule = head.trim().to_ascii_uppercase();
    if rule_info(&rule).is_none() {
        return Err(EngineError::Invalid(unknown_rule_message(&rule)));
    }
    let note = fold_note(rest);
    Ok(AckIntent::Record {
        rule,
        note: (!note.is_empty()).then_some(note),
    })
}

/// The first token that makes an `evolve_ack` value a removal.
const REMOVE_VERB: &str = "remove";

/// The last stop before an `evolve_ack` rewrite is persisted: the bytes have to
/// parse, or nothing is written and the caller hears why.
///
/// Both halves of the key run through it, because neither edits lines in place:
/// a record and a removal each re-render the whole value from the entries that
/// survive ([`set_evolve_ack`]), so what lands is emitter output rather than the
/// file minus a line. A write that persisted unparseable bytes would make the
/// engram invisible to the sweep, to `read_engram` and to search - the one
/// failure this path must never cause while claiming to tidy knowledge up.
/// `act` names which half refused, so a caller reads what did not happen rather
/// than a generic parse complaint.
fn guarded_ack_write(out: String, act: &str, identifier: &str) -> Result<String> {
    parse_engram(&out).map_err(|e| {
        EngineError::Invalid(format!(
            "the {act} would leave '{identifier}' unparseable ({e}); nothing was written"
        ))
    })?;
    Ok(out)
}

/// What a caller hears when an `evolve_ack` assignment carries no value at all.
///
/// It names **both** forms rather than only the record, because this is the
/// message an agent that guessed at the key reads, and the take-back has no
/// other discovery path: nothing in a value it typed wrong hints that `remove
/// <rule-id>` exists. An error text is a teaching surface here, so the cost of
/// the second clause is a line and the benefit is the other half of the verb.
fn ack_value_message() -> String {
    format!(
        "{EVOLVE_ACK_KEY} takes a rule id optionally followed by a note ('V101 lineage citation, keep'), or 'remove <rule-id>' to take an acknowledgment back"
    )
}

/// What a caller hears when the removal form is malformed: the form itself,
/// then the fix.
fn removal_form_message(fix: &str) -> String {
    format!("a removal takes exactly 'remove <rule-id>'; {fix}")
}

/// A note as one line of prose: every run of whitespace, newlines included,
/// folded to a single space.
///
/// Folded rather than refused, deliberately. A note is free text a person pastes
/// into a box - a two-line justification out of a chat window is the ordinary
/// case, not an attack - and refusing it would send them back to reformat prose
/// that nothing ever parses. It is also never machine-read: it is shown back to
/// whoever reads the queue, and one line reads the same as two there.
///
/// What the folding buys is that the frontmatter this lands in stays valid. The
/// emitter escapes control characters too (`crystalline_core::emit`), so this is
/// the readable half of a defense that holds at both ends rather than the only
/// guard.
fn fold_note(note: &str) -> String {
    note.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a caller hears when it names a rule the catalog does not have.
fn unknown_rule_message(rule: &str) -> String {
    format!(
        "'{rule}' is not a rule the sweep knows; the catalog holds {}",
        RULES.iter().map(|r| r.id).collect::<Vec<_>>().join(", ")
    )
}

/// The attachment path an identifier names, or `None` when it names an engram.
///
/// A leading `./` is folded and the reserved folder segment is canonicalized by
/// [`crystalline_core::canonical_asset_path`], so `./Assets/deck.png` and
/// `assets/deck.png` are the same file. Only the prefix decides: an engram
/// never lives under the reserved folder, which is what makes one verb able to
/// serve both without guessing.
fn attachment_identifier(identifier: &str) -> Option<String> {
    let raw = identifier.trim().trim_start_matches("./");
    crystalline_core::is_under_assets(raw)
        .then(|| crystalline_core::canonical_asset_path(raw))
        .flatten()
}

/// The acknowledgments an engram's markdown carries.
fn acks_of(source: &str) -> Vec<EvolveAck> {
    parse_engram(source)
        .ok()
        .and_then(|e| {
            e.frontmatter
                .extra
                .get(EVOLVE_ACK_KEY)
                .map(EvolveAck::parse_list)
        })
        .unwrap_or_default()
}

/// Whether the engram acknowledges `rule` at all.
///
/// Case-folded, like every other rule comparison on this path: a hand-written
/// `- { rule: v101 }` suppresses findings, so it has to be findable - and
/// withdrawable - too.
fn has_ack(source: &str, rule: &str) -> bool {
    acks_of(source)
        .iter()
        .any(|a| a.rule.eq_ignore_ascii_case(rule))
}

/// The engram's markdown with `rule`'s acknowledgment dropped and every other
/// entry left exactly as it was. Removing the last one removes the key rather
/// than leaving an empty one ([`set_evolve_ack`] on an empty slice).
///
/// The one removal both surfaces run: Fluid's withdraw
/// ([`Engine::unacknowledge_finding_as`]) and an agent's `remove <rule-id>`
/// value. They differ only in how they report an entry that is not there - a
/// `false` the REST layer answers as a 404, an error the agent reads - which is
/// why the presence test is [`has_ack`] beside this rather than folded into it.
fn without_ack(source: &str, rule: &str) -> String {
    let kept: Vec<EvolveAck> = acks_of(source)
        .into_iter()
        .filter(|a| !a.rule.eq_ignore_ascii_case(rule))
        .collect();
    set_evolve_ack(source, &kept)
}

/// The engram's acknowledgments with `entry` folded in: one entry per rule, so
/// re-acknowledging a finding replaces what it said rather than stacking a
/// second line nobody reads. The replacement keeps the original position, which
/// keeps a hand-ordered list hand-ordered.
fn merged_acks(source: &str, entry: EvolveAck) -> Vec<EvolveAck> {
    let mut entries = acks_of(source);
    let mut replaced = false;
    entries.retain_mut(|existing| {
        if !existing.rule.eq_ignore_ascii_case(&entry.rule) {
            return true;
        }
        // A hand-edited file may name one rule twice; the entry just written is
        // the survivor and the rest go, so the list stays one entry per rule.
        if replaced {
            return false;
        }
        *existing = entry.clone();
        replaced = true;
        true
    });
    if !replaced {
        entries.push(entry);
    }
    entries
}

/// One acknowledgment as a surface renders it.
fn ack_json(entry: &EvolveAck) -> Value {
    json!({
        "rule": entry.rule,
        "scope": entry.scope,
        "note": entry.note,
        "by": entry.by,
        "at": entry.at.map(|at| at.to_rfc3339()),
    })
}

fn asset_claim(fm: &Frontmatter) -> Option<String> {
    let raw = fm.extra.get("analyzes")?.as_str()?.trim();
    crystalline_core::canonical_asset_path(raw.trim_start_matches("./"))
}

/// The acknowledgments an engram carries, as the sweep reads them.
///
/// The provenance the file records (`by`, `at`) is left behind here on purpose:
/// the detectors decide whether an acknowledgment still matches its evidence,
/// never who gave it. Malformed entries are skipped by
/// [`EvolveAck::parse_list`], so a hand-edited line costs its own entry and
/// nothing else.
fn ack_entries(fm: &Frontmatter) -> Vec<AckEntry> {
    let Some(value) = fm.extra.get(EVOLVE_ACK_KEY) else {
        return Vec::new();
    };
    EvolveAck::parse_list(value)
        .into_iter()
        .map(|a| AckEntry {
            rule: a.rule,
            scope: a.scope,
            note: a.note,
        })
        .collect()
}

/// `assets/deck.pptx` as `assets/deck-2.pptx`: the name an attachment takes
/// when the destination already holds a different file under its own.
///
/// The counter goes before the extension rather than after it, so the file
/// keeps the extension its mime and its allowlist decision rest on, and the
/// stem is shortened as far as it has to be for the result to pass
/// [`crystalline_core::validate_asset_path`]. A path already at the length
/// ceiling would otherwise grow past it, and since this name is what the
/// moving engram's references are rewritten to, an invalid one would be a
/// reference rewritten to a path no write can ever accept. `None` when no
/// valid name can be built even with the stem gone, which leaves the
/// attachment uncarried rather than renamed into nowhere.
fn suffixed_asset_path(path: &str, attempt: usize) -> Option<String> {
    let (dir, name) = match path.rsplit_once('/') {
        Some((dir, name)) => (format!("{dir}/"), name),
        None => (String::new(), path),
    };
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) => (stem, Some(extension)),
        None => (name, None),
    };
    let mut keep = stem.len();
    loop {
        let candidate = match extension {
            Some(extension) => format!("{dir}{}-{attempt}.{extension}", &stem[..keep]),
            None => format!("{dir}{}-{attempt}", &stem[..keep]),
        };
        if crystalline_core::validate_asset_path(&candidate).is_ok() {
            return Some(candidate);
        }
        if keep == 0 {
            return None;
        }
        // One character at a time, never one byte: a stem cut through a
        // multi-byte character would not be a string at all.
        keep -= 1;
        while keep > 0 && !stem.is_char_boundary(keep) {
            keep -= 1;
        }
    }
}

/// The moving engram's text with every renamed attachment reference - in the
/// body and in the `analyzes` claim - pointing at the name the file took at
/// the destination.
///
/// String surgery on both halves, never a re-emit: the frontmatter claim is
/// replaced line-wise by [`set_frontmatter_field`] and the body only where a
/// link destination actually changes, so a move that renames one attachment
/// leaves every other byte of the engram exactly as its author wrote it.
fn rewrite_carried_refs(content: &str, renames: &BTreeMap<String, String>) -> String {
    if renames.is_empty() {
        return content.to_string();
    }
    let Ok(parsed) = parse_engram_lossless(content) else {
        // An engram the parser refuses still moves, so its references still
        // have to follow; without a frontmatter span the whole text is the
        // body.
        return rewrite_asset_refs(content, renames);
    };
    let body = rewrite_asset_refs(&content[parsed.body_span.clone()], renames);
    let mut out = format!("{}{}", &content[..parsed.body_span.start], body);
    if let Some(renamed) = asset_claim(&parsed.engram.frontmatter).and_then(|c| renames.get(&c)) {
        out = set_frontmatter_field(&out, "analyzes", renamed);
    }
    out
}

/// Every `assets/` link destination in a body pointed at its new name.
///
/// A `./` prefix and a `#fragment` are spellings of the reference rather than
/// parts of the path, so both survive untouched and only the path between them
/// changes. Fenced code is skipped exactly as
/// [`crystalline_core::find_asset_refs`] skips it, so an example in a snippet
/// is never rewritten into a path the snippet did not mean.
fn rewrite_asset_refs(body: &str, renames: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut fence: Option<(char, usize)> = None;
    for line in body.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        match fence {
            None => {
                if let Some((marker, count)) = asset_fence_marker(text) {
                    fence = Some((marker, count));
                    out.push_str(line);
                    continue;
                }
            }
            Some((open_marker, open_count)) => {
                if let Some((marker, count)) = asset_fence_marker(text)
                    && marker == open_marker
                    && count >= open_count
                    && text.trim_start()[count..].trim().is_empty()
                {
                    fence = None;
                }
                out.push_str(line);
                continue;
            }
        }
        out.push_str(&rewrite_line_asset_refs(line, renames));
    }
    out
}

/// A fenced code block's opening or closing marker: the character and how many
/// of it, for a line indented no more than three spaces.
///
/// The same rule the core parser reads fences by, restated here because it is
/// crate-private there and this is the only reader of it outside core.
fn asset_fence_marker(line: &str) -> Option<(char, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let first = rest.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let count = rest.chars().take_while(|c| *c == first).count();
    (count >= 3).then_some((first, count))
}

/// One line's `](assets/...)` destinations rewritten, every other byte of the
/// line copied through.
fn rewrite_line_asset_refs(line: &str, renames: &BTreeMap<String, String>) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    // How much of the line is already in `out`, so the untouched runs between
    // two rewritten destinations are copied exactly once.
    let mut copied = 0usize;
    let mut idx = 0usize;
    while let Some(hit) = line[idx..].find("](") {
        let open = idx + hit + 2;
        // Markdown allows balanced parentheses inside a destination, so the
        // closing one is the depth-zero `)`, the same scan core's reference
        // scanner runs.
        let mut depth = 1usize;
        let mut end = None;
        for (offset, byte) in bytes[open..].iter().enumerate() {
            match byte {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        idx = end + 1;
        let inside = &line[open..end];
        // The destination is the first token; a title clause may follow it.
        let Some(target) = inside.split_whitespace().next() else {
            continue;
        };
        // Where that token starts in the line, so the rewrite lands on the
        // path itself rather than on the whitespace in front of it.
        let start = open + (inside.len() - inside.trim_start().len());
        let dot = if target.starts_with("./") { 2 } else { 0 };
        let path_end = target[dot..]
            .find('#')
            .map_or(target.len(), |offset| dot + offset);
        let Some(renamed) = renames.get(&target[dot..path_end]) else {
            continue;
        };
        out.push_str(&line[copied..start + dot]);
        out.push_str(renamed);
        copied = start + path_end;
    }
    out.push_str(&line[copied..]);
    out
}

/// A file's modification instant in the spelling the sync walker records, so a
/// row written here and a row written by a scan compare equal.
fn asset_modified(abs: &Path) -> String {
    let mtime = std::fs::metadata(abs)
        .map(|meta| mtime_secs(&meta))
        .unwrap_or_else(|_| Utc::now().timestamp());
    chrono::DateTime::from_timestamp(mtime, 0)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .to_rfc3339()
}

/// The miss message every attachment verb reports, one spelling.
fn missing_attachment(domain: &str, path: &str) -> String {
    format!("no attachment '{path}' in domain '{domain}'")
}

/// The refusal an over-cap attachment earns, one spelling for the write path
/// and the read path so a file and an upload of the same size are refused in
/// the same words.
fn over_cap_error(path: &str, size: u64) -> String {
    format!(
        "attachment '{path}' is {size} bytes, over the {} byte ceiling",
        crystalline_core::MAX_ATTACHMENT_BYTES
    )
}

/// Whether a forward-slashed domain-relative path lands under the reserved
/// `assets/` prefix, where attachments live and no engram is ever written.
///
/// A folder called `assets-notes` is an ordinary folder, and so is an engram
/// file called `assets.md`: only the folder itself and what sits inside it is
/// reserved.
///
/// The decision itself is [`crystalline_core::is_under_assets`], the one
/// classifier the sync walk and the daemon's watcher ask too, so the
/// reservation cannot mean one thing to a write and another to a scan. This
/// wrapper only strips the leading `./` and the surrounding slashes a caller
/// may have typed. Callers pass an already normalized path (see
/// [`normalize_rel`]), so a `..` segment can no longer walk in behind the
/// check.
fn is_assets_reserved(rel: &str) -> bool {
    crystalline_core::is_under_assets(rel.trim_start_matches("./").trim_matches('/'))
}

/// A caller-supplied domain-relative path as one normalized, forward-slashed
/// string: a leading `./` stripped, surrounding slashes trimmed, empty and `.`
/// segments dropped.
///
/// `..` segments deliberately survive, because normalizing them away would
/// resolve a path the caller never asked for. They are refused instead, by the
/// [`is_contained_rel`] screen every call site runs straight afterwards, which
/// is what makes the reserved-name and reserved-prefix checks that follow
/// decidable on the text alone.
fn normalize_rel(raw: &str) -> String {
    raw.trim_start_matches("./")
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// The refusal a write earns by aiming outside the domain root.
fn escapes_root_error(rel: &str) -> String {
    format!(
        "'{rel}' is not a domain-relative destination: a path may not climb out of the domain root with a `..` segment or name an absolute path."
    )
}

/// The refusal an engram write earns by aiming at the reserved `assets/`
/// prefix.
fn assets_reserved_error(rel: &str) -> String {
    format!(
        "'{rel}' sits under the reserved {} folder: assets is reserved for attachments, so nothing under it is an engram. Choose another folder or destination.",
        crystalline_core::ASSETS_PREFIX
    )
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
/// null bound is dropped because open-ended validity is expressed by absence. A
/// `verified` supplied as metadata is shape-checked the same way, so a
/// verification is either recorded in the OKF form or the write is refused.
#[allow(clippy::too_many_arguments)]
fn build_markdown(
    engram_type: &str,
    title: &str,
    permalink: &str,
    tags: &[String],
    status: &str,
    recorded_at: &str,
    actor: &str,
    now: DateTime<FixedOffset>,
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
    fm.generated = Some(crystalline_core::Generated {
        by: actor.to_string(),
        at: Some(now),
    });
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
    crystalline_core::temporal::normalize_verified(&mut fm)
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
        "type"
            | "title"
            | "permalink"
            | "tags"
            | "status"
            | "recorded_at"
            | "timestamp"
            | "generated"
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

#[cfg(test)]
mod context_rank_tests {
    use super::*;
    use crystalline_index::GraphEdge;

    /// A slice node with the given id and optional salience; the descriptive
    /// fields are filler that context ranking never reads.
    fn node(id: i64, salience: Option<f64>) -> GraphNode {
        GraphNode {
            id: EngramId(id),
            domain: "d".to_string(),
            permalink: format!("p{id}"),
            title: format!("t{id}"),
            engram_type: "engram".to_string(),
            salience,
            status: "current".to_string(),
        }
    }

    /// A relation edge between two ids; direction is irrelevant to the
    /// symmetric adjacency the ranker builds.
    fn edge(from: i64, to: i64) -> GraphEdge {
        GraphEdge {
            from: EngramId(from),
            to: EngramId(to),
            rel_type: "rel".to_string(),
            kind: EdgeKind::Relation,
        }
    }

    fn seeds(ids: &[i64]) -> HashSet<i64> {
        ids.iter().copied().collect()
    }

    /// S - A - C chain seeded at S: mass decays with graph distance from the
    /// seed, so the nearer node A outranks the farther node C.
    #[test]
    fn chain_decays_with_distance() {
        let slice = GraphSlice {
            nodes: vec![node(1, None), node(2, None), node(3, None)],
            edges: vec![edge(1, 2), edge(2, 3)],
        };
        let mass = context_rank(&slice, &seeds(&[1]));
        assert!(
            mass[&2] > mass[&3],
            "A ({}) should outrank C ({})",
            mass[&2],
            mass[&3]
        );
    }

    /// Two seeds both edge to A, one of them also to B: A draws teleport mass
    /// from both seeds while B draws from one, so connectivity beats distance.
    #[test]
    fn connectivity_beats_distance() {
        let slice = GraphSlice {
            nodes: vec![node(1, None), node(2, None), node(3, None), node(4, None)],
            edges: vec![edge(1, 3), edge(2, 3), edge(1, 4)],
        };
        let mass = context_rank(&slice, &seeds(&[1, 2]));
        assert!(
            mass[&3] > mass[&4],
            "A ({}) should outrank B ({})",
            mass[&3],
            mass[&4]
        );
    }

    /// The ranker is a pure function of its inputs: two runs over the same
    /// slice return byte-identical masses (exact f64 equality).
    #[test]
    fn repeat_calls_are_identical() {
        let slice = GraphSlice {
            nodes: vec![node(1, None), node(2, None), node(3, None)],
            edges: vec![edge(1, 2), edge(2, 3)],
        };
        let first = context_rank(&slice, &seeds(&[1]));
        let second = context_rank(&slice, &seeds(&[1]));
        assert_eq!(first, second);
    }

    /// A lone seed with no edges is a dangling node: its mass stays finite,
    /// never NaN, and concentrates on the seed itself.
    #[test]
    fn isolated_seed_is_finite() {
        let slice = GraphSlice {
            nodes: vec![node(1, None)],
            edges: vec![],
        };
        let mass = context_rank(&slice, &seeds(&[1]));
        let m = mass[&1];
        assert!(m.is_finite(), "mass must be finite, got {m}");
        assert!(!m.is_nan(), "mass must not be NaN");
        assert!(
            m > 0.99,
            "mass should concentrate on the lone seed, got {m}"
        );
    }

    /// Personalized PageRank conserves mass: the ranked masses sum to one.
    #[test]
    fn masses_sum_to_one() {
        let slice = GraphSlice {
            nodes: vec![node(1, None), node(2, None), node(3, None)],
            edges: vec![edge(1, 2), edge(2, 3)],
        };
        let mass = context_rank(&slice, &seeds(&[1]));
        let total: f64 = mass.values().sum();
        assert!(
            (total - 1.0).abs() < 1e-6,
            "masses should sum to 1, got {total}"
        );
    }

    /// The edge-collection SQL has no ORDER BY, so `slice.edges` order is not
    /// guaranteed identical across backends or calls. Node 3 here has two
    /// edges (one from each seed), so its inbound sum actually depends on
    /// accumulation order: reversing the edge Vec must still produce
    /// byte-identical masses (exact f64 equality).
    #[test]
    fn edge_order_does_not_change_masses() {
        let nodes = vec![node(1, None), node(2, None), node(3, None), node(4, None)];
        let edges = vec![edge(1, 3), edge(2, 3), edge(1, 4)];
        let forward = GraphSlice {
            nodes: nodes.clone(),
            edges: edges.clone(),
        };
        let mut reversed_edges = edges;
        reversed_edges.reverse();
        let reversed = GraphSlice {
            nodes,
            edges: reversed_edges,
        };
        let first = context_rank(&forward, &seeds(&[1, 2]));
        let second = context_rank(&reversed, &seeds(&[1, 2]));
        assert_eq!(first, second);
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;
    use crystalline_core::config::DomainEntry;
    use crystalline_index::TursoStore;

    /// An engine over an in-memory store whose config registers `domains`.
    async fn engine_with_domains(domains: &[&str]) -> Engine {
        let store = TursoStore::open_in_memory().await.unwrap();
        let mut config = GlobalConfig::default();
        for name in domains {
            config.domains.insert(
                (*name).to_string(),
                DomainEntry::file(format!("/roots/{name}")),
            );
        }
        Engine::new(Arc::new(Mutex::new(store)), config, None, None)
    }

    /// The single-domain origin operations take a caller-supplied name, so an
    /// unregistered one must not leave a lock entry behind: the map is keyed by
    /// name and never pruned, so every unchecked name would be retained for the
    /// life of the process. The error is the same `UnknownDomain` the operation
    /// answered before the check moved ahead of the lock.
    #[tokio::test]
    async fn an_unregistered_name_never_gets_a_lock_entry() {
        let engine = engine_with_domains(&["known"]).await;

        let err = engine.origin_lock_registered("nope").unwrap_err();
        assert!(matches!(err, EngineError::UnknownDomain { .. }), "{err}");
        assert!(
            engine.origin_locks.lock().unwrap().is_empty(),
            "a failing name must not be retained"
        );

        // A registered name behaves exactly as before: one lazily created entry,
        // reused on the next call.
        let first = engine.origin_lock_registered("known").unwrap();
        let second = engine.origin_lock_registered("known").unwrap();
        assert!(Arc::ptr_eq(&first, &second), "the lock is created once");
        assert_eq!(engine.origin_locks.lock().unwrap().len(), 1);
    }

    /// One lock per file, whoever asks for it.
    #[tokio::test]
    async fn one_file_has_one_write_lock() {
        let engine = engine_with_domains(&["known"]).await;
        let first = engine.write_lock(Path::new("/roots/known/alpha.md"));
        let second = engine.write_lock(Path::new("/roots/known/alpha.md"));
        assert!(Arc::ptr_eq(&first, &second), "the lock is created once");
        let other = engine.write_lock(Path::new("/roots/known/beta.md"));
        assert!(!Arc::ptr_eq(&first, &other), "and it is per file");
        assert_eq!(engine.write_locks.lock().unwrap().len(), 2);
    }

    /// Two spellings of one file share a lock, which is the point of keying on
    /// the canonical path: a domain registered through a symlink and the same
    /// domain registered at its real path are two strings for one file, and two
    /// locks over one file are no lock at all.
    #[cfg(unix)]
    #[tokio::test]
    async fn one_file_reached_two_ways_still_has_one_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("eng");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("alpha.md"), "x").unwrap();
        let linked = tmp.path().join("linked");
        std::os::unix::fs::symlink(&root, &linked).unwrap();

        let engine = engine_with_domains(&["known"]).await;
        let direct = engine.write_lock(&root.join("alpha.md"));
        let through_link = engine.write_lock(&linked.join("alpha.md"));
        assert!(
            Arc::ptr_eq(&direct, &through_link),
            "the symlinked spelling resolves to the same file, so to the same lock"
        );
        // And a file that does not exist yet - a create - still resolves
        // through its folder, so the create and the first save of one engram
        // agree on the key.
        let unborn = engine.write_lock(&root.join("beta.md"));
        let unborn_linked = engine.write_lock(&linked.join("beta.md"));
        assert!(Arc::ptr_eq(&unborn, &unborn_linked));
        assert_eq!(engine.write_locks.lock().unwrap().len(), 2);
    }

    /// A file-domain engine over `root`, with whatever files were written into
    /// it already indexed.
    async fn file_engine(root: &Path) -> Arc<Engine> {
        let store = TursoStore::open_in_memory().await.unwrap();
        let mut config = GlobalConfig::default();
        config
            .domains
            .insert("eng".to_string(), DomainEntry::file(root));
        let engine = Arc::new(Engine::new(Arc::new(Mutex::new(store)), config, None, None));
        engine.sync(None).await.unwrap();
        engine
    }

    /// The markdown of a minimal engram, for the tests below.
    fn engram(title: &str, permalink: &str, body: &str) -> String {
        format!(
            "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n{body}\n"
        )
    }

    /// A create checks that the permalink is free *inside* the file's lock, so
    /// two creates of one title cannot both find it free.
    ///
    /// Unlocked, the second create writes over the first's body and answers
    /// "created" rather than the conflict that says the name was taken - the
    /// worse half of the pair, because the caller is told it succeeded. Driven
    /// the same way as the save test: the lock is held from outside while the
    /// create is in flight, the engram it is about to claim is landed and
    /// indexed underneath it, and the create must then refuse.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_create_checks_the_permalink_is_free_inside_the_file_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("eng");
        std::fs::create_dir_all(&root).unwrap();
        let engine = file_engine(&root).await;

        let abs = root.join("beta.md");
        let lock = engine.write_lock(&abs);
        let held = lock.lock().await;

        let creator = engine.clone();
        let task = tokio::spawn(async move {
            creator
                .write_engram(&WriteParams {
                    domain: "eng".to_string(),
                    title: "Beta".to_string(),
                    content: "Mine.".to_string(),
                    folder: None,
                    engram_type: None,
                    tags: Vec::new(),
                    status: None,
                    metadata: None,
                    overwrite: false,
                })
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !task.is_finished(),
            "the create must be waiting on the file lock, not already past its existence check"
        );

        // The other writer got there first: the file lands and is indexed while
        // this create is blocked.
        let theirs = engram("Beta", "beta", "Theirs.");
        std::fs::write(&abs, &theirs).unwrap();
        engine.sync(None).await.unwrap();
        drop(held);

        match task.await.unwrap() {
            Err(EngineError::Conflict(message)) => assert!(
                message.contains("already exists"),
                "the conflict says the name was taken: {message}"
            ),
            other => panic!("the create claimed a permalink that was already gone: {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&abs).unwrap(),
            theirs,
            "and the other writer's engram is untouched"
        );
    }

    /// An edit reads, applies and writes inside the file's lock, so a
    /// concurrent write is built on rather than dropped.
    ///
    /// This edit carries no `expected_checksum`, which is still legal - last
    /// write wins, and serializing is the entire guarantee: what must not
    /// happen is the edit computing from text that has already been replaced
    /// and then writing that computation over the replacement. Here the other
    /// writer's line lands while the edit is blocked, and both lines have to
    /// survive.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_edit_reads_and_writes_inside_the_file_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("eng");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("alpha.md"), engram("Alpha", "alpha", "The body.")).unwrap();
        let engine = file_engine(&root).await;

        let abs = root.join("alpha.md");
        let lock = engine.write_lock(&abs);
        let held = lock.lock().await;

        let editor = engine.clone();
        let task = tokio::spawn(async move {
            let params: EditParams = serde_json::from_value(json!({
                "identifier": "alpha",
                "domain": "eng",
                "operation": "append",
                "content": "From the agent.\n",
            }))
            .unwrap();
            editor.edit_engram(&params).await
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !task.is_finished(),
            "the edit must be waiting on the file lock, not already holding stale text"
        );

        // The other writer's change lands while the edit is blocked.
        std::fs::write(
            &abs,
            engram("Alpha", "alpha", "The body.\n\nFrom the browser."),
        )
        .unwrap();
        drop(held);

        task.await.unwrap().expect("the edit applies");
        let final_text = std::fs::read_to_string(&abs).unwrap();
        assert!(
            final_text.contains("From the browser."),
            "the concurrent write must not be silently dropped: {final_text}"
        );
        assert!(
            final_text.contains("From the agent."),
            "and the edit still applied, on top of it: {final_text}"
        );
    }

    /// A save reports where the engram answers *after* it landed, which is not
    /// always where it was addressed: the document is written verbatim, so an
    /// author may have edited the `permalink` line in it, and the index takes
    /// the permalink from the file. A receipt naming the old address would send
    /// the caller to a permalink nothing resolves.
    #[tokio::test]
    async fn a_save_that_renames_reports_the_new_permalink() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("eng");
        std::fs::create_dir_all(&root).unwrap();
        let original = engram("Alpha", "alpha", "The body.");
        std::fs::write(root.join("alpha.md"), &original).unwrap();
        let engine = file_engine(&root).await;

        let renamed = original.replace("permalink: alpha", "permalink: renamed");
        let receipt = engine
            .save_engram(&SaveParams {
                domain: "eng".to_string(),
                identifier: "alpha".to_string(),
                content: renamed.clone(),
                expected_checksum: sha256_hex(original.as_bytes()),
            })
            .await
            .unwrap();
        assert_eq!(
            receipt["permalink"], "renamed",
            "the receipt names where the engram now answers"
        );
        assert_eq!(receipt["path"], "alpha.md", "the file did not move");
        assert_eq!(
            std::fs::read_to_string(root.join("alpha.md")).unwrap(),
            renamed,
            "and the bytes are the author's own"
        );

        // An ordinary save still reports the address it was given.
        let plain = renamed.replace("The body.", "A sharper body.");
        let receipt = engine
            .save_engram(&SaveParams {
                domain: "eng".to_string(),
                identifier: "renamed".to_string(),
                content: plain.clone(),
                expected_checksum: sha256_hex(renamed.as_bytes()),
            })
            .await
            .unwrap();
        assert_eq!(receipt["permalink"], "renamed");
    }

    /// A save reads the file it is comparing against *inside* the per-file
    /// lock, which is what makes `If-Match` mean anything when two saves of one
    /// engram arrive together.
    ///
    /// Asserted by holding the lock from outside and rewriting the file while
    /// the save is blocked on it, which is exactly what a first writer does.
    /// A save that read and hashed before taking the lock would have compared
    /// against the original bytes, found its token fresh and overwritten the
    /// other author's work; a save that reads inside sees the new bytes and
    /// refuses. Two properties are checked, and the pair is what pins the
    /// order: that the save cannot finish while the lock is held, and that it
    /// then fails against the text that landed in the meantime. Driving it
    /// through two concurrent HTTP saves instead would prove nothing - the
    /// request round trip is long enough that they serialize by themselves,
    /// whether or not anything holds them apart.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_save_compares_inside_the_file_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("eng");
        std::fs::create_dir_all(&root).unwrap();
        let original = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\nThe original.\n";
        std::fs::write(root.join("alpha.md"), original).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let mut config = GlobalConfig::default();
        config
            .domains
            .insert("eng".to_string(), DomainEntry::file(&root));
        let engine = Arc::new(Engine::new(Arc::new(Mutex::new(store)), config, None, None));
        engine.sync(None).await.unwrap();

        let abs = root.join("alpha.md");
        let lock = engine.write_lock(&abs);
        let held = lock.lock().await;

        let saver = engine.clone();
        let mine = original.replace("The original.", "Mine.");
        let expected = sha256_hex(original.as_bytes());
        let task = tokio::spawn(async move {
            saver
                .save_engram(&SaveParams {
                    domain: "eng".to_string(),
                    identifier: "alpha".to_string(),
                    content: mine,
                    expected_checksum: expected,
                })
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !task.is_finished(),
            "the save must be waiting on the file lock, not already past its comparison"
        );

        // What the other writer did while this save was blocked.
        let theirs = original.replace("The original.", "Theirs.");
        std::fs::write(&abs, &theirs).unwrap();
        drop(held);

        let outcome = task.await.unwrap();
        match outcome {
            Err(EngineError::Conflict(message)) => assert!(
                message.starts_with("stale edit"),
                "the conflict speaks the shared wording: {message}"
            ),
            other => panic!("the save compared against bytes that were already gone: {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&abs).unwrap(),
            theirs,
            "and the other writer's version is still the one on disk"
        );
    }
}

#[cfg(test)]
mod attachment_carry_tests {
    use super::*;

    fn candidates() -> Vec<String> {
        vec![
            "assets/shot.png".to_string(),
            "assets/notes/deck.pptx".to_string(),
        ]
    }

    /// The safe default is structural: whatever went wrong while counting
    /// referents, every candidate comes back shared, which copies it and
    /// leaves the source copy in place. An error must never point in the
    /// deleting direction.
    #[test]
    fn a_counting_failure_resolves_to_shared() {
        let candidates = candidates();
        for failure in [
            EngineError::Invalid("broken".into()),
            EngineError::NotFound("gone".into()),
            EngineError::Conflict("busy".into()),
        ] {
            let resolved = resolve_shared(Err(failure), &candidates);
            assert_eq!(
                resolved,
                candidates.iter().cloned().collect::<HashSet<String>>(),
                "a failure to count has to read as 'still in use'"
            );
        }
    }

    /// And a successful count is passed through exactly, so the safe default
    /// costs nothing when the counting worked.
    #[test]
    fn a_successful_count_passes_through() {
        let candidates = candidates();
        let counted: HashSet<String> = std::iter::once(candidates[0].clone()).collect();
        assert_eq!(resolve_shared(Ok(counted.clone()), &candidates), counted);
        assert!(resolve_shared(Ok(HashSet::new()), &candidates).is_empty());
    }

    /// The bound the delete preview enumerates under, pinned at the edge:
    /// exactly [`MAX_PREVIEW_SCAN_ENGRAMS`] engrams still gets the full
    /// enumeration, one more does not. The number itself may move; which side
    /// of it each count falls on must not drift by an off-by-one.
    #[test]
    fn the_preview_scan_bound_includes_its_own_number() {
        assert!(count_within_preview_bound(0));
        assert!(count_within_preview_bound(1));
        assert!(count_within_preview_bound(MAX_PREVIEW_SCAN_ENGRAMS));
        assert!(!count_within_preview_bound(MAX_PREVIEW_SCAN_ENGRAMS + 1));
        assert!(!count_within_preview_bound(50_000));
    }

    #[test]
    fn the_screen_matches_every_spelling_of_a_reference() {
        assert_eq!(asset_tail("assets/shot.png"), "shot.png");
        assert_eq!(asset_tail("assets/notes/deck.pptx"), "notes/deck.pptx");
        // A claim may name the folder in another case; the part below it is
        // what both spellings share, which is why the screen tests that.
        assert!("analyzes: Assets/shot.png".contains(asset_tail("assets/shot.png")));
        assert!("![x](./assets/shot.png#right)".contains(asset_tail("assets/shot.png")));
    }

    #[test]
    fn a_suffixed_name_stays_a_valid_attachment_path() {
        assert_eq!(
            suffixed_asset_path("assets/shot.png", 2).unwrap(),
            "assets/shot-2.png"
        );
        assert_eq!(
            suffixed_asset_path("assets/notes/deck.pptx", 3).unwrap(),
            "assets/notes/deck-3.pptx"
        );

        // At the 256 byte ceiling the stem gives way, never the extension:
        // the name has to stay one a write will accept, because the engram's
        // references are rewritten to it.
        let at_cap = format!("assets/{}.png", "a".repeat(245));
        assert_eq!(at_cap.len(), 256);
        let suffixed = suffixed_asset_path(&at_cap, 2).unwrap();
        assert_eq!(suffixed, format!("assets/{}-2.png", "a".repeat(243)));
        assert!(crystalline_core::validate_asset_path(&suffixed).is_ok());
        let long_suffix = suffixed_asset_path(&at_cap, 100).unwrap();
        assert!(crystalline_core::validate_asset_path(&long_suffix).is_ok());

        // A multi-byte stem is cut on character boundaries, not byte ones:
        // 255 bytes of path with a two-byte stem character, where `-2` no
        // longer fits and exactly one character has to go.
        let wide = format!("assets/{}.png", "é".repeat(122));
        assert_eq!(wide.len(), 255);
        let cut = suffixed_asset_path(&wide, 2).unwrap();
        assert!(crystalline_core::validate_asset_path(&cut).is_ok());
        assert_eq!(cut, format!("assets/{}-2.png", "é".repeat(121)));

        // And a path with no room left at all is refused rather than
        // rewritten into something no write would take.
        let hopeless = format!("assets/{}/x.png", "d".repeat(246));
        assert!(suffixed_asset_path(&hopeless, 2).is_none());
    }
}

#[cfg(test)]
mod receipt_permalink_tests {
    use super::*;

    /// The address the index answers with is the one the receipt names: that
    /// read-back is the whole point of asking, so a hit must pass through.
    #[test]
    fn the_address_the_index_answers_with_is_the_one_reported() {
        assert_eq!(
            receipt_permalink(Ok(Some("notes/moved".to_string())), "notes/old".to_string()),
            "notes/moved"
        );
    }

    /// No row for the path (the write is committed, the index has not caught
    /// up) falls back to the name the caller already knows.
    #[test]
    fn a_missing_row_falls_back_to_the_known_name() {
        assert_eq!(
            receipt_permalink(Ok(None), "notes/old".to_string()),
            "notes/old"
        );
    }

    /// And the case this function exists for: the lookup itself failed. The
    /// write it decorates is already committed on disk and in the index, so
    /// the failure can only cost the receipt its freshest address, never turn
    /// a done write into a reported error.
    #[test]
    fn a_lookup_failure_falls_back_rather_than_failing_a_committed_write() {
        for failure in [
            EngineError::Internal("the database went away".into()),
            EngineError::Invalid("broken".into()),
            EngineError::Conflict("busy".into()),
        ] {
            assert_eq!(
                receipt_permalink(Err(failure), "notes/old".to_string()),
                "notes/old",
                "a receipt lookup must never fail the write it describes"
            );
        }
    }
}

#[cfg(test)]
mod share_actor_tests {
    use super::*;
    use crystalline_core::config::GitHubConfig;
    use crystalline_index::TursoStore;
    use crystalline_remote::TokenIdentity;

    /// An engine whose credentials live in a tempdir file store, never the
    /// developer's real keychain, with no injected provider: the point of these
    /// tests is the credential resolution the override would short-circuit.
    async fn credential_engine(tmp: &tempfile::TempDir, mode: Option<&str>) -> Engine {
        let store = TursoStore::open_in_memory().await.unwrap();
        let tokens = tmp.path().join("tokens");
        std::fs::create_dir_all(&tokens).unwrap();
        let config = GlobalConfig {
            github: Some(GitHubConfig {
                enabled: Some(true),
                share_identity: mode.map(str::to_string),
                ..GitHubConfig::default()
            }),
            ..GlobalConfig::default()
        };
        Engine::new(
            Arc::new(Mutex::new(store)),
            config,
            None,
            Some(tmp.path().join("config.yaml")),
        )
        .with_token_store_dir(tokens)
    }

    /// Writes a token for `identity` exactly where the file-backed store reads
    /// it from, standing in for a `connect` that landed.
    fn write_token(dir: &std::path::Path, identity: &TokenIdentity, user: &str) {
        TokenStore::file_fallback_for(identity, dir)
            .unwrap()
            .save(&StoredToken {
                access_token: format!("{user}-secret"),
                host: "github.com".to_string(),
                user: user.to_string(),
                created_at: Utc::now(),
            })
            .unwrap();
    }

    fn personal(name: &str) -> TokenIdentity {
        TokenIdentity::Personal(name.to_string())
    }

    /// A [`ConnectAuth`] that validates any token as one fixed login and has
    /// no device path: enough to drive a personal connect with no network,
    /// and no keychain, behind it.
    struct AcceptingAuth(&'static str);

    #[async_trait::async_trait]
    impl ConnectAuth for AcceptingAuth {
        async fn start_device_flow(
            &self,
            _auth_base: &str,
            _client_id: &str,
        ) -> std::result::Result<crystalline_remote::DeviceFlowStart, RemoteError> {
            Err(RemoteError::NotConnected)
        }

        async fn run_device_flow(
            &self,
            _auth_base: &str,
            _client_id: &str,
            _start: &crystalline_remote::DeviceFlowStart,
        ) -> std::result::Result<String, RemoteError> {
            Err(RemoteError::NotConnected)
        }

        async fn validate_token(
            &self,
            _api_url: Option<&str>,
            _token: &str,
        ) -> std::result::Result<String, RemoteError> {
            Ok(self.0.to_string())
        }
    }

    /// A [`ConnectAuth`] whose device flow starts and then never finishes:
    /// enough to leave one pending flow standing for a test to cancel.
    struct HangingAuth;

    #[async_trait::async_trait]
    impl ConnectAuth for HangingAuth {
        async fn start_device_flow(
            &self,
            _auth_base: &str,
            _client_id: &str,
        ) -> std::result::Result<crystalline_remote::DeviceFlowStart, RemoteError> {
            Ok(crystalline_remote::DeviceFlowStart {
                device_code: "device".to_string(),
                user_code: "ABCD-1234".to_string(),
                verification_url: "https://github.test/device".to_string(),
                expires_in_secs: 900,
                interval_secs: 5,
            })
        }

        async fn run_device_flow(
            &self,
            _auth_base: &str,
            _client_id: &str,
            _start: &crystalline_remote::DeviceFlowStart,
        ) -> std::result::Result<String, RemoteError> {
            // Never lands: the flow is still waiting on its browser half.
            std::future::pending::<()>().await;
            unreachable!("a pending future never resolves")
        }

        async fn validate_token(
            &self,
            _api_url: Option<&str>,
            _token: &str,
        ) -> std::result::Result<String, RemoteError> {
            Ok("never".to_string())
        }
    }

    /// Forgetting a credential drops the pending device-flow record for the
    /// same identity, the way the Fluid disconnect does, freeing the
    /// one-flow-at-a-time slot. The spawned exchange itself is not stopped
    /// (see [`Engine::forget_cached_credential`]'s doc for the shared
    /// residue); what this pins is the record's removal.
    ///
    /// Observed through the engine's own one-flow-at-a-time rule: while a flow
    /// stands, a second identity's start is refused, so bob starting cleanly is
    /// the proof that alice's record was dropped rather than left standing.
    #[tokio::test]
    async fn forgetting_a_credential_drops_its_pending_device_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, Some("personal"))
            .await
            .with_connect_auth(Arc::new(HangingAuth));

        engine
            .start_github_identity_device_flow("alice")
            .await
            .expect("the flow starts and stays pending");
        assert!(
            matches!(
                engine.start_github_identity_device_flow("bob").await,
                Err(EngineError::ConnectInProgress)
            ),
            "a standing flow is what blocks the next one"
        );

        engine.forget_cached_credential(Some("alice")).unwrap();

        engine
            .start_github_identity_device_flow("bob")
            .await
            .expect("alice's flow was cancelled, so the slot is free");
    }

    /// Connecting an identity that already had one REPLACES the credential
    /// every later write resolves - the cache slot is refreshed rather than
    /// left holding the token that was just superseded. The bug this pins is
    /// silent and expensive: a person who rotates a revoked token would keep
    /// sharing with the revoked one until the process restarted.
    #[tokio::test]
    async fn a_personal_connect_refreshes_that_identitys_cached_credential() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, Some("personal"))
            .await
            .with_connect_auth(Arc::new(AcceptingAuth("alice-gh")));

        let connected = engine
            .connect_github_identity_token("alice", "first-token")
            .await
            .unwrap();
        assert_eq!(connected.login.as_deref(), Some("alice-gh"));
        assert!(connected.connected_at.is_some(), "the card says since when");

        // Resolve once, so the credential is definitely cached.
        let (_api_url, token) = engine
            .resolve_share_credential(&ShareActor::Account("alice".to_string()))
            .expect("alice's credential");
        assert_eq!(token.access_token, "first-token");

        engine
            .connect_github_identity_token("alice", "second-token")
            .await
            .unwrap();
        let (_api_url, token) = engine
            .resolve_share_credential(&ShareActor::Account("alice".to_string()))
            .expect("alice's credential");
        assert_eq!(
            token.access_token, "second-token",
            "the reconnect must not leave the superseded token cached"
        );

        // And a personal connect never touched the machine's own slot.
        let cache = engine.github_tokens.lock().unwrap();
        assert!(
            !cache.contains_key(&credential_cache_key(&TokenIdentity::Instance, None)),
            "a personal connect is not an instance connect"
        );
    }

    /// Disconnecting forgets the credential AND the cached client built from
    /// it: a share right after a disconnect must be refused, not served the
    /// token that was just deleted.
    #[tokio::test]
    async fn disconnecting_an_identity_evicts_its_cached_credential_too() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, Some("personal"))
            .await
            .with_connect_auth(Arc::new(AcceptingAuth("alice-gh")));
        engine
            .connect_github_identity_token("alice", "first-token")
            .await
            .unwrap();
        // Bob stays connected throughout: a disconnect is one credential's.
        engine
            .connect_github_identity_token("bob", "bobs-token")
            .await
            .unwrap();
        engine
            .resolve_share_credential(&ShareActor::Account("alice".to_string()))
            .expect("cached before the disconnect");

        let gone = engine.disconnect_github_identity("alice").await.unwrap();
        assert!(!gone.connected);
        assert!(gone.login.is_none());

        let err = engine
            .resolve_share_credential(&ShareActor::Account("alice".to_string()))
            .expect_err("the deleted credential must not be served from cache");
        assert_eq!(err.to_string(), PERSONAL_TOKEN_MISSING);

        let (_api_url, token) = engine
            .resolve_share_credential(&ShareActor::Account("bob".to_string()))
            .expect("bob is untouched");
        assert_eq!(token.access_token, "bobs-token");

        // Idempotent: disconnecting again is a success, not a 404.
        assert!(
            !engine
                .disconnect_github_identity("alice")
                .await
                .unwrap()
                .connected
        );
    }

    /// Forgetting the MACHINE's credential forgets the machine's cache
    /// entries, every host's, and nobody else's: a personal credential is a
    /// different credential that this call did not delete, and evicting it
    /// would cost its owner a keychain read - a prompt, on a real machine - to
    /// recover something that never changed.
    #[tokio::test]
    async fn an_instance_disconnect_leaves_the_personal_credentials_cached() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, Some("personal"))
            .await
            .with_connect_auth(Arc::new(AcceptingAuth("alice-gh")));
        let tokens = tmp.path().join("tokens");
        write_token(&tokens, &TokenIdentity::Instance, "instance-gh");
        engine
            .connect_github_identity_token("alice", "alices-token")
            .await
            .unwrap();

        // Warm the instance slot for two hosts and alice's for one.
        engine.github_credential(None).unwrap();
        engine
            .github_credential_for(&TokenIdentity::Instance, Some("ghes.example"))
            .unwrap();
        engine
            .resolve_share_credential(&ShareActor::Account("alice".to_string()))
            .unwrap();
        assert_eq!(engine.github_tokens.lock().unwrap().len(), 3);

        engine.github_disconnect().await.unwrap();

        {
            let cache = engine.github_tokens.lock().unwrap();
            assert!(
                !cache.contains_key(&credential_cache_key(&TokenIdentity::Instance, None)),
                "the machine's own entry is gone"
            );
            assert!(
                !cache.contains_key(&credential_cache_key(
                    &TokenIdentity::Instance,
                    Some("ghes.example")
                )),
                "every host's, not only the one this call resolved"
            );
            assert!(
                cache.contains_key(&credential_cache_key(&personal("alice"), None)),
                "alice's credential was neither deleted nor invalidated"
            );
        }

        let (_api_url, token) = engine
            .resolve_share_credential(&ShareActor::Account("alice".to_string()))
            .expect("alice still shares as herself");
        assert_eq!(token.access_token, "alices-token");
    }

    /// A credential the CLI deleted out from under a running daemon stops
    /// being served the moment the daemon is told, rather than at its next
    /// restart: the eviction is what the control socket reaches, and it drops
    /// every host of the named identity and nobody else's.
    ///
    /// The delete itself happened in the other process, which is why this test
    /// removes the file by hand: this call is the eviction alone.
    #[tokio::test]
    async fn a_forgotten_credential_stops_being_served_from_the_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, Some("personal"))
            .await
            .with_connect_auth(Arc::new(AcceptingAuth("alice-gh")));
        let tokens = tmp.path().join("tokens");
        write_token(&tokens, &TokenIdentity::Instance, "instance-gh");
        write_token(&tokens, &personal("alice"), "alice-gh");
        write_token(&tokens, &personal("bob"), "bob-gh");

        // Warm alice on two hosts, plus bob and the machine's own.
        engine
            .github_credential_for(&personal("alice"), None)
            .unwrap();
        engine
            .github_credential_for(&personal("alice"), Some("ghes.example"))
            .unwrap();
        engine
            .github_credential_for(&personal("bob"), None)
            .unwrap();
        engine.github_credential(None).unwrap();
        assert_eq!(engine.github_tokens.lock().unwrap().len(), 4);

        // What `crystalline connect github --personal --disconnect` did over
        // in the CLI process.
        TokenStore::file_fallback_for(&personal("alice"), &tokens)
            .unwrap()
            .delete()
            .unwrap();
        engine.forget_cached_credential(Some("alice")).unwrap();

        let err = engine
            .resolve_share_credential(&ShareActor::Account("alice".to_string()))
            .expect_err("the deleted credential must not be served from cache");
        assert_eq!(err.to_string(), PERSONAL_TOKEN_MISSING);
        {
            let cache = engine.github_tokens.lock().unwrap();
            assert!(
                !cache.contains_key(&credential_cache_key(
                    &personal("alice"),
                    Some("ghes.example")
                )),
                "every host of that identity, not only the default one"
            );
            assert!(
                cache.contains_key(&credential_cache_key(&personal("bob"), None))
                    && cache.contains_key(&credential_cache_key(&TokenIdentity::Instance, None)),
                "and nobody else's: re-reading them would cost a keychain prompt for nothing"
            );
        }

        // A name that could address something other than its own credential is
        // refused here too, rather than quietly evicting nothing.
        assert!(engine.forget_cached_credential(Some("../x")).is_err());
        // The machine's own, addressed by absence.
        engine.forget_cached_credential(None).unwrap();
        assert!(
            !engine
                .github_tokens
                .lock()
                .unwrap()
                .contains_key(&credential_cache_key(&TokenIdentity::Instance, None))
        );
    }

    /// The gap between what the auth store allows as a name and what a
    /// credential can be addressed by is taught where it is discovered - at
    /// connect time, in words that name the fix - rather than left to surface
    /// as the token store's generic refusal on a first share.
    #[tokio::test]
    async fn a_name_that_cannot_address_a_credential_is_refused_at_connect_time() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, Some("personal"))
            .await
            .with_connect_auth(Arc::new(AcceptingAuth("alice-gh")));

        let err = engine
            .connect_github_identity_token("ann+lee", "some-token")
            .await
            .expect_err("'+' is outside the credential name class");
        assert_eq!(
            err.to_string(),
            "your account name 'ann+lee' cannot hold a GitHub identity - account names for sharing use lowercase letters, digits, dots, hyphens and underscores; ask an admin to recreate the account"
        );

        // Every verb on the surface says the same thing, so the card never
        // half-works for such an account.
        for err in [
            engine.github_identity_status("ann+lee").await.unwrap_err(),
            engine
                .start_github_identity_device_flow("ann+lee")
                .await
                .unwrap_err(),
            engine
                .disconnect_github_identity("ann+lee")
                .await
                .unwrap_err(),
        ] {
            assert!(err.to_string().contains("cannot hold a GitHub identity"));
        }

        // A rejected name is quoted through `escape_debug`, so a name carrying
        // a terminal escape cannot smuggle one into a log line or a console.
        let err = engine
            .connect_github_identity_token("ann\u{1b}[31m", "some-token")
            .await
            .expect_err("an escape is outside the class too");
        assert!(
            !err.to_string().contains('\u{1b}'),
            "the escape is rendered, not executed: {err}"
        );
        assert!(err.to_string().contains("\\u{1b}"), "{err}");
    }

    /// The default mode is unchanged behaviour: one instance credential does
    /// every write, whoever the actor is, and it reports the login the token
    /// was connected as.
    #[tokio::test]
    async fn instance_mode_shares_run_on_the_instance_token() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, None).await;
        let tokens = tmp.path().join("tokens");
        write_token(&tokens, &TokenIdentity::Instance, "instance-gh");

        let (_api_url, token) = engine
            .resolve_share_credential(&ShareActor::Owner)
            .expect("the instance credential resolves");
        assert_eq!(token.user_display(), Some("instance-gh"));

        // And an account actor reaches the same one: the mode, not the actor,
        // decides which credential a write runs on.
        let (_api_url, token) = engine
            .resolve_share_credential(&ShareActor::Account("alice".to_string()))
            .expect("the instance credential resolves for any actor");
        assert_eq!(token.user_display(), Some("instance-gh"));
    }

    /// Strictness, locked: no personal token means a teaching refusal, never a
    /// silent fall back to the instance credential.
    #[tokio::test]
    async fn personal_mode_without_a_token_refuses_with_the_teaching_text() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, Some("personal")).await;
        // The instance token is present and must not be reached for.
        write_token(&tmp.path().join("tokens"), &TokenIdentity::Instance, "inst");

        let err = engine
            .resolve_share_credential(&ShareActor::Account("alice".to_string()))
            .expect_err("no personal token for alice");
        assert_eq!(
            err.to_string(),
            "This instance shares with personal GitHub identities. Connect yours in Fluid (profile > GitHub identity) or run 'crystalline connect github --personal', then share again."
        );
    }

    /// Each actor writes as itself: the account's own credential, the machine
    /// owner's under the fixed `owner` name, and the instance token never read.
    #[tokio::test]
    async fn personal_mode_uses_the_actors_own_token() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, Some("personal")).await;
        let tokens = tmp.path().join("tokens");
        write_token(&tokens, &personal("alice"), "alice-gh");
        write_token(&tokens, &personal(OWNER_IDENTITY_NAME), "owner-gh");

        let (_api_url, token) = engine
            .resolve_share_credential(&ShareActor::Account("alice".to_string()))
            .expect("alice's own credential");
        assert_eq!(token.user_display(), Some("alice-gh"));

        let (_api_url, token) = engine
            .resolve_share_credential(&ShareActor::Owner)
            .expect("the machine owner's credential");
        assert_eq!(token.user_display(), Some("owner-gh"));

        // Two identities never share a cached client: one entry each, and the
        // instance slot was never touched.
        let cache = engine.github_tokens.lock().unwrap();
        assert_eq!(cache.len(), 2, "one cache entry per identity");
        assert!(
            !cache.contains_key(&credential_cache_key(&TokenIdentity::Instance, None)),
            "a personal write must not read the instance credential"
        );
    }

    /// An agent over HTTP MCP has no session to be, so an unconfigured instance
    /// is told which setting names one.
    #[tokio::test]
    async fn http_agent_without_agent_identity_refuses_naming_the_setting() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, Some("personal")).await;

        let err = engine
            .resolve_share_credential(&ShareActor::HttpAgent)
            .expect_err("no agent identity is configured");
        assert_eq!(
            err.to_string(),
            "This instance shares with personal GitHub identities and no agent identity is configured: set github.agent_identity to the account whose GitHub connection agent shares should use, or share from Fluid or the CLI."
        );
        assert!(err.to_string().contains("github.agent_identity"), "{err}");
    }

    /// With one configured, the agent writes as that account's connected
    /// identity.
    #[tokio::test]
    async fn http_agent_resolves_through_the_configured_account() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, Some("personal")).await;
        engine
            .configure(&ConfigureAction::Set {
                key: "github.agent_identity".to_string(),
                value: "bot".to_string(),
            })
            .await
            .unwrap();
        write_token(&tmp.path().join("tokens"), &personal("bot"), "bot-gh");

        let (_api_url, token) = engine
            .resolve_share_credential(&ShareActor::HttpAgent)
            .expect("the bot's credential");
        assert_eq!(token.user_display(), Some("bot-gh"));
    }

    /// Reads never move: pulls, polls and probes stay on the one instance
    /// credential in personal mode, so a person with no GitHub connection of
    /// their own still sees everything the instance can see.
    #[tokio::test]
    async fn reads_stay_on_the_instance_token_in_personal_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, Some("personal")).await;
        write_token(
            &tmp.path().join("tokens"),
            &TokenIdentity::Instance,
            "instance-gh",
        );

        let (store, token) = engine
            .github_credential(None)
            .expect("the read side reads the instance credential");
        assert_eq!(
            token.expect("a token").user_display(),
            Some("instance-gh"),
            "{}",
            store.kind()
        );
        // ... while a write in the same mode, with no personal token, refuses.
        assert!(
            engine.resolve_share_credential(&ShareActor::Owner).is_err(),
            "the write side must not borrow the instance credential"
        );
    }

    /// Neither setting is a start-time snapshot: a mode flipped through the
    /// settings path is honoured by the very next resolution, in both
    /// directions, with no restart.
    #[tokio::test]
    async fn a_live_mode_flip_is_honoured_by_the_next_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, None).await;
        let tokens = tmp.path().join("tokens");
        write_token(&tokens, &TokenIdentity::Instance, "instance-gh");
        write_token(&tokens, &personal(OWNER_IDENTITY_NAME), "owner-gh");

        let (_api_url, token) = engine.resolve_share_credential(&ShareActor::Owner).unwrap();
        assert_eq!(token.user_display(), Some("instance-gh"));

        engine
            .configure(&ConfigureAction::Set {
                key: "github.share_identity".to_string(),
                value: "personal".to_string(),
            })
            .await
            .unwrap();
        let (_api_url, token) = engine.resolve_share_credential(&ShareActor::Owner).unwrap();
        assert_eq!(
            token.user_display(),
            Some("owner-gh"),
            "the flip to personal is live"
        );

        engine
            .configure(&ConfigureAction::Unset {
                key: "github.share_identity".to_string(),
            })
            .await
            .unwrap();
        let (_api_url, token) = engine.resolve_share_credential(&ShareActor::Owner).unwrap();
        assert_eq!(
            token.user_display(),
            Some("instance-gh"),
            "and the flip back is live too"
        );
    }

    /// The login a share records as its author is the one the acting credential
    /// was connected as, in BOTH modes: instance mode names the instance's own
    /// login rather than nobody, which is what makes a mixed-mode team's
    /// proposals read consistently.
    ///
    /// Asserted on the credential half, and the test below pins that the
    /// provider half reads the login from exactly here. That split is not
    /// tidiness: building the provider builds a `reqwest` client, which loads
    /// the platform trust store - on macOS that reaches the OS keychain, the
    /// one thing no test in this tree may touch, and it is not hypothetical.
    /// This test used to call the provider half twice and was measured at over
    /// four minutes (killed by the runner's slow timeout) against
    /// milliseconds for every neighbour that resolves only the credential.
    ///
    /// A test-injected provider names whatever login it was given, which is
    /// why every mock-driven share in this tree records a null author.
    #[tokio::test]
    async fn a_share_acts_as_the_login_its_credential_was_connected_as() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, None).await;
        let tokens = tmp.path().join("tokens");
        write_token(&tokens, &TokenIdentity::Instance, "instance-gh");
        write_token(&tokens, &personal("alice"), "alice-gh");

        let (_api_url, token) = engine
            .resolve_share_credential(&ShareActor::Owner)
            .expect("the instance credential resolves");
        assert_eq!(
            token.user_display(),
            Some("instance-gh"),
            "instance mode records the login it shares as too"
        );

        engine
            .configure(&ConfigureAction::Set {
                key: "github.share_identity".to_string(),
                value: "personal".to_string(),
            })
            .await
            .unwrap();
        let (_api_url, token) = engine
            .resolve_share_credential(&ShareActor::Account("alice".to_string()))
            .expect("alice's own credential");
        assert_eq!(
            token.user_display(),
            Some("alice-gh"),
            "the actor's own login"
        );
    }

    /// And the provider half really does take its login from the credential
    /// the test above resolves, rather than from anywhere else.
    ///
    /// A source pin rather than a call, for the reason that test states: the
    /// only thing this function adds to `resolve_share_credential` is an HTTP
    /// client whose construction reads the machine's trust store. What is left
    /// worth checking is the wiring, and the wiring is readable.
    #[test]
    fn the_share_provider_takes_its_login_from_the_resolved_credential() {
        let source = include_str!("engine.rs");
        let start = source
            .find("fn resolve_share_provider")
            .expect("no resolve_share_provider");
        let rest = &source[start..];
        // Up to the closing brace at the function's own indentation, which no
        // nested block can reach. Move this function to another nesting level
        // and this slice stops being its body - so move the pattern with it.
        let body = &rest[..rest.find("\n    }").unwrap_or(rest.len())];
        assert!(
            body.contains("self.resolve_share_credential(actor)?"),
            "the provider resolves the same credential: {body}"
        );
        assert!(
            body.contains("token.user_display()"),
            "and reports the login that credential carries: {body}"
        );
    }

    /// The cache key separates every identity from every other one and from the
    /// instance, per host: a name can never be read as a host or as another
    /// identity, because the allowlist keeps the separator out of a name.
    #[test]
    fn cache_keys_never_collide_across_identities_and_hosts() {
        let keys = [
            credential_cache_key(&TokenIdentity::Instance, None),
            credential_cache_key(&TokenIdentity::Instance, Some("ghes.example")),
            credential_cache_key(&personal("alice"), None),
            credential_cache_key(&personal("alice"), Some("ghes.example")),
            credential_cache_key(&personal("alice.ghes"), None),
            credential_cache_key(&personal("bob"), None),
        ];
        let unique: HashSet<&String> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len(), "{keys:?}");
    }

    /// A personal token that cannot write the repository is the one failure
    /// this mode adds, and the message says what to ask for rather than
    /// reporting a bare 403.
    #[test]
    fn a_personal_403_teaches_the_collaborator_requirement() {
        let err = enrich_write_error(
            RemoteError::Api {
                status: 403,
                message: "Resource not accessible by personal access token".to_string(),
            },
            Some("alice"),
            "team/knowledge",
        );
        assert_eq!(
            err.to_string(),
            "your GitHub account @alice needs write access to team/knowledge - ask a maintainer to add you as a collaborator."
        );
    }

    /// An expired personal token names the reconnect flow: the instance-level
    /// "use configure to sign in again" is the wrong instruction for a person
    /// whose own connection lapsed.
    #[test]
    fn an_expired_personal_token_names_the_reconnect_flow() {
        let err = enrich_write_error(RemoteError::AuthExpired, Some("alice"), "team/knowledge");
        assert!(
            err.to_string().contains("reconnect your GitHub identity"),
            "{err}"
        );
        assert!(
            err.to_string()
                .contains("crystalline connect github --personal"),
            "{err}"
        );
    }

    /// `origin status` names the mode a share would run in, and in personal
    /// mode whether the machine owner has connected an identity at all - the
    /// two facts the CLI renders its connection line from, since a status that
    /// only reported the instance token would tell a caller in personal mode
    /// nothing about whether their next share can go out.
    #[tokio::test]
    async fn origin_status_names_the_share_identity_and_the_owners_connection() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, None).await;
        let tokens = tmp.path().join("tokens");
        write_token(&tokens, &TokenIdentity::Instance, "instance-gh");

        let status = engine.origin_status(None).await.unwrap();
        assert_eq!(status["connection"]["share_identity"], "instance");
        assert!(
            status["connection"].get("owner_identity").is_none(),
            "instance mode has no personal slot to report: {status}"
        );

        engine
            .configure(&ConfigureAction::Set {
                key: "github.share_identity".to_string(),
                value: "personal".to_string(),
            })
            .await
            .unwrap();
        let status = engine.origin_status(None).await.unwrap();
        assert_eq!(status["connection"]["share_identity"], "personal");
        assert_eq!(
            status["connection"]["owner_identity"]["account"],
            OWNER_IDENTITY_NAME
        );
        assert_eq!(status["connection"]["owner_identity"]["connected"], false);
        assert!(
            status["connection"]["connected"].as_bool().unwrap(),
            "the instance credential is still what reads: {status}"
        );

        write_token(&tokens, &personal(OWNER_IDENTITY_NAME), "owner-gh");
        let status = engine.origin_status(None).await.unwrap();
        assert_eq!(status["connection"]["owner_identity"]["connected"], true);
        assert_eq!(status["connection"]["owner_identity"]["user"], "owner-gh");
    }

    /// The agent slot rides beside the owner's, on the same terms: personal
    /// mode only, and only where `github.agent_identity` names an account -
    /// which is what lets an operator running an HTTP agent see whether the
    /// bot's own shares can go out, instead of reading the owner's slot and
    /// drawing the wrong conclusion from it.
    #[tokio::test]
    async fn origin_status_names_the_agent_identity_where_one_is_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, None).await;
        let tokens = tmp.path().join("tokens");
        write_token(&tokens, &TokenIdentity::Instance, "instance-gh");
        engine
            .configure(&ConfigureAction::Set {
                key: "github.agent_identity".to_string(),
                value: "share-bot".to_string(),
            })
            .await
            .unwrap();

        // Instance mode has no personal slot in play at all, agent or owner.
        let status = engine.origin_status(None).await.unwrap();
        assert!(
            status["connection"].get("agent_identity").is_none(),
            "instance mode reports no personal slot: {status}"
        );

        engine
            .configure(&ConfigureAction::Set {
                key: "github.share_identity".to_string(),
                value: "personal".to_string(),
            })
            .await
            .unwrap();
        let status = engine.origin_status(None).await.unwrap();
        let agent = &status["connection"]["agent_identity"];
        assert_eq!(agent["account"], "share-bot");
        assert_eq!(agent["connected"], false, "nothing is on file for it yet");
        assert!(
            status["connection"]["owner_identity"]["account"].is_string(),
            "the owner's slot is untouched beside it: {status}"
        );

        write_token(&tokens, &personal("share-bot"), "bot-gh");
        let status = engine.origin_status(None).await.unwrap();
        assert_eq!(status["connection"]["agent_identity"]["connected"], true);
        assert_eq!(status["connection"]["agent_identity"]["user"], "bot-gh");
        assert_eq!(
            status["connection"]["owner_identity"]["connected"], false,
            "the bot's credential is not the owner's: {status}"
        );
    }

    /// No agent slot where the setting names nobody: an absent
    /// `github.agent_identity` is a deployment with no HTTP agent sharing on
    /// it, not a connection somebody forgot to make.
    #[tokio::test]
    async fn origin_status_reports_no_agent_slot_when_none_is_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = credential_engine(&tmp, None).await;
        write_token(
            &tmp.path().join("tokens"),
            &TokenIdentity::Instance,
            "instance-gh",
        );
        engine
            .configure(&ConfigureAction::Set {
                key: "github.share_identity".to_string(),
                value: "personal".to_string(),
            })
            .await
            .unwrap();

        let status = engine.origin_status(None).await.unwrap();
        assert!(
            status["connection"].get("agent_identity").is_none(),
            "{status}"
        );
    }

    /// Instance-token failures keep today's texts (spec section 8), and any
    /// other failure passes through whoever was acting.
    #[test]
    fn instance_failures_and_other_errors_pass_through_untouched() {
        let untouched = enrich_write_error(
            RemoteError::Api {
                status: 403,
                message: "nope".to_string(),
            },
            None,
            "team/knowledge",
        );
        assert!(untouched.to_string().contains("403"), "{untouched}");

        let offline = enrich_write_error(RemoteError::Offline, Some("alice"), "team/knowledge");
        assert_eq!(offline.to_string(), RemoteError::Offline.to_string());
    }
}
