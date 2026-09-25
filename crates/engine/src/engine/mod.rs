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
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use crystalline_core::config::registration::{
    Registration, RegistrationRequest, decide_registration, validate_domain_name,
};
use crystalline_core::config::{
    DomainEntry, DomainKind as CoreDomainKind, GlobalConfig, OriginConfig, ResponseFormat,
    ShareIdentityMode, VerifyConfig,
};
use crystalline_core::emit::{
    append_body, insert_after_section_reporting, insert_before_section, prepend_body,
    remove_frontmatter_field, replace_section_reporting, set_evolve_ack, set_frontmatter_field,
    set_frontmatter_number, set_stale_after, set_verified, touch_generated,
};
use crystalline_core::relink::Relink;
use crystalline_core::schema::{self, Schema};
use crystalline_core::{
    CrystallineUrl, EVOLVE_ACK_KEY, Engram, EvolveAck, Frontmatter, HarnessKind, LinkTarget,
    Manifest, YamlValue, is_lower_hyphen, parse_engram, parse_engram_lossless, slugify,
};
use crystalline_index::{
    AckCounts, AckEntry, AttachmentRow, ChunkParams, DEFAULT_RETIRED_WEIGHT,
    DEFAULT_SALIENCE_WEIGHT, DomainHost, DomainId, DomainKind, DomainStats, EMBED_PAGE_SIZE,
    EdgeKind, EmbeddingProvider, EngramDescriptor, EngramFacts, EngramId, EngramRecord,
    EngramSummary, FactObservation, Family, FileStamp, Finding, GraphNode, GraphSlice, HostClaim,
    InboundQuery, IndexError, RULES, RebuildKind, RecentFilter, ReindexHooks, SearchMode,
    SearchOrder, SearchQuery, ShareFacts, Store, StoredEngram, SweepInput, SweepOptions,
    SweepReport, SyncReport, apply_scan, chunk_engram, configured_model_id, detect,
    is_retired_status, order_jobs_for_batching, parse_metadata_filters, provider_from_config, rank,
    reindex_domains, resolve_forward_refs, retired_factor, rule_info, salience_prior, scan_domain,
    scan_paths,
};
use crystalline_remote::changes::{LocalChange, LocalChanges};
use crystalline_remote::ops::{self, DiscardTarget};
use crystalline_remote::{
    GitHubProvider, OriginSpec, Provider, RemoteError, StoredToken, TokenIdentity, TokenStore,
};
use indexmap::IndexMap;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::collab::session::AgentPeer;
use crate::domain_view::DomainView;
use crate::origin;
use crate::overlay::{self, EnvOverlay, LoadedConfig};
use crate::params::*;
use crate::poller;
use crate::review::{self, ActorDrafts, FoldChoice, ReviewModeConfirm};
use crate::settings;
use crate::share_staging::{self, PreparedShare};
use crate::similar::{
    self, SIMILAR_BACKLOG_POLL, SIMILAR_BACKLOG_WAIT, SIMILAR_LIMIT, SIMILAR_PAGE, SIMILAR_TIMEOUT,
    SimilarEngram, SimilarProbe,
};

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
     Act only on the evidence stated: this sweep detects by dates, links, graph shape and embedding similarity, and similarity is not a contradiction - it cannot confirm that two engrams disagree. \
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

/// The ceiling [`sanitize_actor`] keeps an actor token to, in kept characters.
/// Exposed with it, because a caller composing an actor out of two halves has
/// to budget against the same number to know what will survive the pass.
#[doc(hidden)]
pub const ACTOR_MAX_CHARS: usize = 120;

/// Normalize a client-supplied identity into an OKF actor token: whitespace
/// runs collapse to a single hyphen, control characters and the flow-mapping
/// punctuation that would need quoting are dropped and the result is capped, so
/// a client that calls itself "Some Client (beta)" still yields a clean
/// `generated.by`.
///
/// Doc-hidden and `pub` rather than `pub(crate)`, because an actor composed
/// out of two halves has to sanitize each half on its own rather than the
/// composition (the service crate's `mcp::acting_actor`): a single pass over
/// the joined string lets the client-supplied half spend the whole budget and
/// truncate away the half the server asserts.
#[doc(hidden)]
pub fn sanitize_actor(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut kept = 0usize;
    let mut pending_gap = false;
    for c in raw.trim().chars() {
        if kept >= ACTOR_MAX_CHARS {
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

/// The model a write records beside the actor it is recording, or `None` when
/// none was reported and whenever the actor is a person.
///
/// **A `human:` actor never carries a model.** The prefix is what OKF's trust
/// tier and the evolve sweep read as "a person wrote this", so a model reported
/// beside it would say a model produced words a person typed. Every other actor
/// takes the model as reported, a client-composed one and a configured agent
/// identity like `team-bot/1.0` alike.
///
/// **The drop lives here rather than at the surface that takes the parameter**,
/// because this is the first place the actor is actually known: `identity.actor`
/// can pin a `human:` form that no caller can see ([`Engine::actor`]). The
/// value is sanitized with [`sanitize_actor`] on the way through, so a reported
/// id can no more break the flow mapping it is written into than an actor can,
/// and an id that sanitizes away is absence.
///
/// The prefix is matched the way the sweep matches it: case-insensitively, over
/// a byte slice taken with `get` so an actor whose sixth byte lands inside a
/// multi-byte character simply does not match.
pub(crate) fn stamped_model(actor: &str, model: Option<&str>) -> Option<String> {
    if actor
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("human:"))
    {
        return None;
    }
    model.map(sanitize_actor).filter(|m| !m.is_empty())
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
    /// The caller is known, may see the thing they addressed, and is not
    /// allowed to do this to it. Distinct from [`EngineError::UnknownDomain`],
    /// which is what a caller who may not see it gets: this variant is only
    /// ever raised about something the caller can already see, so it discloses
    /// nothing by existing. The message names who *can*, because "forbidden"
    /// on a domain somebody reads every day is otherwise indistinguishable
    /// from a bug.
    #[error("{0}")]
    Forbidden(String),
    /// The request would destroy knowledge that only exists here, and it did
    /// not say so. Not a permission problem and not a malformed request: the
    /// caller may do this and asked for it correctly, and the server is
    /// refusing to guess that the loss was intended. The message names the flag
    /// that says it was, on every surface that has one.
    #[error("{0}")]
    ConfirmationRequired(String),
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
    /// The caller asked for something this domain's own shape cannot honor as
    /// asked, and the message names the way in. Not a permission problem: the
    /// caller may well be allowed, and on a domain that takes changes directly
    /// the very same call would land. Distinct from [`EngineError::Invalid`]
    /// too, which is a malformed request - this one is well formed and simply
    /// arrived without something the domain requires, so the message is
    /// teaching text rather than a complaint.
    #[error("{0}")]
    Refused(String),
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

/// The gate a whole-document write goes through before anything is written: a
/// document that is not an engram would poison the index on reindex.
///
/// This is the one hard gate, and it is deliberately narrow: the text must
/// parse (clean UTF-8, frontmatter that is a YAML mapping) and must carry
/// frontmatter that actually says something, because a save that drops it
/// silently strips the engram's type, title, permalink, tags and status at
/// once, leaving the index to fall back to the path slug. An empty block is
/// that same strip wearing delimiters, so it is refused the same way.
/// Everything a document can get wrong while still being an engram - a missing
/// tag, a permalink that is not a slug, an inverted validity window - is the
/// validation endpoint's business to report, not this path's to refuse: an
/// engram that already carries such a flaw must stay editable, since fixing it
/// here is what the editor is for.
///
/// One function for the save and the restore, which have always refused the
/// same shapes in the same words; a co-editing room's saver reaches it through
/// the save it calls.
fn refuse_not_an_engram(content: &str) -> Result<()> {
    let parsed = parse_engram_lossless(content).map_err(|e| EngineError::Invalid(e.to_string()))?;
    if !parsed.has_frontmatter || parsed.raw_frontmatter.trim().is_empty() {
        return Err(EngineError::Invalid(
            "the document carries no frontmatter, so it is not an engram; \
             keep the --- delimited frontmatter block, and the type, title, \
             permalink and tags in it, at the top of the file"
                .into(),
        ));
    }
    Ok(())
}

/// The word one detected change is reported by, everywhere this feature
/// names a kind.
fn change_kind(change: &LocalChange) -> &'static str {
    match change {
        LocalChange::Added { .. } => "added",
        LocalChange::Modified { .. } => "modified",
        LocalChange::Deleted { .. } => "deleted",
    }
}

/// The targets a team discard actually runs, with every digest the caller left
/// out filled in from the change the list shows now.
///
/// `discard_local_files` guards each target against the digest the caller
/// looked at, so an addition or a modification with no digest would refuse as
/// `changed_since` and never be discardable at all. A caller that names no
/// digest is not skipping the guard by accident: it is saying "discard what
/// the list shows", which is what an agent calling without `expected` means.
/// So the digest is read off the detected change itself. A deletion keeps
/// `None`, which is the absence its own guard checks for, and a path that
/// resolves to no change is passed through untouched so the loop refuses it in
/// its own words.
fn fill_unguarded(targets: &[DiscardTarget], local: &LocalChanges) -> Vec<DiscardTarget> {
    targets
        .iter()
        .map(|target| {
            if target.sha256.is_some() {
                return target.clone();
            }
            let filled = match ops::resolve_local_change(local, &target.path) {
                Some(LocalChange::Added { sha256, .. })
                | Some(LocalChange::Modified { sha256, .. }) => Some(sha256.clone()),
                _ => None,
            };
            DiscardTarget {
                path: target.path.clone(),
                sha256: filled,
            }
        })
        .collect()
}

/// What one offline read of a team domain's unshared delta needs: the domain
/// root, its origin state directory, the base every comparison is made
/// against and the delta detection found. What
/// [`Engine::team_local_changes`] hands its three callers.
type TeamChanges = (
    PathBuf,
    PathBuf,
    BTreeMap<String, crystalline_remote::state::BaseStamp>,
    LocalChanges,
);

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
pub(crate) enum ContentSource {
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
    // One embedding pass at a time, whoever asks: the worker, a verb that just
    // wrote, the daemon's startup task or the self-heal tick. See [`EmbedGate`].
    embed_gate: Arc<std::sync::Mutex<EmbedGate>>,
    // What the last successful embedding-model load pruned from the model
    // cache, so `ctl status` after a start says what that start freed. Empty on
    // every install that had nothing to prune, which is every install that
    // never changed model.
    model_cache_pruned: std::sync::RwLock<Vec<(String, u64)>>,
    // Swappable so the daemon can build the (possibly downloading) provider in the
    // background without blocking readiness or text search.
    provider: std::sync::RwLock<Option<Arc<dyn EmbeddingProvider>>>,
    model_id: String,
    chunk_params: ChunkParams,
    // When true the content-mutating methods refuse early with
    // `EngineError::ReadOnly`. Set at construction from the effective mode
    // (explicit flag or `service.read_only`). Index maintenance is unaffected.
    read_only: bool,
    // The first of this file's four test seams: when armed, the next source edit
    // fails on its far side, once. See `Engine::fail_next_source_edit`.
    // Compiled only into a test build (`cfg(test)` for this crate's unit tests,
    // the `testing` feature for its integration tests), so a released binary
    // carries neither the flag nor the branches that read it.
    // The third test seam: how many prune statements the embed pass has sent to
    // the store. The prune's whole point is the statements it does NOT send, and
    // a skipped scan is invisible from the outside. See
    // `Engine::prune_statements_issued`.
    #[cfg(any(test, feature = "testing"))]
    prune_statements: std::sync::atomic::AtomicU64,
    // The fourth: how many detection walks a team domain's change list has paid
    // for. A walk reads and hashes every file in the domain, and answering a
    // diff from one walk rather than one per changed file is invisible in the
    // JSON, which is byte for byte the same either way. See
    // `Engine::detection_walks`.
    #[cfg(any(test, feature = "testing"))]
    detection_walks: std::sync::atomic::AtomicU64,
    #[cfg(any(test, feature = "testing"))]
    fail_next_source_edit: std::sync::atomic::AtomicBool,
    // The second, and it is a stopwatch rather than a failure: when armed, the
    // next edit of a draft sleeps this many milliseconds between reading the
    // draft and writing it back, holding whatever it holds. See
    // `Engine::hold_next_draft_edit`, and the same compile-out applies.
    #[cfg(any(test, feature = "testing"))]
    hold_next_draft_edit: std::sync::atomic::AtomicU64,
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
    // Overrides the state directory the overlay journal is read and written
    // under, for tests: `None` means the real
    // `crystalline_core::config::state_dir`, a real machine path no test may
    // touch - and the journal sweeps are recursive deletes under it, so a test
    // engine that reaches a removal path must set this.
    state_dir_override: Option<PathBuf>,
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
    // Serializes a domain registration against a domain removal, for the
    // whole of each: `Engine::unregister_domain` holds it across its sweep and
    // its tail, and the REST create holds it across its own registration
    // (`RestState::domain_admin` delegates here). It lives on the engine
    // rather than on one surface's state because MCP and REST reach the same
    // verbs and a lock held by only one of them serializes only that one.
    // What it does NOT close is the bare `domain_add_*`/`origin_add` verbs,
    // which take no lock of their own; see `Engine::domain_remove`'s known
    // race.
    domain_admin: tokio::sync::Mutex<()>,
    // The fence a removal raises against new co-editing joins while it sweeps
    // the domain's rooms. Write-held by `Engine::unregister_domain`, read-held
    // by each collab join (`RestState::join_pass` delegates here), so a join
    // and a removal of the same domain cannot interleave.
    join_fence: tokio::sync::RwLock<()>,
    // The open co-editing rooms, so a removal can save and close the rooms of
    // the domain it is about to unregister. A `Weak`, because
    // `CollabSessions` holds an `Arc<Engine>` and a strong handle here would
    // be a cycle neither side ever drops; `None` (nothing installed) is every
    // engine that serves no web surface, which has no rooms to close.
    collab: std::sync::OnceLock<std::sync::Weak<crate::collab::session::CollabSessions>>,
    // The private-domain resolver every scoped read is filtered through,
    // installed once when the HTTP surface starts (`daemon::http_base`, the
    // one funnel both router builders reach, right after the `AuthStore` it
    // wraps exists).
    //
    // A `OnceLock` rather than a field on the constructor because the engine
    // is built long before - and often without - an accounts database: a
    // one-shot CLI command, the embedded stdio MCP stack and every test engine
    // never install one, and `Engine::hidden_domains` answers `None` for them,
    // which is the same answer the `Scope::Unrestricted` those surfaces pass
    // would have produced anyway. Set once and never replaced, so a second
    // router built over one engine keeps the first store rather than silently
    // swapping the authority mid-flight.
    domain_access: std::sync::OnceLock<Arc<crate::scope::DomainAccess>>,
    // The origin rule the HTTP surface was built with, installed once by
    // `http_base`. It answers what this instance is called, for a local caller
    // and an HTTP one alike; absent in every process that serves no HTTP
    // surface, which is the same set that never had one to disagree with.
    web_origin: std::sync::OnceLock<Arc<dyn crate::web_url::WebOrigin>>,
    // The sessions currently working inside somebody else's draft. Always
    // present rather than a `OnceLock` like the resolver above: it is a plain
    // in-memory registry with no store behind it, so an engine that nobody
    // ever joins anything through simply holds an empty one. See
    // [`crate::join`] for why a join belongs to a session and not to an
    // account.
    joins: Arc<crate::join::Joins>,
}

/// One drafted engram, as the share-link surface hands it to the account a
/// link was redeemed by.
///
/// Four fields and no more, because this is the whole of what crosses between
/// two actors' overlays: where the draft stands, what it answers to, what it
/// says, and the version token a save of it has to present. No neighbours, no
/// backlinks, no advisory - a granted draft is one page handed over by its
/// author, not a corner of the index opened up.
#[derive(Clone, Debug)]
pub struct GrantedDraft {
    /// The domain-relative path the draft stands at.
    pub path: String,
    /// The address it answers to, which is how a save of it is addressed.
    pub permalink: String,
    /// The markdown as its author last left it, frontmatter and all.
    pub content: String,
    /// The lowercase hex SHA-256 of `content`: the `expected_checksum` a save
    /// of this draft presents, and the `ETag` a reader of it holds.
    pub checksum: String,
}

/// One granted draft as a read payload: the shape [`Engine::read_engram`]
/// answers with, filled in for a page whose rows belong to somebody else.
///
/// Written out here rather than shared with the read path it mirrors, because
/// half of what that path does cannot be done for a granted draft and the
/// other half must not be. The inbound summary is absent (nothing points at a
/// draft), the outbound edges are all unresolved (a grant widens one path, not
/// a neighbourhood), and two keys are added that no other read carries:
/// `draft` and `draft_owner`, which are what keep a draft standing where the
/// team's page stands from being mistaken for it.
fn granted_draft_json(domain: &str, owner: &str, draft: &GrantedDraft) -> Result<Value> {
    let engram = parse_engram(&draft.content).map_err(|e| EngineError::Invalid(e.to_string()))?;
    let relations: Vec<Value> = engram
        .relations
        .iter()
        .map(|r| {
            json!({
                "line": r.line,
                "rel_type": r.rel_type,
                "target": r.target,
                "resolved": false,
            })
        })
        .collect();
    let links: Vec<Value> = engram
        .links
        .iter()
        .map(|l| json!({ "line": l.line, "target": l.target, "resolved": false }))
        .collect();
    let title = if engram.frontmatter.title.is_empty() {
        draft.permalink.clone()
    } else {
        engram.frontmatter.title.clone()
    };
    Ok(json!({
        "domain": domain,
        "permalink": draft.permalink,
        "title": title,
        "type": engram.frontmatter.engram_type,
        "status": engram.frontmatter.status.clone().unwrap_or_default(),
        "path": draft.path,
        "url": format!("crystalline://{domain}/{}", draft.permalink),
        "content": draft.content,
        "checksum": draft.checksum,
        "frontmatter": engram.frontmatter,
        "observations": engram.observations,
        "relations": relations,
        "links": links,
        "draft": true,
        "draft_owner": owner,
    }))
}

/// What a scoped read hands the store as its domain filter, once the caller's
/// own filter and the domains it may not see have been reconciled.
///
/// Three answers rather than an `Option<Vec<String>>`, because the empty vector
/// is ambiguous where it matters most: the store reads "no filter" as "every
/// domain", so a caller whose entire filter was hidden would be answered with a
/// sweep of everything. [`ScopedDomains::Nothing`] is that case, named.
#[derive(Debug, PartialEq)]
enum ScopedDomains {
    /// Nothing is hidden from this caller, so the query keeps the filter it was
    /// given - empty (every domain) or not. The machine owner's answer, and the
    /// answer on any installation where nobody made a domain private.
    AsAsked,
    /// Query exactly these domains.
    Only(Vec<String>),
    /// Nothing in range is readable by this caller. The answer is an empty
    /// result, which is what a domain nobody registered already produces.
    Nothing,
}

/// Mark one graph node as the reader's own draft, and say nothing at all about
/// a base row.
///
/// Emitted only when there is something to say, so a domain that takes changes
/// directly answers the JSON it always answered, key for key - which is what
/// the byte-identity test pins. A reader only ever meets their own drafts in a
/// slice, so the flag needs no owner beside it.
fn mark_draft(node_json: &mut Value, node: &GraphNode) {
    if node.actor.is_empty() {
        return;
    }
    if let Some(obj) = node_json.as_object_mut() {
        obj.insert("draft".to_string(), json!(true));
    }
}

/// Cut every node in a hidden domain out of a graph slice, and with it every
/// edge that had an end there.
///
/// A dangling edge is as much of a disclosure as the node it points at - it
/// says an engram exists, in a domain the caller was told nothing about - so
/// the two go together, and this runs before anything ranks or caps the slice.
/// A no-op when nothing is hidden, which is every unscoped read.
fn retain_visible(slice: &mut GraphSlice, hidden: &HashSet<String>) {
    if hidden.is_empty() {
        return;
    }
    slice.nodes.retain(|node| !hidden.contains(&node.domain));
    let kept: HashSet<i64> = slice.nodes.iter().map(|node| node.id.0).collect();
    slice
        .edges
        .retain(|edge| kept.contains(&edge.from.0) && kept.contains(&edge.to.0));
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
    /// An authenticated account: Fluid, the REST API and an HTTP MCP session
    /// that authenticated at the door (`auth.mcp`), which acts as the account
    /// whose token opened it.
    Account(String),
    /// An agent over HTTP MCP on an instance that does not make agents
    /// authenticate, so the transport carries no user auth and there is nobody
    /// to be: resolved through the `github.agent_identity` setting.
    HttpAgent,
}

/// What a removal knows about how much knowledge is at stake.
///
/// The two absent cases are not the same fact, and keeping them apart is the
/// whole point of the type: a domain the index holds no row for has synced
/// nothing, while a count that could not be read is a number that exists and is
/// unavailable. Collapsing them into one `None` is how a purge gate fails open,
/// and it is what leaves a confirmation question quietly missing the figure it
/// promises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemovalCount {
    /// The index answered: this many engrams.
    Known(i64),
    /// The index answered and holds no row for this domain.
    Absent,
    /// The index could not be read. For a virtual domain this only reaches a
    /// caller that already set `purge`; without it the unknown count is a
    /// refusal.
    Unreadable,
}

impl RemovalCount {
    /// The count as a preview reports it: the number, or null for either
    /// absent case. `engrams_unknown` beside it is what tells them apart.
    fn as_json(self) -> Value {
        match self {
            RemovalCount::Known(n) => Value::from(n),
            RemovalCount::Absent | RemovalCount::Unreadable => Value::Null,
        }
    }

    /// Whether the count is absent because the index could not be read.
    fn is_unreadable(self) -> bool {
        matches!(self, RemovalCount::Unreadable)
    }
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

pub use crate::scope::OWNER_IDENTITY_NAME;

/// What a write is told when it reaches a domain that reviews changes before
/// they land and nobody can say whose draft it would join.
///
/// A draft belongs to an actor. With no identity there is no actor, so there
/// is nothing to write into - and the one thing that must never happen instead
/// is the write falling through onto the folder the team reviewed. So the verb
/// refuses, and the refusal teaches the way in rather than stating a rule: an
/// agent that reads it can act on it in one step.
pub const OVERLAY_NEEDS_IDENTITY: &str = "this domain reviews changes before they land, so a write needs to know whose draft it joins - connect with your MCP token (issued in Fluid under profile > Agent access) and try again";

/// What somebody who can SEE another person's draft is told when they try to
/// write into it without having joined it.
///
/// A share-link and a join are two different things on purpose: being handed a
/// draft to read is not agreeing to type into somebody else's work, and an
/// agent holding its account's links has not been told to edit anything. So
/// this refusal is not a wall, it is a fork, and it names both ways through -
/// join the draft and the writing lands in its author's overlay where they
/// will see it, or write your own and it lands in yours, where it always did.
/// What somebody working inside a shared draft is told when they reach for a
/// file that draft does not carry.
///
/// A fork rather than a wall, like [`granted_needs_join`] beside it, and it
/// names the file: the two ways forward are asking the person whose work it is,
/// which is the only way somebody else's staged file should ever change, and
/// drafting one's own copy, which needs nobody's permission.
pub fn joined_files_are_the_drafts(owner: &str, draft: &str, path: &str) -> String {
    format!(
        "this session is working inside {owner}'s draft of '{draft}', and a join carries that \
         page and the files it points at - '{path}' is neither. It is somebody else's work: ask \
         its author to change it, or draft your own copy in your own overlay."
    )
}

/// What a write inside somebody's draft is told when it would land somewhere
/// else.
///
/// One sentence for the two surfaces that can be inside a draft - a request
/// carrying a join, and a co-editing room over an overlay document - because
/// it is one rule: you are inside ONE page, and a write that resolved to
/// another page is not that page's. A room reaches it when the document's own
/// frontmatter starts claiming to be a different engram, which is the way an
/// address moves without anybody saying so.
pub fn joined_write_is_elsewhere(owner: &str, draft: &str, path: &str) -> String {
    format!(
        "this session is working inside {owner}'s draft of '{draft}', so a write to '{path}' has \
         nowhere to land: leave that draft first, and the write goes back to being your own"
    )
}

pub fn granted_needs_join(owner: &str, path: &str) -> String {
    format!(
        "'{path}' is {owner}'s draft, shared with you to read: writing into it is a second step. \
         Join the draft and your changes land in {owner}'s copy, where {owner} reviews them; or \
         draft your own copy in your own overlay and leave theirs as it stands."
    )
}

/// What somebody inside a join is told when the draft they joined is no longer
/// there - its author renamed it, folded it, or took it back.
///
/// Without this the refusal for a write at any other path names the draft they
/// joined, which is a sentence about a page that has moved and no way to tell
/// that from a mistyped address.
pub fn joined_draft_is_gone(owner: &str, draft: &str) -> String {
    format!(
        "the draft you joined - {owner}'s '{draft}' - is not there any more: it was renamed, \
         folded into the domain, or taken back. Leave it, and ask {owner} for a fresh link if \
         there is still work to do together."
    )
}

/// What a share hears when its OWN proposal is already open in a domain that
/// reviews changes.
///
/// The next share would stack a layer on that one, which review mode cannot
/// serve (`Engine::refuse_open_proposal_while_reviewing` gives the two
/// reasons), so the way through is to withdraw what is open and share again -
/// which this actor may do, because the proposal is theirs.
pub const REVIEW_NO_STACKING: &str = "this domain reviews changes; stacking on an open layer is not supported while it does - withdraw it and share again";

/// What a share hears when SOMEBODY ELSE's proposal is open in a domain that
/// reviews changes: a `{actor}` slot and the wait.
///
/// A separate string from [`REVIEW_NO_STACKING`] because the way forward is
/// different in kind. This actor cannot withdraw a proposal that is not theirs,
/// so telling them to would be telling them to do something they may not do -
/// and telling them to "share a fresh proposal instead" would be telling them
/// to do the very thing being refused.
const REVIEW_PROPOSAL_IS_ANOTHERS: &str = "a proposal by {actor} is open, and this domain takes one proposal at a time while it reviews changes - wait for theirs to merge, or ask them to withdraw it, then share again";

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
///
/// The collaborator rewrite is deliberately keyed to `Api { status: 403 }`
/// alone. An organization-policy refusal ([`RemoteError::SsoAuthorizationRequired`],
/// [`RemoteError::OauthAppRestricted`]) is also a 403 upstream, but the
/// provider has already turned it into the accurate instruction, and adding
/// a collaborator would not clear either of them.
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

/// The single-flight state of the engine's embedding pass.
///
/// `running` says a pass is walking the backlog; `again` says a request
/// arrived while it was. Two passes over one backlog do not share it - each
/// keeps its own cursor, so the second re-embeds whatever the first has in
/// flight, splitting the CPU and doubling the in-flight pages for no extra
/// coverage. One pass at a time is therefore an engine invariant rather than a
/// call-site convention, and it holds for callers with no worker wired too.
///
/// Both flips happen under the one mutex, which is what keeps the invariant
/// from costing work: a caller either finds the pass running and hands it
/// `again`, or finds it finished and claims the next pass itself. There is no
/// instant where a request is neither served by the running pass nor able to
/// start its own.
#[derive(Default)]
pub(crate) struct EmbedGate {
    running: bool,
    again: bool,
}

/// What a request for an embedding pass did.
///
/// The two are worth telling apart wherever a caller reports the result to a
/// person: a turned-away request is work in progress, not an empty backlog, and
/// rendering it as "0 chunks embedded" is the silently-successful answer this
/// whole area has been fixing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedOutcome {
    /// This call walked the backlog and embedded that many chunks, then, when
    /// the walk left the active model covering every chunk, cleared that many
    /// chunks of another model's vectors.
    Embedded { chunks: usize, pruned: usize },
    /// A pass was already walking the backlog, so this request was folded into
    /// it: that pass walks the backlog again and covers whatever this caller
    /// had just written. Nothing was dropped and nothing needs re-asking.
    AlreadyRunning,
}

impl EmbedOutcome {
    /// The count for a caller that only wants a number, a turned-away request
    /// reading as zero.
    pub fn embedded(self) -> usize {
        match self {
            EmbedOutcome::Embedded { chunks, .. } => chunks,
            EmbedOutcome::AlreadyRunning => 0,
        }
    }
}

/// Holds the claim on the embedding pass, releasing it on drop so a store
/// error, a panic or a dropped future cannot strand it.
pub(crate) struct EmbedPass {
    gate: Arc<std::sync::Mutex<EmbedGate>>,
    released: bool,
}

impl EmbedPass {
    /// Claim the pass, or `None` when one is already running - in which case
    /// the running pass is told to walk the backlog once more, so the caller's
    /// work is served by that walk instead of being dropped.
    fn claim(gate: &Arc<std::sync::Mutex<EmbedGate>>) -> Option<EmbedPass> {
        let mut state = gate.lock().unwrap();
        if state.running {
            state.again = true;
            return None;
        }
        state.running = true;
        // A walk starts at the head of the backlog, so it already covers
        // whatever an earlier request was asking for.
        state.again = false;
        drop(state);
        Some(EmbedPass {
            gate: Arc::clone(gate),
            released: false,
        })
    }

    /// Called once per walk: `true` to walk again because a request arrived
    /// during the one just finished, `false` to end the pass - which releases
    /// the claim there and then, in the same critical section a fresh caller
    /// checks.
    fn walk_again(&mut self) -> bool {
        let mut state = self.gate.lock().unwrap();
        if state.again {
            state.again = false;
            true
        } else {
            state.running = false;
            self.released = true;
            false
        }
    }
}

impl Drop for EmbedPass {
    fn drop(&mut self) {
        if !self.released {
            self.gate.lock().unwrap().running = false;
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
            embed_gate: Arc::default(),
            model_cache_pruned: std::sync::RwLock::new(Vec::new()),
            provider: std::sync::RwLock::new(provider),
            model_id,
            chunk_params,
            read_only: false,
            #[cfg(any(test, feature = "testing"))]
            prune_statements: std::sync::atomic::AtomicU64::new(0),
            #[cfg(any(test, feature = "testing"))]
            detection_walks: std::sync::atomic::AtomicU64::new(0),
            #[cfg(any(test, feature = "testing"))]
            fail_next_source_edit: std::sync::atomic::AtomicBool::new(false),
            #[cfg(any(test, feature = "testing"))]
            hold_next_draft_edit: std::sync::atomic::AtomicU64::new(0),
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
            state_dir_override: None,
            connect_auth: Arc::new(RealConnectAuth),
            pending_connect: std::sync::Mutex::new(None),
            token_store_dir_override: None,
            github_tokens: Arc::default(),
            origin_poller: poller::OriginPollerState::default(),
            routing_virtual: std::sync::RwLock::new(BTreeMap::new()),
            activity: Arc::default(),
            list_subscribers: Arc::default(),
            domain_admin: tokio::sync::Mutex::new(()),
            join_fence: tokio::sync::RwLock::new(()),
            collab: std::sync::OnceLock::new(),
            domain_access: std::sync::OnceLock::new(),
            web_origin: std::sync::OnceLock::new(),
            joins: Arc::new(crate::join::Joins::default()),
        }
    }

    /// Arm a one-shot failure of the next source edit, on its far side.
    ///
    /// A test seam, and the only one in this file. It stands for one thing - a
    /// fault at the moment the source's bytes are committed - and each storage
    /// kind answers that differently, which is the whole point of arming it:
    ///
    /// - a **file** domain fails the reindex that follows the rename, so the
    ///   bytes are already on disk and nothing may be undone;
    /// - a **virtual** domain is handed a compare-and-swap token nothing can
    ///   match, so the store raises its own conflict and rolls the transaction
    ///   back, exactly as a concurrent edit would, and the caller may undo
    ///   whatever it wrote first.
    ///
    /// Neither is reachable from a test any other way: a rename either happens
    /// or does not, a real concurrent edit cannot be timed to land between one
    /// call's read and its write, and every input-shaped failure of the reindex
    /// is refused earlier by [`Engine::plan_split`]. The seam exists because
    /// [`Engine::split_engram_as`] must never undo its own new engram once the
    /// source has been rewritten, and must still undo it when the source is
    /// untouched, and an invariant nothing exercises is an invariant that rots.
    ///
    /// Nothing in the daemon, the CLI or the MCP surface calls this, and the two
    /// branches that read it are one relaxed swap each, one per storage kind. It
    /// is consumed by the next source edit on any domain rather than by the next
    /// split, so arm it immediately before the call under test.
    #[cfg(any(test, feature = "testing"))]
    pub fn fail_next_source_edit(&self) {
        self.fail_next_source_edit
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Hold the next edit of a draft open between its read and its write, once,
    /// so a test can put a second writer of the same draft in that window.
    ///
    /// The lost update this exists to catch cannot be timed from outside: both
    /// writers are futures on one runtime, and whether the reader yields
    /// between its read and its write depends on whether an uncontended mutex
    /// happens to suspend. A sleep inside the window makes the interleaving a
    /// fact instead of a coincidence, which is what lets
    /// `a_capture_and_an_edit_of_one_draft_serialize` fail for the right reason
    /// when the arms take two different locks and pass when they take one.
    ///
    /// Armed for ONE edit and consumed by it, like the failure seam above, and
    /// compiled out of a released binary the same way.
    #[cfg(any(test, feature = "testing"))]
    pub fn hold_next_draft_edit(&self, millis: u64) {
        self.hold_next_draft_edit
            .store(millis, std::sync::atomic::Ordering::Relaxed);
    }

    /// The hold above, consuming the arming. Does nothing at all outside a test
    /// build, where the counter does not exist.
    async fn take_draft_hold(&self) {
        #[cfg(any(test, feature = "testing"))]
        {
            let millis = self
                .hold_next_draft_edit
                .swap(0, std::sync::atomic::Ordering::Relaxed);
            if millis > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
            }
        }
    }

    /// Whether the failure seam is armed, consuming the arming. Constant
    /// `false` outside a test build, where the flag does not exist: the two
    /// source-edit branches then compile to what they would have been without a
    /// seam at all.
    fn take_armed_failure(&self) -> bool {
        #[cfg(any(test, feature = "testing"))]
        {
            self.fail_next_source_edit
                .swap(false, std::sync::atomic::Ordering::Relaxed)
        }
        #[cfg(not(any(test, feature = "testing")))]
        {
            false
        }
    }

    /// Install the co-editing registry, so a removal can close the rooms of the
    /// domain it unregisters.
    ///
    /// Held as a `Weak`: the registry owns an `Arc<Engine>`, so a strong handle
    /// here would be a cycle neither half ever drops. Called once, by
    /// `RestState::new`, which is the only thing that builds a registry. A
    /// second call is ignored, for the reason
    /// [`Engine::set_domain_access`] gives.
    pub fn set_collab_sessions(&self, sessions: &Arc<crate::collab::session::CollabSessions>) {
        let _ = self.collab.set(Arc::downgrade(sessions));
    }

    /// The open co-editing rooms, or `None` on every engine that serves no web
    /// surface - a one-shot CLI command, the embedded stdio stack, most tests.
    ///
    /// `None` is what makes those installs byte-identical to their old selves:
    /// no registry, no rooms, so every read and every edit takes the file or
    /// the row path it always took.
    fn collab_rooms(&self) -> Option<Arc<crate::collab::session::CollabSessions>> {
        self.collab.get().and_then(std::sync::Weak::upgrade)
    }

    /// Hold the domain-admin lock for the whole of a registration or a removal.
    ///
    /// One lock on the engine rather than one per surface: REST's create takes
    /// it, [`Engine::unregister_domain`] takes it, and both surfaces reach the
    /// same verbs, so a lock held by only one of them would serialize only that
    /// one. See the field for what it does not close.
    ///
    /// Lock order where both are taken (a removal): this one, then
    /// [`Engine::fence_joins`]. Nothing else takes both.
    pub async fn domain_admin(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.domain_admin.lock().await
    }

    /// Raise the join fence: while this guard lives, no collab upgrade may open
    /// a room, because [`Engine::join_pass`] is what the upgrade route waits on
    /// before it joins.
    ///
    /// Held by a removal across its sweep and the unregistration, which is what
    /// makes the sweep final rather than a snapshot: a join already in flight
    /// finishes and inserts its room before the guard is granted (so the sweep
    /// collects it), and a join that arrives afterwards waits, then finds a
    /// domain that no longer exists and is refused. Process-wide rather than
    /// per-domain because a removal is short and a second primitive per domain
    /// name would buy nothing measurable.
    pub async fn fence_joins(&self) -> tokio::sync::RwLockWriteGuard<'_, ()> {
        self.join_fence.write().await
    }

    /// The pass a collab upgrade holds across its join, so a join and a removal
    /// of the same domain cannot interleave. See [`Engine::fence_joins`] for
    /// the argument this half completes; the guard is dropped as soon as the
    /// join returns, never held across the socket's life.
    pub async fn join_pass(&self) -> tokio::sync::RwLockReadGuard<'_, ()> {
        self.join_fence.read().await
    }

    /// Install the private-domain resolver. Called once, when the HTTP surface
    /// starts and the accounts store it reads exists.
    ///
    /// A second call is ignored rather than refused: the surfaces that build a
    /// router are the only callers, and an engine that already knows how to
    /// resolve a scope must not have that authority replaced by a later,
    /// possibly different, store.
    pub fn set_domain_access(&self, access: Arc<crate::scope::DomainAccess>) {
        let _ = self.domain_access.set(access);
    }

    /// The origin rule the HTTP surface was built with, installed once by
    /// `http_base`. It is what answers a local caller's `service.public_url`
    /// (the value the daemon STARTED with, so a runtime configure of the key
    /// changes nothing until the next start, on every surface at once) and an
    /// HTTP caller's derived origin.
    pub fn set_web_origin(&self, rule: Arc<dyn crate::web_url::WebOrigin>) {
        let _ = self.web_origin.set(rule);
    }

    /// Where this instance's pages are, for a caller on this machine: the
    /// stdio bridge, the daemon's own socket, the CLI through the control
    /// socket.
    pub fn local_web_base(&self) -> crate::web_url::WebBase {
        let config = self.config.read().unwrap();
        // With a rule installed its snapshot is the answer; without one (a
        // standalone CLI run, the embedded stack) there is no HTTP surface to
        // disagree with, so the live config is read.
        let public_url = match self.web_origin.get() {
            Some(rule) => rule.public_url().map(str::to_string),
            None => config.service_public_url().map(str::to_string),
        };
        crate::web_url::local_base(
            public_url.as_deref(),
            crate::serving::serve_intent().map(|intent| &intent.http),
            config.ui_enabled(),
        )
    }

    /// Where this instance's pages are, for the HTTP caller these headers came
    /// from.
    pub fn request_web_base(&self, headers: &http::HeaderMap) -> crate::web_url::WebBase {
        let ui_enabled = self.config.read().unwrap().ui_enabled();
        match self.web_origin.get() {
            Some(rule) => rule.request_base(headers, ui_enabled),
            None => crate::web_url::WebBase::Unresolved,
        }
    }

    /// The private domains `scope` may not see, or `None` for no filtering at
    /// all.
    ///
    /// `None` in two cases, which are the same case in practice: no resolver is
    /// installed (a one-shot CLI command, the embedded stdio stack, a test
    /// engine - all of which are the machine owner), or the scope is
    /// [`Scope::Unrestricted`]. Everything else gets a set to subtract, empty
    /// on an installation where nobody has made a domain private.
    ///
    /// The privacy answer alone, and it is not the serving screen. A read that
    /// answers from the index takes [`Engine::hidden_for`], which adds the
    /// domains whose rows outlived their registration; this one is for a caller
    /// reasoning about who may see what rather than about what may be served.
    ///
    /// [`Scope::Unrestricted`]: crate::scope::Scope::Unrestricted
    pub async fn hidden_domains(
        &self,
        scope: &crate::scope::Scope,
    ) -> Result<Option<HashSet<String>>> {
        let Some(access) = self.domain_access.get() else {
            return Ok(None);
        };
        access
            .hidden_domains(scope)
            .await
            .map_err(|e| EngineError::Internal(e.to_string()))
    }

    /// What `scope` may do on one domain, for a surface that has to refuse a
    /// write rather than hide the domain.
    ///
    /// [`DomainRight::Own`] when no resolver is installed, which is the same
    /// answer [`Engine::hidden_domains`] gives on that engine: a one-shot CLI
    /// command, the embedded stdio stack and a test engine are all the machine
    /// owner. A resolver that cannot answer is an error rather than a
    /// permissive default - a write that cannot learn what its caller may do
    /// refuses instead of proceeding on an assumption, exactly as the REST
    /// write gate does.
    ///
    /// [`DomainRight::Own`]: crate::scope::DomainRight::Own
    pub async fn domain_right(
        &self,
        scope: &crate::scope::Scope,
        domain: &str,
    ) -> Result<crate::scope::DomainRight> {
        let Some(access) = self.domain_access.get() else {
            return Ok(crate::scope::DomainRight::Own);
        };
        access.right(scope, domain).await.map_err(|e| {
            EngineError::Internal(format!("this domain's membership is unreadable: {e:#}"))
        })
    }

    /// What `scope` may do on one domain when the request is a write: the
    /// domain answer capped by the instance role, which is the rule the JSON
    /// API has always applied and the one the MCP write gate reads here.
    ///
    /// Same two special cases [`Engine::domain_right`] carries, for the same
    /// reasons: no resolver installed is the machine owner, and a resolver that
    /// cannot answer is an error rather than a permissive default.
    pub async fn write_right(
        &self,
        scope: &crate::scope::Scope,
        domain: &str,
    ) -> Result<crate::scope::DomainRight> {
        let Some(access) = self.domain_access.get() else {
            return Ok(crate::scope::DomainRight::Own);
        };
        access.write_right(scope, domain).await.map_err(|e| {
            EngineError::Internal(format!("this domain's membership is unreadable: {e:#}"))
        })
    }

    /// Refuse a domain this caller must not be answered from, and say nothing
    /// about a name the index has never heard of.
    ///
    /// The narrow half of [`Engine::require_domain`], for a verb that already
    /// has its own words for a domain nobody registered and its own order for
    /// saying them. A screened domain is refused here with exactly the bytes an
    /// unregistered one gets, which is the whole point; anything else falls
    /// through untouched, so adding this gate to a verb cannot change what that
    /// verb answered before on any input the index holds rows for.
    ///
    /// [`Engine::hidden_for`] screens two things and both are refused here: a
    /// private domain, and a domain whose rows outlived their registration. The
    /// second is a name the verb behind this gate would have refused for itself
    /// a line later, since nothing unregistered resolves to a content source -
    /// so this is one line earlier, not one refusal more.
    pub async fn refuse_hidden_domain(
        &self,
        name: &str,
        scope: &crate::scope::Scope,
    ) -> Result<()> {
        let hidden = self.hidden_for(scope).await?;
        if hidden.contains(name) {
            self.domain_entry_scoped(name, &hidden)?;
        }
        Ok(())
    }

    /// Every domain name a read must answer as though it were not there: the
    /// private domains this caller may not see, plus every domain the index
    /// still holds that nobody has registered here.
    ///
    /// **The two are one rule, which is why they are one set.** A caller naming
    /// a private domain gets what naming a domain nobody registered gets, which
    /// is nothing; this says the same thing about a caller who named no domain
    /// at all. An index row whose domain is unregistered is not a hit, not a
    /// count and not a facet value - it is left in the index (a filter, never a
    /// deletion, so nothing here can lose knowledge) and simply stops being an
    /// answer. That matters because a removal used to leave rows behind
    /// deliberately, and those rows went on outranking the knowledge their owner
    /// still keeps.
    ///
    /// Resolved once per call and threaded inward. Every verb that answers from
    /// the index asks for it here rather than deciding for itself, so a read
    /// added later inherits the screen instead of having to remember it.
    ///
    /// Either half failing propagates rather than resolving to an empty set: a
    /// read that cannot learn what it may answer from refuses, and never widens.
    #[doc(hidden)]
    pub async fn hidden_for(&self, scope: &crate::scope::Scope) -> Result<HashSet<String>> {
        let mut hidden = self.hidden_domains(scope).await?.unwrap_or_default();
        hidden.extend(self.unregistered_domains().await?);
        Ok(hidden)
    }

    /// The domains the index holds that this instance has no registration for.
    ///
    /// Asked of the index on every call rather than cached, so a domain removed
    /// or registered while this engine runs takes effect on the next read rather
    /// than at the next restart. It is one column of a table with a row per
    /// domain ([`Store::domain_names`], deliberately not `domain_stats`, whose
    /// per-domain counts this would pay for and never look at).
    ///
    /// "Registered" is [`Engine::registered_domain_names`], all three tiers
    /// [`Engine::domain_entry`] resolves a name through, so the named and the
    /// unnamed answer agree: a domain a named read would resolve by re-reading
    /// the configuration file is one an unnamed sweep serves.
    ///
    /// The file is only re-read when it can change the answer - when the index
    /// holds a name the snapshot and the overlay do not know - and then once
    /// for the whole call rather than once per name. An installation with no
    /// orphan (every installation, once Part B has collected) pays the cheap
    /// in-memory check and no file read at all.
    ///
    /// [`Store::domain_names`]: crystalline_index::Store::domain_names
    async fn unregistered_domains(&self) -> Result<HashSet<String>> {
        let known: HashSet<String> = self.known_domain_names().into_iter().collect();
        let names = {
            let store = self.store.lock().await;
            store.domain_names().await?
        };
        let unknown: HashSet<String> = names
            .into_iter()
            .filter(|name| !known.contains(name))
            .collect();
        if unknown.is_empty() {
            return Ok(unknown);
        }
        let registered = self.registered_domain_names();
        Ok(unknown
            .into_iter()
            .filter(|name| !registered.contains(name))
            .collect())
    }

    /// The private domain names and the ones this caller may not see, from one
    /// read of the visibility records.
    ///
    /// For the one caller that needs both: a domain listing subtracts the
    /// hidden names and then marks each row it kept private or shared. Asking
    /// for the two separately would sweep the same table twice for one answer.
    ///
    /// The privacy half only, unlike [`Engine::hidden_for`], and it needs no
    /// more: the only caller is [`Engine::list_domains`], which builds its rows
    /// from the registrations rather than from the index, so a domain nobody
    /// registered has no row there to keep back in the first place.
    ///
    /// An engine with no resolver installed - a one-shot CLI command, the
    /// embedded stdio stack, a test engine - answers with two empty sets:
    /// there is no accounts database on those, so no domain has ever been made
    /// private through one.
    async fn visibility_for(
        &self,
        scope: &crate::scope::Scope,
    ) -> Result<(HashSet<String>, HashSet<String>)> {
        let Some(access) = self.domain_access.get() else {
            return Ok((HashSet::new(), HashSet::new()));
        };
        let visibility = access
            .visibility(scope)
            .await
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        Ok((visibility.private, visibility.hidden.unwrap_or_default()))
    }

    /// The domain list a scoped store query is given: the caller's own filter
    /// with the screened names subtracted, or - when the caller named none and
    /// something is screened out - every domain the store holds minus those.
    ///
    /// `hidden` is [`Engine::hidden_for`]'s whole answer, so "screened out"
    /// covers both halves of it: a private domain this caller may not see, and
    /// a domain the index holds that nobody registered here. An unnamed sweep
    /// therefore narrows to the registered domains that have rows, which is the
    /// same rule a named read gets from [`Engine::domain_entry_scoped`].
    ///
    /// [`ScopedDomains::AsAsked`] is the answer when there is nothing to screen
    /// out at all: nothing is subtracted, no extra query runs and the store sees
    /// exactly the filter it always saw. That is the ordinary installation - no
    /// private domains and no rows outliving their registration - so the sweep
    /// below is a cost only where something actually has to be kept back.
    ///
    /// [`ScopedDomains::Nothing`] is the case an empty list would silently
    /// widen. A caller that named only screened-out domains has asked for
    /// nothing it may read, and `Some(vec![])` is not that request - the search
    /// verbs drop an empty filter and sweep everything. So it is its own answer,
    /// and the caller returns the empty page a name nobody registered would have
    /// produced.
    async fn scoped_domains(
        &self,
        requested: &[String],
        hidden: &HashSet<String>,
    ) -> Result<ScopedDomains> {
        if hidden.is_empty() {
            return Ok(ScopedDomains::AsAsked);
        }
        if !requested.is_empty() {
            let kept: Vec<String> = requested
                .iter()
                .filter(|name| !hidden.contains(*name))
                .cloned()
                .collect();
            return Ok(if kept.is_empty() {
                ScopedDomains::Nothing
            } else {
                ScopedDomains::Only(kept)
            });
        }
        // The store's own domain list, intersected with what may be served
        // rather than assumed to be servable: every name it returns that this
        // instance has no registration for is already in `hidden`, so the
        // filter below is the whole of the rule. A registered domain with no
        // rows yet is absent from the list, which costs nothing - a domain with
        // no rows has nothing to return to any query this list narrows.
        let store = self.store.lock().await;
        let names = store.domain_names().await?;
        drop(store);
        let visible: Vec<String> = names
            .into_iter()
            .filter(|name| !hidden.contains(name))
            .collect();
        Ok(if visible.is_empty() {
            ScopedDomains::Nothing
        } else {
            ScopedDomains::Only(visible)
        })
    }

    /// The vocabulary in scope for a screened caller: the tags, categories,
    /// types and statuses of everything it may be answered from, and of nothing
    /// else.
    ///
    /// Named, the screened domain reports the empty vocabulary an unregistered
    /// name reports. Unnamed, the sweep cannot be one store query with a filter
    /// laid over its answer, because a [`crystalline_index::Vocabulary`] is
    /// aggregated counts with no domain on them: it is one query per served
    /// domain, merged back into one answer. With nothing screened out - the
    /// ordinary installation - it is the single unfiltered query it always was.
    ///
    /// Shared by [`Engine::vocabulary`], which reports it, and
    /// [`Engine::retag`], which prechecks a rename or a merge against it. Keep
    /// the two on this one seam: a tag the listing says is not there must not
    /// be a tag the merge says exists.
    ///
    /// **Base by design**, which is why every scan here names no actor. The
    /// vocabulary is the agreement a domain has reached, so a word one author
    /// is trying out in a draft is not in it, and a person shown the list is
    /// shown the team's. The one surface that reads an actor's own view is the
    /// sweep's tag-drift rule, through [`DomainView::vocabulary`], because that
    /// finding is about what that one author wrote.
    async fn scoped_vocabulary(
        &self,
        domain: Option<&str>,
        hidden: &HashSet<String>,
    ) -> Result<crystalline_index::Vocabulary> {
        Ok(match (domain, hidden.is_empty()) {
            (_, true) => {
                let store = self.store.lock().await;
                store.vocabulary(domain, None).await?
            }
            (Some(domain), false) if hidden.contains(domain) => {
                crystalline_index::Vocabulary::default()
            }
            (Some(domain), false) => {
                let store = self.store.lock().await;
                store.vocabulary(Some(domain), None).await?
            }
            (None, false) => {
                let names = match self.scoped_domains(&[], hidden).await? {
                    ScopedDomains::Only(names) => names,
                    // `AsAsked` cannot arrive with something hidden, and
                    // `Nothing` means there is no domain to sweep.
                    _ => Vec::new(),
                };
                let store = self.store.lock().await;
                let mut parts = Vec::with_capacity(names.len());
                for name in &names {
                    parts.push(store.vocabulary(Some(name), None).await?);
                }
                drop(store);
                crystalline_index::merge_vocabularies(parts)
            }
        })
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

    /// Override the state directory the overlay journal lives under, in place
    /// of the real `crystalline_core::config::state_dir`. Test-only, and the
    /// one override a test cannot do without if it reaches a removal path:
    /// [`crate::overlay_journal::journal_remove_domain`] removes a folder tree,
    /// and without this it would remove one under the developer's own state
    /// directory.
    pub fn with_state_dir(mut self, dir: PathBuf) -> Engine {
        self.state_dir_override = Some(dir);
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

    /// Whether the MCP endpoint authenticates (`auth.mcp`), read the same cheap
    /// way as [`Engine::github_enabled`]. This is the setting that creates the
    /// legacy open tier, and a write gate reads it once per write, so it must
    /// not clone the whole config to get at one bool.
    pub fn auth_mcp(&self) -> bool {
        self.config.read().unwrap().auth_mcp()
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

    /// Whether another instance holds this domain's host lock and is still
    /// heartbeating on it, as of `now`.
    ///
    /// The liveness rule is [`Engine::claim_file_host`]'s, not a second one:
    /// a heartbeat older than `stale_secs` is stale, and a takeover is exactly
    /// what a claim would be allowed to do at that point. Read through the
    /// threshold rather than through a lexical `stale_before` string because
    /// this compares one instant rather than filtering a query, and a parse
    /// that fails is read as live - the direction that keeps rows.
    ///
    /// Only [`Engine::collect_orphaned_domains`] asks. On a shared database
    /// several instances register different domains against one index, so a
    /// domain this instance has no registration for may be another's current
    /// work, and another instance's live registration is a registration.
    fn hosted_elsewhere(&self, row: &DomainStats, now: DateTime<Utc>) -> bool {
        let Some(holder) = row.host_instance_id.as_deref().filter(|h| !h.is_empty()) else {
            return false;
        };
        if holder == self.instance_id {
            return false;
        }
        let Some(beat) = row.host_heartbeat_at.as_deref() else {
            return false;
        };
        match DateTime::parse_from_rfc3339(beat) {
            Ok(beat) => {
                now.signed_duration_since(beat.with_timezone(&Utc))
                    <= Duration::seconds(self.stale_secs)
            }
            Err(_) => true,
        }
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
    ///
    /// Scoped, and async for it: this raises the one error that names every
    /// other domain, so an unfiltered answer here would tell a caller asking
    /// for a domain that does not exist the name of every private domain on the
    /// instance. A domain the caller may not see is refused as an unregistered
    /// one, and the set the refusal lists is the visible set.
    pub async fn require_domain(&self, name: &str, scope: &crate::scope::Scope) -> Result<()> {
        let hidden = self.hidden_for(scope).await?;
        self.domain_entry_scoped(name, &hidden)?;
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
    pub(crate) fn content_source(&self, name: &str) -> Result<ContentSource> {
        let entry = self.domain_entry(name)?;
        Ok(self.source_of(&entry))
    }

    /// [`Engine::content_source`] for a scoped read: a domain the caller may
    /// not see resolves to no source at all, with the same
    /// [`EngineError::UnknownDomain`] a name nobody registered gets, its
    /// `registered` list filtered to the visible set.
    pub(crate) fn content_source_scoped(
        &self,
        name: &str,
        hidden: &HashSet<String>,
    ) -> Result<ContentSource> {
        let entry = self.domain_entry_scoped(name, hidden)?;
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

    /// Whether this domain reviews changes before they land. A name nobody
    /// registers reviews nothing: the verb that asked is about to refuse it as
    /// unregistered anyway, and answering "yes" here would route a write into a
    /// draft of a domain that does not exist.
    #[doc(hidden)]
    pub fn reviews_changes(&self, name: &str) -> bool {
        self.domain_entry(name)
            .map(|entry| entry.is_overlay())
            .unwrap_or(false)
    }

    /// Whose drafts a READ may lay over the base, for the verbs that answer
    /// across domains: search, the similar advisory and the graph.
    ///
    /// [`DomainView::for_read`] asks this question of one domain and answers
    /// `None` where that domain takes changes directly, so a read of an engram
    /// in such a domain sees the folder whatever rows are left in the index.
    /// These three verbs have no single domain to ask - one query spans them -
    /// so they ask whether ANY domain they can touch reviews changes, and name
    /// no actor at all when none does. An instance with no reviewing domain
    /// anywhere therefore asks exactly the question it asked before the actor
    /// dimension existed, and a domain that stopped reviewing with rows still
    /// in the index (a fold that failed halfway, an edited config, an
    /// environment variable unset) stops shadowing its own folder.
    ///
    /// **The residue, because it is real.** A query that spans a reviewing
    /// domain AND one that has stopped reviewing still carries the actor, so
    /// the stale rows of the second are still shadowing there. Screening it per
    /// domain means the index deciding row by row, which is a `SearchQuery`
    /// carrying a set of reviewing domain ids into both backends' statements -
    /// a schema-shaped change this wave's migrations are pinned against. What
    /// closes it instead is the recovery being reachable: the rows are folded
    /// or discarded (`crystalline domain review <name> direct`), and
    /// `restore_overlays` no longer mirrors them back on the next sync.
    fn reading_actor(&self, scope: &crate::scope::Scope, requested: &[String]) -> Option<String> {
        let actor = crate::scope::overlay_actor(scope)?;
        let reviewing = if requested.is_empty() {
            // Every domain in range is every domain this process knows, and the
            // question is asked of what is already in memory: the effective
            // config plus the domains discovered since startup. NOT through
            // `registered_domain_names`, which re-reads and re-parses the
            // config file - this runs on every search, every write receipt's
            // advisory and every graph slice, and a file read on that path is
            // the shape of regression this wave has already paid for once. The
            // cost of the narrower answer is one sync pass: a domain another
            // process put into review mode a moment ago names no actor until
            // this process notices it, which costs its own drafts a place in
            // this reader's search until then and nothing else.
            let config = self.config.read().unwrap();
            let discovered = self.discovered_domains.read().unwrap();
            config.domains.values().any(DomainEntry::is_overlay)
                || discovered.values().any(DomainEntry::is_overlay)
        } else {
            requested.iter().any(|name| self.reviews_changes(name))
        };
        reviewing.then_some(actor)
    }

    /// The open document's text for this engram, when a room is open over it in
    /// this view: the one probe the verbs that guard on a checksum share.
    ///
    /// **Why it is shared.** `read_engram` answers from the room while one is
    /// open, and the checksum it hands back is the LIVE text's - that is what
    /// makes read-then-edit work while somebody is typing. A verb that then
    /// compared the caller's checksum against the stored text would refuse
    /// exactly the caller who did what the receipt told them to do, and send
    /// them back to a read that answers the same value again. So the read, the
    /// split and the delete ask this one question and compare against one
    /// answer.
    ///
    /// **It must be called above every file lock.** The room's saver holds the
    /// session state lock across `Engine::save_engram`, which takes the file
    /// lock, so a probe under that lock closes a cycle nothing times out of.
    /// `no_engine_function_composes_into_a_room_under_a_file_write_lock` scans
    /// for exactly that and names `.live_text(` among its needles, so a caller
    /// that gets this wrong fails the suite rather than the field.
    async fn live_text_at(&self, desc: &EngramDescriptor, view: &DomainView<'_>) -> Option<String> {
        let rooms = self.collab_rooms()?;
        rooms
            .live_text(&desc.domain, &desc.permalink, view.actor())
            .await
    }

    /// [`Engine::actor`] for a write that joins a draft overlay: the identity
    /// the calling surface composed, and never the configured
    /// `identity.actor`.
    ///
    /// The setting is an operator's answer to "what should this machine's
    /// writes be signed as", which is the right answer for work that lands in
    /// the shared folder. A draft is one account's own, so the account is what
    /// its provenance has to say - otherwise every actor's drafts in a reviewed
    /// domain are signed with the same house name and the review cannot tell
    /// them apart.
    fn draft_actor(&self, client: Option<&str>) -> String {
        client
            .map(sanitize_actor)
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| DEFAULT_ACTOR.to_string())
    }

    /// The actor a write records, given where it is landing: the composed
    /// identity for a draft, the configured one for a direct write.
    fn actor_for(&self, client: Option<&str>, overlay: Option<&str>) -> String {
        match overlay {
            Some(_) => self.draft_actor(client),
            None => self.actor(client),
        }
    }

    /// A draft's record: the parsed document with the WHOLE of it kept in the
    /// `content` column, and the permalink the row will answer to - the
    /// frontmatter's when it carries one, the path's slug when it does not.
    pub(crate) fn overlay_record(path: &str, text: &str) -> Result<EngramRecord> {
        let engram = parse_engram(text).map_err(|e| EngineError::Invalid(e.to_string()))?;
        let mut record = EngramRecord::from_engram(&engram, path, virtual_stamp(text));
        record.content = text.to_string();
        Ok(record)
    }

    /// Write one actor's deletion of a base row: a tombstone row standing at
    /// that path under the base row's own identity, and its mirror.
    ///
    /// The row is chunkless, which is the same shape the journal restore
    /// writes, so the two paths produce one thing rather than two: a deletion's
    /// text does not belong in the embedding backlog, and an actor who deletes
    /// a draft they had been writing must not leave that draft's chunks behind.
    /// [`Store::upsert_overlay`] keeps the row id stable across rewrites, so
    /// the chunks are cleared explicitly rather than left keyed to it.
    ///
    /// Answers `Some(warning)` for a mirror that failed after the row landed,
    /// for the reason [`Engine::write_overlay_entry`] gives at length.
    pub(crate) async fn write_overlay_tombstone(
        &self,
        domain: &str,
        actor: &str,
        desc: &EngramDescriptor,
        base_text: &str,
    ) -> Result<Option<String>> {
        let state_dir = self.journal_state_dir()?;
        let record = EngramRecord {
            path: desc.path.clone(),
            // The path, not the base row's permalink, and this is forced
            // rather than chosen: the index holds one permalink per actor per
            // domain (`idx_engram_permalink_actor`), and a move writes a
            // tombstone at the source and an entry at the destination, both
            // inheriting the one permalink the base row carries. A tombstone
            // is not an engram and answers to no address - every reader finds
            // it by path and skips it - so the column holds the row's own
            // identity in this actor's dimension instead. The journal restore
            // rebuilds a tombstone the same way, so the two shapes stay one.
            permalink: desc.path.clone(),
            title: desc.title.clone(),
            engram_type: desc.engram_type.clone(),
            status: desc.status.clone(),
            recorded_at: None,
            valid_from: None,
            valid_to: None,
            timestamp: None,
            description: None,
            content: base_text.to_string(),
            metadata: Value::Object(serde_json::Map::new()),
            tags: Vec::new(),
            observations: Vec::new(),
            relations: Vec::new(),
            links: Vec::new(),
            stamp: virtual_stamp(base_text),
            actor: String::new(),
            tombstone: true,
        };
        self.commit_overlay_row(desc.domain_id, actor, &record)
            .await?;
        let warning = match crate::overlay_journal::journal_tombstone(
            &state_dir, domain, actor, &desc.path,
        ) {
            Ok(()) => None,
            Err(e) => Some(unmirrored(domain, actor, &desc.path, &e, true)),
        };
        if let Some(text) = &warning {
            tracing::warn!(domain, actor, path = desc.path.as_str(), "{text}");
        }
        Ok(warning)
    }

    /// The row half of both overlay writers, in one transaction: the row, its
    /// chunks (none for a tombstone) and the forward references it settles.
    pub(crate) async fn commit_overlay_row(
        &self,
        domain_id: DomainId,
        actor: &str,
        record: &EngramRecord,
    ) -> Result<()> {
        let store = self.store.lock().await;
        store.begin().await?;
        let written = async {
            let id = store.upsert_overlay(domain_id, actor, record).await?;
            let chunks = if record.tombstone {
                Vec::new()
            } else {
                chunk_engram(
                    &record.title,
                    record.description.as_deref(),
                    &record.content,
                    &self.chunk_params,
                )
            };
            store.replace_chunks(id, &chunks).await?;
            // This author's own rows, re-resolved in this author's own view -
            // and the base resolvers deliberately NOT run here. They resolve
            // against the base alone, so they could never bind what a draft
            // write just created, and running them first would settle the new
            // draft's links onto base rows before the author's own rows got
            // their preference. There is nothing for them to do either way: an
            // overlay write adds no base row.
            store.reresolve_actor_references(domain_id, actor).await?;
            Ok::<(), EngineError>(())
        }
        .await;
        match written {
            Ok(()) => {
                store.commit().await?;
                Ok(())
            }
            Err(e) => {
                let _ = store.rollback().await;
                Err(e)
            }
        }
    }

    /// Take every draft the team's folder has caught up with out of the
    /// overlay, and count what is left standing against a base that moved.
    ///
    /// The clear-only pass: with no pull to attribute a divergence to, this
    /// ends what has landed and finds no new conflicts. The pull paths run the
    /// same walk with what the pull applied, which is what switches the
    /// conflicts on.
    ///
    /// **It settles nothing it did not clear.** Every conflict standing in the
    /// domain's record is left exactly as it was, and only the paths this pass
    /// took out of the overlay leave it: a maintenance sweep that ended landed
    /// drafts must never be the reason an actor's surfaced conflict disappears.
    ///
    /// Takes the domain's origin lock, which the inner pass deliberately does
    /// not: every other writer of the convergence record - the two pull paths,
    /// a share, a withdrawal and a resolution - is already inside it, and the
    /// record is saved as one whole struct, so a writer outside the lock could
    /// load it before somebody else's edit and save over it afterwards. This is
    /// the one entry point with no caller holding the lock for it, so it takes
    /// it itself.
    pub async fn converge_overlays(&self, domain: &str) -> Result<ConvergenceReport> {
        let lock = self.origin_lock(domain);
        let _guard = lock.lock().await;
        self.converge_pulled_overlays(domain, &[]).await
    }

    /// The convergence pass a pull runs, given the paths that pull applied.
    ///
    /// **Where it runs.** After every pull that advanced the base: the engine's
    /// own pull path ([`Engine::origin_update_one`]), which is the poller's too
    /// (`origin_poll_tick` decides which domains are due and delegates the pull
    /// here, so polling and on-demand updating stay one code path), and the
    /// pull a share of a reviewing domain opens with
    /// ([`Engine::overlay_share_tree`]) - that pull advances the base like any
    /// other, and a draft the team has since merged would otherwise be proposed
    /// straight back at them. And after a direct commit of a reviewing domain
    /// ([`Engine::origin_share`]'s `Committed` arm): that commit advanced the
    /// base itself, so no later pull would run this, and the actor's drafts
    /// have to become the folder the moment the commit lands.
    ///
    /// **What converges.** A draft whose bytes are now the base's own bytes has
    /// become the folder, and a tombstone converges when the path it deletes is
    /// no longer in the base. Both are read against the base SNAPSHOT under the
    /// state directory rather than the files beside it, for the reason
    /// [`crate::share_staging::build`] reads the same copies: a stray direct
    /// edit of the reviewed folder is nobody's draft and must not be mistaken
    /// for what the team reviewed. Neither test needs to know what the pull
    /// did, so both are asked of every entry.
    ///
    /// **What diverges.** An entry at a path this pull applied that did not
    /// converge: the team's answer at that path moved and the author's draft
    /// still says the older thing. Scoped to the applied paths on purpose - a
    /// draft that simply has not been shared yet is unshared work, not a
    /// conflict, and calling every one of them a conflict after every unrelated
    /// pull would make the count say nothing.
    ///
    /// **An address the pull brings in.** A base file this pull applied whose
    /// permalink a live draft holds at another path is that author's
    /// divergence too - recorded and surfaced, never a silent drop of the draft
    /// and never a rewrite of what the team reviewed. It has to surface because
    /// nothing downstream can carry both rows: a search merges its hits by
    /// permalink and would drop one of the two without a word, and a draft
    /// holding an address the reviewed folder already spends could never be
    /// folded back into that folder. It is the read-side twin of
    /// [`Engine::refuse_permalink_held_elsewhere`] and deliberately not that
    /// function: this one asks the base snapshot, which is current the moment
    /// the pull lands, while the index rows behind `find_engram` are only
    /// current once the sync after it has run.
    ///
    /// **A base path the pull RENAMED takes the draft with it** (ruling: the
    /// draft moves with the base rather than clearing, because the draft's
    /// content still applies - the team filed the same page under a new name,
    /// and ending the draft would throw away work nobody asked to end). The
    /// signal is the address plus the deletion: the pull removed the base at
    /// the draft's path and a base it applied now answers to the address that
    /// draft holds. Row and mirror move as ONE, through
    /// [`Engine::drop_overlay_entry`] and [`Engine::write_overlay_entry`] - a
    /// move that took the row and left the mirror would put the draft back at
    /// the old path on the next `reindex --wipe`. A destination this actor is
    /// already drafting at is a divergence instead: two drafts are never merged
    /// behind their author's back. A local `move_engram` cannot produce this
    /// case at all, because in review mode it moves within the acting actor's
    /// own overlay and never renames a base path, which is why upstream is the
    /// only way in and why this is where the answer lives.
    ///
    /// **A tombstone at a renamed path clears**, and that is the honest answer
    /// rather than a better one: a tombstone's permalink is its own path by
    /// design ([`Engine::write_overlay_tombstone`] says why), so it carries no
    /// address for a rename to be recognized by, and the base it deleted is
    /// gone. The page reappears for that actor under its new name.
    ///
    /// **The record is MERGED, never rebuilt.** A conflict is a fact about one
    /// actor's draft, not about the pull that happened to notice it, so a path
    /// this pass did not touch keeps whatever it was: a path that converged
    /// leaves the record, a path this pass found diverged goes in (replacing
    /// only its own entry), and a path standing from an earlier pull stays
    /// standing. Rebuilding it would mean the next pull about an unrelated file
    /// silently dropping a conflict the machinery had already surfaced, which
    /// on a ticking poller is a window one tick wide. The only other ways out
    /// of the record are the entry itself going away - an actor who no longer
    /// holds a row at a path has no conflict there - and a resolution
    /// (`Engine::settle_convergence`).
    ///
    /// A recorded conflict cannot go stale while it stands. Byte-equality is
    /// asked of EVERY entry on every pass, whatever the pull touched (see
    /// `settle_overlay_entry`'s first arm), so an author who edits their draft
    /// until it says what the folder says has it cleared by the next pass
    /// rather than left listed. What a merge preserves is only the case that is
    /// still true: a draft that still differs from the base that moved under
    /// it.
    ///
    /// A domain that takes changes directly returns at the first line and
    /// touches nothing - no store read, no base file, no journal.
    async fn converge_pulled_overlays(
        &self,
        domain: &str,
        touched: &[String],
    ) -> Result<ConvergenceReport> {
        if !self.reviews_changes(domain) {
            return Ok(ConvergenceReport::default());
        }
        let Ok((_, _, state_dir)) = self.origin_spec_for_domain(domain) else {
            // A reviewing domain with no origin has no base snapshot to
            // converge against. Nothing to do and nothing to report.
            return Ok(ConvergenceReport::default());
        };
        // The read-only id lookup, never an upserting one: a domain this index
        // has never been told about holds no drafts, and asking must not
        // register one.
        let (domain_id, mut held) = {
            let store = self.store.lock().await;
            let Some(domain_id) = store.domain_id(domain).await? else {
                return Ok(ConvergenceReport::default());
            };
            let mut held: Vec<ActorHolding> = Vec::new();
            for (actor, _) in store.overlay_counts(domain_id).await? {
                let entries = store.overlay_entries(domain_id, &actor).await?;
                held.push(ActorHolding {
                    actor,
                    entries,
                    files: Vec::new(),
                });
            }
            (domain_id, held)
        };
        // The files beside the rows, and the actor set is their UNION: an actor
        // holding nothing but files is in no `overlay_counts` answer, so a pass
        // drawn from the rows alone would never look at their files at all -
        // neither to clear one the team has caught up with nor to record the
        // conflict when it has not.
        //
        // A files overlay that could not be read is not an empty one, so this
        // pass leaves those actors' files exactly where they stand rather than
        // reporting them settled; the rows still converge, and the next pull
        // looks again.
        {
            let files = self.overlay_domain_files(domain);
            for (actor, read) in files.per_actor {
                if files.unreadable || read.unreadable {
                    tracing::warn!(
                        domain,
                        actor = actor.as_str(),
                        "the files '{actor}' has drafted could not be listed, so this pull \
                         converged their rows and left their files where they stand"
                    );
                    continue;
                }
                match held.iter_mut().find(|holding| holding.actor == actor) {
                    Some(holding) => holding.files = read.entries,
                    None => held.push(ActorHolding {
                        actor,
                        entries: Vec::new(),
                        files: read.entries,
                    }),
                }
            }
        }
        // **Two state directories, and they are not interchangeable.**
        // `state_dir` above is this domain's own origin state, which is where
        // the base snapshot's copies live; `journal_dir` is the machine's
        // overlay root, which is where the drafts and their files live. A read
        // of one under the other finds nothing and says so quietly.
        let journal_dir = self.journal_state_dir()?;
        let mut record = crate::overlay_journal::journal_record(&journal_dir, domain);
        // A conflict in a draft nobody holds any more is not a conflict. The
        // prune runs whatever the pass then decides, so an actor whose rows all
        // went away leaves no entry behind either.
        prune_settled_conflicts(&mut record, &held);
        if held
            .iter()
            .all(|holding| holding.entries.is_empty() && holding.files.is_empty())
        {
            record.cleared = 0;
            let report = ConvergenceReport {
                cleared: 0,
                diverged: record.diverged(),
            };
            self.save_convergence(&journal_dir, domain, &record);
            return Ok(report);
        }
        let touched: HashSet<&str> = touched.iter().map(String::as_str).collect();
        let addresses = pulled_addresses(&state_dir, &touched)?;

        let mut cleared = 0u64;
        for ActorHolding {
            actor,
            entries,
            files,
        } in &held
        {
            // Named here because convergence answers to nobody: it is a
            // comparison of the base snapshot against each actor's entries, so
            // it reads BOTH sides raw and never through a projection of one
            // over the other. The view is used only as the writer that ends a
            // converged draft, which is why it screens nothing.
            let view = DomainView::for_actor(self, domain, &HashSet::new(), actor)?;
            let own: HashSet<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
            for entry in entries {
                let base = crystalline_remote::state::read_base_file(&state_dir, &entry.path)?;
                match settle_overlay_entry(entry, base.as_deref(), &touched, &addresses, &own) {
                    Settle::Leave => {}
                    Settle::Clear => {
                        view.drop(domain_id, &entry.path).await?;
                        record.settle(actor, &entry.path);
                        cleared += 1;
                    }
                    Settle::Diverge => record.diverge(actor, &entry.path),
                    Settle::MoveTo(dest) => {
                        match self
                            .move_draft_with_the_base(
                                domain, domain_id, actor, &state_dir, entry, &dest,
                            )
                            .await
                        {
                            // The draft stands somewhere else now, or it has
                            // become the folder: either way nothing is left at
                            // the old path for a conflict to be about.
                            Ok(landed) => {
                                record.settle(actor, &entry.path);
                                if landed {
                                    cleared += 1;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    domain,
                                    actor = actor.as_str(),
                                    path = entry.path.as_str(),
                                    "the base moved to '{dest}' and this draft could not follow \
                                     it, so it stands where it was: {e}"
                                );
                                record.diverge(actor, &entry.path);
                            }
                        }
                    }
                }
            }
            // And their files, in the same pass and into the same record: a
            // conflict is one actor's conflict at one path, whatever kind of
            // thing stands there.
            for file in files {
                let base = crystalline_remote::state::read_base_file(&state_dir, &file.path)?;
                let bytes = if file.tombstone {
                    None
                } else {
                    crate::overlay_files::read(&journal_dir, domain, actor, &file.path).map_err(
                        |source| EngineError::Io {
                            path: format!("the files overlay of '{domain}' at '{}'", file.path),
                            source,
                        },
                    )?
                };
                match settle_overlay_file(file, base.as_deref(), bytes.as_deref(), &touched) {
                    Settle::Clear => {
                        crate::overlay_files::clear(&journal_dir, domain, actor, &file.path)
                            .map_err(|source| EngineError::Io {
                                path: format!("the files overlay of '{domain}' at '{}'", file.path),
                                source,
                            })?;
                        record.settle(actor, &file.path);
                        cleared += 1;
                    }
                    Settle::Diverge => record.diverge(actor, &file.path),
                    // A file answers to no address, so no rename can carry one:
                    // `settle_overlay_file` never answers `MoveTo`, and if it
                    // ever did the honest thing would be to leave the bytes
                    // where their author put them.
                    Settle::Leave | Settle::MoveTo(_) => {}
                }
            }
            // Once this actor's entries have settled, their references are read
            // again against what they hold now: a draft this pass ended takes
            // its address back to the base row at that path, and one that moved
            // with a renamed base carries its links to the new name. This is
            // the spec's "re-resolved on the next convergence pass", and it is
            // the one pass that sees every one of an actor's rows at once.
            if !entries.is_empty() {
                let store = self.store.lock().await;
                store.reresolve_actor_references(domain_id, actor).await?;
            }
        }
        record.cleared = cleared;
        let report = ConvergenceReport {
            cleared,
            diverged: record.diverged(),
        };
        self.save_convergence(&journal_dir, domain, &record);
        Ok(report)
    }

    /// Write one domain's convergence record, and never fail a pull over it.
    ///
    /// The record is a report, and the rows and mirrors it describes have
    /// already moved by the time it is written: failing here would turn a
    /// record this machine could not save into a failed pull over work that
    /// landed, so it only warns.
    ///
    /// **That "the next pass corrects it" is only half true.** A stale record
    /// is a subset of the truth for an entry that is still there but should
    /// have been cleared - the next pass that touches that path settles it
    /// correctly either way. It is not a subset for an entry that is MISSING:
    /// a path that diverged this pass and never made it into the saved
    /// record because the save failed settles as `Leave` on every later pass
    /// that does not touch it again, forever. Not a regression against the
    /// in-memory record this replaced, which had the same window (a crash
    /// before it was written loses it the same way) and a wider one (it did
    /// not survive a restart at all either), so this is a known gap rather
    /// than new behaviour.
    ///
    /// **Every caller holds the domain's origin lock.** The whole struct is
    /// written at once - the conflicts and the proposal owners beside them - so
    /// two writers that both loaded before either saved would lose one of the
    /// two edits, and the lock is what makes that impossible. The writers are
    /// the two pull paths ([`Engine::origin_update_one`] and the pull a share
    /// opens with), a share recording whose the proposal is, a withdrawal
    /// forgetting it, a resolution settling one path, and
    /// [`Engine::converge_overlays`], which takes the lock itself because
    /// nothing takes it for that one.
    fn save_convergence(
        &self,
        journal_dir: &Path,
        domain: &str,
        record: &crate::overlay_journal::ConvergenceRecord,
    ) {
        if let Err(e) = crate::overlay_journal::journal_save_record(journal_dir, domain, record) {
            tracing::warn!(domain, "the convergence record could not be saved: {e}");
        }
    }

    /// Move one draft from the path the pull emptied to the path the same
    /// address now stands at. Answers whether it converged there instead.
    ///
    /// The source goes first and the destination second, which is the order
    /// [`Engine::move_within_overlay`] is forced into for the same reason: one
    /// actor holds one row per permalink per domain, so until the source is
    /// gone the destination cannot take the address. Which is why a failed
    /// destination puts the source back - through the unchecked writer, since
    /// what it restores is the state the check had already allowed and the
    /// text lives in that row and nowhere else.
    ///
    /// The destination's mirror is the one half that can fail on its own
    /// without unsaying anything: [`Engine::put_overlay_entry`] writes the row
    /// first and answers a failed mirror as a warning, which is the convention
    /// every overlay write in this file follows. So the move is row-and-mirror
    /// together in intent and the warning is logged rather than swallowed; the
    /// row is what the index serves, and the next `reindex --wipe` is what
    /// would notice the difference.
    async fn move_draft_with_the_base(
        &self,
        domain: &str,
        domain_id: DomainId,
        actor: &str,
        state_dir: &Path,
        entry: &StoredEngram,
        dest: &str,
    ) -> Result<bool> {
        let landed = crystalline_remote::state::read_base_file(state_dir, dest)?
            .is_some_and(|base| base == entry.content.as_bytes());
        let view = DomainView::for_actor(self, domain, &HashSet::new(), actor)?;
        // The removal defers the ending of this draft's share-links and joins,
        // because a move that could not write its destination puts the source
        // back and did not happen - and a move that did not happen must not
        // have ended anybody's link on the way to not happening. Every way out
        // of here below either ends them or is that failure.
        view.drop_mid_move(domain_id, &entry.path).await?;
        if landed {
            // The rename carried this actor's own words with it: the draft is
            // the folder now, under its new name. There is no destination to
            // wait for and nothing left to share - everybody who can read the
            // domain reads that text anyway - so the links end here.
            self.end_draft_grants(domain, actor, &entry.path).await;
            return Ok(true);
        }
        match view.write(domain_id, dest, &entry.content).await {
            Ok(None) => {}
            // The row landed and its mirror did not. Logged rather than
            // swallowed: the index serves the row either way, and it is the
            // next `reindex --wipe` that would notice the difference.
            Ok(Some(warning)) => tracing::warn!(domain, actor, path = dest, "{warning}"),
            Err(e) => {
                if let Err(undo) = view
                    .write_unchecked(domain_id, &entry.path, &entry.content)
                    .await
                {
                    tracing::error!(
                        domain,
                        actor,
                        path = entry.path.as_str(),
                        "a draft could not follow the base to '{dest}' and could not be put back \
                         either: {undo}"
                    );
                }
                return Err(e);
            }
        }
        // The draft stands somewhere else now, so the links minted on the path
        // it left end with it: a grant was minted on a path, and re-sharing the
        // page under its new name is the author's to do.
        self.end_draft_grants(domain, actor, &entry.path).await;
        Ok(false)
    }

    /// What the last convergence has to say to one caller, or `None` when
    /// nothing has ever converged this domain.
    ///
    /// Read off the record beside the journal rather than out of memory, which
    /// is what makes a conflict survive a restart: the pass that found it may
    /// have run in a process that is long gone.
    ///
    /// The counts are the domain's; the paths are the caller's own and nobody
    /// else's. Whoever holds the domain additionally sees how many each actor
    /// is holding open - the same split, and the same shape, Task 8's `drafts`
    /// key already reports counts in, and for the same reason: a count of
    /// somebody's unsettled work is a coordination fact, and what the draft
    /// says is theirs alone.
    fn converged_json(&self, domain: &str, actor: Option<&str>, everyone: bool) -> Option<Value> {
        let journal_dir = self.journal_state_dir().ok()?;
        let record = crate::overlay_journal::journal_record(&journal_dir, domain);
        // Absent until a convergence pass has actually run here. The record can
        // exist for another reason - it also remembers whose open proposal is
        // whose - and a key that appeared the moment somebody shared would say
        // "a pull converged nothing" where nothing had looked yet, which is the
        // distinction this key's absence is FOR. It cannot always keep that
        // promise, though: a pass that ran and genuinely cleared nothing,
        // raising no conflict, leaves `cleared == 0` with `conflicts` empty
        // exactly like a domain nobody has ever pulled - this check answers
        // `None` for either, and the two conflate here.
        if record.cleared == 0 && record.conflicts.is_empty() {
            return None;
        }
        let mine: &[String] = actor
            .and_then(|who| record.conflicts.get(who))
            .map_or(&[], Vec::as_slice);
        let mut value = json!({
            "cleared": record.cleared,
            "diverged": record.diverged(),
            "mine": mine,
        });
        if everyone && let Some(object) = value.as_object_mut() {
            let counts: Vec<(String, u64)> = record
                .conflicts
                .iter()
                .map(|(actor, paths)| (actor.clone(), paths.len() as u64))
                .collect();
            object.insert("actors".to_string(), json!(review::counts_json(&counts)));
        }
        Some(value)
    }

    /// Take one path out of one actor's recorded conflicts, and answer how
    /// many they have left. A resolution settles it whichever way it went, so
    /// this runs for every arm.
    ///
    /// `cleared` beside it is deliberately left alone: it counts what the last
    /// convergence pass took out of the overlay, which a later resolution
    /// cannot change, so after a resolve the two numbers no longer sum to what
    /// that pass saw and that is the honest arithmetic rather than a drift.
    fn settle_convergence(&self, domain: &str, actor: &str, path: &str) -> u64 {
        let Ok(journal_dir) = self.journal_state_dir() else {
            return 0;
        };
        let mut record = crate::overlay_journal::journal_record(&journal_dir, domain);
        let left = record.settle(actor, path);
        self.save_convergence(&journal_dir, domain, &record);
        left
    }

    /// Settle one actor's recorded conflict at one FILE path, under the
    /// domain's origin lock, and only when there is one to settle.
    ///
    /// **The lock is the point.** [`Engine::save_convergence`] writes the whole
    /// record at once - the conflicts and the proposal owners beside them - so
    /// two writers that both loaded before either saved lose one of the two
    /// edits, and its doc states that every caller holds the domain's origin
    /// lock. [`Engine::origin_resolve`] earns its place on that list by taking
    /// one; an upload is not an origin verb and holds nothing of its own, so it
    /// takes the lock here. Without it a draft upload racing a poller tick's
    /// pull would drop another actor's conflicts, or the record of whose open
    /// proposal is whose.
    ///
    /// **The read in front of it is not an optimization alone.** Every draft
    /// upload would otherwise do a full read-modify-write of the record and
    /// queue behind the domain's origin lock to do it, for a path that is not in
    /// conflict at all - which is every upload but the rare one. A record this
    /// read finds nothing in is a record this call has nothing to say about, and
    /// a conflict that appears between the read and the lock is recorded by a
    /// pull that has not finished yet, so the next write of the same path
    /// settles it.
    ///
    /// **A lock that cannot be taken skips the settle rather than failing the
    /// write.** A reviewing domain always has an origin (the mode requires one),
    /// so this is the domain being unregistered underneath; losing somebody's
    /// upload over a bookkeeping write would be the wrong way round.
    async fn settle_file_convergence(&self, domain: &str, actor: &str, path: &str) {
        let Ok(journal_dir) = self.journal_state_dir() else {
            return;
        };
        let record = crate::overlay_journal::journal_record(&journal_dir, domain);
        if !record
            .conflicts
            .get(actor)
            .is_some_and(|paths| paths.iter().any(|held| held == path))
        {
            return;
        }
        let Ok(lock) = self.origin_lock_registered(domain) else {
            tracing::warn!(
                domain,
                actor,
                path,
                "a draft file was written at a path recorded as a conflict, and the domain's \
                 origin lock could not be taken to settle it; the record is behind until the \
                 next write of this path"
            );
            return;
        };
        let _guard = lock.lock().await;
        self.settle_convergence(domain, actor, path);
    }

    /// Record which overlay actor a proposal belongs to, or forget one that has
    /// been withdrawn.
    ///
    /// Best effort: what this feeds is the wording of a refusal, never a write
    /// of anybody's knowledge, and a proposal whose owner is not recorded reads
    /// as somebody else's - which is the safe way round, since it never tells
    /// one actor to withdraw a proposal that is not theirs.
    ///
    /// **Last writer wins, not first**, which matters only once more than one
    /// actor can touch one open proposal: while `REVIEW_NO_STACKING` refuses a
    /// second actor's share against an open proposal, this is never called
    /// twice for the same number by two different actors, so the map's one
    /// entry is always the opener's. If a later design serves stacked
    /// proposals and calls this for a second sharer, the insert here
    /// overwrites the recorded owner with the last sharer rather than keeping
    /// the opener - the map is also never pruned of a merged or closed
    /// proposal, only a withdrawn one. Both are tracked in `plans/backlog.md`
    /// beside the per-actor proposal chain entry, not fixed here: neither is
    /// reachable while one proposal at a time is enforced upstream of this
    /// map.
    fn record_proposal_actor(&self, domain: &str, number: u64, actor: Option<&str>) {
        let Ok(journal_dir) = self.journal_state_dir() else {
            return;
        };
        let mut record = crate::overlay_journal::journal_record(&journal_dir, domain);
        match actor {
            Some(actor) => {
                record
                    .proposals
                    .insert(number.to_string(), actor.to_string());
            }
            None => {
                record.proposals.remove(&number.to_string());
            }
        }
        self.save_convergence(&journal_dir, domain, &record);
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

    /// [`Engine::domain_entry`] for a scoped read: a domain the caller may not
    /// see is answered exactly as a domain nobody registered.
    ///
    /// Both halves matter. The hidden name errors instead of resolving, and the
    /// error's `registered` list has the hidden names taken out of it - an
    /// unfiltered list would name every private domain on the instance in the
    /// error text of a request for a domain that does not exist.
    fn domain_entry_scoped(&self, name: &str, hidden: &HashSet<String>) -> Result<DomainEntry> {
        if hidden.contains(name) {
            return Err(self.unknown_domain(name, hidden));
        }
        match self.domain_entry(name) {
            Err(EngineError::UnknownDomain { domain, .. }) => {
                Err(self.unknown_domain(&domain, hidden))
            }
            other => other,
        }
    }

    /// The [`EngineError::UnknownDomain`] a scoped caller gets: the registered
    /// set it names, minus what the caller may not see. A hidden domain and a
    /// name nobody ever registered produce the same bytes, which is the point -
    /// existence is the secret being kept.
    fn unknown_domain(&self, name: &str, hidden: &HashSet<String>) -> EngineError {
        EngineError::UnknownDomain {
            domain: name.to_string(),
            registered: self
                .known_domain_names()
                .into_iter()
                .filter(|known| !hidden.contains(known))
                .collect(),
        }
    }

    /// Re-read the global config from disk looking for a domain registered
    /// after this engine started. A hit is cached in `discovered_domains` and,
    /// for a file domain on the daemon, reported over `watch_tx` so the watcher
    /// starts watching its root without a restart. A virtual domain has no root,
    /// so it is cached but never watched.
    fn refresh_domain(&self, name: &str) -> Option<DomainEntry> {
        let fresh = self.reread_config()?;
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

    /// The effective config as the file has it right now: the same file this
    /// engine persists to (its `--config` override, else the default global
    /// path) with the environment overlay layered back on, so a post-startup
    /// re-read sees what a fresh load would, environment overrides included.
    /// `None` when the path cannot be resolved or the file cannot be read.
    ///
    /// Shared by [`Engine::refresh_domain`], which caches what it finds and
    /// arms a watch for it, and [`Engine::diagnostic_file_domains`], which
    /// deliberately does neither. Keep the two together: they read the same
    /// file the same way and only differ in what they do with the answer.
    fn reread_config(&self) -> Option<GlobalConfig> {
        let path = self.config_file_path()?;
        let file = overlay::load_file(&path).ok()?;
        Some(self.overlay.apply(&file))
    }

    /// The configuration file this engine reads and persists to: its
    /// `--config` override, else the default global path. `None` when the
    /// default path cannot be resolved at all (no home directory to put it
    /// in), which is the one case where there is no file to speak of.
    ///
    /// Says nothing about whether the file exists or parses; a caller that
    /// needs to know reads it.
    fn config_file_path(&self) -> Option<PathBuf> {
        match &self.config_path {
            Some(p) => Some(p.clone()),
            None => crystalline_core::config::global_config_path().ok(),
        }
    }

    /// The file domains a diagnostic read covers, as `(name, root)` pairs:
    /// everything [`Engine::sync_targets`] would sync, plus every file domain
    /// the config file names right now, so a domain registered after this
    /// daemon started is diagnosed rather than reported as entirely
    /// unindexed. `only` narrows the set to one domain; naming a virtual
    /// domain is an empty answer (a virtual domain has no files to stamp),
    /// and naming a domain nobody registered is an error.
    ///
    /// Deliberately not routed through [`Engine::domain_entry`]: that path
    /// ends in [`Engine::refresh_domain`], which caches the hit and arms a
    /// watch, so merely diagnosing a domain would start indexing it and the
    /// "not indexed yet" a report just printed would quietly fix itself
    /// behind the reader's back. A diagnosis only reads.
    fn diagnostic_file_domains(&self, only: Option<&str>) -> Result<Vec<(String, PathBuf)>> {
        let mut targets = self.sync_targets(None)?;
        let fresh = self.reread_config();
        if let Some(fresh) = &fresh {
            for (name, entry) in &fresh.domains {
                if targets.iter().any(|(n, _)| n == name) {
                    continue;
                }
                if let Some(root) = entry.file_path().filter(|_| !entry.is_virtual()) {
                    targets.push((name.clone(), root));
                }
            }
        }
        if let Some(name) = only {
            let registered = self.known_domain_names().iter().any(|n| n == name)
                || fresh.is_some_and(|c| c.domains.contains_key(name));
            if !registered {
                return Err(EngineError::UnknownDomain {
                    domain: name.to_string(),
                    registered: self.known_domain_names(),
                });
            }
            targets.retain(|(n, _)| n == name);
        }
        Ok(targets)
    }

    /// Every domain name this instance has a registration for, resolved the way
    /// a *named* lookup resolves one: the startup snapshot, the discovered
    /// overlay, then a re-read of the configuration file on disk - the three
    /// tiers of [`Engine::domain_entry`], so what one verb calls registered
    /// another cannot call an orphan.
    ///
    /// A union of the three and not a replacement by the newest: the file is
    /// not a superset of the snapshot (an engine can be built over a
    /// configuration that was never written to that file, which is what a
    /// one-shot command and every test engine are), and `domain_entry` is an OR
    /// across the tiers, so this is too.
    ///
    /// One file read per call, never one per name, and never a write: unlike
    /// [`Engine::refresh_domain`] nothing found here is cached into the
    /// discovered overlay and no watch is armed for it, because merely asking
    /// whether a domain is registered must not start indexing it. The file read
    /// failing is read as no further registrations, so the answer narrows and
    /// never widens on an unreadable configuration.
    ///
    /// **This is the set collection may key on.** A caller deciding that rows
    /// are collectable, or stamping `last_registered` for one that is not, must
    /// resolve "registered" through here rather than through
    /// [`Engine::known_domain_names`], whose two tiers miss a registration this
    /// process has never been asked about by name.
    pub fn registered_domain_names(&self) -> HashSet<String> {
        self.registered_domain_entries().into_keys().collect()
    }

    /// [`Engine::registered_domain_names`] with each name's entry: the same
    /// three-tier union, resolved in the order [`Engine::domain_entry`]
    /// resolves a name (snapshot, then discovered overlay, then the file), so
    /// a name every tier holds is served with the entry a named read would
    /// get. The same one file read, and the same absence of side effects:
    /// nothing found here is cached or watched.
    ///
    /// **This is what a listing iterates.** The CLI's `domain add` registers a
    /// domain by editing the config file from another process and, with a
    /// daemon running, only asks it to sync the new name; the daemon's
    /// snapshot never learns of it. A listing drawn from the snapshot alone
    /// therefore omitted a domain every named verb resolved and every search
    /// hit, until the daemon restarted - which is what Fluid's home screen and
    /// the `list_domains` tool showed a user who had just added one.
    fn registered_domain_entries(&self) -> IndexMap<String, DomainEntry> {
        let mut entries = self.config.read().unwrap().domains.clone();
        for (name, entry) in self.discovered_domains.read().unwrap().iter() {
            entries.entry(name.clone()).or_insert_with(|| entry.clone());
        }
        if let Some(fresh) = self.reread_config() {
            for (name, entry) in fresh.domains {
                entries.entry(name).or_insert(entry);
            }
        }
        entries
    }

    /// [`Engine::registered_domain_names`] for the one caller that may not
    /// accept a narrowed answer: `None` when the configuration file could not
    /// be read, `Some` of the same three-tier union when it could.
    ///
    /// The difference is the whole point. Serving narrows on an unreadable
    /// file and is right to: the worst it costs is a domain that is not
    /// answered for until the file is readable again. A caller that DELETES on
    /// absence cannot narrow, because a file it could not read is not evidence
    /// that anything is absent from it - and a file that is not there at all
    /// is indistinguishable from one in which every domain was just removed.
    /// So a missing file is `None` here as surely as an unparseable one, even
    /// though [`overlay::load_file`] reads a missing file as an empty
    /// configuration (which is the right answer for every other caller: an
    /// installation configured entirely by environment variables has no file
    /// and is not misconfigured).
    ///
    /// Still the union of all three tiers, never the file alone: the file is
    /// not a superset of the startup snapshot, and a domain registered in the
    /// snapshot is registered.
    fn registered_domain_names_checked(&self) -> Option<HashSet<String>> {
        let path = self.config_file_path()?;
        if !path.is_file() {
            return None;
        }
        let file = overlay::load_file(&path).ok()?;
        let fresh = self.overlay.apply(&file);
        let mut names: HashSet<String> = self.known_domain_names().into_iter().collect();
        names.extend(fresh.domains.keys().cloned());
        Some(names)
    }

    /// Every domain name this engine has been *told* about: the startup
    /// snapshot plus anything a named lookup has discovered since.
    ///
    /// Two of the three tiers [`Engine::domain_entry`] resolves through, so it
    /// is not the registered set and must not be used as one: a domain the
    /// configuration file gained since startup, and that nothing has named
    /// here yet, is registered and absent from this. It names what this
    /// instance is currently syncing and what an error message may list, both
    /// of which want the cheap in-memory answer. Use
    /// [`Engine::registered_domain_names`] to decide whether a domain is
    /// registered at all.
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
    /// watcher to stop watching its root.
    ///
    /// The index rows are not touched here because they are not this
    /// function's business: `domain_remove` clears them in the same removal,
    /// and any that outlive it (a removal on another instance, an upgrade that
    /// inherited them) are the orphan collector's, which ages them out on the
    /// daemon's sweep. A reindex is never the remedy for a row.
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
        self.resolve_scoped(identifier, domain, &HashSet::new())
            .await
    }

    /// [`Engine::resolve`] for a call that has already named the domain it acts
    /// on: an identifier may not move the call to a different one.
    ///
    /// Every write verb takes a `domain` beside its identifier, and every
    /// surface gates on THAT name - the REST layer resolves the caller's rights
    /// for it before the verb runs, and an MCP tool call is gated the same way.
    /// The absolute `crystalline://` form, though, overrides the domain hint
    /// wherever it is accepted ([`Engine::resolve_scoped`]'s first branch), so
    /// a write that resolved it would act on a domain nobody gated: a move out
    /// of a private domain into a readable one, a superseded pair written into
    /// a private engram's file, an `evolve_ack` stamped into one. The reads
    /// are safe because they resolve through the scoped resolver and a hidden
    /// domain is simply absent from it; the writes carry the acting scope but
    /// resolve nothing with it, and this is the boundary that makes that
    /// unnecessary.
    ///
    /// So the rule is the narrow one that costs nothing legitimate: on a call
    /// that names a domain, an absolute identifier naming a DIFFERENT one is
    /// refused. The same-domain absolute form still resolves, and a bare
    /// permalink or title is domain-relative as it always was. The refusal is
    /// the [`EngineError::NotFound`] a missing engram produces, built from the
    /// caller's own words, so it discloses nothing about whether that domain
    /// exists at all - a hidden domain, an unregistered one and a permalink
    /// nobody wrote are one answer.
    ///
    /// The comparison is exact. Domain names are matched exactly everywhere
    /// else in this engine (the registry is a map keyed by the name as
    /// written), so folding case here would be this one place disagreeing with
    /// the lookup it stands in front of.
    ///
    /// **The completeness claim this buys**, which the surfaces above rely on:
    /// a write verb resolves only inside the domain its parameters name, so
    /// gating those names - `domain`, plus `destination_domain` on a move,
    /// which are the only domain-valued fields any write parameter carries -
    /// gates the whole call.
    pub(crate) async fn resolve_in(
        &self,
        identifier: &str,
        domain: &str,
    ) -> Result<(EngramDescriptor, ContentSource)> {
        if let Some(url) = CrystallineUrl::parse(identifier)
            && url.domain != domain
        {
            return Err(EngineError::NotFound(format!(
                "no engram '{}' in domain '{}'",
                url.permalink, url.domain
            )));
        }
        self.resolve(identifier, Some(domain)).await
    }

    /// [`Engine::resolve_scoped`] with this reader's own drafts shadowing the
    /// base rows, and the text they should be read through.
    ///
    /// **The registered-set screen composes ahead of the actor dimension, and
    /// this is where that order is kept.** `hidden` has already been resolved
    /// by the caller through [`Engine::hidden_for`] and it is applied first, by
    /// the very same [`Engine::resolve_scoped`] a direct read uses: a draft in
    /// a domain this reader may not see, and a draft in a domain this instance
    /// has no registration for, are both absent before whose-draft-is-it is
    /// ever asked. Asking in the other order would make a draft the one thing
    /// that could name a private domain.
    ///
    /// A bare identifier with no domain named FINDS base rows only: the
    /// cross-domain form counts its matches to decide whether an identifier is
    /// ambiguous, and drafts would make that count depend on who is asking, so
    /// naming the domain is how a reader reaches a draft at a path the files
    /// never held. It still honours a tombstone, and that costs nothing: by the
    /// time one base row has been resolved the domain is known, so "has this
    /// reader deleted it" is one lookup away and a deletion is a deletion
    /// however the engram was addressed.
    async fn resolve_shadowed(
        &self,
        identifier: &str,
        domain: Option<&str>,
        hidden: &HashSet<String>,
        scope: &crate::scope::Scope,
    ) -> Result<(EngramDescriptor, ContentSource, Option<String>)> {
        let named = match CrystallineUrl::parse(identifier) {
            Some(url) => Some(url.domain),
            None => domain.map(str::to_string),
        };
        // A name nobody registered builds no view and is left to
        // `resolve_scoped` to answer, exactly as it was before the view
        // existed: the miss it produces is the one an engram that was never
        // written produces, and an early refusal here would not be.
        let view = named
            .as_deref()
            .filter(|name| !hidden.contains(*name))
            .and_then(|name| DomainView::for_read(self, name, hidden, scope).ok());
        let Some(actor) = view.as_ref().and_then(|view| view.actor()) else {
            let (desc, source) = self.resolve_scoped(identifier, domain, hidden).await?;
            // A bare identifier with no domain named reaches here, and by now
            // the domain IS known: the descriptor says which one. So a path
            // this reader has tombstoned is absent for them however they
            // addressed it. Only the DRAFT half stays conditional on naming a
            // domain - see the doc above.
            let view = DomainView::for_read(self, &desc.domain, hidden, scope)?;
            let Some(actor) = view.actor().map(str::to_string) else {
                return Ok((desc, source, None));
            };
            // The rule is `DomainView::shadow`'s, the same function the write
            // path resolves through: a tombstone says this PATH holds nothing
            // for them, and the address may well have moved to another path of
            // their own. The miss it would raise does not name a domain,
            // because this identifier did not.
            let (desc, source) = view
                .shadow(identifier, Ok((desc, source)), || {
                    format!("no engram matches '{identifier}'")
                })
                .await?;
            return Ok((desc, source, Some(actor)));
        };
        let view = view
            .as_ref()
            .expect("an overlay actor is only resolved for a named domain");
        let name = view.domain().to_string();
        // **A tombstone is about a PATH, and an address can move off it**, and
        // that rule lives in `DomainView::shadow` rather than here: the write
        // path resolves through the very same function, so what this reader can
        // open at an address is what they can save at it.
        let base = self.resolve_scoped(identifier, domain, hidden).await;
        let (desc, source) = view
            .shadow(identifier, base, || {
                format!("no engram '{identifier}' in domain '{name}'")
            })
            .await?;
        Ok((desc, source, Some(actor.to_string())))
    }

    /// [`Engine::resolve`] with the domains the caller may not see subtracted.
    ///
    /// A hidden domain resolves as an empty one rather than as a refusal: the
    /// lookup is skipped and the miss falls through to the very same
    /// [`EngineError::NotFound`] an engram that was never written produces,
    /// byte for byte, because it is produced by the same line. That equality is
    /// the property this whole path exists for - a caller must not be able to
    /// tell "you may not see this" from "there is nothing here" - and it holds
    /// by construction rather than by two messages being kept in step.
    ///
    /// The bare cross-domain form filters its matches before it counts them, so
    /// a hidden domain neither makes an identifier ambiguous nor gets its name
    /// printed into the ambiguity error.
    async fn resolve_scoped(
        &self,
        identifier: &str,
        domain: Option<&str>,
        hidden: &HashSet<String>,
    ) -> Result<(EngramDescriptor, ContentSource)> {
        if let Some(url) = CrystallineUrl::parse(identifier) {
            let found = if hidden.contains(&url.domain) {
                None
            } else {
                let store = self.store.lock().await;
                store.find_engram(&url.domain, &url.permalink).await?
            };
            let d = found.ok_or_else(|| {
                EngineError::NotFound(format!(
                    "no engram '{}' in domain '{}'",
                    url.permalink, url.domain
                ))
            })?;
            let source = self.read_source(&url.domain);
            return Ok((d, source));
        }

        if let Some(dom) = domain {
            let found = if hidden.contains(dom) {
                None
            } else {
                let store = self.store.lock().await;
                store.find_engram(dom, identifier).await?
            };
            let d = found.ok_or_else(|| {
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
            let source = self.read_source(dom);
            return Ok((d, source));
        }

        // Bare identifier across all domains, minus the ones this caller may
        // not see. Filtered before the count, so a hidden twin neither turns a
        // single match into an ambiguity nor names itself in the error.
        let store = self.store.lock().await;
        let mut matches = store.find_engram_any(identifier).await?;
        drop(store);
        matches.retain(|d| !hidden.contains(&d.domain));
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
    pub(crate) async fn load_engram(
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
    pub(crate) async fn load_content(
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
}

// The `impl Engine` sections, one file each; FILES in the split script
// lists them in the order they stood in the single engine.rs.
mod attachments;
mod configure;
mod context;
mod delete;
mod domain_add;
mod domains;
mod edit;
mod evolve;
mod github;
mod move_;
mod origins;
mod read;
mod review_mode;
mod schemas;
mod search;
mod sync;
mod transfer;
mod validate;
mod write;

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

/// What a second connect call adds to the outstanding flow's guidance: the
/// way to give up on a code the person cannot use. Appended only on that
/// branch (see [`Engine::start_device_connect`]), so a first connect never
/// advertises abandoning a code nobody has tried yet.
const RESTART_SENTENCE: &str = "If this code is not usable - it was never seen, or it has gone \
                                stale - call configure again with connect \"github\" and restart \
                                true to abandon it and get a fresh one.";

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

/// The half of a running device flow a surface has to show: the code, where
/// to enter it, and how long is LEFT to do so.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GithubPending {
    pub user_code: String,
    pub verification_url: String,
    /// Seconds REMAINING before the code expires, recomputed on every read
    /// and saturating at 0 - not the flow's original lifetime. A caller that
    /// polls therefore watches it fall, which is what distinguishes a live
    /// sign-in from a wedged one; a countdown in a UI can simply start from
    /// this number.
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
    /// Never reported as such: every surface reports what is LEFT of it,
    /// derived here against `started_at` (see [`Engine::pending_view_for`]).
    expires_in_secs: u64,
    /// When the flow started, so a pending view counts down instead of
    /// repeating the original expiry on every call. A frozen number is what
    /// made a live sign-in look stuck; a falling one is the cheapest possible
    /// proof that the flow is still running.
    ///
    /// `tokio::time::Instant` rather than `std::time::Instant` deliberately:
    /// the daemon's clock here is the runtime's, so a test can pause it and
    /// advance it rather than sleeping through a real code lifetime.
    started_at: tokio::time::Instant,
    /// The background task running this flow, so a restart can abandon it
    /// rather than leave it to land on top of the fresh sign-in. Filled in
    /// immediately after the spawn (the handle does not exist yet when this
    /// record is built), and `None` only in that instant.
    abort: Option<tokio::task::AbortHandle>,
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

impl PendingConnect {
    /// How many seconds of this code's life are left, saturating at 0. The
    /// one place the countdown is computed, so the MCP view and the REST one
    /// can never report different numbers for the same flow.
    fn remaining_secs(&self) -> u64 {
        self.expires_in_secs
            .saturating_sub(self.started_at.elapsed().as_secs())
    }

    /// What a caller is shown about this flow: the code, where to type it, how
    /// long it lives and what to do next.
    ///
    /// A method on the record rather than on the engine, so the answer can be
    /// built from a record in hand as well as from one read back out of the
    /// slot - which is what keeps a flow that HAS started from being reported
    /// as no flow when something clears the slot in between.
    fn view(&self) -> Value {
        json!({
            "pending": true,
            "user_code": self.user_code,
            "verification_url": self.verification_url,
            "expires_in_secs": self.remaining_secs(),
            "next_steps": self.next_steps,
        })
    }
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

/// How a credential's owner is named in a log line: `instance` for the
/// machine's own, `personal:<account>` for one person's. Never a token, never
/// a device code - just enough to tell two concurrent sign-ins apart in a
/// daemon log a colleague pastes into a support thread.
fn identity_label(identity: &TokenIdentity) -> String {
    match identity {
        TokenIdentity::Instance => "instance".to_string(),
        TokenIdentity::Personal(name) => format!("personal:{name}"),
    }
}

/// Runs a [`TokenSavePlan`] on the blocking pool, so the OS keychain write it
/// performs never occupies an async worker. `crystalline_remote::token` bounds
/// every keychain call at fifteen seconds, so this cannot park a blocking
/// thread forever either; the two together are why a wedged keychain now
/// degrades a sign-in instead of freezing the daemon.
///
/// A panic in the save is reported as a credential failure rather than
/// unwrapped: the caller is a background flow whose whole job is to land an
/// outcome, and a task that disappeared without one is exactly the silence
/// this change exists to remove.
async fn save_off_runtime(
    plan: TokenSavePlan,
    token: StoredToken,
) -> std::result::Result<(), RemoteError> {
    // Where the token landed is only known after the write - `save_resolving`
    // falls through to the file store on its own - so the line is emitted
    // here, back on the runtime, rather than inside the blocking closure.
    // Both connect paths save through this function, so both get the line.
    let identity = identity_label(&plan.identity);
    match tokio::task::spawn_blocking(move || plan.save(&token)).await {
        Ok(Ok(store)) => {
            tracing::info!(identity = %identity, store, "github token saved");
            Ok(())
        }
        Ok(Err(e)) => Err(e),
        Err(e) => Err(RemoteError::Credential {
            detail: format!("could not save the GitHub token: {e}"),
        }),
    }
}

impl TokenSavePlan {
    /// Writes `token` once (through the override file or `save_resolving`) then
    /// refreshes this host's cache entry, so the very next `github_credential`
    /// serves the new identity without another keychain read. A connect is
    /// therefore one keychain write and zero reads.
    ///
    /// Answers WHERE the token landed (`"keyring"` or `"file"`), which only
    /// this call knows: `save_resolving` falls through to the file store by
    /// itself when the keychain refuses or does not answer in time.
    /// [`save_off_runtime`] turns that into the one "token saved" log line.
    fn save(&self, token: &StoredToken) -> std::result::Result<&'static str, RemoteError> {
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
        let kind = store.kind();
        let key = credential_cache_key(&self.identity, self.host.as_deref());
        self.cache.lock().unwrap().insert(
            key,
            CachedGithub {
                store,
                token: token.clone(),
            },
        );
        Ok(kind)
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
#[doc(hidden)]
pub fn set_domain_provision_decision(
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
#[doc(hidden)]
pub fn apply_report_json(report: &crystalline_core::provision::ApplyReport) -> Value {
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
#[doc(hidden)]
pub fn status_report_json(report: &crystalline_core::provision::StatusReport) -> Value {
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
    let mut engine = Engine::new(store, file, None, Some(path))
        .with_read_only(read_only)
        .with_env_overlay(overlay);
    // The daemonless engine is told where this machine's state directory is,
    // rather than leaving [`Engine::journal_state_dir`] to resolve it. Both
    // halves matter. In production it is the same path either way, and saying
    // it here is what keeps a one-shot `crystalline write` into a domain in
    // review mode able to mirror the draft it just wrote. Under the test seam
    // the resolver refuses instead of falling back, so without this line every
    // draft a CLI test writes would fail - and with it, the path resolves
    // inside whatever isolated `HOME` that test set, which is the same
    // directory `crystalline reindex --wipe` already restores drafts from.
    if let Ok(state) = crystalline_core::config::state_dir() {
        engine = engine.with_state_dir(state);
    }
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
            Ok(n) => {
                // The count the daemon's startup pass used to log itself. It
                // belongs here now that every pass comes through the worker,
                // and stays at info: the worker coalesces a burst of requests
                // into one pass, so a large first index is one line, not
                // thousands.
                tracing::info!("embedded {n} chunk(s)");
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
/// unknown id with the whole catalog named. An id outside the catalog errors
/// here rather than returning silence.
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

/// A domain's verify overrides, read from the `.crystalline.yaml` at its root
/// through the loader `crystalline verify` uses, so a file that does not parse
/// means no overrides here as there (validate reports it as `M108`). A virtual
/// domain has no root and therefore no overrides, so its engrams take the
/// default budget.
fn domain_verify_config(source: &ContentSource) -> Option<VerifyConfig> {
    let ContentSource::File { root } = source else {
        return None;
    };
    crystalline_core::verify::load_domain_config(root)
        .config
        .verify
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

/// What presenting a draft share-link did.
///
/// Two answers rather than an answer and an error, because binding the link and
/// joining the draft are two steps and only the first of them decides whether
/// the caller may SEE the draft. A reader who may not edit it, and one already
/// working in as many drafts as this instance keeps open for one account, have
/// each redeemed the link and may read what it opens; a read that failed on
/// either would be answering a question nobody asked. Every way the link itself
/// fails to open anything stays an error, because that caller has to be told.
pub enum OpenedLink {
    /// The link bound and this holder is inside the draft.
    Joined {
        /// The key this holder presents the join with.
        key: String,
        /// The join itself, which is what routes a write.
        join: crate::join::Join,
    },
    /// The link bound and the draft is readable; the join was refused, and the
    /// sentence says why. A write is refused with it; a read is not.
    ReadOnly(String),
}

/// The document a wholesale capture would replace, when somebody has it open.
///
/// Both fields are what the question needs and neither is the caller's to
/// derive: the permalink is where the title would land, and `present` is who
/// is in the room with the agent asking - its own slot left out, the way every
/// other answer about a room leaves it out.
pub struct LiveWriteTarget {
    /// The permalink the capture resolved to, so the question names the engram
    /// rather than the title typed at it.
    pub permalink: String,
    /// Who is in the room over it right now, in the order the room reports.
    pub present: Vec<String>,
}

/// What one source edit did: the mirror warning it may owe, and - when a
/// co-editing room was open over the engram - the live document it landed in
/// instead of the file or the row.
///
/// Two fields rather than one return value each, because both travel to the
/// same place: the receipt the caller builds. `live` is `None` for nearly
/// every edit there is, which is the ordinary write path saying it wrote
/// ordinarily.
struct SourceEdited {
    /// The unmirrored-draft warning, when the write owed one.
    warning: Option<String>,
    /// The live document this edit composed into, when one was open.
    live: Option<crate::collab::session::LiveApplied>,
}

/// A source edit that failed, and whether the source may already carry the new
/// bytes when it did.
///
/// The flag is the whole point of the type. [`Engine::split_engram_as`] writes a
/// second engram before it edits the source, and it may only take that engram
/// back while the source is provably as it was; once the source has been
/// rewritten, deleting the new engram is what would lose the moved
/// observations, since the source no longer holds them.
struct SourceEditFailure {
    /// Whether the source's stored bytes may already be the edited ones.
    wrote: bool,
    /// The failure itself, reported to the caller unchanged.
    error: EngineError,
}

impl SourceEditFailure {
    /// A failure with the source still as it was: the read, the checksum
    /// compare, the edit itself, the temporal enforcement or a refused write.
    fn before(error: EngineError) -> SourceEditFailure {
        SourceEditFailure {
            wrote: false,
            error,
        }
    }

    /// A failure with the source's bytes already replaced, or possibly
    /// replaced: the reindex that follows a file write, and any failure of the
    /// virtual store call that both swaps and indexes.
    fn after(error: EngineError) -> SourceEditFailure {
        SourceEditFailure { wrote: true, error }
    }
}

/// What a split resolved to: the text leaving the source, the text staying and
/// how much of each kind of thing moved.
struct SplitPlan {
    /// The selected lines, in source order, with surrounding blank lines
    /// trimmed off.
    moved: String,
    /// The source with those lines gone, frontmatter and all.
    remaining: String,
    /// How many distinct observation bullets moved. A bullet that also sits
    /// inside a moved section is counted here as well, since the caller named
    /// it both ways.
    observations: usize,
    /// How many distinct sections moved: line ranges rather than paths, so two
    /// spellings of one heading count once. A path naming a subsection of
    /// another moved section is a different range and counts separately, which
    /// is the honest answer to "how many sections did you name that moved".
    sections: usize,
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
    crystalline_core::config::registration::canonical_root(entry)
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

/// What a connect without a branch answers when the forge could not say which
/// branch is the repository's default. An error about the connection itself
/// (expired, missing, or blocked by an organization) passes through
/// unchanged, since naming a branch fixes none of them and its own message
/// names the fix; every other failure is refused with the way around it.
/// Never a fall back to `main`.
fn default_branch_refusal(repo: &str, e: RemoteError) -> EngineError {
    match e {
        RemoteError::AuthExpired
        | RemoteError::NotConnected
        | RemoteError::SsoAuthorizationRequired { .. }
        | RemoteError::OauthAppRestricted { .. } => e.into(),
        other => RemoteError::Refused(format!(
            "could not read the default branch of {repo} ({other}), so nothing was added; \
             name the branch to track (--branch on the command line, branch in add_domain \
             and in the JSON API) and try again"
        ))
        .into(),
    }
}

/// Derive a domain name from a folder's basename using the same slug rules as a
/// permalink, falling back to `domain` for a basename that slugifies to nothing
/// (a root path, or one made only of punctuation). Appends `-2`, `-3`... when
/// the name is already registered to a different path, so a derived name never
/// silently collides with an unrelated domain, and the name always passes
/// `validate_domain_name` (a folder called `CON` becomes `con-2`).
fn unique_domain_name(canonical: &Path, cfg: &GlobalConfig) -> String {
    let basename = canonical
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| canonical.display().to_string());
    crystalline_core::config::registration::derive_domain_name(&basename, |candidate| {
        name_taken_by_other(candidate, canonical, cfg)
    })
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
pub(crate) fn join_rel(root: &Path, rel: &str) -> PathBuf {
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
pub(crate) fn is_within_domain(rel: &str) -> bool {
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
pub(crate) fn is_contained_rel(rel: &str) -> bool {
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
pub(crate) fn validate_attachment_path(path: &str) -> Result<()> {
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

/// What an attachment write landed as: the row that now describes it, and
/// whether it is this actor's own draft rather than the domain's file.
///
/// `draft` is the whole of what review mode adds to an upload, so it rides out
/// on the receipt rather than being inferred from the domain's configuration by
/// whoever renders it.
#[derive(Debug, Clone)]
pub struct WrittenAttachment {
    /// The row describing the bytes as stored.
    pub row: AttachmentRow,
    /// Whether the bytes landed in the writer's own files overlay rather than
    /// in the folder the team reviewed.
    pub draft: bool,
}

/// The metadata row describing these bytes at this path. The mime comes from
/// the extension and never from a caller, which is why this cannot be built
/// before [`validate_attachment_path`] has accepted the path.
pub(crate) fn attachment_row(path: &str, bytes: &[u8], modified: String) -> Result<AttachmentRow> {
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

/// One engram whose references a move rewrites, found by
/// [`Engine::move_linkers`] and rewritten by [`Engine::relink_engram`].
#[derive(Debug)]
struct MoveLinker {
    /// Its domain's name.
    domain: String,
    /// Its domain's id.
    domain_id: DomainId,
    /// Its domain-relative path.
    path: String,
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

/// Whether the engram acknowledges `rule` at all, or - when `scope` names a
/// pair - acknowledges that pair.
///
/// Case-folded on the rule, like every other rule comparison on this path: a
/// hand-written `- { rule: v101 }` suppresses findings, so it has to be
/// findable - and withdrawable - too. The scope is compared exactly, being one
/// value the sweep renders rather than something anybody types.
fn has_ack(source: &str, rule: &str, scope: Option<&str>) -> bool {
    acks_of(source).iter().any(|a| ack_names(a, rule, scope))
}

/// Whether one entry is what `rule` and an optional `scope` name. The scope
/// half is what makes a withdrawal able to take one twin pair back and leave
/// the engram's other pair silenced; without one, every entry for the rule is
/// named.
///
/// An entry that carries no scope of its own - a hand-written line, or an
/// acknowledgment given before its rule fired - is never what a named pair
/// means: it was given for nothing in particular, so a request naming a pair
/// leaves it alone rather than removing an answer it did not ask about.
fn ack_names(entry: &EvolveAck, rule: &str, scope: Option<&str>) -> bool {
    entry.rule.eq_ignore_ascii_case(rule)
        && match scope {
            Some(scope) => entry.scope.as_deref() == Some(scope),
            None => true,
        }
}

/// The engram's markdown with the acknowledgment `rule` and an optional
/// `scope` name dropped, and every other entry left exactly as it was.
/// Removing the last one removes the key rather than leaving an empty one
/// ([`set_evolve_ack`] on an empty slice).
///
/// The one removal both surfaces run: Fluid's withdraw
/// ([`Engine::unacknowledge_finding_as`]) and an agent's `remove <rule-id>`
/// value. They differ in how they report an entry that is not there - a
/// `false` the REST layer answers as a 404, an error the agent reads - which is
/// why the presence test is [`has_ack`] beside this rather than folded into it,
/// and in that only the first can name a pair: the agent's value form carries a
/// rule id and nothing else, so it takes every entry the rule has.
fn without_ack(source: &str, rule: &str, scope: Option<&str>) -> String {
    let kept: Vec<EvolveAck> = acks_of(source)
        .into_iter()
        .filter(|a| !ack_names(a, rule, scope))
        .collect();
    set_evolve_ack(source, &kept)
}

/// The engram's acknowledgments with `entry` folded in: **one entry per rule,
/// except for a pair-scoped rule, which keeps one per pair**. Re-acknowledging
/// replaces what the entry said rather than stacking a second line nobody
/// reads. The replacement keeps the original position, which keeps a
/// hand-ordered list hand-ordered.
///
/// The exception is [`crystalline_index::is_pair_scoped`] - `V301` - and it
/// exists because a twin finding is about a pair rather than about the engram:
/// an engram that twins two others carries two twin findings and neither is the
/// engram's answer about the rule. Keying those by rule alone made the second
/// acknowledgment overwrite the first, which silenced one pair and left the
/// other standing with somebody else's note on it. Every other rule's entry
/// **is** that answer, so replacing it on re-acknowledgment is what keeps
/// exactly one entry there however often the evidence moves, and that in turn
/// is what lets a later drift come back marked stale (the sweep can only call
/// an entry stale when it is the only one for its rule). That is the rule even
/// for one that can fire more than once on an engram - `V103` fires once per
/// reciprocal pair - and the cost is deliberate: those findings share the one
/// entry, so the second acknowledgment replaces the first and the finding it
/// was not given for comes back stale wearing that note.
///
/// A scope-less entry - what a hand-written line or an acknowledgment given
/// before the rule fires carries - is a pair of its own under the pair-scoped
/// rule, and keeps matching whatever that rule finds.
fn merged_acks(source: &str, entry: EvolveAck) -> Vec<EvolveAck> {
    let per_pair = crystalline_index::is_pair_scoped(&entry.rule);
    let mut entries = acks_of(source);
    let mut replaced = false;
    entries.retain_mut(|existing| {
        if !existing.rule.eq_ignore_ascii_case(&entry.rule)
            || (per_pair && existing.scope != entry.scope)
        {
            return true;
        }
        // A hand-edited file may name one key twice; the entry just written is
        // the survivor and the rest go, so the list stays one entry per key.
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

/// The `assets/` path an engram's `analyzes` claim names, or `None` when it
/// claims nothing under the folder.
///
/// `analyzes` is ordinary custom frontmatter (the agent's act of claiming an
/// attachment it read), so the value is whatever was written there: a leading
/// `./` is stripped and the folder segment is folded to its canonical
/// spelling, and anything that does not address the reserved folder at all is
/// not a claim.
pub(crate) fn asset_claim(fm: &Frontmatter) -> Option<String> {
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
pub(crate) fn asset_modified(abs: &Path) -> String {
    let mtime = std::fs::metadata(abs)
        .map(|meta| mtime_secs(&meta))
        .unwrap_or_else(|_| Utc::now().timestamp());
    chrono::DateTime::from_timestamp(mtime, 0)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .to_rfc3339()
}

/// The miss message every attachment verb reports, one spelling.
pub(crate) fn missing_attachment(domain: &str, path: &str) -> String {
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

/// Put a mirror's failure on a draft's receipt, when there was one.
///
/// One field on every routed verb, so a caller learns the same thing the same
/// way whichever verb it called, and a receipt with no such field means the
/// draft is mirrored.
pub(crate) fn note_unmirrored(receipt: &mut Value, warning: Option<String>) {
    if let Some(text) = warning {
        receipt["draft_warning"] = json!(text);
    }
}

/// The sentence an edit receipt carries when a section edit dropped a repeat
/// of the section's own heading from its content.
pub const HEADING_STRIPPED_GUIDANCE: &str = "Content repeated the section's own heading; it was dropped, since the heading stays. Send only the body next time.";

/// The sentence an edit or write receipt carries when the MANIFEST it just
/// changed draws an error from the MANIFEST rules.
pub const MANIFEST_BROKEN_GUIDANCE: &str =
    "This MANIFEST now breaks routing for this domain; fix it before you move on.";

/// Add one sentence to a receipt's `guidance`, after whatever is there.
///
/// Appending rather than assigning because more than one part of the server
/// has something to tell the caller about one write - the engine about the
/// text it landed, the similar advisory about the engrams it found beside
/// it - and each of them runs without knowing about the others. An assignment
/// would let the last one to run silently erase what the first said.
pub(crate) fn add_guidance(receipt: &mut Value, sentence: &str) {
    let Value::Object(map) = receipt else {
        return;
    };
    let joined = match map.get("guidance").and_then(Value::as_str) {
        Some(existing) if !existing.is_empty() => format!("{existing} {sentence}"),
        _ => sentence.to_string(),
    };
    map.insert("guidance".to_string(), Value::String(joined));
}

/// Say on an edit receipt that the section edit dropped a repeated heading:
/// which heading, and one sentence on why and what to send next time.
///
/// The repeat is dropped rather than refused because nobody means it, and
/// said out loud because an agent that is only ever corrected in silence
/// learns nothing and keeps sending it.
fn note_heading_stripped(receipt: &mut Value, stripped: Option<String>) {
    if let Some(heading) = stripped {
        receipt["heading_stripped"] = json!(heading);
        add_guidance(receipt, HEADING_STRIPPED_GUIDANCE);
    }
}

/// Whether a domain-relative path is the domain's MANIFEST.
fn is_manifest_path(rel: &str) -> bool {
    rel == "MANIFEST.md"
}

/// Put the MANIFEST rules' findings about `text` on a receipt, when there are
/// any; a clean MANIFEST adds nothing, so its receipt is the one it always was.
///
/// **Why a receipt carries them.** A MANIFEST is the one engram whose text is
/// also configuration: its `## When to Use` bullets are what every session is
/// routed by. An edit that empties that section, or doubles its heading so
/// the reader keeps the empty first one, breaks routing for the whole domain,
/// and nothing said so - the rules that notice ran only when somebody ran
/// validate. The receipt is the moment the agent that did it is still there to
/// fix it. Nothing is refused: a MANIFEST in the middle of a rewrite may be
/// wrong for one step, and the finding is advice.
///
/// The rules are the core's own ([`crystalline_core::verify::check_manifest_source`]),
/// the ones validate runs, under the domain's own `.crystalline.yaml`
/// overrides, so the receipt and validate cannot disagree about one text.
/// With no domain root handed in: `M105` stats the provisioned folders on
/// disk, which says nothing about a draft, a live document or a virtual
/// domain, and validate still raises it.
fn note_manifest_findings(receipt: &mut Value, text: &str, verify: Option<&VerifyConfig>) {
    let issues = crystalline_core::verify::check_manifest_source(
        Path::new("MANIFEST.md"),
        text,
        None,
        verify,
    );
    if issues.is_empty() {
        return;
    }
    let breaks_routing = issues
        .iter()
        .any(|issue| issue.severity == crystalline_core::Severity::Error);
    receipt["manifest_findings"] = Value::Array(
        issues
            .into_iter()
            .map(|issue| {
                json!({
                    "code": issue.rule,
                    "severity": issue.severity,
                    "message": issue.message,
                    "line": issue.line,
                })
            })
            .collect(),
    );
    if breaks_routing {
        add_guidance(receipt, MANIFEST_BROKEN_GUIDANCE);
    }
}

/// What a draft's receipt says when the row landed and this machine could not
/// mirror it under the state directory.
///
/// One string for both shapes a draft's mirror can take: an ordinary write,
/// and a tombstone, which `deleted` tells apart. Teaching text rather than a
/// diagnostic: for a write, the draft is there and usable, the one thing that
/// would lose it is named, and so is the way to make the copy exist again.
/// For a tombstone a wipe does not lose an edit, it drops the tombstone row
/// itself and finds nothing in the journal to restore, so the base row comes
/// back - the deletion is reverted and the engram reappears - and "write it
/// again" is not a remedy there (the path already resolves as absent for its
/// author, so a second delete only answers "no engram"). The underlying error
/// rides along because the cause is almost always a state directory that is
/// not writable, which the reader can see and fix.
pub(crate) fn unmirrored(
    domain: &str,
    actor: &str,
    path: &str,
    reason: &std::io::Error,
    deleted: bool,
) -> String {
    if deleted {
        format!(
            "the deletion of '{path}' landed in the index, but this machine could not mirror it \
             under its state directory ({reason}), so a 'crystalline reindex --wipe' would drop \
             the tombstone and find nothing in the journal to restore, bringing the base row back \
             - the deletion is reverted and the engram reappears; make the overlays folder for \
             domain '{domain}' writable, or share the change while it is still here. Nobody but \
             '{actor}' can see it either way."
        )
    } else {
        format!(
            "the draft of '{path}' landed in the index, but this machine could not mirror it under \
             its state directory ({reason}), so a 'crystalline reindex --wipe' would lose it; make \
             the overlays folder for domain '{domain}' writable and write again to mirror it, or \
             share the change while it is still here. Nobody but '{actor}' can see it either way."
        )
    }
}

#[cfg(test)]
mod unmirrored_tests {
    use super::*;

    fn io_error() -> std::io::Error {
        std::io::Error::other("permission denied")
    }

    /// The tombstone shape names the reappearance on a wipe and never offers
    /// "write again", which is not a remedy for a deletion.
    #[test]
    fn the_tombstone_wording_names_reappearance_and_drops_write_again() {
        let text = unmirrored("jordi", "human:jordi", "notes/old", &io_error(), true);
        assert!(
            text.contains("reappears"),
            "tombstone wording must name the reappearance, got: {text}"
        );
        assert!(
            !text.contains("write again"),
            "tombstone wording must not offer to write again, got: {text}"
        );
        assert!(
            text.contains("share the change while it is still here"),
            "tombstone wording must still offer to share, got: {text}"
        );
    }

    /// The ordinary draft shape keeps offering "write again", which is the
    /// remedy a lost edit actually has.
    #[test]
    fn the_draft_wording_keeps_write_again() {
        let text = unmirrored("jordi", "human:jordi", "notes/old", &io_error(), false);
        assert!(
            text.contains("write again"),
            "draft wording must offer to write again, got: {text}"
        );
    }
}

/// A browse prefix as a lowercased folder prefix: empty for the root, and
/// otherwise ending in the slash that makes it a folder. The Rust counterpart
/// of the backends' own `folder_slash`, used to cut draft paths to the level
/// being browsed.
pub(crate) fn folder_slash_lower(prefix: &str) -> String {
    if prefix.is_empty() {
        return String::new();
    }
    let mut out = prefix.to_lowercase();
    if !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// The descriptor of one overlay entry, read out of the document the row
/// carries.
///
/// `None` for a tombstone, which is a deletion rather than an engram, and for a
/// row whose markdown no longer parses - a draft nobody can read is a draft
/// nothing can traverse either. The `id` is the draft row's OWN id, which is
/// the whole point: it is the key its observations, relations, links and chunks
/// hang off, so a traversal seeded with it walks the edges its author wrote.
pub(crate) fn overlay_descriptor(
    domain: &str,
    domain_id: crystalline_index::DomainId,
    entry: &crystalline_index::StoredEngram,
) -> Option<EngramDescriptor> {
    if entry.tombstone {
        return None;
    }
    let engram = parse_engram(&entry.content).ok()?;
    let record = EngramRecord::from_engram(&engram, &entry.path, virtual_stamp(&entry.content));
    Some(EngramDescriptor {
        id: entry.id,
        domain_id,
        domain: domain.to_string(),
        path: entry.path.clone(),
        permalink: entry.permalink.clone(),
        title: record.title,
        engram_type: record.engram_type,
        status: record.status,
    })
}

/// Describe a browse row by the draft standing at its path: the permalink,
/// title, type and status the reader's own document carries, so a listing says
/// what they would open rather than what the file says.
pub(crate) fn overwrite_from_draft(
    row: &mut EngramDescriptor,
    draft: &crystalline_index::StoredEngram,
) {
    row.permalink = draft.permalink.clone();
    row.id = draft.id;
    if let Ok(engram) = parse_engram(&draft.content) {
        let record = EngramRecord::from_engram(&engram, &draft.path, virtual_stamp(&draft.content));
        row.title = record.title;
        row.engram_type = record.engram_type;
        row.status = record.status;
    }
}

/// What one pull's convergence pass did to the drafts standing over a domain
/// that reviews changes before they land.
///
/// How many drafts ended because the folder now says what they said, and how
/// many stand unsettled against a base that moved under them.
/// `Engine::converge_pulled_overlays` is where each of the two is decided.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConvergenceReport {
    /// Entries taken out of the overlay, row and mirror together.
    pub cleared: u64,
    /// Entries left standing as their author's conflict.
    pub diverged: u64,
}

/// The proposal number a share receipt names, whichever outcome it carried: a
/// fresh proposal reports it at the top level, an update inside the `proposal`
/// it rewrote.
fn proposal_number_of(receipt: &Value) -> Option<u64> {
    receipt["number"]
        .as_u64()
        .or_else(|| receipt["proposal"]["number"].as_u64())
}

/// Drop every recorded conflict whose entry is no longer there: an actor who
/// stopped holding a row at a path has no conflict at it, and an actor holding
/// nothing at all has none anywhere.
///
/// `held` is every actor with rows in this domain, and their rows.
fn prune_settled_conflicts(
    record: &mut crate::overlay_journal::ConvergenceRecord,
    held: &[ActorHolding],
) {
    record.conflicts.retain(|actor, paths| {
        let Some(holding) = held.iter().find(|holding| &holding.actor == actor) else {
            return false;
        };
        // Both lists, or a file conflict this pass recorded would be pruned by
        // the next pull about something else: the entry IS still held, it is
        // simply not a row. That is the window one tick wide this function's
        // doc warns about, on the other kind of entry.
        paths.retain(|at| {
            holding.entries.iter().any(|entry| &entry.path == at)
                || holding.files.iter().any(|file| &file.path == at)
        });
        !paths.is_empty()
    });
}

/// Everything one actor holds in a domain's overlay: the rows they drafted and
/// the files beside them.
///
/// One struct rather than a pair, because every reader of it needs both halves
/// and a pair invited exactly the reading that dropped one of them.
struct ActorHolding {
    actor: String,
    entries: Vec<StoredEngram>,
    files: Vec<crate::overlay_files::FileEntry>,
}

/// What a pull leaves one overlay entry to be.
enum Settle {
    /// Untouched by this pull and still its author's draft.
    Leave,
    /// The folder has caught up with it: take it out, row and mirror.
    Clear,
    /// Its author has a conflict to settle.
    Diverge,
    /// The base it drafts moved to this path, so the draft goes with it.
    MoveTo(String),
}

/// What one pull leaves one entry to be. Pure, so the rule is readable in one
/// place and the engine above it only does the writing.
///
/// `base` is the entry path's own bytes in the base snapshot AFTER the pull,
/// `touched` the paths the pull applied, `addresses` the permalinks those
/// applied paths now answer to, and `own` every path this same actor is
/// holding, which is what keeps a rename from writing over a second draft.
fn settle_overlay_entry(
    entry: &StoredEngram,
    base: Option<&[u8]>,
    touched: &HashSet<&str>,
    addresses: &HashMap<String, String>,
    own: &HashSet<&str>,
) -> Settle {
    let at = entry.path.as_str();
    let pulled = touched.contains(at);
    if entry.tombstone {
        return match base {
            None => Settle::Clear,
            Some(_) if pulled => Settle::Diverge,
            Some(_) => Settle::Leave,
        };
    }
    // The address this draft holds, standing at a path the pull brought in -
    // and never at a path this actor is drafting at themselves, since no
    // rename may write over a second draft and no collision is invented
    // between two rows one person already holds apart.
    let elsewhere = addresses
        .get(&entry.permalink)
        .filter(|held| held.as_str() != at && !own.contains(held.as_str()));
    match base {
        Some(bytes) if bytes == entry.content.as_bytes() => Settle::Clear,
        Some(_) if pulled => Settle::Diverge,
        Some(_) => match elsewhere {
            Some(_) => Settle::Diverge,
            None => Settle::Leave,
        },
        None => match (pulled, elsewhere) {
            // The pull took the base away and the address it carried stands
            // somewhere else now: the page was renamed, and the draft applies
            // to it still.
            (true, Some(dest)) => Settle::MoveTo(dest.clone()),
            // The pull took the base away and nothing carries its address: the
            // team retired the page this draft is a draft of.
            (true, None) => Settle::Diverge,
            // Nothing happened at this path, but the address this draft holds
            // was just spent by a file the team has.
            (false, Some(_)) => Settle::Diverge,
            (false, None) => Settle::Leave,
        },
    }
}

/// What one pull leaves one overlay FILE to be. Pure, beside
/// [`settle_overlay_entry`] and answering the same four-way question with the
/// two arms that cannot apply to a file left out.
///
/// `base` is the path's own bytes in the base snapshot AFTER the pull, `bytes`
/// what this actor holds there (`None` for a deletion), and `touched` the paths
/// the pull applied.
///
/// A file answers to no address, so there is no rename for it to follow and no
/// address for another file to spend: the `MoveTo` and `elsewhere` arms of the
/// row rule have nothing to read here. What is left is the pair that matters -
/// the folder has caught up, or it has moved somewhere else.
fn settle_overlay_file(
    entry: &crate::overlay_files::FileEntry,
    base: Option<&[u8]>,
    bytes: Option<&[u8]>,
    touched: &HashSet<&str>,
) -> Settle {
    let pulled = touched.contains(entry.path.as_str());
    if entry.tombstone {
        return match base {
            // The team deleted it too, so the marker has nothing left to hide.
            None => Settle::Clear,
            Some(_) if pulled => Settle::Diverge,
            Some(_) => Settle::Leave,
        };
    }
    match (base, bytes) {
        // The folder holds exactly these bytes now: the draft is the team's
        // file, under any pull or none. Asked of every entry on every pass, so
        // an author who uploads the team's own version again has it cleared by
        // the next pass rather than left listed.
        (Some(base), Some(bytes)) if base == bytes => Settle::Clear,
        // The pull changed or removed what stood under this file, and what the
        // actor holds is not it.
        (_, _) if pulled => Settle::Diverge,
        _ => Settle::Leave,
    }
}

/// The addresses the base files a pull applied answer to, permalink to path.
///
/// Read from the base snapshot's own copies rather than the index rows, because
/// this runs before the sync that refreshes those rows - in a share it runs
/// well before it - and an address read from a stale row would answer a
/// question about the folder as it was.
fn pulled_addresses(state_dir: &Path, touched: &HashSet<&str>) -> Result<HashMap<String, String>> {
    let mut addresses = HashMap::new();
    for at in touched {
        let Some(bytes) = crystalline_remote::state::read_base_file(state_dir, at)? else {
            continue;
        };
        // A file that is not an engram at all - a README the team keeps beside
        // its knowledge - answers to no address and takes no part in this.
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Ok(engram) = parse_engram(text) else {
            continue;
        };
        let record = EngramRecord::from_engram(&engram, at, virtual_stamp(text));
        addresses.insert(record.permalink, (*at).to_string());
    }
    Ok(addresses)
}

/// A synthesized file stamp for a virtual write: the current epoch seconds, the
/// content byte length and its SHA-256. The sha doubles as the CAS token, so a
/// virtual engram gets the same `(mtime, size, sha256)` shape a file write would
/// without ever touching a filesystem.
pub(crate) fn virtual_stamp(content: &str) -> FileStamp {
    FileStamp {
        mtime: chrono::Utc::now().timestamp(),
        size: content.len() as u64,
        sha256: sha256_hex(content.as_bytes()),
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
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
    model: Option<&str>,
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
        model: model.map(str::to_string),
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
    // `normalize_verified` promotes a caller-supplied `metadata.verified`
    // into the typed field verbatim, model and all: it enforces the entry's
    // SHAPE, not the same actor rule the verb path applies through
    // `stamped_model` when this write stamps its OWN `generated.by`/model.
    // Run every entry through it here too, or a `human:` actor named in
    // `metadata.verified` keeps a model this same write would have dropped
    // had it arrived through the verb instead.
    for entry in &mut fm.verified {
        entry.model = stamped_model(&entry.by, entry.model.as_deref());
    }

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

/// What the daemon adds around each domain of a shared reindex run: the
/// collaboration host claim before a domain is touched, and a refresh of the
/// generated index files after one changed.
///
/// The daemonless CLI needs neither, which is why they are hooks rather than
/// part of [`crystalline_index::reindex_domains`] itself.
struct DaemonReindexHooks<'a> {
    engine: &'a Engine,
    collab: bool,
}

#[async_trait::async_trait]
impl ReindexHooks for DaemonReindexHooks<'_> {
    /// Claim the file-host lock in the driver's first lock window, with the
    /// driver's own store guard, so the claim and the domain's upsert stay in
    /// one window exactly as they were before the loop was shared. A domain
    /// held by another live instance is skipped: the driver neither clears nor
    /// scans it, so a non-host never rebuilds the host's rows out from under
    /// it.
    async fn before_domain(
        &self,
        store: &dyn Store,
        name: &str,
        root: &Path,
    ) -> crystalline_index::Result<bool> {
        if !self.collab {
            return Ok(true);
        }
        // The claim reaches the store through the engine's own helper, which
        // returns the engine's error type; the driver speaks the index crate's,
        // so a failure crosses over as its text. Every failure this call can
        // raise is a store failure to begin with.
        let claim = self
            .engine
            .claim_file_host(store, name, root, false)
            .await
            .map_err(|e| IndexError::Db(e.to_string()))?;
        match claim {
            HostClaim::Acquired => Ok(true),
            HostClaim::HeldByOther(host) => {
                tracing::info!(
                    "skipping reindex of '{name}' hosted by instance {}",
                    host.instance_id
                );
                Ok(false)
            }
        }
    }

    /// Files changed under us, so the generated index files follow. Runs with
    /// no store lock held, and takes none of its own.
    async fn after_apply(&self, name: &str, report: &SyncReport) {
        if changed_anything(report) {
            self.engine.refresh_index_files(name).await;
        }
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
            actor: String::new(),
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
                    share_link: None,
                    model: None,
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
            .save_engram(
                &SaveParams {
                    domain: "eng".to_string(),
                    identifier: "alpha".to_string(),
                    content: renamed.clone(),
                    expected_checksum: sha256_hex(original.as_bytes()),
                },
                &crate::scope::Scope::Unrestricted,
            )
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
            .save_engram(
                &SaveParams {
                    domain: "eng".to_string(),
                    identifier: "renamed".to_string(),
                    content: plain.clone(),
                    expected_checksum: sha256_hex(renamed.as_bytes()),
                },
                &crate::scope::Scope::Unrestricted,
            )
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
                .save_engram(
                    &SaveParams {
                        domain: "eng".to_string(),
                        identifier: "alpha".to_string(),
                        content: mine,
                        expected_checksum: expected,
                    },
                    &crate::scope::Scope::Unrestricted,
                )
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
            .start_github_identity_device_flow("alice", false)
            .await
            .expect("the flow starts and stays pending");
        assert!(
            matches!(
                engine.start_github_identity_device_flow("bob", false).await,
                Err(EngineError::ConnectInProgress)
            ),
            "a standing flow is what blocks the next one"
        );

        engine.forget_cached_credential(Some("alice")).unwrap();

        engine
            .start_github_identity_device_flow("bob", false)
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
                .start_github_identity_device_flow("ann+lee", false)
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
        let source = include_str!("origins.rs");
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

    /// The two organization-policy refusals are 403s upstream too, but the
    /// provider has already said what actually has to happen, and adding a
    /// collaborator clears neither. They reach the caller word for word.
    #[test]
    fn organization_policy_refusals_survive_personal_mode_unchanged() {
        let sso = RemoteError::SsoAuthorizationRequired {
            org: "acme".to_string(),
            url: "https://github.com/orgs/acme/sso?authorization_request=abc".to_string(),
        };
        let expected = sso.to_string();
        let enriched = enrich_write_error(sso, Some("alice"), "acme/knowledge");
        assert_eq!(enriched.to_string(), expected);
        assert!(
            !enriched.to_string().contains("ask a maintainer"),
            "{enriched}"
        );

        let restricted = RemoteError::OauthAppRestricted {
            org: "acme".to_string(),
        };
        let expected = restricted.to_string();
        let enriched = enrich_write_error(restricted, Some("alice"), "acme/knowledge");
        assert_eq!(enriched.to_string(), expected);
        assert!(
            !enriched.to_string().contains("ask a maintainer"),
            "{enriched}"
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

        let status = engine
            .origin_status(None, false, false, &crate::scope::Scope::Unrestricted)
            .await
            .unwrap();
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
        let status = engine
            .origin_status(None, false, false, &crate::scope::Scope::Unrestricted)
            .await
            .unwrap();
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
        let status = engine
            .origin_status(None, false, false, &crate::scope::Scope::Unrestricted)
            .await
            .unwrap();
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
        let status = engine
            .origin_status(None, false, false, &crate::scope::Scope::Unrestricted)
            .await
            .unwrap();
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
        let status = engine
            .origin_status(None, false, false, &crate::scope::Scope::Unrestricted)
            .await
            .unwrap();
        let agent = &status["connection"]["agent_identity"];
        assert_eq!(agent["account"], "share-bot");
        assert_eq!(agent["connected"], false, "nothing is on file for it yet");
        assert!(
            status["connection"]["owner_identity"]["account"].is_string(),
            "the owner's slot is untouched beside it: {status}"
        );

        write_token(&tokens, &personal("share-bot"), "bot-gh");
        let status = engine
            .origin_status(None, false, false, &crate::scope::Scope::Unrestricted)
            .await
            .unwrap();
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

        let status = engine
            .origin_status(None, false, false, &crate::scope::Scope::Unrestricted)
            .await
            .unwrap();
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
