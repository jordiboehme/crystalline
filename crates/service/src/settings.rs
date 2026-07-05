//! The settings registry: the single source of truth for which configuration
//! keys an agent or a user may change through `configure`, their types and
//! bounds and how each one maps onto [`GlobalConfig`].
//!
//! Nothing outside this module may match on a setting key: the ctl `configure`
//! command, the CLI `config` verbs and (in a later task) the MCP `configure`
//! tool all read [`registry`] for the key list and documentation and call
//! [`apply`], [`unset`] and [`snapshot`] to act on one.

use crystalline_core::config::{
    DatabaseBackend, DatabaseConfig, GitHubConfig, GlobalConfig, HttpSetting, ServiceConfig,
};

use crate::overlay::EnvOverlay;

/// An error applying, resetting or looking up a setting. The message is
/// actionable and safe to show an agent or a terminal as-is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SettingsError(String);

/// The value type of one setting, for parsing and for a future typed input
/// schema (the MCP `configure` tool).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    /// A boolean, parsed from `true` or `false`.
    Bool,
    /// An unsigned integer.
    U64,
    /// A string.
    String,
}

/// Where a setting's effective value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingSource {
    /// Never explicitly set; showing the built-in default.
    Default,
    /// Read from the config file.
    Config,
    /// Overridden by a `CRYSTALLINE_*` environment variable. Not yet emitted:
    /// this milestone only lays the enum down, the environment overlay that
    /// produces this variant lands in a later one.
    Env,
}

/// One agent-adjustable setting: its key, its documentation and the typed
/// accessors the registry-level [`apply`], [`unset`] and [`snapshot`]
/// dispatch through. Constructed only by [`registry`]; there is no public
/// constructor, so every setting is declared in exactly one place.
pub struct SettingSpec {
    /// The dotted setting key, for example `github.enabled`.
    pub key: &'static str,
    /// A one-line, user-facing description. Doubles as the future MCP
    /// `configure` tool's per-setting help.
    pub doc: &'static str,
    /// The value type, for parsing and rendering.
    pub kind: SettingKind,
    /// Whether this setting only takes effect the next time the daemon
    /// starts (a running daemon keeps reading its old value), as opposed to
    /// one a running daemon picks up immediately. Drives [`change_note`].
    pub startup_effective: bool,
    /// Parse and validate a string value, then write it into `config`.
    apply: fn(&mut GlobalConfig, &str) -> Result<(), SettingsError>,
    /// Reset this setting to its default, removing it from `config` (and its
    /// parent block, when emptied).
    clear: fn(&mut GlobalConfig),
    /// The effective display value and whether it is explicitly set (`false`)
    /// or a default (`true`).
    effective: fn(&GlobalConfig) -> (String, bool),
}

impl SettingSpec {
    /// The environment variable this setting maps to, mechanically derived
    /// from its key: `github.enabled` becomes `CRYSTALLINE_GITHUB_ENABLED`.
    /// Unused before the environment overlay lands; kept beside the key it
    /// derives from so the mapping is obvious at the declaration site.
    pub fn env_var(&self) -> String {
        format!("CRYSTALLINE_{}", self.key.replace('.', "_").to_uppercase())
    }
}

/// How often the daemon polls a GitHub origin when `github.poll_secs` is
/// absent, seconds. Mirrors the default documented on
/// [`crystalline_core::config::GitHubConfig::poll_secs`].
const DEFAULT_POLL_SECS: u64 = 300;

/// The lowest accepted `github.poll_secs`, seconds. Below this the poll loop
/// would hammer the GitHub API for no practical benefit.
const MIN_POLL_SECS: u64 = 60;

/// The GitHub API base url used when `github.api_url` is absent. Mirrors the
/// default documented on
/// [`crystalline_core::config::GitHubConfig::api_url`].
const DEFAULT_API_URL: &str = "https://api.github.com";

/// The registry: every setting an agent or a user may change, in display
/// order. This is the only place a setting key is declared; every consumer
/// (ctl, CLI, the future MCP tool) reads this list rather than matching keys
/// itself.
pub fn registry() -> &'static [SettingSpec] {
    &[
        SettingSpec {
            key: "github.enabled",
            doc: "Turn GitHub team collaboration on or off",
            kind: SettingKind::Bool,
            startup_effective: false,
            apply: set_enabled,
            clear: clear_enabled,
            effective: enabled_effective,
        },
        SettingSpec {
            key: "github.poll_secs",
            doc: "How often the daemon polls GitHub for changes, in seconds (minimum 60)",
            kind: SettingKind::U64,
            startup_effective: false,
            apply: set_poll_secs,
            clear: clear_poll_secs,
            effective: poll_secs_effective,
        },
        SettingSpec {
            key: "github.api_url",
            doc: "The GitHub API base url, for a GitHub Enterprise Server instance",
            kind: SettingKind::String,
            startup_effective: false,
            apply: set_api_url,
            clear: clear_api_url,
            effective: api_url_effective,
        },
        SettingSpec {
            key: "github.oauth_client_id",
            doc: "A self-hosted OAuth App client id, overriding the embedded default",
            kind: SettingKind::String,
            startup_effective: false,
            apply: set_oauth_client_id,
            clear: clear_oauth_client_id,
            effective: oauth_client_id_effective,
        },
        SettingSpec {
            key: "service.read_only",
            doc: "Serve knowledge read-only, hiding every tool that writes (applies at the next daemon start)",
            kind: SettingKind::Bool,
            startup_effective: true,
            apply: set_read_only,
            clear: clear_read_only,
            effective: read_only_effective,
        },
        SettingSpec {
            key: "service.http",
            doc: "Enable the HTTP transport (true or false) or bind it to a host:port address; the serve --http flag wins when given (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            apply: set_http,
            clear: clear_http,
            effective: http_effective,
        },
        SettingSpec {
            key: "database.backend",
            doc: "Which storage backend serves the derived index, turso or postgres (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            apply: set_database_backend,
            clear: clear_database_backend,
            effective: database_backend_effective,
        },
        SettingSpec {
            key: "database.url",
            doc: "The Postgres connection URL (or a file-path override for the embedded backend); applies at the next daemon start",
            kind: SettingKind::String,
            startup_effective: true,
            apply: set_database_url,
            clear: clear_database_url,
            effective: database_url_effective,
        },
    ]
}

/// Set `key` to the string `value`, validating its type and bounds. Unknown
/// keys error with the full list of known ones.
pub fn apply(config: &mut GlobalConfig, key: &str, value: &str) -> Result<(), SettingsError> {
    let spec = find(key)?;
    (spec.apply)(config, value)
}

/// Reset `key` to its default, removing it (and an emptied parent block)
/// from the config. Unknown keys error with the full list of known ones.
pub fn unset(config: &mut GlobalConfig, key: &str) -> Result<(), SettingsError> {
    let spec = find(key)?;
    (spec.clear)(config);
    Ok(())
}

/// An ordered snapshot for display: every registry key with its effective
/// value, its source and its doc line. `file` is the persisted config, `overlay`
/// the parsed environment overlay. The value shown is the effective one (file
/// plus overlay); the source is `Env` when a variable overrides the key,
/// otherwise `Config` or `Default` read from the file alone, so `config show`
/// tells an operator where each value actually comes from.
pub fn snapshot(file: &GlobalConfig, overlay: &EnvOverlay) -> Vec<SettingView> {
    let effective = overlay.apply(file);
    registry()
        .iter()
        .map(|spec| {
            let (value, _) = (spec.effective)(&effective);
            let source = if overlay.overrides_key(spec.key) {
                SettingSource::Env
            } else if (spec.effective)(file).1 {
                SettingSource::Default
            } else {
                SettingSource::Config
            };
            SettingView {
                key: spec.key.to_string(),
                value,
                source,
                doc: spec.doc.to_string(),
            }
        })
        .collect()
}

/// One setting's effective value for display: the ctl `configure show`
/// payload, a `config set`/`config unset` result and the CLI's `config show`
/// table are all built from these.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SettingView {
    /// The dotted setting key.
    pub key: String,
    /// The effective value, rendered as a string regardless of its
    /// underlying type.
    pub value: String,
    /// Where the effective value comes from.
    pub source: SettingSource,
    /// The setting's one-line documentation.
    pub doc: String,
}

/// A note to attach to a setting's display, when there is one worth
/// surfacing beyond its bare value. `key` is assumed already validated
/// against the registry; an unknown key just yields `None`.
///
/// Two kinds of note can apply, and both are joined into one clean string when
/// they do: a startup-effective setting warns that a running daemon keeps its
/// current value until the next start, and an env-overridden setting warns that
/// a saved value only takes effect once the variable is removed. So a
/// `config set` against a running daemon, or against an env-overridden key,
/// never reads as silently ignored.
pub fn change_note(key: &str, overlay: &EnvOverlay) -> Option<String> {
    let spec = find(key).ok()?;
    let mut notes: Vec<String> = Vec::new();
    if spec.startup_effective {
        notes.push(
            "this setting applies the next time the daemon starts; a running daemon keeps its current value"
                .to_string(),
        );
    }
    if overlay.overrides_key(key) {
        notes.push(format!(
            "the environment variable {} currently overrides this key; the saved value takes effect once that variable is removed",
            spec.env_var()
        ));
    }
    if notes.is_empty() {
        None
    } else {
        Some(notes.join("; "))
    }
}

fn find(key: &str) -> Result<&'static SettingSpec, SettingsError> {
    registry()
        .iter()
        .find(|s| s.key == key)
        .ok_or_else(|| unknown_key(key))
}

fn unknown_key(key: &str) -> SettingsError {
    let known: Vec<&str> = registry().iter().map(|s| s.key).collect();
    SettingsError(format!(
        "Unknown setting {key}. Known settings: {}",
        known.join(", ")
    ))
}

/// Drop the `github` block entirely once every field in it has been cleared,
/// so an unset config round-trips to exactly the pre-feature shape (no empty
/// `github: {}` line).
fn drop_github_if_empty(config: &mut GlobalConfig) {
    if config.github.as_ref() == Some(&GitHubConfig::default()) {
        config.github = None;
    }
}

/// Drop the `service` block entirely once every field in it has been
/// cleared, so an unset config round-trips to exactly the pre-feature shape
/// (no empty `service: {}` line).
fn drop_service_if_empty(config: &mut GlobalConfig) {
    if config.service.as_ref() == Some(&ServiceConfig::default()) {
        config.service = None;
    }
}

/// Drop the `database` block entirely once every field in it has been
/// cleared, so an unset config round-trips to exactly the pre-feature shape
/// (no empty `database: {}` line).
fn drop_database_if_empty(config: &mut GlobalConfig) {
    if config.database.as_ref() == Some(&DatabaseConfig::default()) {
        config.database = None;
    }
}

// --- github.enabled ----------------------------------------------------------

fn set_enabled(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: bool = value.parse().map_err(|_| {
        SettingsError(format!(
            "github.enabled must be true or false, got '{value}'"
        ))
    })?;
    config
        .github
        .get_or_insert_with(GitHubConfig::default)
        .enabled = Some(parsed);
    Ok(())
}

fn clear_enabled(config: &mut GlobalConfig) {
    if let Some(g) = config.github.as_mut() {
        g.enabled = None;
    }
    drop_github_if_empty(config);
}

fn enabled_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.github.as_ref().and_then(|g| g.enabled).is_none();
    (config.github_enabled().to_string(), is_default)
}

// --- github.poll_secs ----------------------------------------------------------

fn set_poll_secs(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: u64 = value.parse().map_err(|_| {
        SettingsError(format!(
            "github.poll_secs must be a whole number of seconds, got '{value}'"
        ))
    })?;
    if parsed < MIN_POLL_SECS {
        return Err(SettingsError(format!(
            "github.poll_secs must be at least {MIN_POLL_SECS} seconds, got {parsed}"
        )));
    }
    config
        .github
        .get_or_insert_with(GitHubConfig::default)
        .poll_secs = Some(parsed);
    Ok(())
}

fn clear_poll_secs(config: &mut GlobalConfig) {
    if let Some(g) = config.github.as_mut() {
        g.poll_secs = None;
    }
    drop_github_if_empty(config);
}

fn poll_secs_effective(config: &GlobalConfig) -> (String, bool) {
    let stored = config.github.as_ref().and_then(|g| g.poll_secs);
    (
        stored.unwrap_or(DEFAULT_POLL_SECS).to_string(),
        stored.is_none(),
    )
}

// --- github.api_url ----------------------------------------------------------

fn set_api_url(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let trimmed = value.trim();
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"));
    match rest {
        Some(host_and_path)
            if !host_and_path.trim().is_empty() && !host_and_path.contains(char::is_whitespace) =>
        {
            config
                .github
                .get_or_insert_with(GitHubConfig::default)
                .api_url = Some(trimmed.to_string());
            Ok(())
        }
        _ => Err(SettingsError(format!(
            "github.api_url must be a http:// or https:// url, got '{value}'"
        ))),
    }
}

fn clear_api_url(config: &mut GlobalConfig) {
    if let Some(g) = config.github.as_mut() {
        g.api_url = None;
    }
    drop_github_if_empty(config);
}

fn api_url_effective(config: &GlobalConfig) -> (String, bool) {
    let stored = config.github.as_ref().and_then(|g| g.api_url.clone());
    let is_default = stored.is_none();
    (
        stored.unwrap_or_else(|| DEFAULT_API_URL.to_string()),
        is_default,
    )
}

// --- github.oauth_client_id ---------------------------------------------------

fn set_oauth_client_id(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SettingsError(
            "github.oauth_client_id must not be empty".to_string(),
        ));
    }
    config
        .github
        .get_or_insert_with(GitHubConfig::default)
        .oauth_client_id = Some(trimmed.to_string());
    Ok(())
}

fn clear_oauth_client_id(config: &mut GlobalConfig) {
    if let Some(g) = config.github.as_mut() {
        g.oauth_client_id = None;
    }
    drop_github_if_empty(config);
}

fn oauth_client_id_effective(config: &GlobalConfig) -> (String, bool) {
    let stored = config
        .github
        .as_ref()
        .and_then(|g| g.oauth_client_id.clone());
    let is_default = stored.is_none();
    // The effective default is the client id baked into the binary, not
    // "nothing": sign-ins work out of the box and the snapshot says so.
    (
        stored.unwrap_or_else(|| crystalline_remote::GITHUB_CLIENT_ID.to_string()),
        is_default,
    )
}

// --- service.read_only ---------------------------------------------------------

fn set_read_only(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: bool = value.parse().map_err(|_| {
        SettingsError(format!(
            "service.read_only must be true or false, got '{value}'"
        ))
    })?;
    config
        .service
        .get_or_insert_with(ServiceConfig::default)
        .read_only = Some(parsed);
    Ok(())
}

fn clear_read_only(config: &mut GlobalConfig) {
    if let Some(s) = config.service.as_mut() {
        s.read_only = None;
    }
    drop_service_if_empty(config);
}

fn read_only_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.service.as_ref().and_then(|s| s.read_only).is_none();
    (config.read_only().to_string(), is_default)
}

// --- service.http ----------------------------------------------------------

fn set_http(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let invalid = || {
        SettingsError(format!(
            "service.http must be true, false or a host:port address, got '{value}'"
        ))
    };
    if value.is_empty() || value.contains(char::is_whitespace) {
        return Err(invalid());
    }
    let setting = match value {
        "true" => HttpSetting::Enabled(true),
        "false" => HttpSetting::Enabled(false),
        addr if addr.contains(':') => HttpSetting::Address(addr.to_string()),
        _ => return Err(invalid()),
    };
    config
        .service
        .get_or_insert_with(ServiceConfig::default)
        .http = Some(setting);
    Ok(())
}

fn clear_http(config: &mut GlobalConfig) {
    if let Some(s) = config.service.as_mut() {
        s.http = None;
    }
    drop_service_if_empty(config);
}

fn http_effective(config: &GlobalConfig) -> (String, bool) {
    match config.service.as_ref().and_then(|s| s.http.as_ref()) {
        Some(HttpSetting::Enabled(v)) => (v.to_string(), false),
        Some(HttpSetting::Address(a)) => (a.clone(), false),
        None => ("false".to_string(), true),
    }
}

// --- database.backend ----------------------------------------------------------

fn set_database_backend(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let backend = match value {
        "turso" => DatabaseBackend::Turso,
        "postgres" => DatabaseBackend::Postgres,
        _ => {
            return Err(SettingsError(format!(
                "database.backend must be turso or postgres, got '{value}'"
            )));
        }
    };
    // Combined backend+url validation stays at store-factory time (see
    // `DatabaseConfig::validate`); a per-key `config set` never rejects a
    // backend on its own, even one that leaves the pair momentarily invalid.
    config
        .database
        .get_or_insert_with(DatabaseConfig::default)
        .backend = backend;
    Ok(())
}

fn clear_database_backend(config: &mut GlobalConfig) {
    if let Some(d) = config.database.as_mut() {
        d.backend = DatabaseBackend::default();
    }
    drop_database_if_empty(config);
}

fn database_backend_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.database.is_none();
    let backend = match config.database().backend {
        DatabaseBackend::Turso => "turso",
        DatabaseBackend::Postgres => "postgres",
    };
    (backend.to_string(), is_default)
}

// --- database.url ----------------------------------------------------------

fn set_database_url(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    if value.trim().is_empty() {
        return Err(SettingsError("database.url must not be empty".to_string()));
    }
    config
        .database
        .get_or_insert_with(DatabaseConfig::default)
        .url = Some(value.to_string());
    Ok(())
}

fn clear_database_url(config: &mut GlobalConfig) {
    if let Some(d) = config.database.as_mut() {
        d.url = None;
    }
    drop_database_if_empty(config);
}

fn database_url_effective(config: &GlobalConfig) -> (String, bool) {
    let stored = config.database.as_ref().and_then(|d| d.url.clone());
    let is_default = stored.is_none();
    (stored.unwrap_or_default(), is_default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known_keys() -> Vec<&'static str> {
        registry().iter().map(|s| s.key).collect()
    }

    #[test]
    fn registry_lists_exactly_the_eight_keys_in_order() {
        assert_eq!(
            known_keys(),
            vec![
                "github.enabled",
                "github.poll_secs",
                "github.api_url",
                "github.oauth_client_id",
                "service.read_only",
                "service.http",
                "database.backend",
                "database.url",
            ]
        );
    }

    #[test]
    fn env_var_derives_mechanically_from_every_key() {
        let derived: Vec<(&str, String)> =
            registry().iter().map(|s| (s.key, s.env_var())).collect();
        assert_eq!(
            derived,
            vec![
                ("github.enabled", "CRYSTALLINE_GITHUB_ENABLED".to_string()),
                (
                    "github.poll_secs",
                    "CRYSTALLINE_GITHUB_POLL_SECS".to_string()
                ),
                ("github.api_url", "CRYSTALLINE_GITHUB_API_URL".to_string()),
                (
                    "github.oauth_client_id",
                    "CRYSTALLINE_GITHUB_OAUTH_CLIENT_ID".to_string()
                ),
                (
                    "service.read_only",
                    "CRYSTALLINE_SERVICE_READ_ONLY".to_string()
                ),
                ("service.http", "CRYSTALLINE_SERVICE_HTTP".to_string()),
                (
                    "database.backend",
                    "CRYSTALLINE_DATABASE_BACKEND".to_string()
                ),
                ("database.url", "CRYSTALLINE_DATABASE_URL".to_string()),
            ]
        );
    }

    #[test]
    fn change_note_is_present_only_for_startup_effective_keys() {
        let no_env = EnvOverlay::default();
        assert!(change_note("github.enabled", &no_env).is_none());
        assert!(change_note("github.poll_secs", &no_env).is_none());
        assert!(change_note("github.api_url", &no_env).is_none());
        assert!(change_note("github.oauth_client_id", &no_env).is_none());
        assert!(change_note("service.read_only", &no_env).is_some());
        assert!(change_note("service.http", &no_env).is_some());
        assert!(change_note("database.backend", &no_env).is_some());
        assert!(change_note("database.url", &no_env).is_some());
        assert!(change_note("github.bogus", &no_env).is_none());
    }

    #[test]
    fn change_note_flags_an_active_env_override() {
        let overlay =
            EnvOverlay::from_vars([("CRYSTALLINE_GITHUB_ENABLED".to_string(), "true".to_string())])
                .unwrap();
        // A non-startup-effective key gets only the override note.
        let note = change_note("github.enabled", &overlay).unwrap();
        assert!(note.contains("CRYSTALLINE_GITHUB_ENABLED"), "{note}");
        assert!(
            note.contains("takes effect once that variable is removed"),
            "{note}"
        );
        assert!(
            !note.contains("the next time the daemon starts"),
            "github.enabled is not startup-effective: {note}"
        );
    }

    #[test]
    fn change_note_joins_the_startup_and_override_notes() {
        let overlay = EnvOverlay::from_vars([(
            "CRYSTALLINE_SERVICE_READ_ONLY".to_string(),
            "true".to_string(),
        )])
        .unwrap();
        // A startup-effective key that is also env-overridden carries both.
        let note = change_note("service.read_only", &overlay).unwrap();
        assert!(note.contains("the next time the daemon starts"), "{note}");
        assert!(note.contains("CRYSTALLINE_SERVICE_READ_ONLY"), "{note}");
    }

    #[test]
    fn apply_github_enabled_happy_path() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "github.enabled", "true").unwrap();
        assert!(cfg.github_enabled());
        apply(&mut cfg, "github.enabled", "false").unwrap();
        assert!(!cfg.github_enabled());
    }

    #[test]
    fn apply_github_enabled_rejects_non_bool() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "github.enabled", "yes").unwrap_err();
        assert!(err.to_string().contains("github.enabled"));
        assert!(err.to_string().contains("yes"));
    }

    #[test]
    fn apply_github_poll_secs_happy_path() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "github.poll_secs", "120").unwrap();
        assert_eq!(cfg.github.as_ref().unwrap().poll_secs, Some(120));
    }

    #[test]
    fn apply_github_poll_secs_rejects_non_u64() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "github.poll_secs", "soon").unwrap_err();
        assert!(err.to_string().contains("github.poll_secs"));
    }

    #[test]
    fn apply_github_poll_secs_rejects_59() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "github.poll_secs", "59").unwrap_err();
        assert!(err.to_string().contains("60"), "{err}");
        assert!(cfg.github.is_none(), "a rejected value must not be written");
    }

    #[test]
    fn apply_github_poll_secs_accepts_60() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "github.poll_secs", "60").unwrap();
        assert_eq!(cfg.github.as_ref().unwrap().poll_secs, Some(60));
    }

    #[test]
    fn apply_github_api_url_happy_path() {
        let mut cfg = GlobalConfig::default();
        apply(
            &mut cfg,
            "github.api_url",
            "https://github.example.com/api/v3",
        )
        .unwrap();
        assert_eq!(
            cfg.github.as_ref().unwrap().api_url.as_deref(),
            Some("https://github.example.com/api/v3")
        );
    }

    #[test]
    fn apply_github_api_url_rejects_non_http_url() {
        let mut cfg = GlobalConfig::default();
        for bad in [
            "ftp://example.com",
            "example.com",
            "",
            "https://",
            "http:// space",
        ] {
            let err = apply(&mut cfg, "github.api_url", bad);
            assert!(err.is_err(), "expected '{bad}' to be rejected");
        }
    }

    #[test]
    fn apply_github_oauth_client_id_happy_path() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "github.oauth_client_id", "abc123").unwrap();
        assert_eq!(
            cfg.github.as_ref().unwrap().oauth_client_id.as_deref(),
            Some("abc123")
        );
    }

    #[test]
    fn apply_github_oauth_client_id_rejects_empty() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "github.oauth_client_id", "   ").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn apply_unknown_key_lists_every_known_key() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "github.bogus", "x").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("github.bogus"));
        for key in known_keys() {
            assert!(message.contains(key), "{message} should list {key}");
        }
    }

    #[test]
    fn unset_unknown_key_also_lists_every_known_key() {
        let mut cfg = GlobalConfig::default();
        let err = unset(&mut cfg, "github.bogus").unwrap_err();
        assert!(err.to_string().contains("github.enabled"));
    }

    #[test]
    fn unset_returns_to_default_and_drops_an_emptied_github_block() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "github.enabled", "true").unwrap();
        assert!(cfg.github.is_some());

        unset(&mut cfg, "github.enabled").unwrap();
        assert!(
            cfg.github.is_none(),
            "the only set field was cleared, so the block should vanish"
        );
        assert!(!cfg.github_enabled(), "back to the off default");

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("github"),
            "an emptied github block must not round-trip into the yaml: {yaml}"
        );
    }

    #[test]
    fn unset_one_field_keeps_the_block_when_siblings_remain_set() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "github.enabled", "true").unwrap();
        apply(&mut cfg, "github.poll_secs", "90").unwrap();

        unset(&mut cfg, "github.enabled").unwrap();
        assert!(
            cfg.github.is_some(),
            "poll_secs is still set, so the block must survive"
        );
        assert_eq!(cfg.github.as_ref().unwrap().poll_secs, Some(90));

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(yaml.contains("poll_secs: 90"), "{yaml}");
        assert!(!yaml.contains("enabled"), "{yaml}");
    }

    #[test]
    fn snapshot_marks_defaults_and_set_values_in_registry_order() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "github.enabled", "true").unwrap();

        let views = snapshot(&cfg, &EnvOverlay::default());
        assert_eq!(views.len(), 8);
        assert_eq!(
            views.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(),
            vec![
                "github.enabled",
                "github.poll_secs",
                "github.api_url",
                "github.oauth_client_id",
                "service.read_only",
                "service.http",
                "database.backend",
                "database.url",
            ]
        );

        let enabled = &views[0];
        assert_eq!(enabled.value, "true");
        assert_eq!(enabled.source, SettingSource::Config);
        assert!(!enabled.doc.is_empty());

        let poll_secs = &views[1];
        assert_eq!(poll_secs.value, "300");
        assert_eq!(poll_secs.source, SettingSource::Default);

        let api_url = &views[2];
        assert_eq!(api_url.value, "https://api.github.com");
        assert_eq!(api_url.source, SettingSource::Default);

        let oauth = &views[3];
        assert_eq!(oauth.value, crystalline_remote::GITHUB_CLIENT_ID);
        assert_eq!(oauth.source, SettingSource::Default);

        let read_only = &views[4];
        assert_eq!(read_only.value, "false");
        assert_eq!(read_only.source, SettingSource::Default);

        let http = &views[5];
        assert_eq!(http.value, "false");
        assert_eq!(http.source, SettingSource::Default);

        let backend = &views[6];
        assert_eq!(backend.value, "turso");
        assert_eq!(backend.source, SettingSource::Default);

        let url = &views[7];
        assert_eq!(url.value, "");
        assert_eq!(url.source, SettingSource::Default);
    }

    #[test]
    fn snapshot_marks_an_env_overridden_key_and_shows_the_env_value() {
        // The file turns github.enabled off; the environment turns it on. The
        // snapshot must show the effective (env) value and mark its source Env,
        // while a key the environment does not touch keeps its file source.
        let mut file = GlobalConfig::default();
        apply(&mut file, "github.enabled", "false").unwrap();
        apply(&mut file, "github.poll_secs", "120").unwrap();

        let overlay =
            EnvOverlay::from_vars([("CRYSTALLINE_GITHUB_ENABLED".to_string(), "true".to_string())])
                .unwrap();
        let views = snapshot(&file, &overlay);

        let enabled = views.iter().find(|v| v.key == "github.enabled").unwrap();
        assert_eq!(enabled.value, "true", "the effective env value is shown");
        assert_eq!(enabled.source, SettingSource::Env);

        let poll = views.iter().find(|v| v.key == "github.poll_secs").unwrap();
        assert_eq!(poll.value, "120");
        assert_eq!(poll.source, SettingSource::Config, "the file value stands");
    }

    // --- service.read_only ---------------------------------------------------------

    #[test]
    fn apply_service_read_only_happy_path() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "service.read_only", "true").unwrap();
        assert!(cfg.read_only());
        apply(&mut cfg, "service.read_only", "false").unwrap();
        assert!(!cfg.read_only());
    }

    #[test]
    fn apply_service_read_only_rejects_non_bool() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "service.read_only", "yes").unwrap_err();
        assert!(err.to_string().contains("service.read_only"));
    }

    #[test]
    fn unset_service_read_only_drops_an_emptied_service_block() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "service.read_only", "true").unwrap();
        assert!(cfg.service.is_some());

        unset(&mut cfg, "service.read_only").unwrap();
        assert!(
            cfg.service.is_none(),
            "the only set field was cleared, so the block should vanish"
        );
        assert!(!cfg.read_only());

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("service"),
            "an emptied service block must not round-trip into the yaml: {yaml}"
        );
    }

    // --- service.http ----------------------------------------------------------

    #[test]
    fn apply_service_http_accepts_bool_spellings_and_an_address() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "service.http", "true").unwrap();
        assert_eq!(
            cfg.service.as_ref().unwrap().http,
            Some(HttpSetting::Enabled(true))
        );

        apply(&mut cfg, "service.http", "false").unwrap();
        assert_eq!(
            cfg.service.as_ref().unwrap().http,
            Some(HttpSetting::Enabled(false))
        );

        apply(&mut cfg, "service.http", "127.0.0.1:7411").unwrap();
        assert_eq!(
            cfg.service.as_ref().unwrap().http,
            Some(HttpSetting::Address("127.0.0.1:7411".to_string()))
        );
    }

    #[test]
    fn apply_service_http_rejects_whitespace_and_non_addresses() {
        let mut cfg = GlobalConfig::default();
        for bad in ["", "yes", "127.0.0.1 7411", "localhost"] {
            let err = apply(&mut cfg, "service.http", bad);
            assert!(err.is_err(), "expected '{bad}' to be rejected");
        }
    }

    #[test]
    fn unset_service_http_drops_an_emptied_service_block() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "service.http", "true").unwrap();
        unset(&mut cfg, "service.http").unwrap();
        assert!(cfg.service.is_none());

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(!yaml.contains("service"), "{yaml}");
    }

    #[test]
    fn service_block_survives_when_a_sibling_field_remains_set() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "service.read_only", "true").unwrap();
        apply(&mut cfg, "service.http", "true").unwrap();

        unset(&mut cfg, "service.http").unwrap();
        assert!(
            cfg.service.is_some(),
            "read_only is still set, so the block must survive"
        );
        assert!(cfg.read_only());
    }

    // --- database.backend ----------------------------------------------------------

    #[test]
    fn apply_database_backend_happy_path() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "database.backend", "postgres").unwrap();
        assert_eq!(
            cfg.database.as_ref().unwrap().backend,
            DatabaseBackend::Postgres
        );
    }

    #[test]
    fn apply_database_backend_rejects_unknown_backend() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "database.backend", "mysql").unwrap_err();
        assert!(err.to_string().contains("database.backend"));
        assert!(err.to_string().contains("mysql"));
    }

    #[test]
    fn apply_database_backend_does_not_validate_against_url() {
        // Setting a postgres backend with no url must succeed at this layer;
        // the combined check happens at store-factory time.
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "database.backend", "postgres").unwrap();
        assert_eq!(cfg.database.as_ref().unwrap().url, None);
    }

    #[test]
    fn unset_database_backend_drops_an_emptied_database_block() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "database.backend", "postgres").unwrap();
        assert!(cfg.database.is_some());

        unset(&mut cfg, "database.backend").unwrap();
        assert!(
            cfg.database.is_none(),
            "the only set field was cleared, so the block should vanish"
        );

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("database"),
            "an emptied database block must not round-trip into the yaml: {yaml}"
        );
    }

    // --- database.url ----------------------------------------------------------

    #[test]
    fn apply_database_url_happy_path() {
        let mut cfg = GlobalConfig::default();
        apply(
            &mut cfg,
            "database.url",
            "postgres://u:p@db:5432/crystalline",
        )
        .unwrap();
        assert_eq!(
            cfg.database.as_ref().unwrap().url.as_deref(),
            Some("postgres://u:p@db:5432/crystalline")
        );
    }

    #[test]
    fn apply_database_url_rejects_empty() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "database.url", "   ").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn unset_database_url_drops_an_emptied_database_block() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "database.url", "/tmp/custom.db").unwrap();
        unset(&mut cfg, "database.url").unwrap();
        assert!(cfg.database.is_none());

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(!yaml.contains("database"), "{yaml}");
    }

    #[test]
    fn database_block_survives_when_a_sibling_field_remains_set() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "database.backend", "postgres").unwrap();
        apply(&mut cfg, "database.url", "postgres://db/crystalline").unwrap();

        unset(&mut cfg, "database.url").unwrap();
        assert!(
            cfg.database.is_some(),
            "backend is still set, so the block must survive"
        );
        assert_eq!(
            cfg.database.as_ref().unwrap().backend,
            DatabaseBackend::Postgres
        );
    }

    #[test]
    fn database_block_survives_when_only_the_backend_is_unset() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "database.backend", "postgres").unwrap();
        apply(&mut cfg, "database.url", "postgres://db/crystalline").unwrap();

        unset(&mut cfg, "database.backend").unwrap();
        let database = cfg.database.as_ref().expect("url keeps the block alive");
        assert_eq!(database.backend, DatabaseBackend::Turso);
        assert_eq!(database.url.as_deref(), Some("postgres://db/crystalline"));
    }
}
