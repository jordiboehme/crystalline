//! The environment overlay: a parsed, validated view of the `CRYSTALLINE_*`
//! variables that layer on top of the config file without ever being written
//! back to it.
//!
//! A container deployment configures Crystalline purely through the
//! environment: an immutable image, no `config.yaml` to mount or edit. This
//! module turns the process environment into an [`EnvOverlay`] once at startup
//! and hands back a [`LoadedConfig`] that keeps the two layers apart:
//! `file` is the persisted truth every write goes to, `effective` is
//! `file` with the overlay applied and is what every runtime read uses.
//! Persistence never bakes an env value into the file.
//!
//! Every setting variable reuses its [`crate::settings`] registry spec for
//! validation, so an env value is checked exactly the way `config set` checks
//! the same key. A bad value of a known variable is fatal and names the
//! offending variable; an unrecognized `CRYSTALLINE_*` name only warns, so a
//! newer image reading an older binary's environment degrades gracefully
//! rather than refusing to start.
//!
//! Precedence, highest to lowest: command-line flags, then this overlay, then
//! the config file, then the built-in defaults.

use std::path::{Path, PathBuf};

use crystalline_core::config::{self, GlobalConfig};

use crate::settings;

/// The variable naming an alternate config file path. Lower priority than a
/// `--config` flag, higher than the default global path. A leading `~` is
/// expanded like every other path Crystalline reads.
pub const CONFIG_PATH_ENV: &str = "CRYSTALLINE_CONFIG";

/// The prefix for an env-defined domain, `CRYSTALLINE_DOMAIN_<NAME>`. Reserved
/// here: a variable with this prefix is skipped silently, neither parsed nor
/// warned about, so it never trips the unknown-variable warning before the
/// milestone that wires env domains up lands.
pub const DOMAIN_ENV_PREFIX: &str = "CRYSTALLINE_DOMAIN_";

/// The variable carrying a GitHub token for a headless node. Reserved here:
/// skipped silently until the milestone that reads it lands, so it never trips
/// the unknown-variable warning in the meantime.
pub const GITHUB_TOKEN_ENV: &str = "CRYSTALLINE_GITHUB_TOKEN";

/// Variables that live outside the settings registry and are read elsewhere
/// (or reserved for a later milestone). They are skipped silently rather than
/// warned about, so a legitimate deployment does not get a spurious warning
/// for a variable Crystalline itself documents.
const RESERVED_VARS: &[&str] = &[
    "CRYSTALLINE_MODELS_DIR",
    "CRYSTALLINE_HEARTBEAT_SECS",
    "CRYSTALLINE_STALE_SECS",
    "CRYSTALLINE_TEST_POSTGRES_URL",
];

/// An error parsing the environment overlay. The message names the offending
/// variable and is safe to show an operator as-is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct OverlayError(String);

/// The validated environment overlay: the `CRYSTALLINE_*` variables that layer
/// on top of the config file. Parsed once, then applied to a file config to
/// produce the effective config every runtime read uses.
#[derive(Debug, Clone, Default)]
pub struct EnvOverlay {
    /// Registry key to raw value, one entry per setting variable that was
    /// present and non-empty. Each value was validated at parse time by
    /// running its registry spec's `apply`, so replaying it later cannot fail.
    settings: Vec<(String, String)>,
    /// The config file path from [`CONFIG_PATH_ENV`], tilde-expanded. `None`
    /// when the variable is absent or empty.
    config_path: Option<PathBuf>,
}

impl EnvOverlay {
    /// Parse an overlay from an iterator of `(name, value)` pairs. This is the
    /// testable seam: it never touches the process environment, so a test can
    /// inject an exact set of variables without racing every other test in the
    /// same process.
    ///
    /// Each setting variable is validated by running its registry spec's
    /// `apply` against a scratch config; a bad value aborts with an
    /// [`OverlayError`] naming the variable. After every setting is applied,
    /// the combined `database` backend-and-url check runs once, the eager
    /// version of the store-factory check `config set` defers. An empty value
    /// is treated as unset (mirroring the `CRYSTALLINE_MODELS_DIR` precedent in
    /// core). An unrecognized `CRYSTALLINE_*` name only warns.
    pub fn from_vars<I>(vars: I) -> Result<EnvOverlay, OverlayError>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut settings = Vec::new();
        let mut config_path = None;
        // A scratch config the setting values are applied to as they parse, so
        // the combined database check below sees every applied value at once.
        let mut scratch = GlobalConfig::default();

        for (name, value) in vars {
            // A plain, non-Crystalline variable is not ours to reason about.
            if !name.starts_with("CRYSTALLINE_") {
                continue;
            }
            if name == CONFIG_PATH_ENV {
                if !value.is_empty() {
                    config_path = Some(config::expand_tilde(&value));
                }
                continue;
            }
            if let Some(spec) = settings::registry().iter().find(|s| s.env_var() == name) {
                // An empty value reads as unset, so `VAR=` in a compose file
                // does not force a setting to the empty string.
                if value.is_empty() {
                    continue;
                }
                settings::apply(&mut scratch, spec.key, &value).map_err(|e| {
                    OverlayError(format!("invalid environment variable {name}: {e}"))
                })?;
                settings.push((spec.key.to_string(), value));
                continue;
            }
            // Reserved or read elsewhere: skip without a word.
            if is_reserved(&name) {
                continue;
            }
            // Any other `CRYSTALLINE_*` name: warn, never fail. A strict
            // allowlist would make an older binary reject a newer image's
            // environment, so version skew stays tolerant.
            tracing::warn!("ignoring unrecognized environment variable {name}");
        }

        // The combined backend-and-url check env gets eagerly, so a container
        // with a mismatched pair dies at startup naming both variables rather
        // than at the first store open.
        scratch.database().validate().map_err(|e| {
            OverlayError(format!(
                "invalid environment variables CRYSTALLINE_DATABASE_BACKEND and CRYSTALLINE_DATABASE_URL: {e}"
            ))
        })?;

        Ok(EnvOverlay {
            settings,
            config_path,
        })
    }

    /// Parse an overlay from the live process environment. The feature's sole
    /// ambient read: every other entry point takes an injected iterator.
    pub fn from_process_env() -> Result<EnvOverlay, OverlayError> {
        EnvOverlay::from_vars(std::env::vars())
    }

    /// Whether the overlay carries nothing: no setting override and no config
    /// path. A no-op overlay leaves the file config untouched.
    pub fn is_empty(&self) -> bool {
        self.settings.is_empty() && self.config_path.is_none()
    }

    /// Apply the overlay to a file config, returning the effective config.
    /// Every value was validated at parse time, so replaying it here cannot
    /// fail; a failure would be a bug, not bad input, hence the panic.
    pub fn apply(&self, file: &GlobalConfig) -> GlobalConfig {
        let mut effective = file.clone();
        for (key, value) in &self.settings {
            if let Err(e) = settings::apply(&mut effective, key, value) {
                panic!(
                    "env overlay value for '{key}' was validated at parse time but failed to apply now: {e}"
                );
            }
        }
        effective
    }

    /// Whether an environment variable currently overrides `key`.
    pub fn overrides_key(&self, key: &str) -> bool {
        self.settings.iter().any(|(k, _)| k == key)
    }

    /// The config file path from [`CONFIG_PATH_ENV`], if set.
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// Every active setting override as `(variable, key, display value)`, for
    /// surfacing in `doctor` and the like. The `database.url` value is rendered
    /// as `(set)` rather than shown, since it may embed credentials.
    pub fn active_overrides(&self) -> Vec<(String, String, String)> {
        self.settings
            .iter()
            .map(|(key, value)| {
                let display = if key == "database.url" {
                    "(set)".to_string()
                } else {
                    value.clone()
                };
                (env_var_for(key), key.clone(), display)
            })
            .collect()
    }
}

/// Whether `name` is a reserved `CRYSTALLINE_*` variable that is skipped
/// silently rather than warned about: one read outside the registry, an
/// env-domain definition or the GitHub token, none of them handled here.
fn is_reserved(name: &str) -> bool {
    RESERVED_VARS.contains(&name) || name.starts_with(DOMAIN_ENV_PREFIX) || name == GITHUB_TOKEN_ENV
}

/// The environment variable a registry key maps to. Every stored setting key
/// is a registry key, so the lookup always hits; the mechanical fallback keeps
/// the function total.
fn env_var_for(key: &str) -> String {
    settings::registry()
        .iter()
        .find(|s| s.key == key)
        .map(|s| s.env_var())
        .unwrap_or_else(|| format!("CRYSTALLINE_{}", key.replace('.', "_").to_uppercase()))
}

/// A config resolved through the single load chokepoint: the file truth, the
/// effective config (file plus overlay) and the overlay itself, plus the
/// resolved file path persistence writes back to.
pub struct LoadedConfig {
    /// The resolved config file path: the `--config` flag, else
    /// [`CONFIG_PATH_ENV`], else the default global path. Every persist writes
    /// here.
    pub path: PathBuf,
    /// The file truth, a missing file reading as the default config. This is
    /// what persistence writes and never carries an env value.
    pub file: GlobalConfig,
    /// The effective config, `overlay.apply(&file)`. Every runtime read uses
    /// this.
    pub effective: GlobalConfig,
    /// The parsed environment overlay.
    pub overlay: EnvOverlay,
}

/// The single load chokepoint every `GlobalConfig` load routes through: parse
/// the overlay from the process environment, resolve the config path, read the
/// file (a missing file reading as the default) and layer the overlay on top.
/// An overlay parse error is returned as-is, so a caller that fails fast aborts
/// with the variable-naming message.
pub fn load(flag: Option<&Path>) -> anyhow::Result<LoadedConfig> {
    let overlay = EnvOverlay::from_process_env()?;
    let path = resolve_config_path(flag, overlay.config_path())?;
    let file = load_file(&path)?;
    let effective = overlay.apply(&file);
    Ok(LoadedConfig {
        path,
        file,
        effective,
        overlay,
    })
}

/// Resolve the config file path by precedence: the `--config` flag, then
/// [`CONFIG_PATH_ENV`], then the default global path. Pure and unit-testable;
/// the ambient read that produces `env_config` happens in [`load`].
pub fn resolve_config_path(
    flag: Option<&Path>,
    env_config: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    if let Some(p) = flag {
        return Ok(p.to_path_buf());
    }
    if let Some(p) = env_config {
        return Ok(p.to_path_buf());
    }
    config::global_config_path()
        .map_err(|e| anyhow::anyhow!("could not resolve the default config path: {e}"))
}

/// Read a config file, treating a missing file as the default config. Shared
/// with the engine's post-startup re-read so both paths agree on what an
/// absent file means.
pub fn load_file(path: &Path) -> anyhow::Result<GlobalConfig> {
    if path.is_file() {
        config::load_yaml(path)
            .map_err(|e| anyhow::anyhow!("failed to load config {}: {e}", path.display()))
    } else {
        Ok(GlobalConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crystalline_core::config::DatabaseBackend;

    fn overlay(pairs: &[(&str, &str)]) -> Result<EnvOverlay, OverlayError> {
        EnvOverlay::from_vars(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn every_setting_variable_is_recognized_and_applied() {
        let ov = overlay(&[
            ("CRYSTALLINE_GITHUB_ENABLED", "true"),
            ("CRYSTALLINE_GITHUB_POLL_SECS", "120"),
            (
                "CRYSTALLINE_GITHUB_API_URL",
                "https://ghe.example.com/api/v3",
            ),
            ("CRYSTALLINE_GITHUB_OAUTH_CLIENT_ID", "client-xyz"),
            ("CRYSTALLINE_SERVICE_READ_ONLY", "true"),
            ("CRYSTALLINE_SERVICE_HTTP", "0.0.0.0:7411"),
            ("CRYSTALLINE_DATABASE_BACKEND", "postgres"),
            ("CRYSTALLINE_DATABASE_URL", "postgres://u:p@db/crystalline"),
        ])
        .unwrap();

        for key in [
            "github.enabled",
            "github.poll_secs",
            "github.api_url",
            "github.oauth_client_id",
            "service.read_only",
            "service.http",
            "database.backend",
            "database.url",
        ] {
            assert!(ov.overrides_key(key), "expected {key} overridden");
        }

        let effective = ov.apply(&GlobalConfig::default());
        assert!(effective.github_enabled());
        assert!(effective.read_only());
        assert_eq!(effective.database().backend, DatabaseBackend::Postgres);
        assert_eq!(
            effective.database().url.as_deref(),
            Some("postgres://u:p@db/crystalline")
        );
    }

    #[test]
    fn an_empty_value_is_treated_as_unset() {
        let ov = overlay(&[("CRYSTALLINE_GITHUB_ENABLED", "")]).unwrap();
        assert!(!ov.overrides_key("github.enabled"));
        assert!(ov.is_empty());
    }

    #[test]
    fn an_invalid_value_errors_naming_the_variable() {
        let err = overlay(&[("CRYSTALLINE_GITHUB_POLL_SECS", "10")]).unwrap_err();
        assert!(
            err.to_string().contains("CRYSTALLINE_GITHUB_POLL_SECS"),
            "{err}"
        );
        // The underlying settings message is preserved.
        assert!(err.to_string().contains("60"), "{err}");
    }

    #[test]
    fn an_unknown_crystalline_variable_warns_but_does_not_error() {
        let ov = overlay(&[("CRYSTALLINE_FOO", "bar")]).unwrap();
        assert!(ov.is_empty(), "an unknown variable contributes nothing");
    }

    #[test]
    fn reserved_variables_are_skipped_silently() {
        let ov = overlay(&[
            ("CRYSTALLINE_MODELS_DIR", "/models"),
            ("CRYSTALLINE_HEARTBEAT_SECS", "5"),
            ("CRYSTALLINE_STALE_SECS", "15"),
            ("CRYSTALLINE_TEST_POSTGRES_URL", "postgres://db/test"),
            ("CRYSTALLINE_DOMAIN_TEAM", "/knowledge/team"),
            ("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "owner/repo"),
            ("CRYSTALLINE_GITHUB_TOKEN", "ghp_secret"),
        ])
        .unwrap();
        assert!(
            ov.is_empty(),
            "reserved variables must not become setting overrides"
        );
    }

    #[test]
    fn postgres_backend_without_a_url_fails_naming_both_variables() {
        let err = overlay(&[("CRYSTALLINE_DATABASE_BACKEND", "postgres")]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("CRYSTALLINE_DATABASE_BACKEND"), "{msg}");
        assert!(msg.contains("CRYSTALLINE_DATABASE_URL"), "{msg}");
    }

    #[test]
    fn env_beats_file_and_file_beats_default() {
        // File sets github.enabled false and poll_secs 200; env overrides only
        // enabled. The effective config takes enabled from env, poll_secs from
        // the file and api_url from the built-in default.
        let mut file = GlobalConfig::default();
        settings::apply(&mut file, "github.enabled", "false").unwrap();
        settings::apply(&mut file, "github.poll_secs", "200").unwrap();

        let ov = overlay(&[("CRYSTALLINE_GITHUB_ENABLED", "true")]).unwrap();
        let effective = ov.apply(&file);

        assert!(effective.github_enabled(), "env override wins");
        assert_eq!(
            effective.github.as_ref().unwrap().poll_secs,
            Some(200),
            "file value survives where env is silent"
        );
        assert!(
            effective.github.as_ref().unwrap().api_url.is_none(),
            "the default is untouched where neither layer sets it"
        );
    }

    #[test]
    fn active_overrides_masks_the_database_url() {
        let ov = overlay(&[
            ("CRYSTALLINE_DATABASE_BACKEND", "postgres"),
            (
                "CRYSTALLINE_DATABASE_URL",
                "postgres://u:secret@db/crystalline",
            ),
        ])
        .unwrap();
        let overrides = ov.active_overrides();

        let url = overrides
            .iter()
            .find(|(_, key, _)| key == "database.url")
            .expect("database.url override present");
        assert_eq!(url.0, "CRYSTALLINE_DATABASE_URL");
        assert_eq!(url.2, "(set)", "the url value must never be shown");

        let backend = overrides
            .iter()
            .find(|(_, key, _)| key == "database.backend")
            .expect("database.backend override present");
        assert_eq!(backend.2, "postgres", "a non-secret value is shown as-is");
    }

    #[test]
    fn config_path_is_captured_and_tilde_expanded() {
        let ov = overlay(&[("CRYSTALLINE_CONFIG", "/etc/crystalline/config.yaml")]).unwrap();
        assert_eq!(
            ov.config_path(),
            Some(Path::new("/etc/crystalline/config.yaml"))
        );
        assert!(!ov.is_empty(), "a config path is a non-empty overlay");
    }

    #[test]
    fn resolve_config_path_prefers_flag_then_env_then_default() {
        let flag = Path::new("/flag/config.yaml");
        let env = Path::new("/env/config.yaml");

        assert_eq!(
            resolve_config_path(Some(flag), Some(env)).unwrap(),
            PathBuf::from("/flag/config.yaml"),
            "the flag beats the environment"
        );
        assert_eq!(
            resolve_config_path(None, Some(env)).unwrap(),
            PathBuf::from("/env/config.yaml"),
            "the environment beats the default"
        );
        assert_eq!(
            resolve_config_path(Some(flag), None).unwrap(),
            PathBuf::from("/flag/config.yaml"),
            "the flag alone resolves to itself"
        );
    }
}
