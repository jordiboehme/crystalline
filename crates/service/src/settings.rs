//! The settings registry: the single source of truth for which configuration
//! keys an agent or a user may change through `configure`, their types and
//! bounds and how each one maps onto [`GlobalConfig`].
//!
//! Nothing outside this module may match on a setting key: the ctl `configure`
//! command, the CLI `config` verbs and (in a later task) the MCP `configure`
//! tool all read [`registry`] for the key list and documentation and call
//! [`apply`], [`unset`] and [`snapshot`] to act on one.

use crystalline_core::config::{GitHubConfig, GlobalConfig};

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
    /// Parse and validate a string value, then write it into `config`.
    apply: fn(&mut GlobalConfig, &str) -> Result<(), SettingsError>,
    /// Reset this setting to its default, removing it from `config` (and its
    /// parent block, when emptied).
    clear: fn(&mut GlobalConfig),
    /// The effective display value and whether it is explicitly set (`false`)
    /// or a default (`true`).
    effective: fn(&GlobalConfig) -> (String, bool),
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
            apply: set_enabled,
            clear: clear_enabled,
            effective: enabled_effective,
        },
        SettingSpec {
            key: "github.poll_secs",
            doc: "How often the daemon polls GitHub for changes, in seconds (minimum 60)",
            kind: SettingKind::U64,
            apply: set_poll_secs,
            clear: clear_poll_secs,
            effective: poll_secs_effective,
        },
        SettingSpec {
            key: "github.api_url",
            doc: "The GitHub API base url, for a GitHub Enterprise Server instance",
            kind: SettingKind::String,
            apply: set_api_url,
            clear: clear_api_url,
            effective: api_url_effective,
        },
        SettingSpec {
            key: "github.oauth_client_id",
            doc: "A self-hosted OAuth App client id, overriding the embedded default",
            kind: SettingKind::String,
            apply: set_oauth_client_id,
            clear: clear_oauth_client_id,
            effective: oauth_client_id_effective,
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
/// value, whether that value is explicitly set or a default and its doc line.
pub fn snapshot(config: &GlobalConfig) -> Vec<SettingView> {
    registry()
        .iter()
        .map(|spec| {
            let (value, is_default) = (spec.effective)(config);
            SettingView {
                key: spec.key.to_string(),
                value,
                is_default,
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
    /// Whether this is the default (`true`) or was explicitly set (`false`).
    pub is_default: bool,
    /// The setting's one-line documentation.
    pub doc: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn known_keys() -> Vec<&'static str> {
        registry().iter().map(|s| s.key).collect()
    }

    #[test]
    fn registry_lists_exactly_the_four_v1_keys_in_order() {
        assert_eq!(
            known_keys(),
            vec![
                "github.enabled",
                "github.poll_secs",
                "github.api_url",
                "github.oauth_client_id",
            ]
        );
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

        let views = snapshot(&cfg);
        assert_eq!(views.len(), 4);
        assert_eq!(
            views.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(),
            vec![
                "github.enabled",
                "github.poll_secs",
                "github.api_url",
                "github.oauth_client_id",
            ]
        );

        let enabled = &views[0];
        assert_eq!(enabled.value, "true");
        assert!(!enabled.is_default);
        assert!(!enabled.doc.is_empty());

        let poll_secs = &views[1];
        assert_eq!(poll_secs.value, "300");
        assert!(poll_secs.is_default);

        let api_url = &views[2];
        assert_eq!(api_url.value, "https://api.github.com");
        assert!(api_url.is_default);

        let oauth = &views[3];
        assert_eq!(oauth.value, crystalline_remote::GITHUB_CLIENT_ID);
        assert!(oauth.is_default);
    }
}
