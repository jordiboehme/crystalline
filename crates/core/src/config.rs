//! Configuration types and IO.
//!
//! All Crystalline configuration is YAML, parsed with the same stack as engram
//! frontmatter. This module holds the global `config.yaml`, the per-domain and
//! repo-local `.crystalline.yaml` models, tilde expansion, atomic saves and the
//! XDG-aware default paths. It computes paths only; there is no daemon logic
//! here.

use std::path::{Path, PathBuf};

use etcetera::BaseStrategy;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// An error loading, parsing or saving configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// An IO error.
    #[error("config io error at {path}: {source}")]
    Io {
        /// The path involved.
        path: String,
        /// The underlying IO error.
        source: std::io::Error,
    },
    /// A YAML (de)serialization error.
    #[error("config yaml error: {0}")]
    Yaml(String),
    /// The home directory could not be resolved.
    #[error("could not resolve the home directory: {0}")]
    Home(String),
    /// The database configuration is invalid, for example a Postgres backend
    /// without a `postgres://` url. Surfaced at store-factory time, never at
    /// parse time.
    #[error("invalid database configuration: {0}")]
    Database(String),
}

// --- global config -----------------------------------------------------------

/// The global `config.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Registered domains, name to entry, in order.
    #[serde(default)]
    pub domains: IndexMap<String, DomainEntry>,
    /// The default root folder new file domains are created under, when the
    /// caller does not give an explicit path: a domain named `foo` lands at
    /// `<domains_root>/foo`. Absent means `~/Documents/Crystalline`, so every
    /// existing config keeps working untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domains_root: Option<PathBuf>,
    /// Service settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<ServiceConfig>,
    /// Embedding provider settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embeddings: Option<EmbeddingsConfig>,
    /// Prompt routing settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PromptConfig>,
    /// Storage backend settings. Absent means the Turso backend at the default
    /// `index.db` path, so every existing config keeps working untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<DatabaseConfig>,
    /// GitHub collaboration settings. Absent means the feature is off: no
    /// collaboration tools are shown and the origin poller never runs, so
    /// every existing config keeps working untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<GitHubConfig>,
}

impl GlobalConfig {
    /// Whether the served content API is read-only, from `service.read_only`.
    /// Absent config or an absent key means read-write (false). This is the
    /// config layer of the effective mode; an explicit `--read-only` flag on
    /// `serve` or `mcp` can force it true on top of this, never false.
    pub fn read_only(&self) -> bool {
        self.service
            .as_ref()
            .and_then(|s| s.read_only)
            .unwrap_or(false)
    }

    /// The effective database configuration: the configured `database` block,
    /// or the Turso default when absent. The store factory validates this
    /// before opening a backend.
    pub fn database(&self) -> DatabaseConfig {
        self.database.clone().unwrap_or_default()
    }

    /// The root folder new file domains are created under, from `domains_root`
    /// (tilde-expanded), or `~/Documents/Crystalline` when unset. A domain
    /// created without an explicit path lands at `<domains_root>/<name>`.
    pub fn domains_root(&self) -> PathBuf {
        match &self.domains_root {
            Some(p) => expand_tilde(&p.to_string_lossy()),
            None => expand_tilde("~/Documents/Crystalline"),
        }
    }

    /// Whether GitHub collaboration is turned on, from `github.enabled`.
    /// Absent config or an absent key means off (false), so an unconfigured
    /// install shows no collaboration tools and runs no origin poller.
    pub fn github_enabled(&self) -> bool {
        self.github
            .as_ref()
            .and_then(|g| g.enabled)
            .unwrap_or(false)
    }
}

/// Which side of the one-truth-per-domain rule a domain lives on: files on
/// disk (the default) or the database.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DomainKind {
    /// Engrams are markdown files on disk; the database is a derived index.
    #[default]
    File,
    /// Engrams live only in the database; there is no filesystem root. Portable
    /// and scale-out friendly, and `crystalline domain export` hands the files
    /// back at any time.
    Virtual,
}

impl DomainKind {
    /// Whether this is the default file kind. Used to keep `kind` out of a
    /// serialized file-domain entry so old configs round-trip byte-for-byte.
    pub fn is_file(&self) -> bool {
        matches!(self, DomainKind::File)
    }

    /// Parse the stored discriminator string a backend keeps in its `domain`
    /// table (`file` or `virtual`); any unknown value reads back as `File`.
    pub fn from_stored(s: &str) -> DomainKind {
        match s {
            "virtual" => DomainKind::Virtual,
            _ => DomainKind::File,
        }
    }
}

/// A registered domain. A file domain carries its root `path`; a virtual domain
/// carries no path and elects the database as its source of truth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEntry {
    /// The domain kind. Absent (the default `file`) is never serialized, so a
    /// file-domain entry writes only its `path` exactly as before.
    #[serde(default, skip_serializing_if = "DomainKind::is_file")]
    pub kind: DomainKind,
    /// The domain root path. Present for a file domain, absent for a virtual
    /// domain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// The GitHub repository this domain syncs with, absent for a domain with
    /// no origin (the common case, and every domain predating this feature).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<OriginConfig>,
    /// Whether this domain's declared artifacts are provisioned into harnesses.
    /// Absent means undecided: the domain's artifacts are surfaced as awaiting a
    /// decision but nothing installs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provision: Option<bool>,
}

impl DomainEntry {
    /// A file-backed domain entry rooted at `path`, with no origin.
    pub fn file(path: impl Into<PathBuf>) -> DomainEntry {
        DomainEntry {
            kind: DomainKind::File,
            path: Some(path.into()),
            origin: None,
            provision: None,
        }
    }

    /// A virtual domain entry (database-backed, no path, no origin).
    pub fn virtual_domain() -> DomainEntry {
        DomainEntry {
            kind: DomainKind::Virtual,
            path: None,
            origin: None,
            provision: None,
        }
    }

    /// Whether this domain keeps its engrams in the database rather than on disk.
    pub fn is_virtual(&self) -> bool {
        matches!(self.kind, DomainKind::Virtual)
    }

    /// The tilde-expanded filesystem root for a file domain, or `None` for a
    /// virtual domain (which has no path).
    pub fn file_path(&self) -> Option<PathBuf> {
        self.path
            .as_ref()
            .map(|p| expand_tilde(&p.to_string_lossy()))
    }
}

/// Which GitHub repository, subfolder and branch a domain syncs with. Present
/// only on domains added with an origin; a plain local domain has none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OriginConfig {
    /// The repository, `owner/name`.
    pub repo: String,
    /// The subfolder within the repository that is the domain root. Absent
    /// means the domain root is the repository root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The branch this domain tracks. Absent means `main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// A per-domain override for how often the daemon polls this origin,
    /// taking priority over `github.poll_secs`. Absent defers to the global
    /// setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_secs: Option<u64>,
}

impl OriginConfig {
    /// The effective branch this domain tracks: the configured `branch`, or
    /// `main` when absent.
    pub fn branch(&self) -> &str {
        self.branch.as_deref().unwrap_or("main")
    }
}

/// Which storage backend backs the derived index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseBackend {
    /// The embedded Turso (SQLite-compatible) backend. The default.
    #[default]
    Turso,
    /// An external PostgreSQL backend.
    Postgres,
}

/// The `database` block: which backend backs the derived index and, for
/// PostgreSQL, its connection URL.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// The storage backend. Absent means Turso.
    #[serde(default)]
    pub backend: DatabaseBackend,
    /// The backend URL. Required for Postgres (`postgres://` or `postgresql://`);
    /// an optional file-path override for Turso, secondary to the `--db` flag
    /// and the default `index.db` path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl DatabaseConfig {
    /// Validate the backend and URL combination. Called at store-factory time,
    /// not at config parse time, so `verify` and `prompt` never trip on it. A
    /// Postgres backend requires a non-empty URL beginning `postgres://` or
    /// `postgresql://`; a Turso backend accepts any URL as a file-path override.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.backend == DatabaseBackend::Postgres {
            match self.url.as_deref() {
                Some(u) if u.starts_with("postgres://") || u.starts_with("postgresql://") => Ok(()),
                Some(_) => Err(ConfigError::Database(
                    "the postgres backend requires a url beginning postgres:// or postgresql://"
                        .to_string(),
                )),
                None => Err(ConfigError::Database(
                    "the postgres backend requires a url".to_string(),
                )),
            }
        } else {
            Ok(())
        }
    }
}

/// The `github` block: whether team collaboration through GitHub is turned
/// on and its provider settings. Reads like a settings-page section - see the
/// `configure` tool, which exposes exactly these keys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GitHubConfig {
    /// Turns GitHub collaboration on or off. Absent means off: no
    /// collaboration tools are shown and the origin poller never runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// How often the daemon polls origins in the background, in seconds.
    /// Absent means 300.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_secs: Option<u64>,
    /// The GitHub API base URL, for a GitHub Enterprise Server instance.
    /// Absent means `https://api.github.com`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    /// A self-hosted OAuth App client id, overriding the embedded default.
    /// Needed only for a GitHub Enterprise Server deployment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_client_id: Option<String>,
}

/// Service configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// The HTTP setting: a bool, or a `host:port` string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpSetting>,
    /// Serve the content API read-only: the four content-mutating tools are
    /// hidden from the MCP surface and refused by the engine, while sync,
    /// reindex, watching and embedding still follow external file changes.
    /// Absent means read-write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    /// Extra `Host` header values the HTTP transport accepts, on top of the
    /// always-allowed loopback set. This is the DNS-rebinding guard: rmcp
    /// rejects any request whose `Host` is not allowed. A single `*` entry
    /// allows any Host (only safe behind a trusted reverse proxy or firewall).
    /// Absent means loopback-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_hosts: Option<Vec<String>>,
}

/// The `service.http` value: either enabled/disabled, or a bind address.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HttpSetting {
    /// A boolean toggle.
    Enabled(bool),
    /// An explicit `host:port` bind address.
    Address(String),
}

/// Embedding provider configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingsConfig {
    /// The provider name, for example `local` or `openai-compatible`.
    pub provider: String,
    /// The model identifier.
    pub model: String,
    /// The endpoint for a remote provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// The environment variable holding the API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

/// Prompt routing configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PromptConfig {
    /// Path-glob to domain-filter rules, in order.
    #[serde(default)]
    pub rules: IndexMap<String, PromptRule>,
}

/// A single prompt routing rule: domain filters for a path glob.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PromptRule {
    /// Domains to include.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    /// Domains to exclude.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

// --- per-domain config -------------------------------------------------------

/// The per-domain `.crystalline.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DomainConfig {
    /// Verify overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifyConfig>,
}

/// Verify overrides for a domain.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VerifyConfig {
    /// Rule id to severity overrides.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub rules: IndexMap<String, String>,
    /// Default per-file token budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<usize>,
    /// Named token budgets.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub token_budgets: IndexMap<String, usize>,
    /// Required file entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_files: Vec<RequiredFile>,
}

/// A required file entry within a domain's verify config.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RequiredFile {
    /// The file path, relative to the domain root.
    pub path: String,
    /// Whether the file must carry frontmatter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_frontmatter: Option<bool>,
    /// Required section headings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_sections: Vec<String>,
    /// Per-section rules.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub sections: IndexMap<String, SectionRule>,
}

/// Rules for a named section in a required file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SectionRule {
    /// Minimum number of zero-indent bullets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_top_level_bullets: Option<usize>,
    /// A fallback section if this one is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_section: Option<String>,
    /// Maximum bullet length in characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bullet_length: Option<usize>,
}

// --- repo-local config -------------------------------------------------------

/// The repo-local `.crystalline.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepoConfig {
    /// Domain ordering hint for prompt generation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferred_domains: Vec<String>,
}

// --- IO ----------------------------------------------------------------------

/// Load and parse a YAML config file.
pub fn load_yaml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.display().to_string(),
        source,
    })?;
    serde_yaml_ng::from_str(&text).map_err(|e| ConfigError::Yaml(e.to_string()))
}

/// Serialize and atomically save a config to a YAML file. Writes a sibling
/// temporary file then renames it into place.
pub fn save_yaml<T: Serialize>(path: &Path, value: &T) -> Result<(), ConfigError> {
    let yaml = serde_yaml_ng::to_string(value).map_err(|e| ConfigError::Yaml(e.to_string()))?;
    save_bytes(path, yaml.as_bytes())
}

/// Atomically write raw bytes to a file: create the parent directory if it is
/// missing, write a sibling temporary file, then rename it into place. This is
/// the IO half of [`save_yaml`], extracted so a caller with an already-encoded
/// payload (JSON, for example) gets the same atomicity and directory-creation
/// guarantees without round-tripping through YAML.
pub fn save_bytes(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, bytes).map_err(|source| ConfigError::Io {
        path: tmp.display().to_string(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| ConfigError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".tmp.{}", std::process::id()));
    path.with_file_name(name)
}

/// Expand a leading `~` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir().unwrap_or_else(|_| PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

fn home_dir() -> Result<PathBuf, ConfigError> {
    etcetera::home_dir().map_err(|e| ConfigError::Home(e.to_string()))
}

// --- default paths -----------------------------------------------------------

const APP: &str = "crystalline";

fn base() -> Result<impl BaseStrategy, ConfigError> {
    etcetera::choose_base_strategy().map_err(|e| ConfigError::Home(e.to_string()))
}

/// The application config directory, for example `~/.config/crystalline`.
pub fn config_dir() -> Result<PathBuf, ConfigError> {
    Ok(base()?.config_dir().join(APP))
}

/// The global config file path, `<config_dir>/config.yaml`.
pub fn global_config_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("config.yaml"))
}

/// The application state directory, for example `~/.local/state/crystalline`.
pub fn state_dir() -> Result<PathBuf, ConfigError> {
    let b = base()?;
    let root = b.state_dir().unwrap_or_else(|| b.data_dir());
    Ok(root.join(APP))
}

/// The derived index database path, `<state_dir>/index.db`.
pub fn index_db_path() -> Result<PathBuf, ConfigError> {
    Ok(state_dir()?.join("index.db"))
}

/// The single-instance lock path, `<state_dir>/service.lock`. Lock only: the
/// owner holds an exclusive OS lock on this file for its lifetime and the
/// contents are meaningless. The owner's record lives at [`service_info_path`],
/// never in this file, because Windows region locks are mandatory: reading or
/// writing a locked file through any other handle fails there.
pub fn service_lock_path() -> Result<PathBuf, ConfigError> {
    Ok(state_dir()?.join("service.lock"))
}

/// The daemon record path, `<state_dir>/service.json`: the owner's pid, socket
/// and version, written once the socket is bound and removed on shutdown. Kept
/// apart from `service.lock` so reading the record never touches the locked
/// file (see [`service_lock_path`]).
pub fn service_info_path() -> Result<PathBuf, ConfigError> {
    Ok(state_dir()?.join("service.json"))
}

/// The service socket path, `<state_dir>/service.sock`.
pub fn service_sock_path() -> Result<PathBuf, ConfigError> {
    Ok(state_dir()?.join("service.sock"))
}

/// The spawned daemon's stderr log, `<state_dir>/daemon.log`. A daemon spawned
/// by the MCP bridge is fully detached, so without this file a startup failure
/// is invisible; the log made the difference between a one-log field diagnosis
/// and a blind one.
pub fn daemon_log_path() -> Result<PathBuf, ConfigError> {
    Ok(state_dir()?.join("daemon.log"))
}

/// The directory holding every origin's on-disk state, `<state_dir>/origins`.
pub fn origins_state_dir() -> Result<PathBuf, ConfigError> {
    Ok(state_dir()?.join("origins"))
}

/// One domain's origin state directory, `<state_dir>/origins/<domain>`. Holds
/// the origin state file, the base snapshot and any recorded conflicts.
pub fn origin_state_dir(domain: &str) -> Result<PathBuf, ConfigError> {
    Ok(origins_state_dir()?.join(domain))
}

/// The instance-id path, `<state_dir>/instance-id`. Holds this machine and
/// state-directory's stable identity for shared-database collaboration.
pub fn instance_id_path() -> Result<PathBuf, ConfigError> {
    Ok(state_dir()?.join("instance-id"))
}

/// Read this instance's stable id, generating and persisting a fresh UUID v4 on
/// first use. The id lives at `<state_dir>/instance-id` and is stable across
/// restarts, so a shared-database deployment can tell one worker from another
/// (host locks, ownership surfacing). Two state directories on one machine (two
/// isolated `HOME`s) get two ids, exactly the isolation multi-instance testing
/// needs.
pub fn read_or_create_instance_id() -> Result<String, ConfigError> {
    let path = instance_id_path()?;
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    std::fs::write(&path, id.as_bytes()).map_err(|source| ConfigError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(id)
}

/// The application cache directory, for example `~/.cache/crystalline`.
pub fn cache_dir() -> Result<PathBuf, ConfigError> {
    Ok(base()?.cache_dir().join(APP))
}

/// The environment variable that overrides `models_dir()`. Container images
/// that bake the embedding model in set this to a non-volume path so the
/// baked files are never shadowed by a bind mount over the cache directory.
const MODELS_DIR_ENV: &str = "CRYSTALLINE_MODELS_DIR";

/// The embedding model cache directory. `<cache_dir>/models` by default, or
/// the value of `CRYSTALLINE_MODELS_DIR` when that variable is set to a
/// non-empty value (a leading `~` is expanded via `expand_tilde`). An unset
/// or empty variable falls back to the default.
pub fn models_dir() -> Result<PathBuf, ConfigError> {
    if let Ok(dir) = std::env::var(MODELS_DIR_ENV)
        && !dir.is_empty()
    {
        return Ok(expand_tilde(&dir));
    }
    Ok(cache_dir()?.join("models"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_absent_defaults_to_turso() {
        let cfg: GlobalConfig = serde_yaml_ng::from_str("domains: {}").unwrap();
        assert!(cfg.database.is_none());
        let effective = cfg.database();
        assert_eq!(effective.backend, DatabaseBackend::Turso);
        assert_eq!(effective.url, None);
        assert!(effective.validate().is_ok());
    }

    #[test]
    fn database_block_parses_postgres_with_url() {
        let yaml = "database:\n  backend: postgres\n  url: postgres://u:p@db:5432/crystalline\n";
        let cfg: GlobalConfig = serde_yaml_ng::from_str(yaml).unwrap();
        let db = cfg.database.expect("database block");
        assert_eq!(db.backend, DatabaseBackend::Postgres);
        assert_eq!(
            db.url.as_deref(),
            Some("postgres://u:p@db:5432/crystalline")
        );
        assert!(db.validate().is_ok());
    }

    #[test]
    fn turso_default_round_trips_without_a_database_key() {
        // An absent database block must never write a `database:` line, so old
        // configs stay byte-identical.
        let cfg = GlobalConfig::default();
        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("database"),
            "default config should not serialize a database block: {yaml}"
        );
    }

    #[test]
    fn database_backend_serializes_lowercase() {
        let cfg = GlobalConfig {
            database: Some(DatabaseConfig {
                backend: DatabaseBackend::Postgres,
                url: Some("postgresql://localhost/db".to_string()),
            }),
            ..GlobalConfig::default()
        };
        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(yaml.contains("backend: postgres"), "{yaml}");
        assert!(yaml.contains("url: postgresql://localhost/db"), "{yaml}");
    }

    #[test]
    fn turso_url_is_an_optional_path_override_and_always_valid() {
        let db = DatabaseConfig {
            backend: DatabaseBackend::Turso,
            url: Some("/tmp/custom-index.db".to_string()),
        };
        assert!(db.validate().is_ok());
    }

    #[test]
    fn postgres_without_url_fails_validation() {
        let db = DatabaseConfig {
            backend: DatabaseBackend::Postgres,
            url: None,
        };
        assert!(db.validate().is_err());
    }

    #[test]
    fn postgres_with_a_non_postgres_url_fails_validation() {
        let db = DatabaseConfig {
            backend: DatabaseBackend::Postgres,
            url: Some("mysql://localhost/db".to_string()),
        };
        assert!(db.validate().is_err());
    }

    #[test]
    fn file_domain_entry_round_trips_without_a_kind_line() {
        // A file domain must serialize only its path, no `kind:`, so old
        // configs stay byte-identical.
        let mut domains = IndexMap::new();
        domains.insert("eng".to_string(), DomainEntry::file("/knowledge/eng"));
        let cfg = GlobalConfig {
            domains,
            ..GlobalConfig::default()
        };
        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("kind"),
            "no kind line for a file domain: {yaml}"
        );
        assert!(yaml.contains("path: /knowledge/eng"), "{yaml}");
        let back: GlobalConfig = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn virtual_domain_entry_serializes_kind_and_no_path() {
        let mut domains = IndexMap::new();
        domains.insert("notes".to_string(), DomainEntry::virtual_domain());
        let cfg = GlobalConfig {
            domains,
            ..GlobalConfig::default()
        };
        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(yaml.contains("kind: virtual"), "{yaml}");
        assert!(
            !yaml.contains("path"),
            "a virtual domain writes no path: {yaml}"
        );
        let back: GlobalConfig = serde_yaml_ng::from_str(&yaml).unwrap();
        let entry = back.domains.get("notes").unwrap();
        assert!(entry.is_virtual());
        assert_eq!(entry.file_path(), None);
    }

    #[test]
    fn save_bytes_creates_parent_dirs_and_writes_the_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("state.json");
        save_bytes(&path, b"{\"v\":1}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"v\":1}");
        // A second write overwrites in place rather than failing because the
        // destination already exists.
        save_bytes(&path, b"{\"v\":2}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"v\":2}");
    }

    #[test]
    fn legacy_bare_path_entry_parses_as_a_file_domain() {
        // The historical shape (`name: { path: ... }`) still parses to a file
        // domain with the default kind.
        let yaml = "domains:\n  eng:\n    path: /knowledge/eng\n";
        let cfg: GlobalConfig = serde_yaml_ng::from_str(yaml).unwrap();
        let entry = cfg.domains.get("eng").unwrap();
        assert!(!entry.is_virtual());
        assert_eq!(entry.file_path(), Some(PathBuf::from("/knowledge/eng")));
    }

    #[test]
    fn constructors_default_provision_to_none() {
        assert_eq!(DomainEntry::file("/knowledge/eng").provision, None);
        assert_eq!(DomainEntry::virtual_domain().provision, None);
    }

    #[test]
    fn pre_existing_shape_round_trips_byte_identically_with_provision_absent() {
        // A config written before `provision` existed - a bare path entry with
        // no kind, no origin, no provision - must serialize back to exactly
        // the same bytes, with no `provision:` key anywhere.
        let yaml = "domains:\n  eng:\n    path: /knowledge/eng\n";
        let cfg: GlobalConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(cfg.domains.get("eng").unwrap().provision, None);
        let out = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(
            !out.contains("provision"),
            "no provision line for a pre-existing entry: {out}"
        );
        assert_eq!(out, yaml);
    }

    #[test]
    fn provision_true_and_false_round_trip() {
        let mut domains = IndexMap::new();
        let mut allowed = DomainEntry::file("/knowledge/allowed");
        allowed.provision = Some(true);
        domains.insert("allowed".to_string(), allowed);
        let mut denied = DomainEntry::file("/knowledge/denied");
        denied.provision = Some(false);
        domains.insert("denied".to_string(), denied);
        let cfg = GlobalConfig {
            domains,
            ..GlobalConfig::default()
        };

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(yaml.contains("provision: true"), "{yaml}");
        assert!(yaml.contains("provision: false"), "{yaml}");
        let back: GlobalConfig = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(back.domains.get("allowed").unwrap().provision, Some(true));
        assert_eq!(back.domains.get("denied").unwrap().provision, Some(false));
    }
}
