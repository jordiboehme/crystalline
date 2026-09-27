//! `crystalline doctor`: diagnose the index, registered domains and service
//! state, optionally repairing what can be repaired automatically.
//!
//! Checks: (a) DB orphans, an indexed file whose path no longer exists on
//! disk; (b) files on disk that are not yet indexed; (c) encoding problems
//! (BOM or null bytes), which reuses `verify`'s `E006` rule rather than
//! re-implementing the check; (d) stale service artifacts, a lock file with a
//! dead pid or a socket file left behind by a killed daemon; (e) config
//! sanity, a registered domain whose path is missing or lacks a
//! `MANIFEST.md`; (f) an embedding staleness summary, the stored model
//! against the configured one, plus the cached model directories with sizes,
//! marking any the config does not use as stale (visible between a config
//! change and the next daemon start, which prunes them on a writable
//! instance running a local model); (g) when `github.enabled`, whether this
//! machine is connected to GitHub and, per team domain, whether its local
//! origin state is present and its base snapshot still matches what was
//! recorded (`verify_base`); (h) which `CRYSTALLINE_*` environment variables
//! are active, purely informational; (i) for Claude Code, Codex and Copilot,
//! whether the coding-harness integration `crystalline install` wires up
//! leaves any trace on disk and, when it does, whether its settings/hooks
//! file parses and carries the `SessionStart`, `Stop` and `UserPromptSubmit`
//! hooks (Copilot's `UserPromptSubmit` copy is reported present but noted
//! inert - a config-file prompt hook's output is dropped there) and how
//! many of the four managed skills are installed or locally modified (against the
//! install receipt when one exists) and whether a receipt version skew or
//! retired leftovers await the next session-start refresh - filesystem
//! only, with no shell-out to the harness's own CLI, so this check stays
//! fast and works offline; (j) when at least one registered domain declares
//! a `## Provisioning` section, every declaring domain's decision and
//! shipped artifact counts (plus, for a team domain with an out-of-subtree
//! declaration, whether its artifact mirror has been pulled down), every
//! installed harness's drift, locally edited, orphaned and missing counts
//! against the provisioning receipt, and every domain still awaiting a
//! decision -
//! entirely read-only, straight off `crystalline_core::provision::status`,
//! never reconciling anything itself; (k) domain names: a declared name
//! another domain holds here (shadowed), one several domains claim, an alias
//! that does not resolve, a team domain whose MANIFEST declares no name, a
//! derived domain still waiting to take its declared name, and links that
//! spell a domain by a name only this machine uses. `--fix` removes orphan
//! rows, the empty index rows removed domains left behind, and stale service
//! artifacts, and respells those links; the rest, including the whole
//! GitHub, environment, harnesses and provisioning sections, are
//! report-only, and every finding
//! that has a fix points at the right next command.
//!
//! The index reads are socket-first, the same shape `sync_dispatch` uses: a
//! running daemon holds the index file, so its stamps are asked for over ctl
//! and only a machine with no daemon (or an invocation an explicit
//! `--db`/`--config` sends down the direct path) opens the file here. When
//! neither route can read it, doctor does not abort: every check that does
//! not need the index still runs, [`DoctorReport::index`] names what stopped
//! the ones that do and what to do about it, and that counts as one
//! unresolved problem so the exit code still says something is wrong.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::{Result, anyhow};
use crystalline_core::config::{self, DatabaseBackend, DomainEntry, GlobalConfig, OriginConfig};
use crystalline_core::provision;
use crystalline_core::verify::{self, VerifyOptions};
use crystalline_core::{HarnessKind, harness_paths};
use crystalline_index::{
    FileStamp, Store, TagCluster, cached_model_dirs, configured_model_id, local_model,
    tag_clusters_with_aliases,
};
use crystalline_remote::TokenStore;
use crystalline_remote::github::auth::auth_base;
use crystalline_remote::state::{OriginState, verify_base};
use crystalline_service::EnvOverlay;
use crystalline_service::instance;
use serde::{Deserialize, Serialize};

use crate::cmd;
use crate::install;
use crate::receipt;

/// How `doctor` read the index this run.
///
/// Every index-backed check - orphan rows, unindexed files, a virtual
/// domain's engram count, the embedding summary, tag hygiene - needs one of
/// these routes to be open, and a diagnostic tool must not abort because
/// none of them was. The report therefore says which route it took, and the
/// human render reads the same field rather than guessing why a section is
/// thin.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum IndexAccess {
    /// No index file exists yet: a fresh install that has never synced.
    /// Neither an error nor a problem.
    #[default]
    Absent,
    /// Opened here, in this process. The route a machine with no running
    /// daemon takes, and the one an explicit `--db` or `--config` always
    /// takes (see [`crystalline_service::use_daemon`]).
    Direct,
    /// Served by the running daemon over its control socket, because that
    /// daemon holds the index file. Orphan rows and unindexed files come
    /// from its stamps and read exactly as they do on the direct path; the
    /// checks that need the open store itself (embedding coverage, tag
    /// hygiene, a virtual domain's engram count) sit this run out.
    Daemon,
    /// Neither route could read the index. Every check that does not need it
    /// still ran.
    Unavailable {
        /// What stopped the index-backed checks and what to do about it, as
        /// guidance a person can act on rather than a bare locking error.
        reason: String,
    },
}

/// One domain's diagnostics.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DomainDoctor {
    /// The domain name.
    pub name: String,
    /// The domain kind, `file` or `virtual`.
    pub kind: String,
    /// The domain's resolved root path, or `(virtual)` for a virtual domain.
    pub path: String,
    /// Whether this is a virtual (database-backed) domain, whose on-disk checks
    /// do not apply.
    pub is_virtual: bool,
    /// The database engram count, reported for a virtual domain in place of the
    /// on-disk orphan and unindexed checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engrams: Option<i64>,
    /// Whether this domain's index-backed checks ran at all: the orphan and
    /// unindexed sets below, and a virtual domain's engram count. False when
    /// no route to the index was open, in which case `orphans` and
    /// `unindexed` are empty because nothing was read, not because nothing
    /// was found - [`DoctorReport::index`] says why.
    pub index_checked: bool,
    /// Whether the path exists on disk.
    pub path_exists: bool,
    /// Whether `MANIFEST.md` is present at the root.
    pub manifest_present: bool,
    /// Indexed paths whose file no longer exists on disk.
    pub orphans: Vec<String>,
    /// How many of `orphans` were removed by `--fix`.
    pub orphans_removed: usize,
    /// On-disk `.md` files not yet present in the index. Holds only files that
    /// parse; a file whose frontmatter fails to parse is never merely
    /// unsynced, so it is reported under `unsyncable` instead.
    pub unindexed: Vec<String>,
    /// On-disk `.md` files that cannot be indexed at all, because their
    /// frontmatter fails to parse (`verify` rule `E001`). Running `sync`
    /// again never resolves these; the frontmatter itself needs a fix.
    pub unsyncable: Vec<UnsyncableFile>,
    /// Encoding problems, sourced from `verify`'s `E006` rule.
    pub encoding_issues: Vec<EncodingIssue>,
    /// MANIFEST policy keys - `generated_indexes`, `sharing` - whose declared
    /// value is not one the domain recognizes, each with the value it is read
    /// as. Empty when every declared policy parses and for a domain with no
    /// MANIFEST.
    pub policy_problems: Vec<PolicyProblem>,
    /// The instance currently hosting this file domain in a shared database, or
    /// `None` when unhosted (single-instance deployments, and virtual domains,
    /// which never take a host lock).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_instance_id: Option<String>,
    /// The host's last heartbeat, RFC 3339, when hosted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_heartbeat_at: Option<String>,
    /// When a forced rebuild of this domain was stamped as started, RFC 3339,
    /// or `None` when none is in flight. Read from the index, so a run served
    /// by the daemon leaves it absent rather than claiming there is none.
    ///
    /// A rebuild clears nothing, so a domain carrying this still holds its
    /// previous complete rows: the finding is that the refresh did not land,
    /// never that the data is gone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebuild_started: Option<String>,
    /// Which verb stamped [`DomainDoctor::rebuild_started`] - `full` or `wipe`
    /// - or `None` when none is in flight or the kind is not recorded.
    ///
    /// The two verbs leave opposite states behind, so the finding is worded
    /// from this: a forced rebuild destroyed nothing, a wipe destroyed every row
    /// and every embedding before it began.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebuild_kind: Option<String>,
}

/// One `E006` encoding finding, reported by `doctor`, fixed by `verify`.
#[derive(Debug, Clone, Serialize)]
pub struct EncodingIssue {
    /// The file path.
    pub path: String,
    /// The source line, when known.
    pub line: Option<usize>,
    /// The human message from `verify`.
    pub message: String,
}

/// A MANIFEST policy key whose declared value nobody recognizes, with what
/// the domain reads it as. Reported, never fixed: a policy is a decision.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyProblem {
    /// The frontmatter key, as `policy_registry` names it.
    pub key: String,
    /// The value the MANIFEST declares, quoted back as a reader wrote it.
    pub declared: String,
    /// The value the domain obeys instead.
    pub read_as: String,
}

/// One `E001` finding: a file whose frontmatter does not parse at all, so no
/// `sync` will ever index it until the frontmatter itself is fixed.
#[derive(Debug, Clone, Serialize)]
pub struct UnsyncableFile {
    /// The file path, relative to the domain root, forward-slashed.
    pub path: String,
    /// The human message from `verify`'s `E001` rule.
    pub message: String,
}

/// Service-level (not per-domain) diagnostics.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ServiceDoctor {
    /// Whether a lock file is present.
    pub lock_present: bool,
    /// The pid recorded in the lock file, when parseable.
    pub lock_pid: Option<u32>,
    /// Whether the lock is stale: present, its record names no live process
    /// (or names none at all), *and* the OS lock itself probes free. That
    /// last clause is load bearing - a lock with no readable record can still
    /// be held by something else entirely, and such a holder is the
    /// `holder_unknown` case, never this one, so `--fix` never deletes a
    /// lock file that is actually held.
    pub lock_stale: bool,
    /// Whether `--fix` removed the stale lock file.
    pub lock_removed: bool,
    /// Whether a socket (or, on Windows, pipe-backed) file is present.
    pub socket_present: bool,
    /// Whether the socket file is orphaned (present but no live owner).
    pub socket_orphaned: bool,
    /// Whether `--fix` removed the orphaned socket file.
    pub socket_removed: bool,
    /// Whether a live, verified crystalline daemon holds the index lock but
    /// answers nothing on its socket: the wedge no client can attach to or
    /// take over.
    pub daemon_unresponsive: bool,
    /// Whether `--fix` dislodged that wedged daemon.
    pub daemon_dislodged: bool,
    /// Set when the lock is held, nothing answers and the holder could not be
    /// identified: the reason, for a message that tells a person where to
    /// look. Nothing is ever signalled in this state, `--fix` included.
    pub holder_unknown: Option<String>,
}

/// One team domain's origin diagnostics: whether its local origin state is
/// present and, when it is, whether the base snapshot `verify_base` checks
/// against still matches what was recorded.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OriginDoctor {
    /// The domain name.
    pub name: String,
    /// The GitHub repository this domain tracks, `owner/name`.
    pub repo: String,
    /// Whether `state.json` is present for this domain. Absent means the
    /// origin state was lost or the domain was never fully connected.
    pub state_present: bool,
    /// Base snapshot paths that are missing or no longer match their
    /// recorded checksum, from `verify_base`. Empty when the base tree is
    /// fully intact, or when `state_present` is false (nothing to check).
    pub base_mismatches: Vec<String>,
    /// Whether this team domain is defined by an environment variable. An
    /// env-defined domain with no origin state yet is not a problem: it
    /// bootstraps itself when the daemon connects, so `remaining_problems`
    /// skips it where a config-file domain would count.
    pub env_defined: bool,
}

/// GitHub collaboration diagnostics, present only when `github.enabled` is
/// true (`doctor` skips the whole section rather than showing it empty
/// otherwise, matching how `embeddings` is `None` with no index yet).
#[derive(Debug, Clone, Default, Serialize)]
pub struct GithubDoctor {
    /// Whether a GitHub token is on file for this machine (or supplied by
    /// `CRYSTALLINE_GITHUB_TOKEN`).
    pub connected: bool,
    /// The connected user's login. `None` when not connected, and also for
    /// the environment token store, whose synthesized identity has no login
    /// attached (see `StoredToken::user_display`).
    pub user: Option<String>,
    /// Which backend holds (or would hold) the token: `"keyring"`, `"file"`
    /// or `"environment"`, from `TokenStore::kind`. Reported regardless of
    /// whether a token is actually saved yet, mirroring `origin_status`.
    pub token_store: String,
    /// Diagnostics for every domain connected to an origin (filtered by
    /// `--domain` like every other section).
    pub origins: Vec<OriginDoctor>,
}

/// One flat setting override from the environment, `(variable, key, value)`
/// reshaped into a record. Sourced from [`EnvOverlay::active_overrides`],
/// filtered to drop `domain.*` and `github.token` rows: those get the richer
/// dedicated [`EnvironmentDoctor::domains`] and
/// [`EnvironmentDoctor::github_token`] fields instead, so the flat list never
/// duplicates them. A credential-carrying key already arrives masked as
/// `"(set)"` (see `EnvOverlay::active_overrides`, which reads the registry's
/// secret flag).
#[derive(Debug, Clone, Serialize)]
pub struct EnvOverride {
    /// The environment variable, for example `CRYSTALLINE_DATABASE_BACKEND`.
    pub var: String,
    /// The settings registry key it overrides, for example
    /// `database.backend`.
    pub key: String,
    /// The overridden value, masked to `"(set)"` for a credential-carrying
    /// key.
    pub value: String,
}

/// One env-defined domain, from [`EnvOverlay::env_domains`].
#[derive(Debug, Clone, Serialize)]
pub struct EnvDomainReport {
    /// The variable that defined the domain, `CRYSTALLINE_DOMAIN_<NAME>`.
    pub var: String,
    /// The mapped domain name.
    pub name: String,
    /// The domain's root path.
    pub path: String,
    /// The attached GitHub origin, rendered `owner/repo[/subpath]@branch`,
    /// when a matching `_ORIGIN` variable was present.
    pub origin: Option<String>,
}

/// Which `CRYSTALLINE_*` environment variables are active, surfaced purely
/// for visibility: never counted as a problem, and never present at all
/// (`None`) when the environment overlay carries nothing (mirroring how
/// [`DoctorReport::github`] is absent when collaboration is off). No value
/// here is a secret: every credential-carrying key and the GitHub token are
/// masked exactly as [`EnvOverlay::active_overrides`] masks them, and the
/// token itself is reduced to a boolean.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EnvironmentDoctor {
    /// The `CRYSTALLINE_CONFIG` value, when set. A path, never a secret.
    pub config_path_var: Option<String>,
    /// Every active setting override, `domain.*` and `github.token` rows
    /// excluded (see [`EnvDomainReport`] and `github_token` below).
    pub overrides: Vec<EnvOverride>,
    /// Every env-defined domain.
    pub domains: Vec<EnvDomainReport>,
    /// Whether `CRYSTALLINE_GITHUB_TOKEN` is set. The value itself never
    /// appears anywhere in this report.
    pub github_token: bool,
}

/// One coding harness's onboarding trace: whether its settings/hooks file
/// exists, parses, carries our three managed hooks and how many of the four
/// managed skills are installed at its skills folder. Checked purely from
/// the filesystem, reusing `install`'s own presence predicate and skill
/// list, with no shell-out to the harness's own CLI (`claude`, `codex` or
/// `copilot`), so this stays fast and works offline; user scope only, since
/// doctor reports on the ambient environment rather than any one
/// repository's `--project` setup.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HarnessDoctor {
    /// The harness's stable identifier, `"claude-code"`, `"codex"` or
    /// `"copilot"` - the same spelling `crystalline install <name>` takes.
    pub name: String,
    /// The settings/hooks file this harness reads (`settings.json` for
    /// Claude Code, `hooks.json` for Codex).
    pub settings_path: String,
    /// Whether the settings file exists on disk.
    pub settings_present: bool,
    /// The parse error, when the file is present but is not valid JSON or
    /// not a JSON object. `None` when the file is absent or parses cleanly -
    /// the only field on this struct that
    /// [`DoctorReport::remaining_problems`] counts, since a harness that was
    /// simply never installed is not itself a problem.
    pub settings_parse_error: Option<String>,
    /// Whether the `SessionStart` routing hook is present, matcher-insensitive
    /// (a hand-written recipe counts, not only one `crystalline install`
    /// wrote).
    pub session_start_hook: bool,
    /// Whether the `Stop` capture-nudge hook is present.
    pub stop_hook: bool,
    /// Whether the `UserPromptSubmit` recall hook is present. Every harness
    /// has a prompt hook entry to check for as of the 2026-09-21 ruling
    /// (Copilot's copy included, though inert - see
    /// [`crate::install::prompt_hook_command`] and
    /// [`crate::install::prompt_hook_output_is_honoured`]), so `None` is
    /// reserved for a harness [`crate::install::prompt_hook_command`]
    /// answers `None` for (none today); a settings file that fails to parse
    /// reads as `Some(false)`, exactly like `session_start_hook` and
    /// `stop_hook` answer plain `false` for that same case, rather than as
    /// `None` - a corrupt file is "checked, found absent (we could not read
    /// it)", not "nothing to check here".
    pub prompt_hook: Option<bool>,
    /// How many of the four managed skills have a `SKILL.md` at this
    /// harness's skills folder, whether or not its content still matches the
    /// embedded copy.
    pub skills_installed: usize,
    /// How many of the skills counted in `skills_installed` were locally
    /// modified: present, but matching neither the embedded copy nor the
    /// install receipt's recorded hash for that name.
    pub skills_modified: usize,
    /// The binary version that last reconciled this harness's user-scope
    /// install, from the install receipt. `None` when no receipt entry
    /// exists (never installed, or installed before receipts existed).
    pub receipt_version: Option<String>,
    /// Leftover folders of skills this binary no longer ships: names the
    /// receipt or the retired list knows that still have a `SKILL.md` on
    /// disk. A re-run of `crystalline install` retires them.
    pub retired_leftovers: Vec<String>,
}

/// One domain's provisioning diagnostics, straight off
/// [`provision::status`]: its decision, how many artifacts of each kind it
/// ships and, for a team domain with at least one out-of-subtree
/// `Provisioning` declaration, whether its artifact mirror has been pulled
/// down from the origin yet. Only domains that declare a `Provisioning`
/// section at all appear here - a domain with nothing to ship has nothing to
/// report.
#[derive(Debug, Clone, Serialize)]
pub struct ProvisioningDomainDoctor {
    /// The domain name.
    pub name: String,
    /// `"allowed"`, `"denied"` or `"undecided"`.
    pub decision: String,
    /// [`crystalline_core::manifest::ArtifactType::id`] to how many
    /// artifacts of that kind the domain ships.
    pub counts: BTreeMap<String, usize>,
    /// Whether the artifact mirror is present at this team domain's origin
    /// state directory. `None` unless this is a team domain with at least
    /// one out-of-subtree declaration (a `../`-climbing path) - the only
    /// case a mirror is ever expected. The mirror itself is populated by the
    /// same poller that keeps the domain's engrams current, never by doctor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirror_present: Option<bool>,
}

/// One installed harness's provisioning diagnostics: installed counts read
/// straight from the receipt, drift and orphaned counts compared against
/// the harness's current desired set, and locally edited and missing counts
/// compared against the installed files themselves. Gated to installed
/// harnesses only, the same [`provision::installed_harnesses`] gate `apply`
/// and `status` use - a harness that was never onboarded has nothing to
/// compare against.
#[derive(Debug, Clone, Serialize)]
pub struct ProvisioningHarnessDoctor {
    /// The harness's stable identifier.
    pub harness: String,
    /// How many files the receipt records as installed for this harness.
    pub installed_files: usize,
    /// How many MCP servers the receipt records as installed for this
    /// harness.
    pub installed_mcps: usize,
    /// How many recorded rows (files and mcps together) have drifted: their
    /// domain now ships different bytes than the receipt last recorded, so
    /// the next reconcile would update them. Counted only, never
    /// reconciled here.
    pub drift: usize,
    /// How many installed files differ locally from the receipt's hash - a
    /// user's own edit since the last reconcile, left alone by design.
    pub edited: usize,
    /// How many recorded rows (files and mcps together) are orphaned:
    /// recorded but no longer part of what any opted-in domain would ship,
    /// whether that domain opted out, was removed from the config entirely
    /// or its manifest stopped declaring the artifact. The next reconcile
    /// retires them.
    pub orphaned: usize,
    /// How many installed files the receipt records but that are no longer
    /// on disk at the harness - deleted by hand since the last reconcile,
    /// which reinstalls them.
    pub missing: usize,
}

/// One domain still awaiting a provisioning decision, named with the counts
/// it would ship.
#[derive(Debug, Clone, Serialize)]
pub struct ProvisioningPendingDoctor {
    /// The domain name.
    pub domain: String,
    /// [`crystalline_core::manifest::ArtifactType::id`] to how many
    /// artifacts of that kind the domain would ship.
    pub counts: BTreeMap<String, usize>,
}

/// Provisioning diagnostics: every declaring domain's decision and shipped
/// counts, every installed harness's drift/edited/orphaned/missing counts
/// against the provisioning receipt, and every domain still awaiting a
/// decision. Read-only throughout, straight off [`provision::status`]:
/// never writes the receipt, never touches a harness's own config directory
/// and never spawns a harness CLI.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProvisioningDoctor {
    /// Every domain that declares a `Provisioning` section.
    pub domains: Vec<ProvisioningDomainDoctor>,
    /// Every installed harness's diagnostics.
    pub harnesses: Vec<ProvisioningHarnessDoctor>,
    /// Domains still awaiting a decision.
    pub pending: Vec<ProvisioningPendingDoctor>,
}

/// One domain the index still holds rows for and nobody registers any more:
/// what a 0.17.0 removal left behind, and what any row that outlives its
/// registration becomes.
///
/// Its rows are already unanswerable - search, counts and facets all skip a
/// domain this instance has no registration for - so this section is about
/// reclaiming the disk they sit on, never about what an answer contains.
#[derive(Debug, Clone, Serialize)]
pub struct OrphanedDomainDoctor {
    /// The domain name the index knows the rows under.
    pub name: String,
    /// The domain kind, `file` or `virtual`.
    pub kind: String,
    /// How many engram rows are at stake.
    pub engrams: i64,
    /// How long this domain has been absent from the configuration, in whole
    /// days. `None` when the index has never recorded it as registered, which
    /// is every row inherited from a version that did not stamp them: no age
    /// rather than an age of nothing.
    pub age_days: Option<i64>,
    /// Whether `--fix` would collect these rows, and did when this run
    /// carried it. False for the rows nothing collects: a virtual domain's,
    /// and every domain's on a read-only instance.
    pub collectable: bool,
    /// Whether this run actually collected them.
    pub collected: bool,
    /// Why a row that was kept was kept, in the engine's own word:
    /// `virtual`, `no_rows`, `grace`, `unstamped`, `read_only` or
    /// `hosted_elsewhere`. `None` when it was collected or would be. Only
    /// `virtual`, `read_only` and `hosted_elsewhere` can reach a `doctor`
    /// run, which asks on the on-demand path and consults no stamp, but the
    /// word is carried verbatim rather than narrowed: the render reads it
    /// through [`KeptReason`], whose vocabulary is the whole of the engine's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
    /// Whether this run dropped the domain's empty row itself, so its name is
    /// free for a rename or an adoption. Only `--fix` drops one: the row of a
    /// domain it just collected, or one an older version's `domain remove`
    /// left behind with nothing in it.
    pub row_dropped: bool,
    /// Whether a `--fix` run would drop the empty row. An empty row left
    /// behind is a problem until it is dropped: it holds the name against a
    /// rename or an adoption onto it.
    pub row_droppable: bool,
}

/// Rows whose domain nobody registers any more, and, when the whole check
/// declined, why.
///
/// `None` on [`DoctorReport::orphaned_rows`] when the check did not run at
/// all: a `--domain` run (an unregistered domain can never be the one named),
/// a machine with no index yet, or no route to the one it has.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OrphanedRowsDoctor {
    /// One entry per domain the index holds rows for and the configuration
    /// does not name. A domain whose rows are already gone is not listed:
    /// there is nothing at stake and nothing to do.
    pub domains: Vec<OrphanedDomainDoctor>,
    /// Why nothing was collected, when nothing could be: a read-only
    /// instance, or a configuration that could not be read (and a domain
    /// cannot be shown absent from a file nobody can read).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    /// What went wrong when the check could not be made at all: the daemon
    /// refused the request, or the index failed under it. Reported rather
    /// than swallowed, because an empty section and a section that could not
    /// be filled look identical to a reader and mean opposite things.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The engine's word for why a kept row was kept, as a closed set.
///
/// The render matches this exhaustively, so a word added here without a line
/// to print for it does not compile. A word the engine grows and this build
/// does not know parses as `None` and gets a sentence that stays true whatever
/// it turns out to mean - the one thing a render must never do is assert
/// something about a reason it cannot read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeptReason {
    /// A virtual domain's rows are its only copy.
    Virtual,
    /// Nothing left to collect.
    NoRows,
    /// Absent from the configuration, but not for long enough yet.
    Grace,
    /// Absent, and never recorded as registered, so its clock starts now.
    Unstamped,
    /// This instance is read-only.
    ReadOnly,
    /// Another instance holds this domain's host lock and is still serving it.
    HostedElsewhere,
}

impl KeptReason {
    /// The engine's word, or `None` for one this build does not know.
    fn from_word(word: &str) -> Option<KeptReason> {
        match word {
            "virtual" => Some(KeptReason::Virtual),
            "no_rows" => Some(KeptReason::NoRows),
            "grace" => Some(KeptReason::Grace),
            "unstamped" => Some(KeptReason::Unstamped),
            "read_only" => Some(KeptReason::ReadOnly),
            "hosted_elsewhere" => Some(KeptReason::HostedElsewhere),
            _ => None,
        }
    }
}

/// Advisory tag-hygiene diagnostics: near-duplicate tag clusters across the
/// whole index. Purely informational, the same stance provisioning takes:
/// never feeds [`DoctorReport::remaining_problems`], since consolidating tags is
/// a judgment call for a person, not a fault to fail on.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TagsDoctor {
    /// Near-duplicate tag clusters to consider merging.
    pub clusters: Vec<TagCluster>,
}

/// A domain whose declared name is another domain's local name here.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShadowedNameDoctor {
    /// The shadowed domain's local name.
    pub domain: String,
    /// The name its MANIFEST declares.
    pub canonical: String,
    /// The domain that answers to that name here.
    #[serde(default)]
    pub held_by: Option<String>,
}

/// A declared name more than one domain claims, none registered under it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NameConflictDoctor {
    /// The contested name.
    pub name: String,
    /// The local names of the domains that declare it, sorted.
    pub claimants: Vec<String>,
}

/// A machine-local alias that does not resolve because its spelling is taken.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DroppedAliasDoctor {
    /// The domain that lists the alias.
    pub domain: String,
    /// The alias.
    pub alias: String,
    /// The domain that owns the spelling, `None` when no single one does.
    #[serde(default)]
    pub held_by: Option<String>,
}

/// A team domain whose MANIFEST declares no `domain_name`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamNameDoctor {
    /// The local name this machine uses.
    pub domain: String,
    /// The repository it comes from.
    pub repo: String,
}

/// A derived domain whose declared name it has not taken on yet.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdoptionPendingDoctor {
    /// The local name it still has.
    pub domain: String,
    /// The name its MANIFEST declares.
    pub canonical: String,
    /// The declared name the last adoption recorded, if any.
    #[serde(default)]
    pub canonical_seen: Option<String>,
    /// Why the rename has not happened, when that is known.
    #[serde(default)]
    pub reason: Option<String>,
}

/// One engram's links that spell a domain by a name only this machine uses.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalSpellingDoctor {
    /// The domain the engram lives in.
    pub domain: String,
    /// The engram's domain-relative path.
    pub path: String,
    /// The spelling it uses: a local name or an alias.
    pub spelling: String,
    /// The domain's declared name, which `--fix` writes instead.
    pub canonical: String,
    /// How many links and URLs spell it that way.
    pub count: u64,
    /// Whether the fix already sits in the owner's draft of a domain that
    /// reviews changes: written, and waiting for review rather than for
    /// `--fix`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_draft: bool,
}

/// The domain name findings. Shadowed and contested names, dropped aliases,
/// team domains without a declared name and adoptions still waiting are
/// warnings and hints: they never feed [`DoctorReport::remaining_problems`].
/// Links spelled with a name only this machine uses are problems until
/// `--fix` respells them.
///
/// `None` on [`DoctorReport::names`] when the check did not run: a `--domain`
/// run, or no route to the index.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NamesDoctor {
    #[serde(default)]
    pub shadowed: Vec<ShadowedNameDoctor>,
    #[serde(default)]
    pub conflicts: Vec<NameConflictDoctor>,
    #[serde(default)]
    pub dropped_aliases: Vec<DroppedAliasDoctor>,
    #[serde(default)]
    pub team_without_domain_name: Vec<TeamNameDoctor>,
    #[serde(default)]
    pub adoption_pending: Vec<AdoptionPendingDoctor>,
    #[serde(default)]
    pub local_spellings: Vec<LocalSpellingDoctor>,
    /// How many links `--fix` respelled, present only on a `--fix` run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<u64>,
    /// Why `--fix` could not respell them (a read-only instance), when it
    /// could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_error: Option<String>,
    /// What went wrong when the check could not be made at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The full `doctor` report.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DoctorReport {
    /// How the index was read this run, and, when it could not be, why.
    pub index: IndexAccess,
    /// Per-domain diagnostics.
    pub domains: Vec<DomainDoctor>,
    /// Service lock and socket diagnostics.
    pub service: ServiceDoctor,
    /// Which `CRYSTALLINE_*` environment variables are active, `None` when
    /// none are.
    pub environment: Option<EnvironmentDoctor>,
    /// GitHub collaboration diagnostics, `None` when `github.enabled` is
    /// false.
    pub github: Option<GithubDoctor>,
    /// Embedding staleness summary, `None` when there is no index yet.
    pub embeddings: Option<serde_json::Value>,
    /// The contradiction check: the configured profile, its model, whether
    /// the model is downloaded, pending and failing pairs and a load failure
    /// when a running daemon answered `status`, and NLI checkpoints no
    /// profile uses now. Read from config and the model cache alone, so it is
    /// there whatever route the index took, even under a running daemon
    /// (plan correction 15: doctor never reads the index for this). Never a
    /// problem: `remaining_problems` is unchanged by it.
    pub contradictions: Option<serde_json::Value>,
    /// Onboarding trace for the Claude Code and Codex integrations
    /// `crystalline install` wires up. `None` when neither harness leaves
    /// any trace on disk at all: no settings/hooks file and no managed
    /// skill installed.
    pub harnesses: Option<Vec<HarnessDoctor>>,
    /// Provisioning diagnostics. `None` when no registered domain declares a
    /// `Provisioning` section at all.
    pub provisioning: Option<ProvisioningDoctor>,
    /// Rows whose domain nobody registers any more. `None` when the check
    /// did not run: a `--domain` run, no index yet, or no route to it.
    pub orphaned_rows: Option<OrphanedRowsDoctor>,
    /// Advisory tag-hygiene diagnostics. `None` when there is no index yet;
    /// present (possibly with an empty cluster list) once one exists.
    pub tags: Option<TagsDoctor>,
    /// Domain name findings. `None` when the check did not run: a `--domain`
    /// run, or no route to the index.
    pub names: Option<NamesDoctor>,
    /// A domain rename an earlier run left half done. `None` when no rename
    /// journal waits in the state directory.
    pub rename: Option<RenameDoctor>,
    /// Whether this report was produced with `--fix`.
    pub fix: bool,
}

/// A rename journal waiting in the state directory: a domain rename an
/// earlier run left half done.
///
/// A journal that belongs to this machine's own index and configuration is
/// finished by the daemon when it starts and by the next plain command.
/// One recorded against another spelling of them (the database url or the
/// configuration path written differently since) is finished by nobody on
/// its own: the report names the command that finishes it, with the index
/// and configuration the journal recorded, and `--discard-rename` is the
/// explicit way to drop it. `--fix` never touches it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RenameDoctor {
    /// The local name the domain had.
    pub old: String,
    /// The local name it gets.
    pub new: String,
    /// Whether the rename leaves the MANIFEST and links alone.
    pub local_only: bool,
    /// The steps the rename already ran, in order.
    pub done: Vec<String>,
    /// The steps still to run, in order.
    pub remaining: Vec<String>,
    /// The index, configuration and state directory the journal records;
    /// `None` for a journal that does not say.
    pub owner: Option<String>,
    /// This machine's own index, configuration and state directory.
    pub this_machine: Option<String>,
    /// Whether the journal belongs to this machine's own index and
    /// configuration, so the daemon or the next plain command finishes it.
    pub belongs_here: bool,
    /// What to put back first, for a journal recorded against another
    /// spelling of this machine's configuration or database: the journal is
    /// finished only against this machine's own, so the spelling it recorded
    /// has to become this machine's own again.
    pub restore: Vec<String>,
    /// The command that finishes the rename, once `restore` is done; `None`
    /// when the journal can only be dropped with `--discard-rename`.
    pub finish: Option<String>,
    /// Whether `--discard-rename` deleted the journal in this run.
    pub discarded: bool,
    /// Why the journal could not be read or discarded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DoctorReport {
    /// Problems still unresolved after any `--fix` pass. `doctor` exits 1
    /// when this is nonzero, 0 otherwise.
    pub fn remaining_problems(&self) -> usize {
        let mut n = 0;
        // An index nobody could read is one problem, counted once for the
        // machine rather than once per domain: the cause is shared, and the
        // per-domain `index_checked` flags only record which checks it took
        // down with it. Counting it at all is what keeps the exit code
        // honest, since a partial report otherwise looks like a clean one.
        if matches!(self.index, IndexAccess::Unavailable { .. }) {
            n += 1;
        }
        for d in &self.domains {
            if !d.path_exists || !d.manifest_present {
                n += 1;
            }
            // A rebuild that never finished is a problem a person finishes by
            // re-running it: the domain is serving complete rows, but they are
            // the ones from before the rebuild, and nothing clears the marker
            // on its own.
            if d.rebuild_started.is_some() {
                n += 1;
            }
            n += d.orphans.len().saturating_sub(d.orphans_removed);
            n += d.unindexed.len();
            n += d.unsyncable.len();
            n += d.encoding_issues.len();
        }
        if self.service.lock_stale && !self.service.lock_removed {
            n += 1;
        }
        if self.service.socket_orphaned && !self.service.socket_removed {
            n += 1;
        }
        // A wedged daemon blocks every client until it is replaced, and a lock
        // held by something unidentifiable blocks them with no automatic way
        // out at all. Both are problems until they are gone.
        if self.service.daemon_unresponsive && !self.service.daemon_dislodged {
            n += 1;
        }
        if self.service.holder_unknown.is_some() {
            n += 1;
        }
        // Not being connected to GitHub is not itself a problem (an
        // unconnected machine is a normal, expected state); missing or
        // corrupt origin state for an already-connected team domain is.
        if let Some(g) = &self.github {
            for o in &g.origins {
                // An env-defined team domain with no origin state bootstraps
                // itself when the daemon connects, so it is not a problem; a
                // config-file domain with no state genuinely is.
                if !o.state_present && !o.env_defined {
                    n += 1;
                }
                n += o.base_mismatches.len();
            }
        }
        // A harness that was never installed is not a problem; an
        // unparseable settings/hooks file for one that was is. A receipt
        // version skew and a retired leftover skill are never counted here:
        // both self-heal, the skew at the next session start and the
        // leftover at the next `crystalline install`, so neither should fail
        // doctor's exit code on a machine that fixes itself.
        if let Some(harnesses) = &self.harnesses {
            n += harnesses
                .iter()
                .filter(|h| h.settings_parse_error.is_some())
                .count();
        }
        // Rows whose domain is gone count only while something can be done
        // about them: a collectable set nobody has collected yet. A virtual
        // domain's rows, and every row on a read-only instance, are reported
        // and never counted - no `--fix` collects them, so counting them
        // would fail doctor forever over a state that has no remedy here.
        // An empty row left behind is a warning, not counted: it only holds
        // a name against a rename or an adoption, and the line says `--fix`
        // drops it.
        if let Some(o) = &self.orphaned_rows {
            n += o
                .domains
                .iter()
                .filter(|d| d.collectable && !d.collected)
                .count();
        }
        // A link spelled with a name only this machine uses breaks for every
        // colleague who reads it, and `--fix` respells it: one problem per
        // file until then. A file whose fix already waits in a review draft
        // is done as far as `--fix` goes. The other name findings are
        // warnings and hints.
        if let Some(names) = &self.names {
            let files: HashSet<(&str, &str)> = names
                .local_spellings
                .iter()
                .filter(|s| !s.in_draft)
                .map(|s| (s.domain.as_str(), s.path.as_str()))
                .collect();
            n += files.len();
        }
        // A rename journal nobody finishes on their own: it was recorded
        // against another index or configuration than the ones this machine
        // opens now, and its domain stays half moved until a person runs the
        // command the report names or discards it.
        if let Some(r) = &self.rename
            && !r.discarded
            && (!r.belongs_here || r.error.is_some())
        {
            n += 1;
        }
        // Provisioning never contributes here, the same stance environment
        // takes: an undecided domain is a normal state awaiting a person's
        // answer, and drift, edited and orphaned rows all self-heal at the
        // next `crystalline provision` (edited rows are left alone by
        // design, never "fixed").
        n
    }
}

/// Run every check, applying fixes when `fix` is set.
pub async fn run(
    domain_filter: Option<&str>,
    fix: bool,
    discard_rename: bool,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
) -> Result<DoctorReport> {
    // The single load chokepoint: the effective config drives every target and
    // the db factory. The whole `LoadedConfig` stays in scope so a later
    // milestone can surface the environment overlay in the report.
    let loaded = cmd::load(config_override)?;
    let cfg = &loaded.effective;
    let targets = select_domains(cfg, domain_filter)?;
    let db = cmd::db_path(db_override)?;

    // Ahead of the store, deliberately. A wedged daemon holds the index
    // database as well as the service lock, so opening the store first would
    // fail with a locking error before `--fix` ever got the chance to dislodge
    // the process causing it - the one state where doctor matters most.
    let service = check_service(fix).await?;

    // Ahead of doctor's own store, and deliberately: the collection needs the
    // index too, and it asks the daemon first or opens the file itself. Doing
    // it while doctor holds its own handle (and its own lock, taken a few
    // lines below and held to the end of the pass) would be a second opener
    // of the same file waiting on the first.
    let orphaned_rows = check_orphaned_rows(
        domain_filter,
        fix,
        config_override,
        db_override,
        &db,
        cfg.database().backend,
    )
    .await;

    // After the collection, which frees the names of the rows it drops, and
    // before doctor's own store for the same reason the collection comes
    // first: the names check asks the daemon or opens the index itself.
    let names = check_names(
        domain_filter,
        fix,
        config_override,
        db_override,
        &db,
        cfg.database().backend,
    )
    .await;

    // The index read, socket-first, in the same shape `sync_dispatch` uses. A
    // healthy daemon holds the index file, so asking it for the stamps is the
    // only way the ordinary case (a diagnosis run while the service is up)
    // gets a report at all. Probed after `check_service` and never before it:
    // a `--fix` that just dislodged a wedged holder has already run, so this
    // answer is the current one and the ordering above is preserved.
    let bypassed = !crystalline_service::use_daemon(db_override, config_override);
    let mut daemon_stamps = if bypassed {
        None
    } else {
        daemon_file_stamps(domain_filter).await
    };

    let mut index = IndexAccess::Absent;
    let mut store = None;
    if daemon_stamps.is_some() {
        index = IndexAccess::Daemon;
    } else if db.is_file() {
        match crystalline_index::open_store(&cfg.database(), Some(&db), false).await {
            Ok(opened) => {
                index = IndexAccess::Direct;
                store = Some(opened);
            }
            // Not an abort. Every check that does not need the index still
            // runs below, and the reason travels in the report as guidance.
            Err(e) => {
                index = IndexAccess::Unavailable {
                    reason: index_unavailable_reason(&db, &e.to_string(), &service, bypassed),
                };
            }
        }
    }
    // Lock once for the whole diagnostic pass: a one-shot CLI command has no
    // concurrent store users, and the helpers take a plain `&dyn Store`.
    let guard = match &store {
        Some(s) => Some(s.lock().await),
        None => None,
    };
    let store_ref: Option<&dyn Store> = guard.as_ref().map(|g| &**g as &dyn Store);

    // The rebuild markers, read once for the whole run rather than per domain.
    // Only the direct route can read them: the daemon's doctor answer carries
    // file stamps and nothing else, so a daemon-served run leaves the field
    // absent rather than reporting a rebuild that is not there.
    let mut rebuild_markers: HashMap<String, (String, Option<String>)> = HashMap::new();
    if let Some(store) = store_ref
        && let Ok(stats) = store.domain_stats().await
    {
        for d in stats {
            if let Some(started) = d.rebuild_started {
                rebuild_markers.insert(d.name, (started, d.rebuild_kind));
            }
        }
    }

    let mut domains = Vec::with_capacity(targets.len());
    for (name, entry) in &targets {
        // Taken out of the daemon's answer rather than borrowed, so each
        // domain's stamps are consumed once. A file domain the daemon
        // answered for but has no rows for reads as an empty set, which is
        // exactly what the direct path produces for an unsynced domain.
        let stamps = daemon_stamps
            .as_mut()
            .map(|by_domain| by_domain.remove(name).unwrap_or_default());
        domains.push(
            check_domain(
                name,
                entry,
                store_ref,
                stamps,
                rebuild_markers.get(name).cloned(),
                fix,
            )
            .await?,
        );
    }

    let environment = check_environment(&loaded.overlay);

    let github = if cfg.github_enabled() {
        Some(check_github(cfg, &loaded.overlay, &targets)?)
    } else {
        None
    };

    let embeddings = match store_ref {
        Some(store) => Some(embedding_summary(store, cfg).await?),
        None => None,
    };

    // Only when a daemon already answered this run's file stamps: reusing
    // that daemon's own numbers is one extra round trip on the route that
    // already has one, never a second blind probe on the direct or absent
    // routes, and the contradictions row never opens the index itself (plan
    // correction 15).
    let daemon_status = if matches!(index, IndexAccess::Daemon) {
        crystalline_service::ctl_if_running(serde_json::json!({ "v": 1, "cmd": "status" }))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let contradictions = Some(contradiction_summary(cfg, daemon_status.as_ref()));

    let harnesses = check_harnesses();

    let provisioning = check_provisioning(cfg, &loaded.overlay, &targets)?;

    let tags = check_tags(store_ref, cfg).await?;

    let rename = check_rename_journal(discard_rename);

    Ok(DoctorReport {
        index,
        domains,
        service,
        environment,
        github,
        embeddings,
        contradictions,
        harnesses,
        provisioning,
        orphaned_rows,
        tags,
        names,
        rename,
        fix,
    })
}

/// The rename journal waiting in this machine's state directory, compared
/// with this machine's own index and configuration; with `discard`, deleted
/// (only while this process can take the state directory, so a daemon
/// running the journal is never cut off). Never an error: a journal that
/// cannot be read is reported as such.
fn check_rename_journal(discard: bool) -> Option<RenameDoctor> {
    let state_dir = config::state_dir().ok()?;
    let pending = match crystalline_service::pending_rename(&state_dir) {
        Ok(Some(pending)) => pending,
        Ok(None) => return None,
        Err(e) => {
            return Some(RenameDoctor {
                error: Some(e.to_string()),
                ..RenameDoctor::default()
            });
        }
    };
    let here = crystalline_service::machine_rename_owner();
    let mut report = rename_doctor(&pending, here.as_ref().ok());
    if let Err(e) = &here {
        report.error = Some(format!(
            "this machine's own index could not be named: {e:#}"
        ));
    }
    if discard {
        match crystalline_service::discard_rename_journal() {
            Ok(_) => report.discarded = true,
            Err(e) => report.error = Some(format!("{e:#}")),
        }
    }
    Some(report)
}

/// [`RenameDoctor`] for `pending`, set against `here`, this machine's own
/// index, configuration and state directory when they could be named.
fn rename_doctor(
    pending: &crystalline_service::PendingRename,
    here: Option<&crystalline_service::RenameOwner>,
) -> RenameDoctor {
    // Both sides spelled canonically as of now, so a journal or an owner
    // recorded before a file existed compares as the engine compares it.
    let recorded = pending.owner.as_ref().map(|o| o.normalized());
    let here_now = here.map(|h| h.normalized());
    let here = here_now.as_ref();
    let belongs_here = matches!((&recorded, here), (Some(owner), Some(here)) if owner == here);
    let local = if pending.local_only { " --local" } else { "" };
    let plain = format!(
        "crystalline domain rename {} {}{local}",
        pending.old, pending.new
    );
    // Only this machine's own index and configuration ever finish a
    // journal, so one recorded against another spelling of them is finished
    // by putting that spelling back, never by a command pointed at it. What
    // cannot be put back that way (another state directory, or a Turso file
    // that is not this state directory's own index) can only be dropped.
    let (finish, restore) = if belongs_here {
        (Some(plain), Vec::new())
    } else {
        match (&recorded, here) {
            (Some(owner), Some(here)) if owner.state_dir == here.state_dir => {
                let mut restore = Vec::new();
                let mut restorable = true;
                if owner.config != here.config {
                    match &owner.config {
                        Some(config) => restore.push(format!(
                            "point CRYSTALLINE_CONFIG at {config}, or put the configuration back \
                             there"
                        )),
                        None => restorable = false,
                    }
                }
                if owner.index != here.index {
                    match owner.index.as_deref() {
                        // A Postgres index is named by its host and database,
                        // which the configuration's url chooses.
                        Some(index) if !Path::new(index).is_absolute() => restore.push(format!(
                            "set database.url (or CRYSTALLINE_DATABASE_URL) back to the database \
                             at {index}"
                        )),
                        _ => restorable = false,
                    }
                }
                if restorable {
                    (Some(plain), restore)
                } else {
                    (None, Vec::new())
                }
            }
            _ => (None, Vec::new()),
        }
    };
    RenameDoctor {
        old: pending.old.clone(),
        new: pending.new.clone(),
        local_only: pending.local_only,
        done: pending.done.iter().map(|s| s.name().to_string()).collect(),
        remaining: pending
            .remaining
            .iter()
            .map(|s| s.name().to_string())
            .collect(),
        owner: pending.owner.as_ref().map(|o| o.describe()),
        this_machine: here.map(|h| h.describe()),
        belongs_here,
        restore,
        finish,
        discarded: false,
        error: None,
    }
}

/// The rename journal section of the human report.
fn render_rename(out: &mut String, r: &RenameDoctor) {
    use std::fmt::Write as _;
    let _ = writeln!(out, "rename:");
    if r.old.is_empty() {
        if let Some(error) = &r.error {
            let _ = writeln!(out, "  [problem] {error}");
        }
        return;
    }
    let steps = |list: &[String]| {
        if list.is_empty() {
            "none".to_string()
        } else {
            list.join(", ")
        }
    };
    let what = format!(
        "the rename of '{}' to '{}' is half done: done {}; still to run {}",
        r.old,
        r.new,
        steps(&r.done),
        steps(&r.remaining)
    );
    if r.discarded {
        let _ = writeln!(
            out,
            "  [discarded] {what}. The journal is gone; the steps already done stay as they are, \
             so finish or undo them by hand"
        );
        return;
    }
    if r.belongs_here {
        let _ = writeln!(
            out,
            "  [warning] {what}. The daemon finishes it when it starts, and so does the next \
             command run without --db and --config"
        );
        let _ = writeln!(
            out,
            "  crystalline doctor --discard-rename would drop it instead and leave it half moved"
        );
    } else {
        let _ = writeln!(
            out,
            "  [problem] {what}. It belongs to {}, and this machine now opens {}, so nothing \
             finishes it on its own",
            r.owner.as_deref().unwrap_or("no recorded index"),
            r.this_machine
                .as_deref()
                .unwrap_or("an index that could not be named")
        );
        for step in &r.restore {
            let _ = writeln!(out, "  first: {step}");
        }
        if let Some(finish) = &r.finish {
            let _ = writeln!(out, "  run: {finish}");
        }
        let _ = writeln!(
            out,
            "  or drop it with: crystalline doctor --discard-rename (the steps already done stay \
             as they are)"
        );
    }
    if let Some(error) = &r.error {
        let _ = writeln!(out, "  [problem] {error}");
    }
}

/// The domain name findings, asked of the daemon that owns the index and
/// otherwise of a directly opened one, the same shape
/// [`check_orphaned_rows`] takes; with `fix`, the links spelled with a name
/// only this machine uses are respelled first, through the ordinary write
/// path.
///
/// Skipped on a `--domain` run for the reason the orphan check is: the fix
/// respells links in every domain, which would act on domains the reader did
/// not name. Never an error: a check that could not be made says why.
async fn check_names(
    domain_filter: Option<&str>,
    fix: bool,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    db: &Path,
    backend: DatabaseBackend,
) -> Option<NamesDoctor> {
    if domain_filter.is_some() {
        return None;
    }
    let daemon_may_answer = crystalline_service::use_daemon(db_override, config_override)
        && instance::read_lock_info().is_some_and(|i| instance::process_alive(i.pid));
    if !orphan_check_has_a_route(
        daemon_may_answer,
        matches!(backend, DatabaseBackend::Turso),
        db.is_file(),
    ) {
        return None;
    }
    let report = match crystalline_service::name_report(fix, db_override, config_override).await {
        Ok(report) => report,
        Err(e) => {
            return Some(NamesDoctor {
                error: Some(format!("{e:#}")),
                ..NamesDoctor::default()
            });
        }
    };
    match serde_json::from_value::<NamesDoctor>(report) {
        Ok(names) => Some(names),
        Err(e) => Some(NamesDoctor {
            error: Some(format!("the name report did not parse: {e}")),
            ..NamesDoctor::default()
        }),
    }
}

/// Rows whose domain nobody registers any more, asked of the daemon that owns
/// the index and otherwise read from the index directly - the same
/// daemon-first shape the file stamps above take, with the difference that
/// this one *writes* when `fix` is set, and can, because the daemon that owns
/// the index does the writing.
///
/// The grace period an unattended sweep waits out is never applied here: a
/// person running `doctor` is the signal it waits for, so an index inherited
/// from a version that stranded its rows clears on the first `--fix` rather
/// than a week after it.
///
/// Never an error. A daemon that did not answer and an index that would not
/// open both leave the section out, and [`DoctorReport::index`] says why in
/// the run's own words.
async fn check_orphaned_rows(
    domain_filter: Option<&str>,
    fix: bool,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    db: &Path,
    backend: DatabaseBackend,
) -> Option<OrphanedRowsDoctor> {
    // A `--domain` run answers about the domain it names, and an unregistered
    // one can never be that: `select_domains` resolves registered names only.
    // Collecting here would act on domains the reader did not name.
    if domain_filter.is_some() {
        return None;
    }
    let daemon_may_answer = crystalline_service::use_daemon(db_override, config_override)
        && instance::read_lock_info().is_some_and(|i| instance::process_alive(i.pid));
    if !orphan_check_has_a_route(
        daemon_may_answer,
        matches!(backend, DatabaseBackend::Turso),
        db.is_file(),
    ) {
        return None;
    }
    let report =
        match crystalline_service::collect_orphaned_domains(!fix, db_override, config_override)
            .await
        {
            Ok(report) => report,
            // Said out loud rather than dropped. A daemon that refused the
            // request and an index that failed under it both land here, and
            // an absent section would be indistinguishable from a machine
            // with nothing to report.
            Err(e) => {
                return Some(OrphanedRowsDoctor {
                    error: Some(format!("{e:#}")),
                    ..OrphanedRowsDoctor::default()
                });
            }
        };
    let dry_run = report.get("dry_run").and_then(serde_json::Value::as_bool) != Some(false);
    let mut domains = Vec::new();
    for row in report
        .get("considered")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let kept = row.get("kept").and_then(serde_json::Value::as_str);
        let row_dropped = row.get("row_dropped").and_then(serde_json::Value::as_bool) == Some(true);
        let row_droppable = row
            .get("row_droppable")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        // A domain whose rows are already gone has nothing at stake, and is
        // reported only for its empty row: one `--fix` drops (or just
        // dropped) so the name is free again. A row nothing here may drop (a
        // journal that could not be read, a read-only instance) would be a
        // line that never goes away, so it is left out.
        if kept == Some("no_rows") && !row_dropped && !row_droppable {
            continue;
        }
        let collectable = row.get("collected").and_then(serde_json::Value::as_bool) == Some(true);
        domains.push(OrphanedDomainDoctor {
            name: row
                .get("domain")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            kind: row
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("file")
                .to_string(),
            engrams: row
                .get("engrams")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
            age_days: row.get("age_days").and_then(serde_json::Value::as_i64),
            collectable,
            // On a preview `collected` is what a real run would take; only a
            // run that was allowed to write actually took it.
            collected: collectable && !dry_run,
            kept: kept.map(str::to_string),
            row_dropped,
            row_droppable,
        });
    }
    Some(OrphanedRowsDoctor {
        domains,
        skipped: report
            .get("skipped")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        error: None,
    })
}

/// Whether the orphan check has any route to the index worth trying.
///
/// The file question is the narrow one it looks like: a Turso install that has
/// never synced has no index file, and the direct route would create an empty
/// one to discover that a machine with no knowledge in it has no orphaned
/// rows. It says nothing about the other two shapes. A daemon answers over its
/// socket whatever backend it serves, and a Postgres install keeps its index
/// in a server rather than in that file, so gating either on the file would
/// suppress the section on an install that has plenty to report.
fn orphan_check_has_a_route(daemon_may_answer: bool, file_backed: bool, db_exists: bool) -> bool {
    daemon_may_answer || !file_backed || db_exists
}

/// The per-domain file stamps a running daemon serves over its ctl socket,
/// keyed by domain name and then by domain-relative path, or `None` when no
/// daemon answered.
///
/// Never an error: a daemon that is not running, one that refuses the request
/// and one whose answer does not parse all mean the same thing to the caller,
/// which is that the direct open is the route to try next.
async fn daemon_file_stamps(
    domain: Option<&str>,
) -> Option<HashMap<String, HashMap<String, FileStamp>>> {
    let data = crystalline_service::ctl_if_running(
        serde_json::json!({ "v": 1, "cmd": "file_stamps", "domain": domain }),
    )
    .await
    .ok()??;
    serde_json::from_value(data.get("domains")?.clone()).ok()
}

/// Why the index-backed checks did not run, written as guidance: what holds
/// the index, and the command that gets a full report. A person who runs
/// `doctor` while a daemon is up used to see nothing but the raw locking
/// error, which named neither.
fn index_unavailable_reason(
    db: &Path,
    error: &str,
    service: &ServiceDoctor,
    bypassed: bool,
) -> String {
    let db = db.display();
    let pid = service
        .lock_pid
        .map(|p| p.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let skipped = "so the orphan, unindexed, embedding and tag checks did not run";
    // Not a holder problem: see `instance::words_for_holder`.
    if crystalline_index::is_schema_too_new_text(error) {
        return format!("the index at {db} cannot be used by this binary, {skipped}. {error}");
    }
    // The wedge first: it is the one holder `--fix` can do something about.
    if service.daemon_unresponsive && !service.daemon_dislodged {
        return format!(
            "an unresponsive daemon (pid {pid}) holds the index at {db} and answers nothing on its socket, {skipped}. Rerun `crystalline doctor --fix` to replace it. The index reported: {error}"
        );
    }
    let live_daemon = instance::read_lock_info().is_some_and(|i| instance::process_alive(i.pid));
    if live_daemon && bypassed {
        return format!(
            "the running Crystalline daemon (pid {pid}) owns the index at {db}, and --db or --config told doctor to read that file directly instead of asking the daemon, {skipped}. Run `crystalline doctor` without --db and --config to have the daemon answer them, or stop it first with `crystalline ctl shutdown`. The index reported: {error}"
        );
    }
    if live_daemon {
        return format!(
            "the running Crystalline daemon (pid {pid}) owns the index at {db} and did not answer doctor's request for its file stamps, {skipped}. Stop it with `crystalline ctl shutdown` and run `crystalline doctor` again. The index reported: {error}"
        );
    }
    format!(
        "the index at {db} could not be opened, {skipped}. Check that the file is readable and that no other process is holding it; `crystalline doctor --fix` clears a lock or socket file a killed daemon left behind. The index reported: {error}"
    )
}

fn select_domains(cfg: &GlobalConfig, only: Option<&str>) -> Result<Vec<(String, DomainEntry)>> {
    match only {
        Some(name) => {
            let entry = cfg
                .domains
                .get(name)
                .ok_or_else(|| anyhow!("no domain named '{name}' is registered"))?;
            Ok(vec![(name.to_string(), entry.clone())])
        }
        None => Ok(cfg
            .domains
            .iter()
            .map(|(n, e)| (n.clone(), e.clone()))
            .collect()),
    }
}

/// One domain's diagnostics. `daemon_stamps` carries the file stamps a
/// running daemon served for this domain, which is the index read whenever
/// the daemon holds the index file; `store` is the direct open, used when
/// there is no daemon to ask. At most one of the two is ever `Some`, and both
/// produce the identical orphan, unindexed and unsyncable sets - the split
/// between "not indexed yet" and "cannot be indexed until the frontmatter is
/// fixed" is computed here, off the stamps, whichever route delivered them.
async fn check_domain(
    name: &str,
    entry: &DomainEntry,
    store: Option<&dyn Store>,
    daemon_stamps: Option<HashMap<String, FileStamp>>,
    rebuild_marker: Option<(String, Option<String>)>,
    fix: bool,
) -> Result<DomainDoctor> {
    // The marker is stamped onto every shape of report, not only the one the
    // on-disk checks run to the end of. A domain whose folder has gone is
    // exactly how a rebuild gets interrupted in the first place, and that
    // report must still say a rebuild did not finish rather than only that the
    // path is missing.
    let mut d = check_domain_checks(name, entry, store, daemon_stamps, fix).await?;
    if let Some((started, kind)) = rebuild_marker {
        d.rebuild_started = Some(started);
        d.rebuild_kind = kind;
    }
    Ok(d)
}

/// Every registry key the MANIFEST at `path` declares with a value the
/// registry does not list, read through the core parser so the finding says
/// exactly what the domain obeys. An unreadable or unparseable MANIFEST
/// yields nothing here: `manifest_present` and the E001 check own that.
fn policy_problems(path: &Path) -> Vec<PolicyProblem> {
    let Ok(source) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(engram) = crystalline_core::parse_engram(&source) else {
        return Vec::new();
    };
    let manifest = crystalline_core::Manifest::from_engram(&engram, &source);
    crystalline_core::policy_registry()
        .iter()
        .filter_map(|spec| {
            let (declared, effective) = manifest.policy(spec.key)?;
            let declared = declared?;
            (!spec.accepts(declared)).then(|| PolicyProblem {
                key: spec.key.to_string(),
                declared: declared.to_string(),
                read_as: effective.to_string(),
            })
        })
        .collect()
}

/// The values one registry key takes, or nothing for a key the registry does
/// not know - which no [`PolicyProblem`] ever carries, since the finding is
/// built from the registry itself.
fn policy_values(key: &str) -> &'static [&'static str] {
    crystalline_core::policy_registry()
        .iter()
        .find(|spec| spec.key == key)
        .map(|spec| spec.values)
        .unwrap_or(&[])
}

/// `a`, `a or b`, `a, b or c`: the values a key takes, read as a sentence
/// rather than as a list a reader has to parse. A [`PolicyKind::Text`] key
/// (`values` empty) has no enumerable list to join, so it reads as "a valid
/// domain name" instead - the only free-text key today, and what an invalid
/// declaration of it actually needs to become.
fn join_or(values: &[&str]) -> String {
    match values {
        [] => "a valid domain name".to_string(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// What a domain reads a policy key as, for the printed line: the value
/// itself, or - for a [`PolicyKind::Text`] key an invalid declaration leaves
/// with nothing to fall back to - "ignored; the local name holds".
fn policy_read_as(read_as: &str) -> &str {
    if read_as.is_empty() {
        "ignored; the local name holds"
    } else {
        read_as
    }
}

/// [`check_domain`] without the rebuild marker: the path, MANIFEST, orphan,
/// unindexed and encoding checks themselves.
async fn check_domain_checks(
    name: &str,
    entry: &DomainEntry,
    store: Option<&dyn Store>,
    daemon_stamps: Option<HashMap<String, FileStamp>>,
    fix: bool,
) -> Result<DomainDoctor> {
    // A virtual domain has no filesystem, so the on-disk checks (path, MANIFEST,
    // orphans, unindexed, encoding) do not apply. Report its database engram
    // count instead.
    if entry.is_virtual() {
        let mut d = DomainDoctor {
            name: name.to_string(),
            kind: "virtual".to_string(),
            path: "(virtual)".to_string(),
            is_virtual: true,
            path_exists: true,
            manifest_present: true,
            ..Default::default()
        };
        // The count needs the store itself, so a run served by the daemon
        // leaves it absent rather than reporting a fabricated zero.
        if let Some(store) = store {
            let count = store
                .list_engrams(name, None, None)
                .await
                .map(|e| e.len() as i64)
                .unwrap_or(0);
            d.engrams = Some(count);
            d.index_checked = true;
        }
        return Ok(d);
    }

    let path = cmd::resolve_domain_path(entry).unwrap_or_default();
    let path_exists = path.is_dir();
    let manifest_present = path_exists && path.join("MANIFEST.md").is_file();

    let mut d = DomainDoctor {
        name: name.to_string(),
        kind: "file".to_string(),
        path: path.display().to_string(),
        is_virtual: false,
        engrams: None,
        path_exists,
        manifest_present,
        ..Default::default()
    };

    if manifest_present {
        d.policy_problems = policy_problems(&path.join("MANIFEST.md"));
    }

    if !path_exists {
        return Ok(d);
    }

    // (c) Encoding problems and (b') unsyncable files: one verify_paths call
    // sources both. Encoding delegates to E006 rather than re-implementing
    // BOM/null-byte detection; E001 (frontmatter that fails to parse at all)
    // is kept keyed by its root-relative, forward-slashed path so it can be
    // matched against the unindexed set below - `verify` reports an absolute
    // path, the unindexed set does not, so they are normalised to the same
    // shape before comparing.
    let mut unsyncable_by_path: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(report) = verify::verify_paths([&path], &VerifyOptions::default()) {
        for issue in report.issues {
            match issue.rule {
                "E006" => {
                    d.encoding_issues.push(EncodingIssue {
                        path: issue.path.display().to_string(),
                        line: issue.line,
                        message: issue.message,
                    });
                }
                "E001" => {
                    unsyncable_by_path
                        .insert(relative_slash_path(&path, &issue.path), issue.message);
                }
                _ => {}
            }
        }
    }

    // (a) + (b): DB orphans and unindexed files, from whichever route reached
    // the index. `domain_id` stays `None` on the daemon-served route, which is
    // what keeps `--fix` from pretending it can delete rows through it.
    let mut domain_id = None;
    let stamps = match daemon_stamps {
        Some(stamps) => Some(stamps),
        None => match store {
            Some(store) => {
                let id = store
                    .upsert_domain(
                        name,
                        Some(&path.to_string_lossy()),
                        crystalline_index::DomainKind::File,
                    )
                    .await
                    .map_err(|e| anyhow!("could not read domain '{name}': {e}"))?;
                domain_id = Some(id);
                Some(
                    store
                        .file_stamps(id)
                        .await
                        .map_err(|e| anyhow!("could not read file stamps for '{name}': {e}"))?,
                )
            }
            None => None,
        },
    };
    if let Some(stamps) = stamps {
        d.index_checked = true;
        let on_disk = markdown_rel_paths(&path);
        let disk_set: HashSet<&str> = on_disk.iter().map(String::as_str).collect();
        let db_set: HashSet<&str> = stamps.keys().map(String::as_str).collect();

        let mut orphans: Vec<String> = stamps
            .keys()
            .filter(|p| !disk_set.contains(p.as_str()))
            .cloned()
            .collect();
        orphans.sort();
        let mut unindexed: Vec<String> = on_disk
            .into_iter()
            .filter(|p| !db_set.contains(p.as_str()))
            .collect();
        unindexed.sort();

        // A path with an E001 finding is not merely unsynced, it cannot be
        // indexed at all until its frontmatter is fixed - split it out.
        let mut unsyncable: Vec<UnsyncableFile> = Vec::new();
        unindexed.retain(|p| match unsyncable_by_path.remove(p) {
            Some(message) => {
                unsyncable.push(UnsyncableFile {
                    path: p.clone(),
                    message,
                });
                false
            }
            None => true,
        });
        d.unsyncable = unsyncable;

        // Only the direct route can delete. Over a daemon the orphans are
        // still reported, with the render saying plainly what removing them
        // takes, rather than being silently left in place.
        if let (true, Some(store), Some(domain_id)) = (fix, store, domain_id) {
            for p in &orphans {
                store.delete_engram(domain_id, p).await?;
                d.orphans_removed += 1;
            }
        }
        d.orphans = orphans;
        d.unindexed = unindexed;

        // Ownership: who hosts this file domain in a shared database. Unhosted
        // (single-instance) domains leave this `None`.
        if let (Some(store), Some(domain_id)) = (store, domain_id)
            && let Ok(Some(host)) = store.domain_host(domain_id).await
        {
            d.host_instance_id = Some(host.instance_id);
            d.host_heartbeat_at = Some(host.heartbeat_at);
        }
    }

    Ok(d)
}

/// Every `.md` file under `root`, relative and forward-slashed, skipping
/// dot-directories, dot-files and the OKF reserved filenames. Mirrors the sync
/// engine's own walk so orphan and unindexed detection line up with what a sync
/// would compute.
fn markdown_rel_paths(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let walker = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(&e.file_name().to_string_lossy()));
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy();
        // The OKF reserved names are never indexed (the sync walk skips them),
        // so counting them here would report every generated index file as
        // unindexed.
        if is_hidden(&fname)
            || crystalline_core::is_reserved_file(&fname)
            || !fname.to_lowercase().ends_with(".md")
        {
            continue;
        }
        out.push(relative_slash_path(root, entry.path()));
    }
    out
}

/// `p`, relative to `root` and forward-slashed, matching the shape the sync
/// engine's own walk produces (and, in turn, what the unindexed and orphan
/// sets are keyed by). `verify::Issue::path` is constructed from the same
/// root but stays a platform `PathBuf`, so any comparison against those sets
/// goes through this first.
///
/// The fast path is a literal component-prefix strip: free, and exact for
/// every caller that built `p` by walking `root` itself (`markdown_rel_paths`
/// below, always). It can still fail for a path that names the same file but
/// was produced by a second, independent walk of "the same" root -
/// `check_domain` passes its own `path` into `verify::verify_paths` by
/// reference, but a config-round-tripped or Windows-verbatim-prefixed
/// (`\\?\C:\...`) form can disagree with a plain one even when both resolve
/// to the identical file (seen on Windows CI: a duplicate-key E001 finding
/// fell back into `unindexed` instead of `unsyncable`, because keeping the
/// unstripped absolute path on a failed strip can never equal a relative
/// entry, so the mismatch produced a wrong bucket with nothing on screen to
/// say so). Canonicalizing both sides and retrying converges them regardless
/// of which one carries the mismatched form - `dunce::canonicalize`
/// specifically, not `std::fs::canonicalize`, because it strips a Windows
/// verbatim prefix from its result rather than risking adding one, so two
/// paths naming the same file end up in the same shape either way. If even
/// that fails (one side no longer exists, a permission error), the file's
/// own name is returned - still relative, in the shape callers expect,
/// rather than the absolute string a silent mismatch used to produce. That
/// last fallback is the one answer this function cannot vouch for: a bare
/// name does not equal a nested key, and two files of the same name in
/// different folders collapse onto one. Both would land a finding in the
/// wrong bucket exactly as the original bug did, so the fallback logs a
/// warning and the next double fault leaves a trail instead of nothing.
fn relative_slash_path(root: &Path, p: &Path) -> String {
    if let Some(rel) = strip_to_slash(root, p) {
        return rel;
    }
    if let (Ok(canon_root), Ok(canon_p)) = (dunce::canonicalize(root), dunce::canonicalize(p))
        && let Some(rel) = strip_to_slash(&canon_root, &canon_p)
    {
        return rel;
    }
    // The double fault: neither the literal strip nor the canonicalized retry
    // could relate the two. A bare file name is the best answer left, and it
    // may not match the key the caller compares it against, so the fallback
    // says so rather than repeating the silence this helper exists to end.
    tracing::warn!(
        root = %root.display(),
        path = %p.display(),
        "could not relate a path to its domain root, falling back to its file name"
    );
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The literal, zero-cost half of [`relative_slash_path`]: `Some` only when
/// `root` is exactly a component prefix of `p`.
fn strip_to_slash(root: &Path, p: &Path) -> Option<String> {
    let rel = p.strip_prefix(root).ok()?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') && name != "." && name != ".."
}

/// The report's words for who a stale lock used to belong to. A `lock_stale`
/// verdict now requires the OS lock to probe free (see [`ServiceDoctor::lock_stale`]),
/// so a genuinely stale lock can still carry no readable record at all - a
/// killed daemon that never got as far as `service.json`, or a legacy record
/// this reader could not parse - and "dead pid None" said nothing a person
/// could act on. "no record" says exactly what is known: not who it was, only
/// that nothing living claims it now.
fn stale_lock_holder_desc(pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!("dead pid {pid}"),
        None => "no record".to_string(),
    }
}

async fn check_service(fix: bool) -> Result<ServiceDoctor> {
    // The record's primary home is `service.json`; a still-present pre-split
    // daemon's record sitting in the lock file itself counts as present too
    // (see `instance::read_lock_info`'s legacy fallback), so an upgraded
    // doctor still flags and cleans up an old-format leftover.
    let info_path = config::service_info_path()
        .map_err(|e| anyhow!("could not resolve the service record path: {e}"))?;
    let legacy_path = config::service_lock_path()
        .map_err(|e| anyhow!("could not resolve the service lock path: {e}"))?;
    let sock_path = config::service_sock_path()
        .map_err(|e| anyhow!("could not resolve the service socket path: {e}"))?;

    let lock_present = info_path.is_file() || legacy_path.is_file();
    let info = instance::read_lock_info();
    let pid = info.as_ref().map(|i| i.pid);
    let alive = info
        .as_ref()
        .is_some_and(|i| instance::process_alive(i.pid));
    // `!alive` alone is not enough: a lock file with no record, or a record
    // naming a dead pid, both read `alive = false` even when something else
    // entirely still holds the OS lock (the 2026-09-23 incident - a daemon
    // that had dropped its record but not yet exited kept the file locked
    // for minutes). A genuinely stale lock is also a *free* one, so the
    // real-lock probe is required alongside the record check; a lock that is
    // still held with no readable record falls to `holder_unknown` instead,
    // which never deletes anything.
    let lock_stale = lock_present && !alive && instance::service_lock_is_free();

    let socket_present = sock_path.exists();
    // The same guard as `lock_stale`: a socket beside a lock somebody holds
    // belongs to that holder, record or not, and deleting it would cut a
    // running daemon off from every client that has not connected yet.
    let socket_orphaned =
        socket_present && !(lock_present && alive) && instance::service_lock_is_free();

    // Who actually holds the lock, probed read-only: this is the check that
    // sees the wedge (a live daemon holding the lock while its socket answers
    // nothing), which every field above reads as a perfectly healthy service.
    let holder = instance::diagnose_holder().await;
    let daemon_unresponsive = matches!(holder, instance::HolderState::Unresponsive { .. });
    let holder_unknown = match &holder {
        instance::HolderState::Unknown { detail } => {
            Some(instance::unknown_holder_error(detail).to_string())
        }
        _ => None,
    };

    let mut s = ServiceDoctor {
        lock_present,
        lock_pid: pid,
        lock_stale,
        lock_removed: false,
        socket_present,
        socket_orphaned,
        socket_removed: false,
        daemon_unresponsive,
        daemon_dislodged: false,
        holder_unknown,
    };

    if fix {
        // The wedge goes first: dislodging it releases the lock and removes
        // the record and socket file the checks below would otherwise trip
        // over. This is the same bounded, identity-verified dislodge a
        // connecting client performs, refusals included.
        if s.daemon_unresponsive {
            match instance::dislodge_unresponsive().await {
                Ok(instance::DislodgeOutcome::Dislodged { .. }) => s.daemon_dislodged = true,
                // The holder recovered between the diagnosis and the fix;
                // nothing to dislodge and nothing to report as unresolved.
                Ok(instance::DislodgeOutcome::NotNeeded) => s.daemon_unresponsive = false,
                Err(e) => s.holder_unknown = Some(e.to_string()),
            }
        }
        // Re-probed right before the delete, not trusted from the diagnosis
        // above: `diagnose_holder`'s socket probe alone can take up to
        // `HOLDER_PROBE_TIMEOUT`, and a daemon starting up in that window
        // would take the lock after this run decided it was free but before
        // it acted on that verdict - the exact "delete a lock a starting
        // daemon holds" shape the 2026-09-23 incident's doctor produced. A
        // lock that is held now is reported as recovered rather than
        // removed, the same treatment `daemon_unresponsive`'s `NotNeeded`
        // case gets above.
        if s.lock_stale && !instance::service_lock_is_free() {
            s.lock_stale = false;
        }
        if s.lock_stale {
            let info_removed = std::fs::remove_file(&info_path).is_ok();
            let legacy_removed = std::fs::remove_file(&legacy_path).is_ok();
            s.lock_removed = info_removed || legacy_removed;
        }
        if s.socket_orphaned && !instance::service_lock_is_free() {
            s.socket_orphaned = false;
        }
        if s.socket_orphaned {
            s.socket_removed = std::fs::remove_file(&sock_path).is_ok();
        }
    }
    Ok(s)
}

/// Which `CRYSTALLINE_*` environment variables are active, straight off the
/// parsed overlay. `None` when the overlay is empty, the same "omit the
/// section rather than show it empty" rule [`check_github`] follows. Purely
/// informational: nothing here ever feeds `remaining_problems`.
fn check_environment(overlay: &EnvOverlay) -> Option<EnvironmentDoctor> {
    if overlay.is_empty() {
        return None;
    }

    let overrides = overlay
        .active_overrides()
        .into_iter()
        .filter(|(_, key, _)| !key.starts_with("domain.") && key != "github.token")
        .map(|(var, key, value)| EnvOverride { var, key, value })
        .collect();

    let domains = overlay
        .env_domains()
        .map(|(name, env_domain)| EnvDomainReport {
            var: env_domain.var.clone(),
            name: name.clone(),
            path: env_domain
                .entry
                .file_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            origin: env_domain.entry.origin.as_ref().map(render_origin),
        })
        .collect();

    Some(EnvironmentDoctor {
        config_path_var: overlay.config_path().map(|p| p.display().to_string()),
        overrides,
        domains,
        github_token: overlay.github_token().is_some(),
    })
}

/// Renders an [`OriginConfig`] as `owner/repo[/subpath]@branch`, the same
/// grammar `CRYSTALLINE_DOMAIN_<NAME>_ORIGIN` parses, so the report reads as
/// something an operator could paste back into that variable.
fn render_origin(origin: &OriginConfig) -> String {
    let mut s = origin.repo.clone();
    if let Some(path) = &origin.path {
        s.push('/');
        s.push_str(path);
    }
    s.push('@');
    s.push_str(origin.branch());
    s
}

/// Filesystem-only diagnostics for each coding harness `crystalline
/// install` wires up (`claude-code`, `codex` and `copilot`): whether a
/// settings/hooks file exists, whether it parses, whether it carries our two
/// managed hooks and how many of the four managed skills are present. No
/// shell-out to any harness CLI, so this stays fast and works offline; user
/// scope only, reusing `install`'s own presence predicate and skill list
/// rather than duplicating either. `None` when no harness leaves any trace
/// at all, the same "omit the section rather than show it empty" rule
/// [`check_environment`] and [`check_github`] follow.
///
/// The install receipt is loaded once here rather than inside
/// [`check_one_harness`], since every harness's user-scope entry lives in
/// the same file: a missing or corrupt receipt reads as the empty one
/// (nothing recorded), exactly like `install` itself treats it as disposable
/// derived state.
fn check_harnesses() -> Option<Vec<HarnessDoctor>> {
    let book = receipt::receipt_path()
        .ok()
        .and_then(|p| receipt::load(&p).ok())
        .unwrap_or_default();
    let harnesses: Vec<HarnessDoctor> = [
        HarnessKind::ClaudeCode,
        HarnessKind::Codex,
        HarnessKind::Copilot,
    ]
    .into_iter()
    .map(|kind| check_one_harness(kind, book.find(kind.id(), "user", None)))
    .collect();
    let any_trace = harnesses
        .iter()
        .any(|h| h.settings_present || h.skills_installed > 0);
    any_trace.then_some(harnesses)
}

/// One harness's diagnostics: read its settings/hooks file read-only, check
/// the three managed hooks via [`install::harness_hook_present`] (which
/// knows each harness's file shape and session start command) and count how
/// many of [`install::managed_skills`] are present (and, of those, how many
/// were locally modified against either the embedded copy or `entry`'s
/// recorded hash) at its skills folder. `entry` is this harness's user-scope
/// install receipt record, `None` when it was never installed or predates
/// receipts.
fn check_one_harness(
    harness: HarnessKind,
    entry: Option<&receipt::InstallRecord>,
) -> HarnessDoctor {
    let paths = harness_paths(harness, false);
    let settings_present = paths.settings.is_file();

    let (session_start_hook, stop_hook, prompt_hook, settings_parse_error) =
        match install::read_settings(&paths.settings) {
            Ok(root) => (
                install::harness_hook_present(
                    harness,
                    &root,
                    "SessionStart",
                    install::session_start_command(harness),
                ),
                install::harness_hook_present(harness, &root, "Stop", install::STOP_COMMAND),
                install::prompt_hook_command(harness).map(|_| {
                    install::harness_hook_present(
                        harness,
                        &root,
                        "UserPromptSubmit",
                        install::PROMPT_COMMAND,
                    )
                }),
                None,
            ),
            Err(e) => (false, false, Some(false), Some(e.to_string())),
        };

    let recorded_hash: std::collections::HashMap<&str, &str> = entry
        .map(|e| {
            e.skills
                .iter()
                .map(|s| (s.name.as_str(), s.sha256.as_str()))
                .collect()
        })
        .unwrap_or_default();

    let mut skills_installed = 0;
    let mut skills_modified = 0;
    for &(name, content) in install::managed_skills().iter() {
        let path = paths.skills_dir.join(name).join("SKILL.md");
        if let Ok(existing) = std::fs::read(&path) {
            skills_installed += 1;
            let matches_embedded = existing == content.as_bytes();
            let matches_receipt = matches_embedded
                || recorded_hash
                    .get(name)
                    .is_some_and(|&h| h == receipt::sha256_hex(&existing));
            if !matches_receipt {
                skills_modified += 1;
            }
        }
    }

    // Leftovers: every name the receipt or the static retired list still
    // remembers, deduplicated, that is not a currently managed skill and
    // whose `SKILL.md` still sits on disk. Mirrors the retirement logic in
    // `install::reconcile_skill_set` without shelling out to it.
    let managed: HashSet<&str> = install::managed_skills().iter().map(|&(n, _)| n).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut retired_leftovers = Vec::new();
    let candidate_names = entry
        .into_iter()
        .flat_map(|e| e.skills.iter().map(|s| s.name.as_str()))
        .chain(install::RETIRED_SKILLS.iter().copied());
    for name in candidate_names {
        if managed.contains(name) || !seen.insert(name) {
            continue;
        }
        // Receipt names are attacker-writable local state; this check is
        // read-only here, but the same guard keeps a hostile name out of the
        // report rather than have doctor point a person at retiring it by
        // hand. See `install::is_plain_skill_name`.
        if !install::is_plain_skill_name(name) {
            continue;
        }
        if paths.skills_dir.join(name).join("SKILL.md").is_file() {
            retired_leftovers.push(name.to_string());
        }
    }

    HarnessDoctor {
        name: harness.id().to_string(),
        settings_path: paths.settings.display().to_string(),
        settings_present,
        settings_parse_error,
        session_start_hook,
        stop_hook,
        prompt_hook,
        skills_installed,
        skills_modified,
        receipt_version: entry.map(|e| e.version.clone()),
        retired_leftovers,
    }
}

/// Provisioning diagnostics: every declaring domain's decision and counts,
/// every installed harness's drift/edited/orphaned/missing counts and every
/// domain still awaiting a decision. `None` when no domain declares a
/// `Provisioning` section at all, the same "omit rather than show empty"
/// rule [`check_environment`], [`check_github`] and [`check_harnesses`]
/// follow. The domain-keyed lists (`domains`, `pending`) are filtered by
/// `--domain` through `targets`, matching how the per-domain and github
/// sections behave; the harness rollup stays whole-machine on purpose, since
/// the provisioning receipt is shared across every domain, the same way
/// `apply` reconciles them. Calls straight into [`provision::status`], which
/// only scans the filesystem and reads the provisioning receipt - it never
/// writes the receipt, never touches a harness's config directory and never
/// spawns a harness CLI, so this stays as read-only as the rest of doctor.
fn check_provisioning(
    cfg: &GlobalConfig,
    overlay: &EnvOverlay,
    targets: &[(String, DomainEntry)],
) -> Result<Option<ProvisioningDoctor>> {
    if !provision::any_domain_declares(cfg) {
        return Ok(None);
    }

    let receipt_path = provision::receipt_path()
        .map_err(|e| anyhow!("could not resolve the provisioning receipt path: {e}"))?;
    let install_receipt_path = provision::install_receipt_path()
        .map_err(|e| anyhow!("could not resolve the install receipt path: {e}"))?;
    let harnesses = provision::installed_harnesses(&install_receipt_path);
    // Named so an env-defined domain never surfaces in `pending`: its
    // decision can never be recorded, see `provision::apply`'s doc comment.
    let env_domains: HashSet<&str> = overlay
        .env_domains()
        .map(|(name, _)| name.as_str())
        .collect();
    let report = provision::status(cfg, &receipt_path, &harnesses, &env_domains)
        .map_err(|e| anyhow!("could not read provisioning status: {e}"))?;

    let selected: HashSet<&str> = targets.iter().map(|(name, _)| name.as_str()).collect();

    let domains = report
        .domains
        .iter()
        .filter(|d| d.declares && selected.contains(d.domain.as_str()))
        .map(|d| ProvisioningDomainDoctor {
            name: d.domain.clone(),
            decision: provisioning_decision_label(d.decision).to_string(),
            counts: d.counts.clone(),
            mirror_present: mirror_present(cfg, &d.domain),
        })
        .collect();

    let harnesses = report
        .harnesses
        .iter()
        .map(|h| ProvisioningHarnessDoctor {
            harness: h.harness.id().to_string(),
            installed_files: h.installed_files,
            installed_mcps: h.installed_mcps,
            drift: h.drift,
            edited: h.edited,
            orphaned: h.orphaned,
            missing: h.missing,
        })
        .collect();

    let pending = report
        .pending
        .iter()
        .filter(|p| selected.contains(p.domain.as_str()))
        .map(|p| ProvisioningPendingDoctor {
            domain: p.domain.clone(),
            counts: p.counts.clone(),
        })
        .collect();

    Ok(Some(ProvisioningDoctor {
        domains,
        harnesses,
        pending,
    }))
}

/// Advisory tag-hygiene check: the near-duplicate tag clusters across every
/// domain this machine has registered. Read-only, mirroring
/// `check_provisioning`'s stance - it reads the vocabulary and groups it, never
/// touching a file. `None` when there is no index to read.
///
/// A domain the index holds rows for and the configuration does not register is
/// left out, because the advice is to run `crystalline tags merge` and that verb
/// will not touch such a domain: a cluster only its rows produce would be a
/// finding nobody can act on. The registrations come from `cfg`, the whole
/// effective configuration and deliberately not the `--domain` selection, which
/// would narrow this whole-index check into a different one.
///
/// The sweep is the single all-domain query it has always been unless an
/// unregistered domain is actually there; only then does it become one query
/// per registered domain, merged - the shape `Engine::vocabulary` takes for the
/// same reason, since a `Vocabulary` is aggregated counts with no domain on
/// them to filter by afterwards.
async fn check_tags(store: Option<&dyn Store>, cfg: &GlobalConfig) -> Result<Option<TagsDoctor>> {
    let Some(store) = store else {
        return Ok(None);
    };
    let indexed = store
        .domain_names()
        .await
        .map_err(|e| anyhow!("could not read the domain list: {e}"))?;
    let registered: Vec<&String> = indexed
        .iter()
        .filter(|name| cfg.domains.contains_key(*name))
        .collect();
    // The team's own list, whoever is drafting: `doctor` reports what the
    // domain has agreed on, and a word one author is trying out in a draft is
    // not that.
    let vocab = if registered.len() == indexed.len() {
        store
            .vocabulary(None, None)
            .await
            .map_err(|e| anyhow!("could not read the vocabulary: {e}"))?
    } else {
        let mut parts = Vec::with_capacity(registered.len());
        for name in registered {
            parts.push(
                store
                    .vocabulary(Some(name), None)
                    .await
                    .map_err(|e| anyhow!("could not read the vocabulary: {e}"))?,
            );
        }
        crystalline_index::merge_vocabularies(parts)
    };
    Ok(Some(TagsDoctor {
        // Fold declared aliases out first, so a cluster an alias already explains
        // is never surfaced as tag drift.
        clusters: tag_clusters_with_aliases(&vocab.tags, &vocab.aliases),
    }))
}

/// A stable human label for one [`provision::Decision`] variant, the same
/// spelling `crystalline provision status` already uses.
fn provisioning_decision_label(decision: provision::Decision) -> &'static str {
    use provision::Decision::*;
    match decision {
        Allowed => "allowed",
        Denied => "denied",
        Undecided => "undecided",
    }
}

/// Whether `name`'s artifact mirror is present at its origin state
/// directory, for a team domain with at least one out-of-subtree
/// `Provisioning` declaration. `None` when `name` is not a team domain, or is
/// one but declares no out-of-subtree path - a mirror is never expected in
/// either case. A resolved source root under `<origin_state_dir>/artifacts`
/// is exactly how [`provision::resolve_source_roots`] documents an
/// out-of-subtree decl resolving for a team domain, so its presence there is
/// the gate.
fn mirror_present(cfg: &GlobalConfig, name: &str) -> Option<bool> {
    let entry = cfg.domains.get(name)?;
    entry.origin.as_ref()?;
    let roots = provision::resolve_source_roots(name, entry);
    let mirror_root = config::origin_state_dir(name).ok()?.join("artifacts");
    roots
        .iter()
        .any(|(_, root)| root.starts_with(&mirror_root))
        .then(|| mirror_root.is_dir())
}

/// GitHub collaboration diagnostics: this machine's connection and, per team
/// domain in `targets`, whether its local origin state is present and its
/// base snapshot still matches what was recorded. Read-only: resolving the
/// token store and calling `verify_base` never write anything, so this runs
/// the same whether or not `--fix` is set. When the overlay carries
/// `CRYSTALLINE_GITHUB_TOKEN`, that store is used directly instead of probing
/// the keychain or the token file, so a headless node's diagnostics never
/// touch a credential store that variable makes irrelevant.
fn check_github(
    cfg: &GlobalConfig,
    overlay: &EnvOverlay,
    targets: &[(String, DomainEntry)],
) -> Result<GithubDoctor> {
    let api_url = cfg.github.as_ref().and_then(|g| g.api_url.clone());
    let host = cmd::bare_host(&auth_base(api_url.as_deref()));
    let (store, token) = match overlay.github_token() {
        Some(token) => {
            let store = TokenStore::env(token, host.as_deref());
            let stored = store
                .load()
                .map_err(|e| anyhow!("could not read the saved GitHub token: {e}"))?;
            (store, stored)
        }
        // The non-env doctor read fuses the backend probe and the token load
        // into a single keychain access (down from two), the same one-read
        // path the engine uses.
        None => {
            let state_base = config::origins_state_dir()
                .map_err(|e| anyhow!("could not resolve the origins state directory: {e}"))?;
            TokenStore::resolve_and_load(host.as_deref(), &state_base)
                .map_err(|e| anyhow!("could not read the saved GitHub token: {e}"))?
        }
    };

    let mut origins = Vec::new();
    for (name, entry) in targets {
        let Some(origin) = &entry.origin else {
            continue;
        };
        let dir = config::origin_state_dir(name)
            .map_err(|e| anyhow!("could not resolve origin state for '{name}': {e}"))?;
        let state = OriginState::load(&dir)
            .map_err(|e| anyhow!("could not read origin state for '{name}': {e}"))?;
        let (state_present, base_mismatches) = match &state {
            Some(s) => {
                let bad = verify_base(&dir, &s.files)
                    .map_err(|e| anyhow!("could not verify the base snapshot for '{name}': {e}"))?;
                (true, bad)
            }
            None => (false, Vec::new()),
        };
        origins.push(OriginDoctor {
            name: name.clone(),
            repo: origin.repo.clone(),
            state_present,
            base_mismatches,
            env_defined: overlay.env_domain(name).is_some(),
        });
    }

    Ok(GithubDoctor {
        connected: token.is_some(),
        user: token
            .as_ref()
            .and_then(|t| t.user_display())
            .map(str::to_string),
        token_store: store.kind().to_string(),
        origins,
    })
}

async fn embedding_summary(store: &dyn Store, cfg: &GlobalConfig) -> Result<serde_json::Value> {
    let coverage = store
        .embedding_coverage()
        .await
        .map_err(|e| anyhow!("could not read embedding coverage: {e}"))?;
    let configured = configured_model_id(cfg.embeddings.as_ref());
    let embedded_with_configured = coverage.embedded_for(&configured);
    let stale_chunks: usize = coverage
        .models
        .iter()
        .filter(|m| m.model != configured)
        .map(|m| m.count)
        .sum();
    // The repo behind the id, when the table recognizes it, and what the
    // cache actually holds. `local_model` matches by id or by repo whatever
    // the provider, so a remote config that happens to name a known repo
    // (the local text-embeddings-inference case Task 2 flagged) still
    // resolves here; an id the table has never heard of leaves both absent
    // rather than wrong.
    let entry = local_model(&configured);
    // NLI checkpoints share the model cache but are not embedding models: the
    // contradictions row lists them and marks them stale on its own terms, so
    // they are filtered out here rather than shown twice with two different
    // verdicts (plan correction 14).
    let cached: Vec<(String, u64)> = config::models_dir()
        .map(|dir| cached_model_dirs(&dir))
        .unwrap_or_default()
        .into_iter()
        .filter(|(repo, _)| is_embedding_listing(repo))
        .collect();
    let configured_repo = entry.map(|m| m.repo);
    let configured_model_bytes =
        configured_repo.and_then(|repo| cached.iter().find(|(r, _)| r == repo).map(|(_, b)| *b));
    let cached_models: Vec<serde_json::Value> = cached
        .iter()
        .map(|(repo, bytes)| {
            serde_json::json!({
                "repo": repo,
                "bytes": bytes,
                "stale": Some(repo.as_str()) != configured_repo,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "configured_model": configured,
        "configured_repo": configured_repo,
        "configured_model_bytes": configured_model_bytes,
        "total_chunks": coverage.total_chunks,
        "embedded_with_configured_model": embedded_with_configured,
        "stale_chunks": stale_chunks,
        "models": coverage.models,
        "cached_models": cached_models,
    }))
}

/// Whether a cached repository belongs in the embedding listing: anything but
/// a checkpoint of the contradiction check's model table (plan correction
/// 14). The contradictions row lists NLI checkpoints on its own terms, so a
/// checkpoint would otherwise show up twice with two different verdicts.
fn is_embedding_listing(repo: &str) -> bool {
    crystalline_index::nli::nli_model_by_repo(repo).is_none()
}

/// The contradictions row: the profile and its model from config, whether the
/// model's weights are in the cache (filesystem only, no index read - plan
/// correction 15, so this runs the same under a running daemon as without
/// one), and NLI checkpoints no profile uses now. `daemon_status` is `ctl
/// status`'s answer when a daemon served this run's file stamps; its
/// `pending_pairs`, `failing_pairs`, `last_error`, `load_failed` and
/// `embedding_pending` are read from there and never recomputed (lesson 36) -
/// a direct read has no worker, so those stay null/false, the same shape
/// `crystalline status`'s standalone fallback reports.
fn contradiction_summary(
    cfg: &GlobalConfig,
    daemon_status: Option<&serde_json::Value>,
) -> serde_json::Value {
    use crystalline_index::nli::{
        LOCAL_NLI_AVAILABLE, NLI_FEATURE_MISSING, NLI_MODELS, NliProfile, nli_model, weights_cached,
    };
    let setting = cfg.evolve_contradictions();
    let model = NliProfile::from_setting(setting).map(nli_model);
    let models_dir = config::models_dir().ok();
    let cached = |repo: &str| {
        models_dir.as_deref().is_some_and(|dir| {
            NLI_MODELS
                .iter()
                .any(|m| m.repo == repo && weights_cached(dir, m))
        })
    };
    let stale: Vec<&str> = NLI_MODELS
        .iter()
        .map(|m| m.repo)
        .filter(|repo| Some(*repo) != model.map(|m| m.repo) && cached(repo))
        .collect();
    let live = daemon_status.map(|d| &d["contradictions"]);
    let pending = live.and_then(|c| c["pending_pairs"].as_u64());
    let failing = live.and_then(|c| c["failing_pairs"].as_u64());
    let load_failed = live.is_some_and(|c| c["load_failed"].as_bool().unwrap_or(false));
    let embedding_pending = live.is_some_and(|c| c["embedding_pending"].as_bool().unwrap_or(false));
    let last_error = live
        .and_then(|c| c["last_error"].as_str())
        .map(str::to_string);
    match model {
        None => serde_json::json!({
            "profile": setting, "model": null, "repo": null, "downloaded": null,
            "reason": null, "pending_pairs": null, "failing_pairs": null,
            "last_error": null, "load_failed": false, "embedding_pending": false,
            "stale_checkpoints": stale,
        }),
        Some(m) => {
            let downloaded = cached(m.repo);
            // A load failure already gets its own line from
            // `contradiction_wait_reason` (the model could not be loaded:
            // <error>); repeating `last_error` here too would print it twice.
            // "Not downloaded" alone is still correct: the weights are not on
            // disk, whatever the reason the load never finished.
            let reason = (!downloaded && !load_failed).then(|| {
                last_error.clone().unwrap_or_else(|| {
                    if LOCAL_NLI_AVAILABLE {
                        "the daemon downloads it on its first pass".to_string()
                    } else {
                        NLI_FEATURE_MISSING.to_string()
                    }
                })
            });
            serde_json::json!({
                "profile": setting, "model": m.id, "repo": m.repo, "downloaded": downloaded,
                "reason": reason, "pending_pairs": pending, "failing_pairs": failing,
                "last_error": last_error, "load_failed": load_failed,
                "embedding_pending": embedding_pending, "stale_checkpoints": stale,
            })
        }
    }
}

/// Whole megabytes, decimal (the unit every other surface names a model's
/// size in: the release notes, deployment.md's image table), for a size
/// beside a model id.
fn mb(bytes: u64) -> String {
    format!("{} MB", bytes / 1_000_000)
}

/// The `SessionStart`/`Stop`/`UserPromptSubmit` hook lines and the trailing
/// "partial setup" notice, exactly as [`render_human`] prints them under a
/// harness's settings-file line. Factored out so a test can exercise the
/// rendering without building a whole [`DoctorReport`].
///
/// Copilot's `UserPromptSubmit` hook renders like the other two harnesses'
/// present/absent line, but carries a trailing note on why it does nothing
/// today whether the entry is present or absent (a hand-deleted entry is
/// exactly as inert as a present one, so the note stays), and it never
/// enters the "partial setup" count: Copilot's copy is permanently inert
/// rather than merely not yet installed, so neither its presence nor its
/// absence should ever nudge a person to "fix" a setup that already is what
/// it can be. Which harness that is comes from
/// [`install::prompt_hook_output_is_honoured`], never a string compare on
/// `h.name` here - the fact belongs beside `prompt_hook_command`, not
/// duplicated in the renderer.
fn hook_lines(h: &HarnessDoctor) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "    SessionStart hook: {}\n",
        if h.session_start_hook {
            "present"
        } else {
            "absent"
        }
    ));
    out.push_str(&format!(
        "    Stop hook: {}\n",
        if h.stop_hook { "present" } else { "absent" }
    ));
    // Unrecognized names never reach doctor (every entry comes from
    // `check_harnesses`'s fixed `HarnessKind` list), so this defaults to
    // "honoured" only as a defensive fallback for a hand-built fixture.
    let honoured = HarnessKind::from_id(&h.name)
        .map(install::prompt_hook_output_is_honoured)
        .unwrap_or(true);
    match h.prompt_hook {
        Some(present) => {
            let note = if !honoured {
                " (Copilot does not honour a config-file prompt hook's output today)"
            } else {
                ""
            };
            out.push_str(&format!(
                "    UserPromptSubmit hook: {}{note}\n",
                if present { "present" } else { "absent" }
            ));
        }
        None => {
            // Unreachable today: every harness `check_one_harness` builds a
            // report for gets a prompt hook command
            // ([`install::prompt_hook_command`] is `Some` for all three),
            // and a parse error reads as `Some(false)`, not `None` - see
            // `HarnessDoctor::prompt_hook`'s doc. Kept as a real match arm
            // rather than a `.unwrap_or(...)`, so a future harness with no
            // `UserPromptSubmit`-shaped event at all (a `None` from
            // `prompt_hook_command` itself) renders honestly instead of
            // panicking.
            out.push_str(
                "    UserPromptSubmit hook: not available (this harness has no UserPromptSubmit output channel)\n",
            );
        }
    }
    let applicable: [Option<bool>; 3] = if honoured {
        [Some(h.session_start_hook), Some(h.stop_hook), h.prompt_hook]
    } else {
        [Some(h.session_start_hook), Some(h.stop_hook), None]
    };
    let present = applicable.iter().flatten().filter(|p| **p).count();
    let expected = applicable.iter().flatten().count();
    if present > 0 && present < expected {
        out.push_str(&format!(
            "    partial setup - run: crystalline install {}\n",
            h.name
        ));
    }
    out
}

/// Render a report for a human.
pub fn render_human(report: &DoctorReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    // The index route first, so a reader meets the reason before the thin
    // domain sections it explains. A plain direct open, and a machine with no
    // index yet, say nothing here: only a route worth knowing about does.
    match &report.index {
        IndexAccess::Daemon => {
            let _ = writeln!(
                out,
                "index: read through the running daemon, which owns the index file"
            );
        }
        IndexAccess::Unavailable { reason } => {
            let _ = writeln!(out, "index:");
            let _ = writeln!(out, "  [problem] {reason}");
        }
        IndexAccess::Absent | IndexAccess::Direct => {}
    }

    for d in &report.domains {
        let _ = writeln!(out, "{} ({})", d.name, d.path);
        // Ownership in a shared database: who hosts this file domain. Unhosted
        // domains print nothing extra.
        if let Some(host) = &d.host_instance_id {
            let hb = d
                .host_heartbeat_at
                .as_deref()
                .map(|h| format!(" (last heartbeat {h})"))
                .unwrap_or_default();
            let _ = writeln!(out, "  hosted by instance {host}{hb}");
        }
        // A rebuild that was stamped and never cleared. Nothing was destroyed -
        // the rows below are the complete ones from before it - so the finding
        // is a refresh to finish, and the command that finishes it is the one
        // that was interrupted.
        //
        // "has not finished" rather than "never finished": doctor sees no
        // activity snapshot (the markers only reach it on the direct route),
        // so it has not checked whether a rebuild is running this second and
        // must not say it is not. The remedy is the same either way - re-run
        // it - and a re-run while one is in flight waits on the store lock
        // rather than colliding.
        if let Some(started) = &d.rebuild_started {
            let _ = match d.rebuild_kind.as_deref() {
                // A wipe emptied the index before it began, so the rows here are
                // whatever its rebuild managed and every embedding is gone.
                // Saying anything else is the misreading the marker exists for.
                Some("wipe") => writeln!(
                    out,
                    "  [problem] a wipe started {started} has not finished; this domain's rows and every embedding it had were destroyed before it began. Run: crystalline reindex --full"
                ),
                Some("full") => writeln!(
                    out,
                    "  [problem] a full rebuild started {started} has not finished; this domain's rows are the ones from before it. Run: crystalline reindex --full"
                ),
                // A marker a binary older than the kind column stamped: say what
                // is known and claim nothing about the rows either way.
                _ => writeln!(
                    out,
                    "  [problem] a rebuild started {started} has not finished. Run: crystalline reindex --full"
                ),
            };
        }
        if d.is_virtual {
            match d.engrams {
                Some(n) => {
                    let _ = writeln!(out, "  ok (virtual, {n} engram(s) in the database)");
                }
                // A virtual domain lives entirely in the index, so with no
                // route to it there is nothing to count and nothing to
                // claim.
                None => {
                    let _ = writeln!(out, "  ok (virtual, engram count not read)");
                }
            }
            continue;
        }
        if !d.path_exists {
            let _ = writeln!(out, "  [problem] domain path does not exist");
            continue;
        }
        if !d.manifest_present {
            let _ = writeln!(out, "  [problem] no MANIFEST.md at the domain root");
        }
        for p in &d.policy_problems {
            let _ = writeln!(
                out,
                "  [problem] MANIFEST {}: {} is not {}; read as {}",
                p.key,
                p.declared,
                join_or(policy_values(&p.key)),
                policy_read_as(&p.read_as)
            );
        }
        // Said once per domain so an empty orphan and unindexed list is never
        // mistaken for a clean bill of health. The cause, and its remedy, are
        // in the index section above.
        if !d.index_checked && matches!(report.index, IndexAccess::Unavailable { .. }) {
            let _ = writeln!(
                out,
                "  index checks skipped (orphan rows, unindexed files); see the index section above"
            );
        }
        if !d.orphans.is_empty() {
            if d.orphans_removed > 0 {
                let _ = writeln!(
                    out,
                    "  removed {} orphan row(s): {}",
                    d.orphans_removed,
                    d.orphans.join(", ")
                );
            } else if report.index == IndexAccess::Daemon {
                // Removing a row is a write, and this run reached the index
                // through a read verb on the daemon that holds it. Say what
                // that takes instead of pointing at a --fix that would do
                // nothing.
                let _ = writeln!(
                    out,
                    "  [problem] {} orphan row(s) (file missing on disk): {}. The running daemon owns the index, so removing them needs it stopped: run `crystalline ctl shutdown`, then `crystalline doctor --fix`",
                    d.orphans.len(),
                    d.orphans.join(", ")
                );
            } else {
                let _ = writeln!(
                    out,
                    "  [problem] {} orphan row(s) (file missing on disk), rerun with --fix to remove: {}",
                    d.orphans.len(),
                    d.orphans.join(", ")
                );
            }
        }
        if !d.unindexed.is_empty() {
            let _ = writeln!(
                out,
                "  [problem] {} file(s) not indexed yet, run: crystalline sync --domain {}",
                d.unindexed.len(),
                d.name
            );
            for p in &d.unindexed {
                let _ = writeln!(out, "    {p}");
            }
        }
        if !d.unsyncable.is_empty() {
            let _ = writeln!(
                out,
                "  [problem] {} file(s) cannot be indexed until the frontmatter is fixed (verify rule E001):",
                d.unsyncable.len()
            );
            for f in &d.unsyncable {
                let _ = writeln!(out, "    {}: {}", f.path, f.message);
            }
        }
        if !d.encoding_issues.is_empty() {
            let _ = writeln!(
                out,
                "  [problem] {} encoding issue(s), see verify rule E006:",
                d.encoding_issues.len()
            );
            for e in &d.encoding_issues {
                let _ = writeln!(out, "    {}: {}", e.path, e.message);
            }
        }
        // "ok" is a claim about everything, so a domain whose index checks
        // never ran does not get to make it.
        if d.manifest_present
            && d.policy_problems.is_empty()
            && d.orphans.is_empty()
            && d.unindexed.is_empty()
            && d.unsyncable.is_empty()
            && d.encoding_issues.is_empty()
            && (d.index_checked || !matches!(report.index, IndexAccess::Unavailable { .. }))
        {
            let _ = writeln!(out, "  ok");
        }
    }

    // Domains the index still holds rows for and the configuration does not
    // name. Kept apart from the per-domain sections above, and worded apart
    // from them: an "orphan row" there is one indexed file whose file is
    // gone, and these are whole domains. The one thing every line must say is
    // what 0.17.0's message did not, which is that the rows answer nothing
    // any more and that there is a proportionate way to end them - and the
    // one thing no line may do is promise a collection that will not happen,
    // which is why the kept reasons are matched rather than defaulted.
    if let Some(o) = &report.orphaned_rows {
        if let Some(err) = &o.error {
            // A header naming domains would claim the check found some.
            let _ = writeln!(out, "rows whose domain is gone:");
            let _ = writeln!(out, "  not checked: {err}");
        } else if o.domains.is_empty() {
            // Nothing was considered, so nothing may be called deregistered:
            // an unreadable configuration is exactly the state in which no
            // domain can be shown absent from anything.
            if let Some(skipped) = &o.skipped {
                let _ = writeln!(out, "rows whose domain is gone:");
                let _ = writeln!(out, "  not checked: {skipped}");
            }
        } else {
            let _ = writeln!(out, "domains no longer registered:");
            for d in &o.domains {
                let _ = writeln!(out, "{}", orphaned_domain_line(d));
            }
            if let Some(skipped) = &o.skipped {
                let _ = writeln!(out, "  nothing was collected: {skipped}");
            }
        }
    }

    let s = &report.service;
    let _ = writeln!(out, "service:");
    if s.lock_stale {
        let holder = stale_lock_holder_desc(s.lock_pid);
        if s.lock_removed {
            let _ = writeln!(out, "  removed stale lock file ({holder})");
        } else {
            let _ = writeln!(
                out,
                "  [problem] stale lock file ({holder}), rerun with --fix to remove"
            );
        }
    }
    if s.socket_orphaned {
        if s.socket_removed {
            let _ = writeln!(out, "  removed orphaned socket file");
        } else {
            let _ = writeln!(
                out,
                "  [problem] orphaned socket file, rerun with --fix to remove"
            );
        }
    }
    if s.daemon_unresponsive {
        let pid = s
            .lock_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        if s.daemon_dislodged {
            let _ = writeln!(
                out,
                "  dislodged an unresponsive daemon (pid {pid}); the next client starts a fresh one"
            );
        } else {
            let _ = writeln!(
                out,
                "  [problem] daemon unresponsive (pid {pid} per its record): it holds the index lock but answers nothing on its socket. A connecting client will replace it, or rerun with --fix"
            );
        }
    }
    if let Some(detail) = &s.holder_unknown {
        let _ = writeln!(out, "  [problem] {detail}");
    }
    if !s.lock_stale && !s.socket_orphaned && !s.daemon_unresponsive && s.holder_unknown.is_none() {
        let _ = writeln!(out, "  ok");
    }
    if let Ok(log_path) = config::daemon_log_path() {
        let _ = writeln!(out, "  daemon log: {}", log_path.display());
    }

    if let Some(e) = &report.environment {
        let _ = writeln!(out, "environment:");
        if let Some(path) = &e.config_path_var {
            let _ = writeln!(out, "  CRYSTALLINE_CONFIG points at {path}");
        }
        for o in &e.overrides {
            let _ = writeln!(out, "  {} overrides {} = {}", o.var, o.key, o.value);
        }
        for d in &e.domains {
            match &d.origin {
                Some(origin) => {
                    let _ = writeln!(
                        out,
                        "  {} defines domain '{}' at {} (origin {origin})",
                        d.var, d.name, d.path
                    );
                }
                None => {
                    let _ = writeln!(out, "  {} defines domain '{}' at {}", d.var, d.name, d.path);
                }
            }
        }
        if e.github_token {
            let _ = writeln!(
                out,
                "  CRYSTALLINE_GITHUB_TOKEN provides the GitHub token (read-only)"
            );
        }
    }

    if let Some(g) = &report.github {
        let _ = writeln!(out, "github:");
        if g.connected && g.token_store == "environment" {
            let _ = writeln!(
                out,
                "  connected via CRYSTALLINE_GITHUB_TOKEN (environment token store)"
            );
        } else if g.connected {
            let _ = writeln!(
                out,
                "  connected as {} ({} token store)",
                g.user.as_deref().unwrap_or("?"),
                g.token_store
            );
        } else {
            let _ = writeln!(
                out,
                "  not connected ({} token store). Run: crystalline connect github",
                g.token_store
            );
        }
        if g.origins.is_empty() {
            let _ = writeln!(out, "  no team domains connected to an origin");
        }
        for o in &g.origins {
            if !o.state_present && o.env_defined {
                let _ = writeln!(
                    out,
                    "  {} ({}): env-defined team domain, bootstraps itself when the daemon connects",
                    o.name, o.repo
                );
            } else if !o.state_present {
                let _ = writeln!(
                    out,
                    "  [problem] {} ({}): no origin state on disk; add the domain from its origin first",
                    o.name, o.repo
                );
            } else if !o.base_mismatches.is_empty() {
                let _ = writeln!(
                    out,
                    "  [problem] {} ({}): {} base snapshot file(s) missing or modified: {}",
                    o.name,
                    o.repo,
                    o.base_mismatches.len(),
                    o.base_mismatches.join(", ")
                );
            } else {
                let _ = writeln!(out, "  {} ({}): ok", o.name, o.repo);
            }
        }
    }

    if let Some(e) = &report.embeddings {
        let repo = e["configured_repo"].as_str();
        let size = e["configured_model_bytes"].as_u64();
        let named = match (repo, size) {
            (Some(r), Some(b)) => format!(" ({r}, {} on disk)", mb(b)),
            (Some(r), None) => format!(" ({r}, not downloaded)"),
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "embeddings: {}/{} chunks embedded with '{}'{named} ({} stale chunk(s) from a different model)",
            e["embedded_with_configured_model"],
            e["total_chunks"],
            e["configured_model"].as_str().unwrap_or_default(),
            e["stale_chunks"]
        );
        if let Some(cached) = e["cached_models"].as_array().filter(|c| !c.is_empty()) {
            let listed: Vec<String> = cached
                .iter()
                .map(|m| {
                    let mark = if m["stale"] == serde_json::Value::Bool(true) {
                        " [stale]"
                    } else {
                        ""
                    };
                    format!(
                        "{} {}{mark}",
                        m["repo"].as_str().unwrap_or_default(),
                        mb(m["bytes"].as_u64().unwrap_or(0))
                    )
                })
                .collect();
            // Weights this install no longer uses: on a writable instance
            // running a local model, the next daemon start removes them;
            // a read-only instance or a remote provider never prunes, so
            // there they stay marked stale until someone clears them by hand.
            let _ = writeln!(out, "  cached models: {}", listed.join("; "));
        }
        // The coverage figure never goes out bare while a rebuild is
        // unfinished: the incident was a coverage number read as normal when it
        // was the middle of something. Coverage can only ever rise across a
        // rebuild now, and saying so is what stops the number being misread in
        // the other direction too.
        let unfinished: Vec<&DomainDoctor> = report
            .domains
            .iter()
            .filter(|d| d.rebuild_started.is_some())
            .collect();
        let names = unfinished
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        // One unfinished wipe is enough to make the whole figure a post-wipe
        // one: the wipe emptied the index, not one domain's corner of it.
        let wiped = unfinished
            .iter()
            .any(|d| d.rebuild_kind.as_deref() == Some("wipe"));
        if wiped {
            let _ = writeln!(
                out,
                "  counted while an unfinished wipe of {names} stands; the wipe destroyed every embedding before it began, so this figure is what has been re-embedded since, not the one from before it"
            );
        } else if !unfinished.is_empty() {
            let _ = writeln!(
                out,
                "  counted while an unfinished rebuild of {names} stands; nothing was destroyed, so this figure is the one from before it"
            );
        }
    } else {
        // Absent for three different reasons, and a person acts on each of
        // them differently, so none of them may print as "no index yet".
        match &report.index {
            IndexAccess::Daemon => {
                let _ = writeln!(
                    out,
                    "embeddings: not read here, the running daemon owns the index; run: crystalline status"
                );
            }
            IndexAccess::Unavailable { .. } => {
                let _ = writeln!(
                    out,
                    "embeddings: not read, the index checks did not run (see the index section above)"
                );
            }
            IndexAccess::Absent | IndexAccess::Direct => {
                let _ = writeln!(out, "embeddings: no index yet");
            }
        }
    }

    if let Some(c) = &report.contradictions {
        match c["model"].as_str() {
            None => {
                let _ = writeln!(out, "contradictions: off");
            }
            Some(model) => {
                let state = match (c["downloaded"].as_bool(), c["reason"].as_str()) {
                    (Some(true), _) => "downloaded".to_string(),
                    (_, Some(reason)) => format!("not downloaded: {reason}"),
                    _ => "not downloaded".to_string(),
                };
                let pending = match c["pending_pairs"].as_u64() {
                    Some(n) => format!("{n} {} pending", crate::cmd::pair_word(n)),
                    None => "not counted yet".to_string(),
                };
                let mut line = format!(
                    "contradictions: {}, {model} ({state}), {pending}",
                    c["profile"].as_str().unwrap_or_default()
                );
                if let Some(reason) = crate::cmd::contradiction_wait_reason(c) {
                    line.push_str(&format!(", {reason}"));
                }
                let _ = writeln!(out, "{line}");
                if let Some(remedy) = crate::cmd::contradiction_reload_remedy(c) {
                    for l in remedy {
                        let _ = writeln!(out, "{l}");
                    }
                }
            }
        }
        if let Some(stale) = c["stale_checkpoints"].as_array().filter(|s| !s.is_empty()) {
            let names: Vec<&str> = stale.iter().filter_map(|v| v.as_str()).collect();
            let _ = writeln!(
                out,
                "  stale NLI checkpoint(s) no profile uses now, still on disk: {}",
                names.join(", ")
            );
        }
    }

    if let Some(harnesses) = &report.harnesses {
        let _ = writeln!(out, "harnesses:");
        for h in harnesses {
            let _ = writeln!(out, "  {} ({})", h.name, h.settings_path);
            if let Some(err) = &h.settings_parse_error {
                let _ = writeln!(out, "    [problem] settings file is not valid JSON: {err}");
            } else if !h.settings_present {
                let _ = writeln!(out, "    not installed (no settings/hooks file yet)");
            } else {
                out.push_str(&hook_lines(h));
            }
            if h.skills_installed > 0 {
                let modified = if h.skills_modified > 0 {
                    format!(", {} locally modified", h.skills_modified)
                } else {
                    String::new()
                };
                let _ = writeln!(
                    out,
                    "    skills: {}/{} installed{modified}",
                    h.skills_installed,
                    install::managed_skills().len()
                );
            } else {
                let _ = writeln!(out, "    skills: none installed");
            }
            if let Some(v) = &h.receipt_version
                && v != env!("CARGO_PKG_VERSION")
            {
                let _ = writeln!(
                    out,
                    "    installed by {v}, this binary is {} (refreshes at next session start)",
                    env!("CARGO_PKG_VERSION")
                );
            }
            for name in &h.retired_leftovers {
                let _ = writeln!(
                    out,
                    "    leftover retired skill: {name} (crystalline install removes it)"
                );
            }
        }
    }

    if let Some(p) = &report.provisioning {
        let _ = writeln!(out, "provisioning:");
        for d in &p.domains {
            let _ = writeln!(
                out,
                "  {}: {}, {}",
                d.name,
                d.decision,
                render_provision_counts(&d.counts)
            );
            if let Some(mirror) = d.mirror_present {
                let _ = writeln!(
                    out,
                    "    artifact mirror: {}",
                    if mirror {
                        "present"
                    } else {
                        "not pulled down yet"
                    }
                );
            }
        }
        for h in &p.harnesses {
            let _ = writeln!(
                out,
                "  {}: {} file(s) installed, {} mcp(s) installed, {} drifted, {} edited, {} orphaned, {} missing",
                h.harness,
                h.installed_files,
                h.installed_mcps,
                h.drift,
                h.edited,
                h.orphaned,
                h.missing
            );
        }
        if !p.pending.is_empty() {
            let _ = writeln!(out, "  awaiting a decision:");
            for pd in &p.pending {
                let _ = writeln!(
                    out,
                    "    {}: {} - run `crystalline provision allow {}`.",
                    pd.domain,
                    render_provision_counts(&pd.counts),
                    pd.domain
                );
            }
        }
    }

    if let Some(names) = &report.names {
        render_names(&mut out, names, report.fix);
    }

    if let Some(rename) = &report.rename {
        render_rename(&mut out, rename);
    }

    // Advisory tag hygiene: near-duplicate clusters, never a counted problem.
    if let Some(t) = &report.tags
        && !t.clusters.is_empty()
    {
        let _ = writeln!(out, "tags:");
        for c in &t.clusters {
            let _ = writeln!(
                out,
                "  near-duplicate: {} ({}) - consolidate with `crystalline tags merge`.",
                c.tags.join(", "),
                c.reason
            );
        }
    }

    let remaining = report.remaining_problems();
    let _ = writeln!(out, "{remaining} problem(s) remaining");
    out
}

/// Render an artifact-kind-to-count map as `"2 skills, 1 mcps"`, or `"no
/// artifacts"` when empty - the doctor-local twin of `cmd::format_counts`,
/// operating on the typed map `provision::status` returns rather than a JSON
/// value.
/// One line for one domain the index holds rows for and nobody registers.
///
/// Every branch is written to stay true of the state it describes: only a row
/// something will actually collect is promised a collection, and a reason this
/// build cannot read gets a sentence that asserts nothing about it.
fn orphaned_domain_line(d: &OrphanedDomainDoctor) -> String {
    let name = &d.name;
    let engrams = d.engrams;
    let age = match d.age_days {
        Some(days) => format!("last seen registered {days} day(s) ago"),
        // Not an age of zero: an index inherited from a version that never
        // recorded a registration has no evidence either way.
        None => "never seen registered by this version".to_string(),
    };
    if d.collected {
        let row = if d.row_dropped {
            ", and dropped its empty row so the name is free again"
        } else {
            ""
        };
        return format!(
            "  collected {engrams} engram row(s) of '{name}' ({age}); the files on disk are untouched{row}"
        );
    }
    // An empty row left behind, a file domain's or a virtual one's: the only
    // thing at stake is the name it holds.
    if !d.collectable {
        if d.row_dropped {
            return format!(
                "  dropped the empty row of removed domain '{name}'; the name is free again"
            );
        }
        if d.row_droppable {
            return format!(
                "  {name}: an empty row left behind by a removed domain ({age}). It holds the name '{name}' against a rename or an adoption; `crystalline doctor --fix` drops it"
            );
        }
    }
    if d.collectable {
        return format!(
            "  [problem] {name}: {engrams} engram row(s), {age}. They are not served any more and will be collected; to clear them now run: crystalline doctor --fix"
        );
    }
    match d.kept.as_deref().and_then(KeptReason::from_word) {
        Some(KeptReason::Virtual) => format!(
            "  {name}: {engrams} engram row(s) in a virtual domain, {age}. They are not served any more, and a virtual domain's rows are its only copy, so nothing collects them on its own: end it with `crystalline domain remove {name} --purge`, which asks first"
        ),
        // The live peer is serving these rows. Nothing here will ever collect
        // them, on either path, so nothing here may say it will.
        Some(KeptReason::HostedElsewhere) => format!(
            "  {name}: {engrams} engram row(s), {age}. Another instance hosts this domain over the shared database and is still serving those rows, so they are not this instance's to collect"
        ),
        // A read-only instance collects nothing at all. The skipped line below
        // says the same thing about the run; this says it about the rows,
        // without promising a collection that needs a writable instance.
        Some(KeptReason::ReadOnly) => format!(
            "  {name}: {engrams} engram row(s), {age}. This instance is read-only and collects nothing: they stay until a writable instance sweeps them, or until `crystalline doctor --fix` is run against one"
        ),
        // Neither reaches a `doctor` run (both need a grace period, and both
        // doctor routes ask on the on-demand path), but both are honest about
        // a domain that is only waiting.
        Some(KeptReason::Grace) | Some(KeptReason::Unstamped) => format!(
            "  {name}: {engrams} engram row(s), {age}. They are not served any more and will be collected"
        ),
        // Filtered out before the render; a line that claims nothing is the
        // right answer if one ever arrives here anyway.
        Some(KeptReason::NoRows) => {
            format!("  {name}: no engram rows left, {age}. There is nothing here to collect")
        }
        // A reason this build does not know. Say only what is true of every
        // kept row: this instance is not serving them and is not collecting
        // them, and name the word so the reader can look it up.
        None => {
            let word = d.kept.as_deref().unwrap_or("no reason given");
            format!(
                "  {name}: {engrams} engram row(s), {age}. They are not served any more, and this instance is not collecting them ({word})"
            )
        }
    }
}

/// The domain name section: nothing at all when there is nothing to say.
fn render_names(out: &mut String, names: &NamesDoctor, fix: bool) {
    use std::fmt::Write as _;
    let lines = name_lines(names, fix);
    if lines.is_empty() {
        return;
    }
    let _ = writeln!(out, "domain names:");
    for line in lines {
        let _ = writeln!(out, "  {line}");
    }
}

/// One line per name finding, each naming its next step. Only the
/// local-only spellings line is marked a problem; the rest are warnings and
/// hints.
fn name_lines(names: &NamesDoctor, fix: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(err) = &names.error {
        lines.push(format!("not checked: {err}"));
        return lines;
    }
    for s in &names.shadowed {
        let c = &s.canonical;
        lines.push(format!(
            "domain '{}' says its name is '{c}', but '{c}' is another domain here; links that name '{c}' reach that one. Rename one of them: crystalline domain rename <domain> <new>",
            s.domain
        ));
    }
    for c in &names.conflicts {
        let quoted: Vec<String> = c.claimants.iter().map(|n| format!("'{n}'")).collect();
        let (who, reach) = match quoted.as_slice() {
            [a, b] => (format!("{a} and {b} both"), "neither"),
            [rest @ .., last] => (
                format!("{} and {last} all", rest.join(", ")),
                "none of them",
            ),
            [] => (String::new(), "none of them"),
        };
        lines.push(format!(
            "domains {who} call themselves '{}'; links that name '{}' reach {reach}. Rename one of them",
            c.name, c.name
        ));
    }
    for d in &names.dropped_aliases {
        lines.push(match &d.held_by {
            Some(held_by) => format!(
                "alias '{}' of '{}' is ignored: '{}' already names '{held_by}'",
                d.alias, d.domain, d.alias
            ),
            None => format!(
                "alias '{}' of '{}' is ignored: more than one domain here answers to '{}'",
                d.alias, d.domain, d.alias
            ),
        });
    }
    for a in &names.adoption_pending {
        let why = match &a.reason {
            Some(reason) => reason.clone(),
            None => format!(
                "the next sync tries the rename again; if it keeps failing, the daemon log says why, or run: crystalline domain rename {} {} --local",
                a.domain, a.canonical
            ),
        };
        lines.push(format!(
            "domain '{}' says its name is '{}' but is still called '{}' here: {why}",
            a.domain, a.canonical, a.domain
        ));
    }
    for t in &names.team_without_domain_name {
        lines.push(format!(
            "team domain '{}' declares no domain_name in its MANIFEST, so each colleague may name it differently. Ask the owner to add `domain_name: {}` to the MANIFEST",
            t.domain, t.domain
        ));
    }
    if let Some(fixed) = names.fixed
        && fixed > 0
    {
        lines.push(format!(
            "wrote the domain's name into {fixed} link(s) that named a domain by a name only this machine uses"
        ));
    }
    if let Some(err) = &names.fix_error {
        lines.push(format!(
            "could not write the domain's name into the links: {err}"
        ));
    }
    let (drafted, open): (Vec<&LocalSpellingDoctor>, Vec<&LocalSpellingDoctor>) =
        names.local_spellings.iter().partition(|s| s.in_draft);
    if !drafted.is_empty() {
        let total: u64 = drafted.iter().map(|s| s.count).sum();
        lines.push(format!(
            "{total} link(s) that name a domain by a name only this machine uses are fixed in a draft that waits for review"
        ));
        for s in &drafted {
            lines.push(format!(
                "  {}/{}: {} x '{}', the domain's name is '{}'",
                s.domain, s.path, s.count, s.spelling, s.canonical
            ));
        }
    }
    if !open.is_empty() {
        let total: u64 = open.iter().map(|s| s.count).sum();
        let tail = match (fix, &names.fix_error) {
            (false, _) => "rerun with --fix to write the domain's name instead",
            (true, Some(_)) => "--fix could not write them, see the line above",
            (true, None) => {
                "these files could not be written; check that they can be, then rerun with --fix"
            }
        };
        lines.push(format!(
            "[problem] {total} links name a domain by a name only this machine uses; {tail}"
        ));
        for s in &open {
            lines.push(format!(
                "  {}/{}: {} x '{}', the domain's name is '{}'",
                s.domain, s.path, s.count, s.spelling, s.canonical
            ));
        }
    }
    lines
}

fn render_provision_counts(counts: &BTreeMap<String, usize>) -> String {
    if counts.is_empty() {
        return "no artifacts".to_string();
    }
    counts
        .iter()
        .map(|(kind, n)| format!("{n} {kind}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crystalline_core::config::EvolveConfig;
    use crystalline_index::{TursoStore, sync_domain};

    fn owner(index: &str, config: &str, state_dir: &str) -> crystalline_service::RenameOwner {
        crystalline_service::RenameOwner {
            index: Some(index.to_string()),
            config: Some(config.to_string()),
            state_dir: state_dir.to_string(),
        }
    }

    fn pending(owner: crystalline_service::RenameOwner) -> crystalline_service::PendingRename {
        crystalline_service::PendingRename {
            old: "eng".to_string(),
            new: "platform".to_string(),
            local_only: true,
            owner: Some(owner),
            done: Vec::new(),
            remaining: Vec::new(),
        }
    }

    /// A journal recorded against a Postgres url written differently is not
    /// finished by a command pointed at it (only this machine's own index
    /// ever finishes a journal): doctor says to put the url back, then run
    /// the plain command.
    #[test]
    fn a_journal_on_a_postgres_url_written_differently_names_the_url_to_put_back() {
        let here = owner("localhost:5432/kb", "/home/a/config.yaml", "/state");
        let recorded = owner("127.0.0.1:5432/kb", "/home/a/config.yaml", "/state");
        let r = rename_doctor(&pending(recorded), Some(&here));
        assert!(!r.belongs_here);
        assert_eq!(r.restore.len(), 1, "{:?}", r.restore);
        assert!(
            r.restore[0].contains("database.url") && r.restore[0].contains("127.0.0.1:5432/kb"),
            "{:?}",
            r.restore
        );
        assert_eq!(
            r.finish.as_deref(),
            Some("crystalline domain rename eng platform --local")
        );
        let mut out = String::new();
        render_rename(&mut out, &r);
        assert!(!out.contains("--db") && !out.contains("--config"), "{out}");
    }

    /// A journal recorded in another state directory (a moved one) cannot be
    /// put back from here at all: doctor names only --discard-rename.
    #[test]
    fn a_journal_from_a_moved_state_directory_can_only_be_dropped() {
        let here = owner("/new/state/index.db", "/home/a/config.yaml", "/new/state");
        let recorded = owner("/old/state/index.db", "/home/a/config.yaml", "/old/state");
        let r = rename_doctor(&pending(recorded), Some(&here));
        assert!(!r.belongs_here);
        assert!(r.finish.is_none() && r.restore.is_empty(), "{r:?}");
        let mut out = String::new();
        render_rename(&mut out, &r);
        assert!(
            out.contains("--discard-rename") && !out.contains("run:"),
            "{out}"
        );
    }

    /// A minimal harness fixture: only the fields [`hook_lines`] reads are
    /// worth setting, everything else keeps its `Default`.
    fn harness(
        name: &str,
        session_start_hook: bool,
        stop_hook: bool,
        prompt_hook: Option<bool>,
    ) -> HarnessDoctor {
        HarnessDoctor {
            name: name.to_string(),
            session_start_hook,
            stop_hook,
            prompt_hook,
            ..Default::default()
        }
    }

    /// The "partial setup" line fires whenever some but not all of a
    /// harness's applicable hooks are present, across all three hooks now -
    /// and Copilot's `UserPromptSubmit` hook, permanently inert rather than
    /// merely not-yet-installed, never enters that count: a Copilot entry
    /// with the prompt hook missing (or present) alongside two present hooks
    /// must never read as a partial setup, while the same shape for Claude
    /// Code or Codex must.
    #[test]
    fn partial_setup_fires_across_three_hooks_and_not_for_copilot_missing_a_prompt_hook() {
        // Claude Code: all three present, no partial-setup line.
        let complete = harness("claude-code", true, true, Some(true));
        assert!(!hook_lines(&complete).contains("partial setup"));

        // Claude Code: the prompt hook alone is missing - a partial setup.
        let missing_prompt = harness("claude-code", true, true, Some(false));
        let lines = hook_lines(&missing_prompt);
        assert!(lines.contains("UserPromptSubmit hook: absent"));
        assert!(
            lines.contains("partial setup - run: crystalline install claude-code"),
            "{lines}"
        );

        // Codex: the SessionStart hook alone is missing - still a partial
        // setup, the prompt hook counts toward the total for a non-Copilot
        // harness.
        let missing_session_start = harness("codex", false, true, Some(true));
        assert!(hook_lines(&missing_session_start).contains("partial setup"));

        // Copilot with both real hooks but no prompt hook: the inert note
        // still prints on the absent line too (a hand-deleted entry is
        // exactly as inert as a present one), and it is still not a partial
        // setup, since Copilot's prompt hook never enters the count.
        let copilot_no_prompt = harness("copilot", true, true, Some(false));
        let lines = hook_lines(&copilot_no_prompt);
        assert!(
            lines.contains(
                "UserPromptSubmit hook: absent (Copilot does not honour a config-file prompt hook's output today)"
            ),
            "{lines}"
        );
        assert!(!lines.contains("partial setup"), "{lines}");

        // Copilot with the prompt hook present carries the inert note and
        // still never trips partial setup.
        let copilot_with_prompt = harness("copilot", true, true, Some(true));
        let lines = hook_lines(&copilot_with_prompt);
        assert!(
            lines.contains(
                "UserPromptSubmit hook: present (Copilot does not honour a config-file prompt hook's output today)"
            ),
            "{lines}"
        );
        assert!(!lines.contains("partial setup"), "{lines}");

        // Copilot missing one of its two counted hooks is still a partial
        // setup, whatever its prompt hook says.
        let copilot_missing_stop = harness("copilot", true, false, Some(true));
        assert!(hook_lines(&copilot_missing_stop).contains("partial setup"));
    }

    /// Sync a temp file domain holding a MANIFEST (whose tail is `manifest_tail`,
    /// so a test can add a `## Tag Aliases` section) plus two engrams whose tags
    /// are a near-duplicate pair, then return its doctor tag diagnostics.
    async fn tags_doctor_over(manifest_tail: &str) -> TagsDoctor {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("MANIFEST.md"),
            format!(
                "---\ntype: manifest\ntitle: KB\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# KB\n\n## Scope\n\n- kb\n\n## When to Use\n\n- routing\n{manifest_tail}"
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("a.md"),
            "---\ntype: engram\ntitle: A\npermalink: a\ntags:\n  - colours\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody a\n",
        )
        .unwrap();
        std::fs::write(
            root.join("b.md"),
            "---\ntype: engram\ntitle: B\npermalink: b\ntags:\n  - colour\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody b\n",
        )
        .unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        sync_domain(&store, "kb", root).await.unwrap();
        let store_ref: &dyn Store = &store;
        let mut cfg = GlobalConfig::default();
        cfg.domains
            .insert("kb".to_string(), DomainEntry::file(root.to_path_buf()));
        check_tags(Some(store_ref), &cfg).await.unwrap().unwrap()
    }

    /// The tag check's advice points the reader at `crystalline tags merge`,
    /// and that verb will not touch a domain this instance has no registration
    /// for - so a cluster drawn from one is advice that cannot be taken.
    #[tokio::test]
    async fn an_unregistered_domains_tags_are_not_advised_on() {
        fn engram(tag: &str) -> String {
            format!(
                "---\ntype: engram\ntitle: {tag}\npermalink: {tag}\ntags:\n  - {tag}\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n"
            )
        }
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join("kept");
        let orphan = dir.path().join("orphan");
        for (root, pair) in [
            (&kept, ["colours", "colour"]),
            (&orphan, ["flavours", "flavour"]),
        ] {
            std::fs::create_dir_all(root).unwrap();
            for tag in pair {
                std::fs::write(root.join(format!("{tag}.md")), engram(tag)).unwrap();
            }
        }
        let store = TursoStore::open_in_memory().await.unwrap();
        sync_domain(&store, "kept", &kept).await.unwrap();
        sync_domain(&store, "orphan", &orphan).await.unwrap();
        let mut cfg = GlobalConfig::default();
        cfg.domains
            .insert("kept".to_string(), DomainEntry::file(kept.clone()));

        let store_ref: &dyn Store = &store;
        let doctor = check_tags(Some(store_ref), &cfg).await.unwrap().unwrap();
        assert!(
            doctor
                .clusters
                .iter()
                .any(|c| c.tags.contains(&"colours".to_string())),
            "the registered domain's tag drift is still reported: {:?}",
            doctor.clusters
        );
        assert!(
            !doctor
                .clusters
                .iter()
                .any(|c| c.tags.iter().any(|t| t.starts_with("flavour"))),
            "the unregistered domain's is not: {:?}",
            doctor.clusters
        );
    }

    #[tokio::test]
    async fn near_duplicate_tags_cluster_without_an_alias() {
        let doctor = tags_doctor_over("").await;
        assert!(
            doctor.clusters.iter().any(|c| {
                c.tags.contains(&"colours".to_string()) && c.tags.contains(&"colour".to_string())
            }),
            "colours and colour cluster when no alias explains them: {:?}",
            doctor.clusters
        );
    }

    /// One file domain with `orphans` recorded and nothing else wrong, read
    /// through `index`.
    fn report_with_orphans(index: IndexAccess, orphans: &[&str]) -> DoctorReport {
        DoctorReport {
            index,
            domains: vec![DomainDoctor {
                name: "eng".to_string(),
                kind: "file".to_string(),
                path: "/kb/eng".to_string(),
                index_checked: true,
                path_exists: true,
                manifest_present: true,
                orphans: orphans.iter().map(|p| p.to_string()).collect(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// A rebuild marker that outlived the run that set it: the finding names
    /// the instant and the command that finishes it, says plainly that the
    /// rows are still there, counts toward the exit code, and puts the caveat
    /// on the embedding coverage figure rather than letting it go out bare -
    /// the number read as normal mid-rebuild is the incident this exists for.
    #[test]
    fn an_unfinished_rebuild_is_a_problem_that_qualifies_the_coverage_figure() {
        let mut report = report_with_orphans(IndexAccess::Direct, &[]);
        report.domains[0].rebuild_started = Some("2026-09-14T09:00:00Z".to_string());
        report.domains[0].rebuild_kind = Some("full".to_string());
        report.embeddings = Some(serde_json::json!({
            "embedded_with_configured_model": 768,
            "total_chunks": 23598,
            "configured_model": "m",
            "stale_chunks": 0,
        }));

        let out = render_human(&report);
        assert!(
            out.contains(
                "[problem] a full rebuild started 2026-09-14T09:00:00Z has not finished; this domain's rows are the ones from before it. Run: crystalline reindex --full"
            ),
            "{out}"
        );
        assert!(
            out.contains("counted while an unfinished rebuild of eng stands"),
            "the coverage figure never goes out bare while a rebuild is unfinished: {out}"
        );
        assert_eq!(
            report.remaining_problems(),
            1,
            "an unfinished rebuild is one problem, so doctor exits non-zero"
        );

        // A wipe is the opposite verb: it destroyed the rows and every
        // embedding before it started, so neither sentence may reassure.
        report.domains[0].rebuild_kind = Some("wipe".to_string());
        let wiped = render_human(&report);
        assert!(
            wiped.contains(
                "[problem] a wipe started 2026-09-14T09:00:00Z has not finished; this domain's rows and every embedding it had were destroyed before it began. Run: crystalline reindex --full"
            ),
            "{wiped}"
        );
        assert!(
            !wiped.contains("nothing was destroyed") && !wiped.contains("the ones from before it"),
            "the coverage caveat says the figure is what has been re-embedded since: {wiped}"
        );
        // A marker a binary older than the kind column stamped says what is
        // known and claims nothing about the rows either way.
        report.domains[0].rebuild_kind = None;
        let unknown = render_human(&report);
        assert!(
            unknown.contains(
                "[problem] a rebuild started 2026-09-14T09:00:00Z has not finished. Run: crystalline reindex --full"
            ),
            "{unknown}"
        );

        // Cleared, it is neither a finding nor a caveat.
        report.domains[0].rebuild_started = None;
        let clean = render_human(&report);
        assert!(!clean.contains("has not finished"), "{clean}");
        assert!(!clean.contains("unfinished rebuild"), "{clean}");
        assert_eq!(report.remaining_problems(), 0);
    }

    /// Doctor names the repo and size behind the configured model id, and
    /// lists what else is sitting in the model cache so a person can see the
    /// old weights before the next daemon start removes them.
    #[test]
    fn the_embeddings_section_names_the_repo_and_the_cached_models() {
        let mut report = report_with_orphans(IndexAccess::Direct, &[]);
        report.embeddings = Some(serde_json::json!({
            "embedded_with_configured_model": 23598,
            "total_chunks": 23598,
            "configured_model": "granite-embedding-97m-multilingual-r2",
            "configured_repo": "ibm-granite/granite-embedding-97m-multilingual-r2",
            "configured_model_bytes": 220_206_187u64,
            "stale_chunks": 0,
            "cached_models": [
                { "repo": "BAAI/bge-small-en-v1.5", "bytes": 133_169_152u64, "stale": true },
                { "repo": "ibm-granite/granite-embedding-97m-multilingual-r2", "bytes": 220_206_187u64, "stale": false },
            ],
        }));

        let out = render_human(&report);
        assert!(
            out.contains("ibm-granite/granite-embedding-97m-multilingual-r2"),
            "{out}"
        );
        assert!(
            out.contains("220 MB"),
            "the configured model's size is beside its id: {out}"
        );
        assert!(out.contains("cached models:"), "{out}");
        assert!(
            out.contains("BAAI/bge-small-en-v1.5 133 MB [stale]"),
            "a cached model the config does not use is marked: {out}"
        );
        assert!(
            !out.contains("ibm-granite/granite-embedding-97m-multilingual-r2 220 MB [stale]"),
            "the model in use is not marked stale: {out}"
        );
    }

    /// The pre-existing shape, with none of the new keys, still renders: the
    /// report comes from a daemon that may be older than this binary.
    #[test]
    fn an_embeddings_section_without_the_new_keys_still_renders() {
        let mut report = report_with_orphans(IndexAccess::Direct, &[]);
        report.embeddings = Some(serde_json::json!({
            "embedded_with_configured_model": 10,
            "total_chunks": 20,
            "configured_model": "m",
            "stale_chunks": 0,
        }));
        let out = render_human(&report);
        assert!(out.contains("10/20 chunks embedded with 'm'"), "{out}");
        assert!(!out.contains("cached models:"), "{out}");
    }

    /// Over a daemon the doctor reads the index through a read verb, so
    /// `--fix` cannot delete an orphan row however it is spelled. The finding
    /// is still reported, with what removing it actually takes, rather than
    /// pointing at a flag that would silently do nothing.
    #[test]
    fn an_orphan_found_over_a_daemon_says_what_removing_it_takes() {
        let daemon = render_human(&report_with_orphans(IndexAccess::Daemon, &["gone.md"]));
        assert!(
            daemon.contains("[problem] 1 orphan row(s) (file missing on disk): gone.md."),
            "{daemon}"
        );
        assert!(
            daemon.contains(
                "The running daemon owns the index, so removing them needs it stopped: run `crystalline ctl shutdown`, then `crystalline doctor --fix`"
            ),
            "{daemon}"
        );
        assert!(
            !daemon.contains("rerun with --fix to remove"),
            "the direct path's advice would be false here: {daemon}"
        );

        let direct = render_human(&report_with_orphans(IndexAccess::Direct, &["gone.md"]));
        assert!(
            direct.contains("rerun with --fix to remove: gone.md"),
            "the direct path keeps the advice that works there: {direct}"
        );
    }

    /// An index nobody could read counts once for the machine, so the exit
    /// code says something is wrong without inflating the count by one per
    /// domain.
    #[test]
    fn an_unreadable_index_counts_as_exactly_one_problem() {
        let mut report = DoctorReport {
            index: IndexAccess::Unavailable {
                reason: "the index at /kb/index.db could not be opened".to_string(),
            },
            domains: vec![
                DomainDoctor {
                    name: "eng".to_string(),
                    path_exists: true,
                    manifest_present: true,
                    ..Default::default()
                },
                DomainDoctor {
                    name: "docs".to_string(),
                    path_exists: true,
                    manifest_present: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(report.remaining_problems(), 1);

        report.index = IndexAccess::Daemon;
        assert_eq!(
            report.remaining_problems(),
            0,
            "a run served by the daemon read the index, so there is nothing to report"
        );
    }

    #[tokio::test]
    async fn a_declared_alias_suppresses_its_cluster() {
        let doctor = tags_doctor_over("\n## Tag Aliases\n\n- colours -> colour\n").await;
        assert!(
            !doctor
                .clusters
                .iter()
                .any(|c| c.tags.contains(&"colours".to_string())),
            "the alias folds colours onto colour, so the cluster is suppressed: {:?}",
            doctor.clusters
        );
    }

    // `relative_slash_path` - the E001-to-unindexed matching helper. These run
    // on every platform, including the Windows CI leg the regression showed up
    // on: a duplicate-key finding was silently falling back into `unindexed`
    // instead of `unsyncable` because a failed `strip_prefix` used to keep the
    // absolute path, which can never equal a relative entry.

    #[test]
    fn relative_slash_path_matches_a_nested_file_under_the_same_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let nested = root.join("a").join("b").join("c.md");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, "x").unwrap();

        // The fast path: `nested` was built by literally joining `root`, the
        // shape every real caller (`markdown_rel_paths`'s own walk) produces,
        // so this must resolve without ever touching the canonicalize fallback.
        assert_eq!(relative_slash_path(root, &nested), "a/b/c.md");
    }

    #[test]
    fn relative_slash_path_falls_back_to_a_file_name_rather_than_an_absolute_path() {
        // A multi-component absolute path (forward slashes parse as
        // separators on every platform, Windows included) that shares no
        // component prefix with `root` and does not exist, so both the fast
        // path and the canonicalize fallback miss. This is the "verbatim
        // prefix disagrees with a plain root" failure's general shape - two
        // absolute paths that cannot be reconciled at all - proving the
        // degrade-safely half: a total non-match still returns a short,
        // non-empty, root-relative-looking value (just the file name) rather
        // than the full absolute string the old code kept on a failed strip,
        // which could never equal a relative `unindexed` entry and was the
        // bug. The Windows-specific `\\?\C:\...` prefix parsing itself is not
        // reproducible on a non-Windows path (backslashes are plain filename
        // characters there, not separators) - that trigger is only provable
        // by Windows CI; this test and the next one prove the fallback
        // mechanism handles a genuine mismatch correctly once one occurs.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let unrelated = Path::new("/definitely/not/under/root/a/b/bad.md");

        let result = relative_slash_path(root, unrelated);
        assert_eq!(
            result, "bad.md",
            "falls back to just the file name, never the full absolute path"
        );
    }

    #[cfg(unix)]
    #[test]
    fn relative_slash_path_matches_when_the_root_and_the_path_reach_the_same_file_through_different_forms()
     {
        // A symlink stands in for the Windows failure's actual shape: two
        // absolute paths, both real and both naming the same file, that do
        // not share a literal component prefix (the verbatim-prefixed root
        // config canonicalized against a plain re-walk, there; a symlinked
        // root against its real target, here). `strip_prefix` fails on both
        // for the same reason - the component sequences genuinely differ -
        // and `dunce::canonicalize` is what reconciles them in both cases, so
        // this proves the fallback mechanism the Windows fix relies on
        // actually works, even though it cannot reproduce the Windows-only
        // verbatim-prefix trigger itself (Windows CI is the proof for that).
        let dir = tempfile::tempdir().unwrap();
        let real_root = dir.path().join("real");
        std::fs::create_dir_all(real_root.join("a").join("b")).unwrap();
        std::fs::write(real_root.join("a").join("b").join("c.md"), "x").unwrap();

        let linked_root = dir.path().join("linked");
        std::os::unix::fs::symlink(&real_root, &linked_root).unwrap();

        // `root` is given through the symlink; `p` is the file's canonical,
        // non-symlinked path - the same mismatch shape a canonicalized config
        // path and a plain re-walked one would produce.
        let p = real_root.join("a").join("b").join("c.md");
        assert_eq!(relative_slash_path(&linked_root, &p), "a/b/c.md");
    }

    // --- rows whose domain nobody registers any more --------------------------

    /// A report carrying nothing but one orphaned-domain section, which is
    /// what the render and the problem count are read on below. The binary
    /// tests cover the never-stamped case a real 0.17.0 index produces; these
    /// cover the branches a fixture cannot age into.
    fn orphan_report(
        domains: Vec<OrphanedDomainDoctor>,
        skipped: Option<String>,
        fix: bool,
    ) -> DoctorReport {
        DoctorReport {
            orphaned_rows: Some(OrphanedRowsDoctor {
                domains,
                skipped,
                error: None,
            }),
            fix,
            ..DoctorReport::default()
        }
    }

    fn orphan(name: &str, engrams: i64, age_days: Option<i64>) -> OrphanedDomainDoctor {
        OrphanedDomainDoctor {
            name: name.to_string(),
            kind: "file".to_string(),
            engrams,
            age_days,
            collectable: true,
            collected: false,
            kept: None,
            row_dropped: false,
            row_droppable: false,
        }
    }

    /// The message 0.17.0 got wrong: it told the truth about the rows and then
    /// named the heaviest command in the tool as the only way out. The honest
    /// text says the rows answer nothing any more and names the one command
    /// that ends them now.
    #[test]
    fn an_aged_orphan_is_named_with_its_age_and_a_proportionate_remedy() {
        let report = orphan_report(vec![orphan("gone", 30, Some(13))], None, false);
        let out = render_human(&report);
        assert!(out.contains("gone: 30 engram row(s)"), "{out}");
        assert!(out.contains("last seen registered 13 day(s) ago"), "{out}");
        assert!(out.contains("not served any more"), "{out}");
        assert!(out.contains("crystalline doctor --fix"), "{out}");
        assert!(
            !out.to_lowercase().contains("reindex"),
            "a full reindex is never the advice: {out}"
        );
        assert_eq!(
            report.remaining_problems(),
            1,
            "rows nobody has collected yet are one problem apiece"
        );
    }

    /// Collected is not a problem: the run that was asked did the work, and it
    /// says what it did without claiming anything about the files on disk,
    /// which it never touched.
    #[test]
    fn a_collected_orphan_is_reported_and_not_counted() {
        let mut row = orphan("gone", 30, Some(13));
        row.collected = true;
        let report = orphan_report(vec![row], None, true);
        let out = render_human(&report);
        assert!(
            out.contains("collected 30 engram row(s) of 'gone'"),
            "{out}"
        );
        assert!(out.contains("files on disk are untouched"), "{out}");
        assert_eq!(report.remaining_problems(), 0);
    }

    /// A virtual domain's rows are the knowledge itself rather than a copy of
    /// it, so no sweep and no `--fix` ends them: the line names the command
    /// that asks first, and counting it a problem would fail doctor forever
    /// over a state with no remedy here.
    #[test]
    fn a_virtual_orphan_is_reported_and_never_counted() {
        let row = OrphanedDomainDoctor {
            name: "vault".to_string(),
            kind: "virtual".to_string(),
            engrams: 12,
            age_days: Some(409),
            collectable: false,
            collected: false,
            kept: Some("virtual".to_string()),
            row_dropped: false,
            row_droppable: false,
        };
        let report = orphan_report(vec![row], None, false);
        let out = render_human(&report);
        assert!(
            out.contains("vault: 12 engram row(s) in a virtual domain"),
            "{out}"
        );
        assert!(
            out.contains("crystalline domain remove vault --purge"),
            "{out}"
        );
        assert!(
            !out.contains("crystalline doctor --fix"),
            "a --fix that would do nothing is not offered: {out}"
        );
        assert_eq!(report.remaining_problems(), 0);
    }

    /// A read-only instance collects nothing and says so, with the rows it
    /// would have collected still named: that operator is exactly the one who
    /// wants to know what is sitting there.
    #[test]
    fn a_read_only_instance_lists_the_rows_and_says_nothing_was_collected() {
        let row = OrphanedDomainDoctor {
            collectable: false,
            kept: Some("read_only".to_string()),
            ..orphan("gone", 30, Some(13))
        };
        let report = orphan_report(
            vec![row],
            Some(
                "this instance is read-only; the registered domains were stamped and nothing was \
                 removed"
                    .to_string(),
            ),
            false,
        );
        let out = render_human(&report);
        assert!(out.contains("gone: 30 engram row(s)"), "{out}");
        assert!(
            out.contains("This instance is read-only and collects nothing"),
            "the row says what will happen to it: {out}"
        );
        assert!(
            out.contains("until a writable instance sweeps them")
                && out.contains("`crystalline doctor --fix` is run against one"),
            "and where a collection can be had: {out}"
        );
        assert!(
            !out.contains("will be collected;"),
            "never a collection this instance will not make: {out}"
        );
        assert!(
            out.contains("nothing was collected: this instance is read-only"),
            "and the run's own line agrees with the row's: {out}"
        );
        assert_eq!(report.remaining_problems(), 0);
    }

    /// A domain another instance hosts over a shared database is being served
    /// by that instance right now, and the engine guards its rows on both
    /// paths, so the two things the old fallthrough said about it - not served,
    /// and due for collection - were both false.
    #[test]
    fn a_domain_hosted_by_another_instance_is_its_peers_to_serve() {
        let row = OrphanedDomainDoctor {
            collectable: false,
            kept: Some("hosted_elsewhere".to_string()),
            ..orphan("shared", 30, Some(13))
        };
        let report = orphan_report(vec![row], None, false);
        let out = render_human(&report);
        assert!(
            out.contains("Another instance hosts this domain over the shared database"),
            "the reason is named: {out}"
        );
        assert!(
            out.contains("not this instance's to collect"),
            "and the consequence: {out}"
        );
        assert!(
            !out.contains("will be collected"),
            "nothing promises a collection that will never happen: {out}"
        );
        assert!(
            !out.contains("not served any more"),
            "and nothing claims the peer stopped serving them: {out}"
        );
        assert_eq!(report.remaining_problems(), 0);
    }

    /// A reason this build does not know must not borrow the sentence of one
    /// it does. The line says only what is true of every kept row and names
    /// the word.
    #[test]
    fn a_kept_reason_this_build_does_not_know_claims_nothing() {
        let row = OrphanedDomainDoctor {
            collectable: false,
            kept: Some("some_future_reason".to_string()),
            ..orphan("gone", 30, Some(13))
        };
        let out = render_human(&orphan_report(vec![row], None, false));
        assert!(
            out.contains("this instance is not collecting them (some_future_reason)"),
            "{out}"
        );
        assert!(!out.contains("will be collected"), "{out}");
    }

    /// The state Task 3 left for a container configured entirely by
    /// environment variables: nothing could be considered, so nothing may be
    /// called deregistered.
    #[test]
    fn an_unreadable_configuration_is_not_reported_as_a_deregistration() {
        let report = orphan_report(
            Vec::new(),
            Some(
                "the configuration could not be read, and a domain cannot be shown absent from a \
                 file nobody can read; nothing was stamped and nothing collected"
                    .to_string(),
            ),
            false,
        );
        let out = render_human(&report);
        assert!(
            !out.contains("domains no longer registered"),
            "a header that asserts a deregistration nobody established: {out}"
        );
        assert!(
            out.contains("not checked: the configuration could not be read"),
            "the reader is told why instead: {out}"
        );
        assert_eq!(report.remaining_problems(), 0);
    }

    /// A daemon that refused the request, or an index that failed under it,
    /// reads as a check that did not happen rather than as a clean bill.
    #[test]
    fn an_engine_failure_is_said_out_loud_rather_than_left_blank() {
        let report = DoctorReport {
            orphaned_rows: Some(OrphanedRowsDoctor {
                error: Some("unknown ctl command 'collect_orphaned_domains'".to_string()),
                ..OrphanedRowsDoctor::default()
            }),
            ..DoctorReport::default()
        };
        let out = render_human(&report);
        assert!(
            out.contains("not checked: unknown ctl command"),
            "the failure is in the report: {out}"
        );
        assert!(
            !out.contains("domains no longer registered"),
            "and it claims nothing about what is there: {out}"
        );
    }

    /// The route gate is a question about a Turso file, and only a Turso file.
    /// A Postgres install keeps its index in a server and a daemon answers
    /// over a socket, so gating either on that file suppressed the section on
    /// an install with plenty to report.
    #[test]
    fn the_route_gate_only_asks_about_a_turso_file() {
        // Turso, no daemon: the file is the whole question.
        assert!(orphan_check_has_a_route(false, true, true));
        assert!(
            !orphan_check_has_a_route(false, true, false),
            "a machine that never synced is not given an index to prove it has no orphans"
        );
        // Postgres, no daemon: there is no file to ask about.
        assert!(
            orphan_check_has_a_route(false, false, false),
            "a Postgres install is checked over its own backend"
        );
        // A daemon answers whatever it serves.
        assert!(
            orphan_check_has_a_route(true, true, false),
            "a daemon answers over its socket, file or no file"
        );
    }

    /// An empty row a removed domain left behind is a warning that says what
    /// it blocks and that `--fix` drops it; it never counts toward the exit
    /// code, before or after the drop.
    #[test]
    fn an_empty_leftover_row_is_a_warning_until_dropped() {
        let mut row = orphan("platform", 0, None);
        row.collectable = false;
        row.kept = Some("no_rows".to_string());
        row.row_droppable = true;
        let report = orphan_report(vec![row.clone()], None, false);
        let out = render_human(&report);
        assert!(
            out.contains("platform: an empty row left behind by a removed domain")
                && out.contains("`crystalline doctor --fix` drops it")
                && !out.contains("[problem] platform"),
            "{out}"
        );
        assert_eq!(report.remaining_problems(), 0);

        row.row_droppable = false;
        row.row_dropped = true;
        let report = orphan_report(vec![row], None, true);
        let out = render_human(&report);
        assert!(
            out.contains("dropped the empty row of removed domain 'platform'"),
            "{out}"
        );
        assert_eq!(report.remaining_problems(), 0);
    }

    /// A waiting adoption is a warning that says why when that is known and
    /// names the manual rename when it is not; neither is counted.
    #[test]
    fn a_waiting_adoption_is_explained_and_not_counted() {
        let report = DoctorReport {
            names: Some(NamesDoctor {
                adoption_pending: vec![
                    AdoptionPendingDoctor {
                        domain: "eng".to_string(),
                        canonical: "platform".to_string(),
                        canonical_seen: None,
                        reason: Some("run `crystalline doctor --fix` to drop it".to_string()),
                    },
                    AdoptionPendingDoctor {
                        domain: "ops".to_string(),
                        canonical: "operations".to_string(),
                        canonical_seen: None,
                        reason: None,
                    },
                ],
                ..NamesDoctor::default()
            }),
            ..DoctorReport::default()
        };
        let out = render_human(&report);
        assert!(
            out.contains(
                "domain 'eng' says its name is 'platform' but is still called 'eng' here: run `crystalline doctor --fix` to drop it"
            ),
            "{out}"
        );
        assert!(
            out.contains("crystalline domain rename ops operations --local"),
            "{out}"
        );
        assert_eq!(report.remaining_problems(), 0);
    }

    /// Local-only spellings count once per file, and a file whose fix waits
    /// in a review draft is shown as such and not counted, with no line
    /// claiming the file could not be written.
    #[test]
    fn local_spellings_count_per_file_and_a_drafted_fix_is_not_counted() {
        let entry = |path: &str, spelling: &str, in_draft: bool| LocalSpellingDoctor {
            domain: "ops".to_string(),
            path: path.to_string(),
            spelling: spelling.to_string(),
            canonical: format!("{spelling}-team"),
            count: 1,
            in_draft,
        };
        let report = DoctorReport {
            names: Some(NamesDoctor {
                local_spellings: vec![
                    entry("a.md", "eng", false),
                    entry("a.md", "ops", false),
                    entry("b.md", "eng", true),
                ],
                fixed: Some(0),
                ..NamesDoctor::default()
            }),
            fix: true,
            ..DoctorReport::default()
        };
        assert_eq!(report.remaining_problems(), 1, "one open file");
        let out = render_human(&report);
        assert!(
            out.contains("1 link(s) that name a domain by a name only this machine uses are fixed in a draft that waits for review"),
            "{out}"
        );

        let drafted_only = DoctorReport {
            names: Some(NamesDoctor {
                local_spellings: vec![entry("b.md", "eng", true)],
                fixed: Some(0),
                ..NamesDoctor::default()
            }),
            fix: true,
            ..DoctorReport::default()
        };
        assert_eq!(drafted_only.remaining_problems(), 0);
        let out = render_human(&drafted_only);
        assert!(!out.contains("could not be written"), "{out}");
        assert!(!out.contains("[problem]"), "{out}");
    }

    /// Task 7: the contradictions row - off, or the model with its download
    /// state and pending count - never counts toward the exit code, and its
    /// stale-checkpoint line is separate from the embedding row's.
    #[test]
    fn the_contradictions_row_says_off_or_names_the_model_and_never_counts_as_a_problem() {
        let mut report = report_with_orphans(IndexAccess::Direct, &[]);
        report.contradictions = Some(serde_json::json!({
            "profile": "off", "model": null, "repo": null, "downloaded": null,
            "reason": null, "pending_pairs": null, "failing_pairs": null,
            "last_error": null, "load_failed": false, "embedding_pending": false,
            "stale_checkpoints": [],
        }));
        assert!(render_human(&report).contains("contradictions: off"));

        report.contradictions = Some(serde_json::json!({
            "profile": "full", "model": "mdeberta-v3-base-xnli-2mil7",
            "repo": "MoritzLaurer/mDeBERTa-v3-base-xnli-multilingual-nli-2mil7",
            "downloaded": false, "reason": "the daemon downloads it on its first pass",
            "pending_pairs": 3, "failing_pairs": 0, "last_error": null,
            "load_failed": false, "embedding_pending": false,
            "stale_checkpoints": ["MoritzLaurer/multilingual-MiniLMv2-L12-mnli-xnli"],
        }));
        let out = render_human(&report);
        assert!(
            out.contains("contradictions: full, mdeberta-v3-base-xnli-2mil7 (not downloaded: the daemon downloads it on its first pass), 3 pairs pending"),
            "{out}"
        );
        assert!(
            out.contains("  stale NLI checkpoint(s) no profile uses now, still on disk: MoritzLaurer/multilingual-MiniLMv2-L12-mnli-xnli"),
            "{out}"
        );
        assert_eq!(
            report.remaining_problems(),
            0,
            "the row never changes the exit code"
        );
    }

    /// L1, shared with `crystalline status` through `contradiction_wait_reason`
    /// (lesson 36: doctor never recomputes its own diagnosis): a load failure
    /// and a parked batch failure render differently even though both carry
    /// `last_error`, and a pending count of zero says so when coverage is
    /// still incomplete.
    #[test]
    fn the_contradictions_row_names_the_daemons_wait_reason_like_status_does() {
        let mut report = report_with_orphans(IndexAccess::Daemon, &[]);
        report.contradictions = Some(serde_json::json!({
            "profile": "full", "model": "mdeberta-v3-base-xnli-2mil7",
            "repo": "MoritzLaurer/mDeBERTa-v3-base-xnli-multilingual-nli-2mil7",
            "downloaded": true, "reason": null, "pending_pairs": 1,
            "failing_pairs": 0, "last_error": "contradiction model error: offline",
            "load_failed": true, "embedding_pending": false,
            "stale_checkpoints": [],
        }));
        let out = render_human(&report);
        assert!(
            out.contains(
                "1 pair pending, the model could not be loaded: contradiction model error: offline"
            ),
            "{out}"
        );
        assert!(
            out.contains(
                "not retried until evolve.contradictions is set again or the daemon restarts"
            ),
            "the remedy is on its own line: {out}"
        );
        assert!(
            out.contains("crystalline config set evolve.contradictions full"),
            "{out}"
        );

        report.contradictions = Some(serde_json::json!({
            "profile": "full", "model": "mdeberta-v3-base-xnli-2mil7",
            "repo": "MoritzLaurer/mDeBERTa-v3-base-xnli-multilingual-nli-2mil7",
            "downloaded": true, "reason": null, "pending_pairs": 0,
            "failing_pairs": 0, "last_error": null, "load_failed": false,
            "embedding_pending": true, "stale_checkpoints": [],
        }));
        let out = render_human(&report);
        assert!(
            out.contains("0 pairs pending, embedding not finished for some candidates"),
            "{out}"
        );
    }

    /// The common offline-first-use shape: the model is not downloaded AND
    /// the load failed, so `contradiction_summary` must not also parrot
    /// `last_error` into the "not downloaded" parenthetical - the wait
    /// reason already carries it once.
    #[test]
    fn a_failed_load_that_never_downloaded_names_the_error_exactly_once() {
        let mut report = report_with_orphans(IndexAccess::Daemon, &[]);
        report.contradictions = Some(serde_json::json!({
            "profile": "full", "model": "mdeberta-v3-base-xnli-2mil7",
            "repo": "MoritzLaurer/mDeBERTa-v3-base-xnli-multilingual-nli-2mil7",
            "downloaded": false, "reason": null, "pending_pairs": 1,
            "failing_pairs": 0, "last_error": "contradiction model error: offline",
            "load_failed": true, "embedding_pending": false,
            "stale_checkpoints": [],
        }));
        let out = render_human(&report);
        assert_eq!(
            out.matches("contradiction model error: offline").count(),
            1,
            "{out}"
        );
        assert!(
            out.contains("(not downloaded), 1 pair pending, the model could not be loaded: contradiction model error: offline"),
            "{out}"
        );
    }

    /// A direct read (no daemon answered) never claims to know the pending
    /// count.
    #[test]
    fn the_contradictions_row_says_pending_is_not_counted_without_a_daemon() {
        let mut report = report_with_orphans(IndexAccess::Direct, &[]);
        report.contradictions = Some(serde_json::json!({
            "profile": "light", "model": "multilingual-minilmv2-l12-mnli-xnli",
            "repo": "MoritzLaurer/multilingual-MiniLMv2-L12-mnli-xnli",
            "downloaded": true, "reason": null, "pending_pairs": null,
            "failing_pairs": null, "last_error": null, "load_failed": false,
            "embedding_pending": false, "stale_checkpoints": [],
        }));
        let out = render_human(&report);
        assert!(out.contains("not counted yet"), "{out}");
    }

    /// Plan correction 14: the embedding row's cached-model listing must not
    /// mark an NLI checkpoint stale - the contradictions row lists those.
    #[test]
    fn nli_checkpoints_are_not_listed_as_stale_embedding_models() {
        assert!(!is_embedding_listing(
            crystalline_index::nli::NLI_MODELS[0].repo
        ));
        assert!(is_embedding_listing("BAAI/bge-small-en-v1.5"));
    }

    // --- contradiction_summary, driven directly ------------------------
    //
    // Every test above builds the JSON `report.contradictions` by hand and
    // only exercises `render_human`; that let two of the bugs the review
    // round caught (the doubled load-failure error, and a reason string
    // that should have been suppressed) through with every other test
    // green. These drive `contradiction_summary` itself, the function that
    // actually assembles the row.

    /// Guards every test below: `contradiction_summary` reads
    /// `CRYSTALLINE_MODELS_DIR` through `config::models_dir()`, env vars are
    /// process-global, and `cargo test --workspace` runs this file's tests
    /// on multiple threads (nextest gives each test its own process, but the
    /// canonical fallback does not) - the same convention as
    /// `crystalline-core`'s `MODELS_DIR_ENV_LOCK`.
    static MODELS_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Points `CRYSTALLINE_MODELS_DIR` at a directory for its lifetime and
    /// restores whatever the environment had on drop, even on panic.
    struct ModelsDirOverride {
        previous: Option<String>,
    }

    impl ModelsDirOverride {
        fn set(dir: &Path) -> ModelsDirOverride {
            let previous = std::env::var("CRYSTALLINE_MODELS_DIR").ok();
            unsafe {
                std::env::set_var("CRYSTALLINE_MODELS_DIR", dir);
            }
            ModelsDirOverride { previous }
        }
    }

    impl Drop for ModelsDirOverride {
        fn drop(&mut self) {
            match &self.previous {
                Some(v) => unsafe { std::env::set_var("CRYSTALLINE_MODELS_DIR", v) },
                None => unsafe { std::env::remove_var("CRYSTALLINE_MODELS_DIR") },
            }
        }
    }

    /// A `GlobalConfig` with `evolve.contradictions` set to `profile`.
    fn cfg_with_profile(profile: &str) -> GlobalConfig {
        GlobalConfig {
            evolve: Some(EvolveConfig {
                contradictions: Some(profile.to_string()),
            }),
            ..GlobalConfig::default()
        }
    }

    /// Fabricates a cached NLI checkpoint at
    /// `<dir>/<hub name>/snapshots/<id>/model.safetensors`, the shape
    /// `weights_cached` reads (a config alone is not a download).
    fn seed_nli_checkpoint(dir: &Path, repo: &str) {
        let snap = dir
            .join(crystalline_index::hub_dir_name(repo))
            .join("snapshots/abc");
        std::fs::create_dir_all(&snap).unwrap();
        std::fs::write(snap.join("model.safetensors"), b"w").unwrap();
    }

    #[test]
    fn contradiction_summary_off_names_no_model_and_no_pending() {
        let _guard = MODELS_DIR_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let _env = ModelsDirOverride::set(tmp.path());
        let summary = contradiction_summary(&GlobalConfig::default(), None);
        assert_eq!(summary["profile"], "off");
        assert_eq!(summary["model"], serde_json::Value::Null);
        assert_eq!(summary["repo"], serde_json::Value::Null);
        assert_eq!(summary["downloaded"], serde_json::Value::Null);
        assert_eq!(summary["reason"], serde_json::Value::Null);
        assert_eq!(summary["pending_pairs"], serde_json::Value::Null);
        assert_eq!(summary["failing_pairs"], serde_json::Value::Null);
        assert_eq!(summary["load_failed"], false);
        assert_eq!(summary["embedding_pending"], false);
        assert_eq!(summary["stale_checkpoints"], serde_json::json!([]));
    }

    #[test]
    fn contradiction_summary_full_with_pending_from_a_running_daemon() {
        use crystalline_index::nli::{NliProfile, nli_model};
        let _guard = MODELS_DIR_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        seed_nli_checkpoint(tmp.path(), nli_model(NliProfile::Full).repo);
        let _env = ModelsDirOverride::set(tmp.path());
        let daemon = serde_json::json!({ "contradictions": {
            "pending_pairs": 12, "failing_pairs": 0, "last_error": null,
            "load_failed": false, "embedding_pending": false,
        }});
        let summary = contradiction_summary(&cfg_with_profile("full"), Some(&daemon));
        assert_eq!(summary["profile"], "full");
        assert_eq!(summary["model"], nli_model(NliProfile::Full).id);
        assert_eq!(summary["repo"], nli_model(NliProfile::Full).repo);
        assert_eq!(summary["downloaded"], true);
        assert_eq!(summary["reason"], serde_json::Value::Null);
        assert_eq!(summary["pending_pairs"], 12);
        assert_eq!(summary["stale_checkpoints"], serde_json::json!([]));
    }

    /// L7/lesson 62: no daemon answered (a direct read, or one that has not
    /// walked yet), so the pending count is unknown - `null`, never `0`.
    #[test]
    fn contradiction_summary_with_no_daemon_answer_leaves_the_count_unknown_not_zero() {
        let _guard = MODELS_DIR_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let _env = ModelsDirOverride::set(tmp.path());
        let summary = contradiction_summary(&cfg_with_profile("light"), None);
        assert_eq!(summary["pending_pairs"], serde_json::Value::Null);
        assert_ne!(summary["pending_pairs"], serde_json::json!(0), "{summary}");
        assert_eq!(summary["failing_pairs"], serde_json::Value::Null);
        assert_eq!(summary["load_failed"], false);
        assert_eq!(summary["embedding_pending"], false);
        assert_eq!(summary["downloaded"], false);
        assert_eq!(
            summary["reason"],
            serde_json::json!("the daemon downloads it on its first pass")
        );
    }

    /// The bug the review round caught: a load failure must not also set the
    /// "not downloaded" reason to `last_error`, since `contradiction_wait_reason`
    /// already renders that error on its own line.
    #[test]
    fn contradiction_summary_a_failed_load_suppresses_the_download_reason() {
        let _guard = MODELS_DIR_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let _env = ModelsDirOverride::set(tmp.path());
        let daemon = serde_json::json!({ "contradictions": {
            "pending_pairs": 1, "failing_pairs": 0,
            "last_error": "contradiction model error: offline",
            "load_failed": true, "embedding_pending": false,
        }});
        let summary = contradiction_summary(&cfg_with_profile("full"), Some(&daemon));
        assert_eq!(summary["downloaded"], false);
        assert_eq!(summary["load_failed"], true);
        assert_eq!(summary["last_error"], "contradiction model error: offline");
        assert_eq!(
            summary["reason"],
            serde_json::Value::Null,
            "the wait reason already carries the error; the download reason must not repeat it: {summary}"
        );
    }

    /// A parked batch failure (the model loaded fine; some pairs failed
    /// scoring) is carried through untouched, with no download reason since
    /// a batch failure only happens once the model is downloaded.
    #[test]
    fn contradiction_summary_carries_failing_pairs_through() {
        use crystalline_index::nli::{NliProfile, nli_model};
        let _guard = MODELS_DIR_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        seed_nli_checkpoint(tmp.path(), nli_model(NliProfile::Full).repo);
        let _env = ModelsDirOverride::set(tmp.path());
        let daemon = serde_json::json!({ "contradictions": {
            "pending_pairs": 2, "failing_pairs": 2,
            "last_error": "the batch failed", "load_failed": false,
            "embedding_pending": false,
        }});
        let summary = contradiction_summary(&cfg_with_profile("full"), Some(&daemon));
        assert_eq!(summary["failing_pairs"], 2);
        assert_eq!(summary["last_error"], "the batch failed");
        assert_eq!(summary["load_failed"], false);
        assert_eq!(summary["downloaded"], true);
        assert_eq!(summary["reason"], serde_json::Value::Null);
    }

    /// The exact combination that produced the doubled-error bug:
    /// `load_failed` must win over a nonzero `failing_pairs` for the download
    /// reason too (it stays suppressed), even though both are set at once -
    /// a stale failing-pairs count left over from before the load broke.
    #[test]
    fn contradiction_summary_load_failed_suppresses_the_reason_even_with_failing_pairs_set() {
        let _guard = MODELS_DIR_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let _env = ModelsDirOverride::set(tmp.path());
        let daemon = serde_json::json!({ "contradictions": {
            "pending_pairs": 3, "failing_pairs": 2,
            "last_error": "contradiction model error: offline",
            "load_failed": true, "embedding_pending": false,
        }});
        let summary = contradiction_summary(&cfg_with_profile("full"), Some(&daemon));
        assert_eq!(summary["load_failed"], true);
        assert_eq!(summary["failing_pairs"], 2);
        assert_eq!(
            summary["reason"],
            serde_json::Value::Null,
            "load_failed suppresses the download reason even with failing pairs parked too: {summary}"
        );
        assert_eq!(summary["last_error"], "contradiction model error: offline");
    }

    /// Plan correction 14, from `contradiction_summary`'s own side: a
    /// checkpoint cached for a profile the config does not run is listed as
    /// stale, and the configured model's own (uncached) state is unaffected.
    #[test]
    fn contradiction_summary_lists_an_unused_cached_checkpoint_as_stale() {
        use crystalline_index::nli::{NliProfile, nli_model};
        let _guard = MODELS_DIR_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        seed_nli_checkpoint(tmp.path(), nli_model(NliProfile::Light).repo);
        let _env = ModelsDirOverride::set(tmp.path());
        let summary = contradiction_summary(&cfg_with_profile("full"), None);
        let stale: Vec<&str> = summary["stale_checkpoints"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(stale, vec![nli_model(NliProfile::Light).repo]);
        assert_eq!(
            summary["downloaded"], false,
            "the configured (full) model itself is not the one that is cached: {summary}"
        );
    }
}
