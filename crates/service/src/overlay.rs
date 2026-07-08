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
//!
//! The overlay also carries env-defined domains: `CRYSTALLINE_DOMAIN_<NAME>`
//! registers a file domain rooted at a path, and an optional
//! `CRYSTALLINE_DOMAIN_<NAME>_ORIGIN=owner/repo[/subpath][@branch]` attaches a
//! GitHub origin to it so a headless node provisions the team domain itself on
//! first contact. These domains are merged into the effective config last (env
//! wins over a file entry of the same name) and are never written back to the
//! file. Two grammar consequences follow from the `_ORIGIN` suffix rule and
//! are load-bearing: a domain whose env fragment would end in `_ORIGIN` cannot
//! be env-defined (that spelling is always read as an origin attachment), and
//! a file domain whose name contains an underscore cannot be env-shadowed
//! (env names map `_` to `-`, so they never collide with an underscore name).
//!
//! `CRYSTALLINE_GITHUB_TOKEN` carries a GitHub token for a headless node: see
//! [`GITHUB_TOKEN_ENV`] and [`EnvOverlay::github_token`]. It is never applied
//! to a `GlobalConfig` (there is no config field for it), never appears in
//! [`EnvOverlay::active_overrides`] beyond a `(set)` placeholder and never
//! prints through this type's `Debug` impl, which redacts it by hand.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crystalline_core::config::{self, DomainEntry, GlobalConfig, OriginConfig};

use crate::origin;
use crate::settings;

/// The variable naming an alternate config file path. Lower priority than a
/// `--config` flag, higher than the default global path. A leading `~` is
/// expanded like every other path Crystalline reads.
pub const CONFIG_PATH_ENV: &str = "CRYSTALLINE_CONFIG";

/// The prefix for an env-defined domain, `CRYSTALLINE_DOMAIN_<NAME>`. A
/// variable with this prefix defines a file domain (or, with the `_ORIGIN`
/// suffix, attaches an origin to one); see [`EnvOverlay::from_vars`] for the
/// full grammar.
pub const DOMAIN_ENV_PREFIX: &str = "CRYSTALLINE_DOMAIN_";

/// The suffix on the variable that attaches an origin to an env-defined
/// domain: `CRYSTALLINE_DOMAIN_<NAME>_ORIGIN`.
const DOMAIN_ORIGIN_SUFFIX: &str = "_ORIGIN";

/// The variable carrying a GitHub token for a headless node:
/// `CRYSTALLINE_GITHUB_TOKEN`. Checked before the keyring and the file store
/// (see `Engine::resolve_token_store`), so a container with this variable set
/// never needs an interactive sign-in. Read-only: a saved token is never
/// consulted while this is set, `connect github` refuses and `crates/remote`
/// never reads it itself (see [`crate::engine::Engine::resolve_token_store`]
/// and the `crystalline_remote::token` module docs).
pub const GITHUB_TOKEN_ENV: &str = "CRYSTALLINE_GITHUB_TOKEN";

/// Variables that live outside the settings registry and are read elsewhere.
/// They are skipped silently rather than warned about, so a legitimate
/// deployment does not get a spurious warning for a variable Crystalline
/// itself documents. [`GITHUB_TOKEN_ENV`] is handled separately in
/// [`EnvOverlay::from_vars`], not through this list, since it becomes real
/// overlay state rather than being ignored.
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

/// One env-defined domain: the variable that defined it and the resulting
/// [`DomainEntry`]. The `var` is kept so a guard message (a refused
/// `domain remove`, an `origin_add` name clash) can name the exact variable an
/// operator has to unset to manage the domain in the config file instead.
#[derive(Debug, Clone)]
pub struct EnvDomain {
    /// The variable that defined the domain, `CRYSTALLINE_DOMAIN_<NAME>`.
    pub var: String,
    /// The file-kind domain entry, carrying its root path and, when a matching
    /// `_ORIGIN` variable was present, its GitHub origin.
    pub entry: DomainEntry,
}

/// The validated environment overlay: the `CRYSTALLINE_*` variables that layer
/// on top of the config file. Parsed once, then applied to a file config to
/// produce the effective config every runtime read uses.
///
/// Deliberately does not derive `Debug`: [`EnvOverlay::github_token`] carries
/// a secret, so the impl below is written by hand to redact it, the same way
/// `crystalline_remote::StoredToken` redacts `access_token`.
#[derive(Clone, Default)]
pub struct EnvOverlay {
    /// Registry key to raw value, one entry per setting variable that was
    /// present and non-empty. Each value was validated at parse time by
    /// running its registry spec's `apply`, so replaying it later cannot fail.
    settings: Vec<(String, String)>,
    /// Env-defined domains keyed by domain name (the mapped, lowercased,
    /// hyphenated form). Merged into the effective domain map last, so an env
    /// domain wins over a file entry of the same name.
    domains: IndexMap<String, EnvDomain>,
    /// The GitHub token from [`GITHUB_TOKEN_ENV`], when set and non-empty.
    /// Never written back to the config file and never the `Default`/`Clone`
    /// derive's business to print: see the manual `Debug` impl below.
    github_token: Option<String>,
    /// The config file path from [`CONFIG_PATH_ENV`], tilde-expanded. `None`
    /// when the variable is absent or empty.
    config_path: Option<PathBuf>,
}

/// Redacts `github_token`: an `EnvOverlay` is long-lived on `Engine` and far
/// more likely to reach a log line or a test failure message via `Debug` than
/// via any deliberate print, so the secret is masked unconditionally rather
/// than trusting every future caller to remember not to print it.
impl std::fmt::Debug for EnvOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvOverlay")
            .field("settings", &self.settings)
            .field("domains", &self.domains)
            .field(
                "github_token",
                &self.github_token.as_ref().map(|_| "<redacted>"),
            )
            .field("config_path", &self.config_path)
            .finish()
    }
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
    ///
    /// Env-defined domains are parsed too. `CRYSTALLINE_DOMAIN_<NAME>=<path>`
    /// registers a file domain: `<NAME>` must be non-empty and match
    /// `[A-Za-z0-9_]+`, and the domain name is `<NAME>` lowercased with `_`
    /// mapped to `-` (so `TEAM_KNOWLEDGE` becomes `team-knowledge`); an empty
    /// or whitespace-only path is fatal, and the path is tilde-expanded.
    /// `CRYSTALLINE_DOMAIN_<NAME>_ORIGIN=owner/repo[/subpath][@branch]`
    /// attaches a GitHub origin to the `<NAME>` domain: every variable ending
    /// in `_ORIGIN` is read as an origin attachment, and one whose base
    /// `CRYSTALLINE_DOMAIN_<NAME>` is not itself defined is fatal. All
    /// `CRYSTALLINE_DOMAIN_*` variables are collected first, so an origin may
    /// appear before its base domain in the iterator without failing.
    ///
    /// [`GITHUB_TOKEN_ENV`], when present and non-empty, is captured as
    /// [`EnvOverlay::github_token`]; an empty value reads as unset, the same
    /// as every setting variable.
    pub fn from_vars<I>(vars: I) -> Result<EnvOverlay, OverlayError>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut settings = Vec::new();
        let mut config_path = None;
        let mut github_token = None;
        // A scratch config the setting values are applied to as they parse, so
        // the combined database check below sees every applied value at once.
        let mut scratch = GlobalConfig::default();
        // Env-domain variables are collected across the whole iterator and
        // resolved afterwards, so an `_ORIGIN` attachment finds its base domain
        // regardless of the order the two variables arrive in.
        let mut domain_vars: Vec<(String, String)> = Vec::new();
        let mut origin_vars: Vec<(String, String)> = Vec::new();

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
            if name == GITHUB_TOKEN_ENV {
                if !value.is_empty() {
                    github_token = Some(value);
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
            if let Some(fragment) = name.strip_prefix(DOMAIN_ENV_PREFIX) {
                // Every `_ORIGIN`-suffixed name is an origin attachment; every
                // other is a domain definition. Both are resolved below. An
                // empty `_ORIGIN` value reads as "no attachment", matching the
                // empty-is-unset convention of every other variable (an empty
                // domain PATH stays an error: the base variable declares a
                // domain, so it has to say where the domain lives).
                if fragment.ends_with(DOMAIN_ORIGIN_SUFFIX) {
                    if !value.is_empty() {
                        origin_vars.push((name, value));
                    }
                } else {
                    domain_vars.push((name, value));
                }
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

        let domains = resolve_env_domains(domain_vars, origin_vars)?;

        Ok(EnvOverlay {
            settings,
            domains,
            github_token,
            config_path,
        })
    }

    /// Parse an overlay from the live process environment. The feature's sole
    /// ambient read: every other entry point takes an injected iterator.
    pub fn from_process_env() -> Result<EnvOverlay, OverlayError> {
        EnvOverlay::from_vars(std::env::vars())
    }

    /// Whether the overlay carries nothing: no setting override, no env
    /// domain, no GitHub token and no config path. A no-op overlay leaves the
    /// file config untouched.
    pub fn is_empty(&self) -> bool {
        self.settings.is_empty()
            && self.domains.is_empty()
            && self.github_token.is_none()
            && self.config_path.is_none()
    }

    /// Apply the overlay to a file config, returning the effective config.
    /// Every value was validated at parse time, so replaying it here cannot
    /// fail; a failure would be a bug, not bad input, hence the panic.
    ///
    /// Domains merge last, after the settings: an env domain overwrites a file
    /// entry of the same name (keeping the file entry's position in the map),
    /// so `CRYSTALLINE_DOMAIN_X` wins over a `config.yaml` domain `x`.
    pub fn apply(&self, file: &GlobalConfig) -> GlobalConfig {
        let mut effective = file.clone();
        for (key, value) in &self.settings {
            if let Err(e) = settings::apply(&mut effective, key, value) {
                panic!(
                    "env overlay value for '{key}' was validated at parse time but failed to apply now: {e}"
                );
            }
        }
        for (name, env_domain) in &self.domains {
            effective
                .domains
                .insert(name.clone(), env_domain.entry.clone());
        }
        effective
    }

    /// Whether an environment variable currently overrides `key`.
    pub fn overrides_key(&self, key: &str) -> bool {
        self.settings.iter().any(|(k, _)| k == key)
    }

    /// The env-defined domain named `name`, if the environment defines one.
    pub fn env_domain(&self, name: &str) -> Option<&EnvDomain> {
        self.domains.get(name)
    }

    /// Every env-defined domain as `(name, definition)`, in the order the
    /// variables were seen.
    pub fn env_domains(&self) -> impl Iterator<Item = (&String, &EnvDomain)> {
        self.domains.iter()
    }

    /// The names of env-defined domains that shadow a file entry of the same
    /// name, so a caller can warn once at daemon startup (rather than on every
    /// `apply`, which runs repeatedly). Empty when no env domain collides with
    /// the file config.
    pub fn shadowed_domains(&self, file: &GlobalConfig) -> Vec<&str> {
        self.domains
            .keys()
            .filter(|name| file.domains.contains_key(name.as_str()))
            .map(String::as_str)
            .collect()
    }

    /// The config file path from [`CONFIG_PATH_ENV`], if set.
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// The GitHub token from [`GITHUB_TOKEN_ENV`], if the environment set
    /// one. `Engine::resolve_token_store` checks this before the keyring and
    /// the file store; nothing else in the process reads
    /// `CRYSTALLINE_GITHUB_TOKEN` directly.
    pub fn github_token(&self) -> Option<&str> {
        self.github_token.as_deref()
    }

    /// Every active override as `(variable, key, display value)`, for surfacing
    /// in `doctor` and the like: first the setting overrides, then the
    /// env-defined domains (keyed `domain.<name>`, their path as the display
    /// value), then the GitHub token, if set (keyed `github.token`). The
    /// `database.url` value and the GitHub token are both rendered as `(set)`
    /// rather than shown, since either may be a credential; a domain path
    /// carries no secret and is shown as-is.
    pub fn active_overrides(&self) -> Vec<(String, String, String)> {
        let mut out: Vec<(String, String, String)> = self
            .settings
            .iter()
            .map(|(key, value)| {
                let display = if key == "database.url" {
                    "(set)".to_string()
                } else {
                    value.clone()
                };
                (env_var_for(key), key.clone(), display)
            })
            .collect();
        for (name, env_domain) in &self.domains {
            let path = env_domain
                .entry
                .file_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            out.push((env_domain.var.clone(), format!("domain.{name}"), path));
        }
        if self.github_token.is_some() {
            out.push((
                GITHUB_TOKEN_ENV.to_string(),
                "github.token".to_string(),
                "(set)".to_string(),
            ));
        }
        out
    }
}

/// Resolves the collected `CRYSTALLINE_DOMAIN_*` variables into the overlay's
/// domain map: every domain-definition variable becomes a file [`DomainEntry`],
/// then every `_ORIGIN` variable attaches a parsed [`OriginConfig`] to its base
/// domain. An origin with no matching base domain, an invalid name, an empty
/// path or a malformed origin value is fatal, each error naming the offending
/// variable.
fn resolve_env_domains(
    domain_vars: Vec<(String, String)>,
    origin_vars: Vec<(String, String)>,
) -> Result<IndexMap<String, EnvDomain>, OverlayError> {
    let mut domains: IndexMap<String, EnvDomain> = IndexMap::new();
    // The `<NAME>` fragment (for example `TEAM_KNOWLEDGE`) to the mapped domain
    // name, so an `_ORIGIN` attachment finds the domain its base defines.
    let mut fragment_to_name: HashMap<String, String> = HashMap::new();

    for (var, value) in domain_vars {
        // The fragment after `CRYSTALLINE_DOMAIN_` names the domain.
        let fragment = var
            .strip_prefix(DOMAIN_ENV_PREFIX)
            .expect("collected with the domain prefix");
        if fragment.is_empty()
            || !fragment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(OverlayError(format!(
                "invalid environment variable {var}: the domain name must be non-empty and use only letters, digits and underscores"
            )));
        }
        if value.trim().is_empty() {
            return Err(OverlayError(format!(
                "invalid environment variable {var}: the domain path must not be empty"
            )));
        }
        // The mapped domain name: lowercased with underscores turned to
        // hyphens, so it never collides with a slug Crystalline itself
        // generates (slugs never carry underscores).
        let name = fragment.to_ascii_lowercase().replace('_', "-");
        let entry = DomainEntry::file(config::expand_tilde(&value));
        fragment_to_name.insert(fragment.to_string(), name.clone());
        domains.insert(
            name,
            EnvDomain {
                var: var.clone(),
                entry,
            },
        );
    }

    for (var, value) in origin_vars {
        let fragment = var
            .strip_prefix(DOMAIN_ENV_PREFIX)
            .expect("collected with the domain prefix");
        // The base fragment is the name with the `_ORIGIN` suffix removed.
        let base = fragment
            .strip_suffix(DOMAIN_ORIGIN_SUFFIX)
            .expect("collected by its origin suffix");
        let Some(name) = fragment_to_name.get(base) else {
            return Err(OverlayError(format!(
                "environment variable {var} has no matching {DOMAIN_ENV_PREFIX}{base}"
            )));
        };
        let origin = parse_env_origin(&value)
            .map_err(|e| OverlayError(format!("invalid environment variable {var}: {e}")))?;
        domains
            .get_mut(name)
            .expect("name recorded when the base domain was resolved")
            .entry
            .origin = Some(origin);
    }

    Ok(domains)
}

/// Parses a `CRYSTALLINE_DOMAIN_<NAME>_ORIGIN` value,
/// `owner/repo[/subpath][@branch]`, into an [`OriginConfig`]. An optional
/// `@branch` is split off the last `@` (an empty branch is an error), then the
/// `owner/repo[/subpath]` remainder is parsed the same way the CLI `--origin`
/// flag is. `poll_secs` is left unset: an env-origin domain defers to the
/// global `github.poll_secs`.
fn parse_env_origin(value: &str) -> Result<OriginConfig, String> {
    let (repo_spec, branch) = match value.rsplit_once('@') {
        Some((_, "")) => {
            return Err("the branch after '@' must not be empty".to_string());
        }
        Some((repo_spec, branch)) => (repo_spec, Some(branch.to_string())),
        None => (value, None),
    };
    let (repo, subpath) = origin::parse_origin_spec(repo_spec)?;
    Ok(OriginConfig {
        repo,
        path: subpath,
        branch,
        poll_secs: None,
    })
}

/// Whether `name` is a reserved `CRYSTALLINE_*` variable that is skipped
/// silently rather than warned about: read outside the registry, not handled
/// here. Env-domain variables and [`GITHUB_TOKEN_ENV`] are not reserved: both
/// are parsed into real `EnvOverlay` state before this check ever runs.
fn is_reserved(name: &str) -> bool {
    RESERVED_VARS.contains(&name)
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
            ("CRYSTALLINE_DOMAINS_ROOT", "/srv/knowledge"),
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
            "domains_root",
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
        assert_eq!(
            effective.domains_root().display().to_string(),
            "/srv/knowledge"
        );
        assert!(effective.github_enabled());
        assert!(effective.read_only());
        assert_eq!(effective.database().backend, DatabaseBackend::Postgres);
        assert_eq!(
            effective.database().url.as_deref(),
            Some("postgres://u:p@db/crystalline")
        );
    }

    #[test]
    fn domains_root_is_a_setting_not_an_env_domain() {
        // CRYSTALLINE_DOMAINS_ROOT must be read as the domains_root setting, and
        // never mistaken for a CRYSTALLINE_DOMAIN_<NAME> env-defined domain.
        let ov = overlay(&[("CRYSTALLINE_DOMAINS_ROOT", "/srv/knowledge")]).unwrap();
        assert!(ov.overrides_key("domains_root"));
        assert_eq!(ov.env_domains().count(), 0);
        let effective = ov.apply(&GlobalConfig::default());
        assert_eq!(
            effective.domains_root().display().to_string(),
            "/srv/knowledge"
        );
        assert!(effective.domains.is_empty());
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

    // --- env-defined domains -------------------------------------------------

    #[test]
    fn a_domain_name_is_lowercased_with_underscores_mapped_to_hyphens() {
        let ov = overlay(&[
            ("CRYSTALLINE_DOMAIN_TEAM_KNOWLEDGE", "/k/team"),
            ("CRYSTALLINE_DOMAIN_BRAND", "/k/brand"),
            ("CRYSTALLINE_DOMAIN_TEAM_A", "/k/a"),
            ("CRYSTALLINE_DOMAIN_TEAM123", "/k/123"),
        ])
        .unwrap();

        assert!(ov.env_domain("team-knowledge").is_some());
        assert!(ov.env_domain("brand").is_some());
        assert!(ov.env_domain("team-a").is_some());
        assert!(ov.env_domain("team123").is_some());

        let team = ov.env_domain("team-knowledge").unwrap();
        assert_eq!(team.var, "CRYSTALLINE_DOMAIN_TEAM_KNOWLEDGE");
        assert_eq!(
            team.entry.file_path().as_deref(),
            Some(Path::new("/k/team"))
        );
        assert!(team.entry.origin.is_none());
        assert!(!ov.is_empty());
    }

    #[test]
    fn a_domain_name_with_an_illegal_character_is_rejected_naming_the_variable() {
        let err = overlay(&[("CRYSTALLINE_DOMAIN_TEAM-X", "/k/team")]).unwrap_err();
        assert!(
            err.to_string().contains("CRYSTALLINE_DOMAIN_TEAM-X"),
            "{err}"
        );
    }

    #[test]
    fn an_empty_domain_path_is_rejected_naming_the_variable() {
        let err = overlay(&[("CRYSTALLINE_DOMAIN_TEAM", "   ")]).unwrap_err();
        assert!(err.to_string().contains("CRYSTALLINE_DOMAIN_TEAM"), "{err}");
        assert!(err.to_string().contains("path"), "{err}");
    }

    #[test]
    fn an_origin_attaches_a_repo_with_no_subpath_or_branch() {
        let ov = overlay(&[
            ("CRYSTALLINE_DOMAIN_TEAM", "/k/team"),
            ("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "acme/brand-knowledge"),
        ])
        .unwrap();
        let origin = ov
            .env_domain("team")
            .unwrap()
            .entry
            .origin
            .as_ref()
            .unwrap();
        assert_eq!(origin.repo, "acme/brand-knowledge");
        assert_eq!(origin.path, None);
        assert_eq!(origin.branch, None);
        assert_eq!(origin.branch(), "main");
        assert_eq!(origin.poll_secs, None);
    }

    #[test]
    fn an_empty_origin_value_reads_as_no_attachment() {
        // `VAR=` means unset for every other variable; a blanked `_ORIGIN`
        // therefore leaves a plain local domain instead of failing startup,
        // even when no base variable exists for it at all.
        let ov = overlay(&[
            ("CRYSTALLINE_DOMAIN_TEAM", "/k/team"),
            ("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", ""),
            ("CRYSTALLINE_DOMAIN_LONER_ORIGIN", ""),
        ])
        .unwrap();
        assert!(ov.env_domain("team").unwrap().entry.origin.is_none());
        assert!(ov.env_domain("loner").is_none());
    }

    #[test]
    fn an_origin_reads_a_subpath() {
        let ov = overlay(&[
            ("CRYSTALLINE_DOMAIN_TEAM", "/k/team"),
            (
                "CRYSTALLINE_DOMAIN_TEAM_ORIGIN",
                "acme/monorepo/teams/brand",
            ),
        ])
        .unwrap();
        let origin = ov
            .env_domain("team")
            .unwrap()
            .entry
            .origin
            .as_ref()
            .unwrap();
        assert_eq!(origin.repo, "acme/monorepo");
        assert_eq!(origin.path.as_deref(), Some("teams/brand"));
        assert_eq!(origin.branch, None);
    }

    #[test]
    fn an_origin_reads_a_branch_off_the_last_at() {
        let ov = overlay(&[
            ("CRYSTALLINE_DOMAIN_TEAM", "/k/team"),
            ("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "acme/brand@release"),
        ])
        .unwrap();
        let origin = ov
            .env_domain("team")
            .unwrap()
            .entry
            .origin
            .as_ref()
            .unwrap();
        assert_eq!(origin.repo, "acme/brand");
        assert_eq!(origin.path, None);
        assert_eq!(origin.branch.as_deref(), Some("release"));
    }

    #[test]
    fn an_origin_reads_a_subpath_and_a_branch_together() {
        let ov = overlay(&[
            ("CRYSTALLINE_DOMAIN_TEAM", "/k/team"),
            (
                "CRYSTALLINE_DOMAIN_TEAM_ORIGIN",
                "acme/monorepo/teams/brand@release",
            ),
        ])
        .unwrap();
        let origin = ov
            .env_domain("team")
            .unwrap()
            .entry
            .origin
            .as_ref()
            .unwrap();
        assert_eq!(origin.repo, "acme/monorepo");
        assert_eq!(origin.path.as_deref(), Some("teams/brand"));
        assert_eq!(origin.branch.as_deref(), Some("release"));
    }

    #[test]
    fn an_origin_with_an_empty_branch_is_rejected_naming_the_variable() {
        let err = overlay(&[
            ("CRYSTALLINE_DOMAIN_TEAM", "/k/team"),
            ("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "acme/brand@"),
        ])
        .unwrap_err();
        assert!(
            err.to_string().contains("CRYSTALLINE_DOMAIN_TEAM_ORIGIN"),
            "{err}"
        );
        assert!(err.to_string().contains("branch"), "{err}");
    }

    #[test]
    fn an_orphan_origin_with_no_base_domain_is_rejected() {
        let err = overlay(&[("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "acme/brand")]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("CRYSTALLINE_DOMAIN_TEAM_ORIGIN"), "{msg}");
        assert!(msg.contains("CRYSTALLINE_DOMAIN_TEAM"), "{msg}");
    }

    #[test]
    fn an_origin_may_precede_its_base_domain_in_the_iterator() {
        // The two variables are collected before either is resolved, so order
        // in the environment does not matter.
        let ov = overlay(&[
            ("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "acme/brand"),
            ("CRYSTALLINE_DOMAIN_TEAM", "/k/team"),
        ])
        .unwrap();
        assert_eq!(
            ov.env_domain("team")
                .unwrap()
                .entry
                .origin
                .as_ref()
                .unwrap()
                .repo,
            "acme/brand"
        );
    }

    #[test]
    fn an_env_domain_beats_a_file_domain_of_the_same_name() {
        let mut file = GlobalConfig::default();
        file.domains
            .insert("team".to_string(), DomainEntry::file("/file/team"));

        let ov = overlay(&[("CRYSTALLINE_DOMAIN_TEAM", "/env/team")]).unwrap();
        let effective = ov.apply(&file);

        assert_eq!(
            effective
                .domains
                .get("team")
                .unwrap()
                .file_path()
                .as_deref(),
            Some(Path::new("/env/team")),
            "the env domain wins over the file entry"
        );
        assert_eq!(ov.shadowed_domains(&file), vec!["team"]);
    }

    #[test]
    fn shadowed_domains_is_empty_when_no_env_domain_collides() {
        let mut file = GlobalConfig::default();
        file.domains
            .insert("brand".to_string(), DomainEntry::file("/file/brand"));
        let ov = overlay(&[("CRYSTALLINE_DOMAIN_TEAM", "/env/team")]).unwrap();
        assert!(ov.shadowed_domains(&file).is_empty());
    }

    #[test]
    fn env_domains_lists_every_env_domain() {
        let ov = overlay(&[
            ("CRYSTALLINE_DOMAIN_TEAM", "/k/team"),
            ("CRYSTALLINE_DOMAIN_BRAND", "/k/brand"),
        ])
        .unwrap();
        let names: Vec<&String> = ov.env_domains().map(|(name, _)| name).collect();
        assert!(names.iter().any(|n| n.as_str() == "team"));
        assert!(names.iter().any(|n| n.as_str() == "brand"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn active_overrides_includes_env_domains() {
        let ov = overlay(&[
            ("CRYSTALLINE_GITHUB_ENABLED", "true"),
            ("CRYSTALLINE_DOMAIN_TEAM", "/k/team"),
        ])
        .unwrap();
        let overrides = ov.active_overrides();

        let domain = overrides
            .iter()
            .find(|(_, key, _)| key == "domain.team")
            .expect("the env domain is listed among the active overrides");
        assert_eq!(domain.0, "CRYSTALLINE_DOMAIN_TEAM");
        assert_eq!(domain.2, "/k/team");

        assert!(
            overrides.iter().any(|(_, key, _)| key == "github.enabled"),
            "setting overrides are still listed alongside domains"
        );
    }

    // --- the GitHub token -----------------------------------------------------

    #[test]
    fn the_github_token_is_picked_up() {
        let ov = overlay(&[("CRYSTALLINE_GITHUB_TOKEN", "gho_SECRETSECRET")]).unwrap();
        assert_eq!(ov.github_token(), Some("gho_SECRETSECRET"));
        assert!(!ov.is_empty());
    }

    #[test]
    fn an_empty_github_token_is_ignored() {
        let ov = overlay(&[("CRYSTALLINE_GITHUB_TOKEN", "")]).unwrap();
        assert_eq!(ov.github_token(), None);
        assert!(ov.is_empty());
    }

    #[test]
    fn no_github_token_variable_leaves_the_accessor_none() {
        let ov = overlay(&[]).unwrap();
        assert_eq!(ov.github_token(), None);
    }

    #[test]
    fn active_overrides_masks_the_github_token() {
        let ov = overlay(&[("CRYSTALLINE_GITHUB_TOKEN", "gho_SECRETSECRET")]).unwrap();
        let overrides = ov.active_overrides();
        let token = overrides
            .iter()
            .find(|(_, key, _)| key == "github.token")
            .expect("the token is listed among the active overrides");
        assert_eq!(token.0, "CRYSTALLINE_GITHUB_TOKEN");
        assert_eq!(token.2, "(set)", "the token value must never be shown");
        for (_, _, value) in &overrides {
            assert!(!value.contains("SECRET"), "{value}");
        }
    }

    #[test]
    fn active_overrides_omits_the_github_token_entry_when_unset() {
        let ov = overlay(&[("CRYSTALLINE_GITHUB_ENABLED", "true")]).unwrap();
        assert!(
            !ov.active_overrides()
                .iter()
                .any(|(_, key, _)| key == "github.token")
        );
    }

    #[test]
    fn debug_output_of_the_overlay_never_shows_the_github_token_or_a_prefix_of_it() {
        let ov = overlay(&[
            ("CRYSTALLINE_GITHUB_ENABLED", "true"),
            ("CRYSTALLINE_GITHUB_TOKEN", "gho_SECRETSECRET"),
        ])
        .unwrap();
        let debugged = format!("{ov:?}");
        assert!(!debugged.contains("SECRET"), "{debugged}");
        assert!(debugged.contains("<redacted>"), "{debugged}");
        // Everything else still prints, so the redaction is targeted rather
        // than blanking the whole struct.
        assert!(debugged.contains("github.enabled"), "{debugged}");
    }
}
