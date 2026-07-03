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
}

// --- global config -----------------------------------------------------------

/// The global `config.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Registered domains, name to entry, in order.
    #[serde(default)]
    pub domains: IndexMap<String, DomainEntry>,
    /// Service settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<ServiceConfig>,
    /// Embedding provider settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embeddings: Option<EmbeddingsConfig>,
    /// Prompt routing settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PromptConfig>,
}

/// A registered domain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEntry {
    /// The domain root path.
    pub path: PathBuf,
}

/// Service configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// The HTTP setting: a bool, or a `host:port` string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpSetting>,
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
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, yaml.as_bytes()).map_err(|source| ConfigError::Io {
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

/// The single-instance lock path, `<state_dir>/service.lock`.
pub fn service_lock_path() -> Result<PathBuf, ConfigError> {
    Ok(state_dir()?.join("service.lock"))
}

/// The service socket path, `<state_dir>/service.sock`.
pub fn service_sock_path() -> Result<PathBuf, ConfigError> {
    Ok(state_dir()?.join("service.sock"))
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
