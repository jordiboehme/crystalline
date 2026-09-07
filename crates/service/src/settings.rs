//! The settings registry: the single source of truth for which configuration
//! keys an agent or a user may change through `configure`, their types and
//! bounds and how each one maps onto [`GlobalConfig`].
//!
//! Nothing outside this module may match on a setting key: the ctl `configure`
//! command, the CLI `config` verbs and (in a later task) the MCP `configure`
//! tool all read [`registry`] for the key list and documentation and call
//! [`apply`], [`unset`] and [`snapshot`] to act on one.

use std::path::PathBuf;

use crystalline_core::config::{
    AuthConfig, CaptureConfig, DatabaseBackend, DatabaseConfig, GitHubConfig, GlobalConfig,
    HttpSetting, IdentityConfig, IndexConfig, OidcConfig, ResponseFormat, SearchConfig,
    ServiceConfig, ShareIdentityMode, SkillsConfig, SkillsServe,
};
use crystalline_index::{DEFAULT_RETIRED_WEIGHT, DEFAULT_SALIENCE_WEIGHT};
use crystalline_remote::{MAX_IDENTITY_NAME_BYTES, valid_identity_name};

use crate::overlay::EnvOverlay;
use crate::rest::Role;

/// What a credential-carrying setting renders as instead of its value:
/// whether one is configured, and nothing more. The core marker, re-exported
/// so [`crate::overlay::EnvOverlay::active_overrides`] and the config's own
/// `Debug` render a secret the same way wherever it is displayed.
pub use crystalline_core::config::SECRET_DISPLAY;

/// Whether a settings key carries a credential, so nothing may render its
/// value. Read off the registry, where [`SettingSpec::secret`] is declared
/// beside the key it belongs to: one flag makes a value invisible in
/// `config show`, the `configure` tool, `crystalline doctor` and the
/// overlay's `Debug` alike, and a key the registry does not know is not a
/// secret.
pub fn is_secret_key(key: &str) -> bool {
    registry().iter().any(|s| s.key == key && s.secret)
}

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
    /// A floating-point value.
    F64,
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
    /// Whether the value is a credential. A secret setting is stored like any
    /// other, but every display of it goes through [`SettingSpec::display`]
    /// and renders [`SECRET_DISPLAY`] instead of the value: an operator learns
    /// whether one is configured, and nothing more. Declared here, beside the
    /// key, so masking is one flag rather than a hand-written `effective`
    /// per key and a matching list somewhere else.
    pub secret: bool,
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
    /// The value to show for this setting and whether it is a default: the
    /// effective value, masked to [`SECRET_DISPLAY`] when the setting is a
    /// secret and something is configured. The only path from a config to a
    /// rendered value, which is what makes [`SettingSpec::secret`] sufficient.
    pub fn display(&self, config: &GlobalConfig) -> (String, bool) {
        let (value, is_default) = (self.effective)(config);
        if self.secret && !value.is_empty() {
            (SECRET_DISPLAY.to_string(), is_default)
        } else {
            (value, is_default)
        }
    }

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
            key: "domains_root",
            doc: "The default root folder new file domains are created under (default ~/Documents/Crystalline)",
            kind: SettingKind::String,
            startup_effective: false,
            secret: false,
            apply: set_domains_root,
            clear: clear_domains_root,
            effective: domains_root_effective,
        },
        SettingSpec {
            key: "github.enabled",
            doc: "Turn GitHub team collaboration on or off",
            kind: SettingKind::Bool,
            startup_effective: false,
            secret: false,
            apply: set_enabled,
            clear: clear_enabled,
            effective: enabled_effective,
        },
        SettingSpec {
            key: "github.stacks",
            doc: "Stack a new proposal on the open one when sharing again, where the forge serves stacked pull requests (default true); false keeps a single proposal per domain, updated in place",
            kind: SettingKind::Bool,
            startup_effective: false,
            secret: false,
            apply: set_stacks,
            clear: clear_stacks,
            effective: stacks_effective,
        },
        SettingSpec {
            key: "github.share_identity",
            doc: "Whose GitHub identity shares run under: instance (default) uses the one connected token for everything; personal requires each person to connect their own GitHub identity for sharing, while pulling stays on the instance token",
            kind: SettingKind::String,
            startup_effective: false,
            secret: false,
            apply: set_share_identity,
            clear: clear_share_identity,
            effective: share_identity_effective,
        },
        SettingSpec {
            key: "github.agent_identity",
            doc: "The crystalline account whose connected GitHub identity agent shares over HTTP MCP run under when share_identity is personal; unset means those shares are refused",
            kind: SettingKind::String,
            startup_effective: false,
            secret: false,
            apply: set_agent_identity,
            clear: clear_agent_identity,
            effective: agent_identity_effective,
        },
        SettingSpec {
            key: "github.poll_secs",
            doc: "How often the daemon polls GitHub for changes, in seconds (minimum 60)",
            kind: SettingKind::U64,
            startup_effective: false,
            secret: false,
            apply: set_poll_secs,
            clear: clear_poll_secs,
            effective: poll_secs_effective,
        },
        SettingSpec {
            key: "github.api_url",
            doc: "The GitHub API base url, for a GitHub Enterprise Server instance",
            kind: SettingKind::String,
            startup_effective: false,
            secret: false,
            apply: set_api_url,
            clear: clear_api_url,
            effective: api_url_effective,
        },
        SettingSpec {
            key: "github.oauth_client_id",
            doc: "A self-hosted OAuth App client id, overriding the embedded default",
            kind: SettingKind::String,
            startup_effective: false,
            secret: false,
            apply: set_oauth_client_id,
            clear: clear_oauth_client_id,
            effective: oauth_client_id_effective,
        },
        SettingSpec {
            key: "service.read_only",
            doc: "Serve knowledge read-only, hiding every tool that writes (applies at the next daemon start)",
            kind: SettingKind::Bool,
            startup_effective: true,
            secret: false,
            apply: set_read_only,
            clear: clear_read_only,
            effective: read_only_effective,
        },
        SettingSpec {
            key: "service.http",
            doc: "The HTTP endpoint: on at 127.0.0.1:7411 by default; false turns it off, true spells the default, or bind a host:port address; the serve --http flag wins when given (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_http,
            clear: clear_http,
            effective: http_effective,
        },
        SettingSpec {
            key: "service.ui",
            doc: "Serve the embedded Fluid web UI on the HTTP endpoint (default true); false serves the API and MCP only, the shape a separate Fluid deployment fronts. Read once when the HTTP surface starts",
            kind: SettingKind::Bool,
            startup_effective: true,
            secret: false,
            apply: set_ui,
            clear: clear_ui,
            effective: ui_effective,
        },
        SettingSpec {
            key: "service.api",
            doc: "Serve the JSON API under /api/v1 on the HTTP endpoint (default true); false also disables the web UI, leaving MCP and /health only. Read once when the HTTP surface starts",
            kind: SettingKind::Bool,
            startup_effective: true,
            secret: false,
            apply: set_api,
            clear: clear_api,
            effective: api_effective,
        },
        SettingSpec {
            key: "service.allowed_hosts",
            doc: "Comma-separated Host header values the HTTP transport accepts (DNS-rebinding guard); loopback is always allowed and a single * allows any Host; the serve --allowed-host flag wins when given (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_allowed_hosts,
            clear: clear_allowed_hosts,
            effective: allowed_hosts_effective,
        },
        SettingSpec {
            key: "service.response_format",
            doc: "How list-shaped MCP tool results are encoded: toon (token-efficient, default) or json",
            kind: SettingKind::String,
            startup_effective: false,
            secret: false,
            apply: set_response_format,
            clear: clear_response_format,
            effective: response_format_effective,
        },
        SettingSpec {
            key: "skills.serve",
            doc: "Serve the shipped agent skills over MCP: the skills tool, skill:// resources and the onboarding and connector prompts. auto (default) serves them to every client except a stdio session spawned by a harness this machine's install receipt already onboarded with session hooks, which has the skills as files already; true always serves them, false never does. Applies at the next daemon start",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_skills_serve,
            clear: clear_skills_serve,
            effective: skills_serve_effective,
        },
        SettingSpec {
            key: "database.backend",
            doc: "Which storage backend serves the derived index, turso or postgres (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_database_backend,
            clear: clear_database_backend,
            effective: database_backend_effective,
        },
        SettingSpec {
            key: "database.url",
            doc: "The Postgres connection URL (or a file-path override for the embedded backend); it may carry a password, so it is always treated as a secret: whatever it holds, a file path included, is only ever shown as (set) and never echoed back (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: true,
            apply: set_database_url,
            clear: clear_database_url,
            effective: database_url_effective,
        },
        SettingSpec {
            key: "search.salience_weight",
            doc: "How strongly a salient engram is lifted in hybrid ranking, 0.0 to 1.0 (default 0.15); a soft prior that reorders within a relevance band and never filters",
            kind: SettingKind::F64,
            startup_effective: false,
            secret: false,
            apply: set_salience_weight,
            clear: clear_salience_weight,
            effective: salience_weight_effective,
        },
        SettingSpec {
            key: "search.retired_weight",
            doc: "The ranking multiplier for engrams whose status is deprecated, superseded, archived or legacy, 0.0 to 1.0 (default 0.6, 1.0 disables); a soft fade that reorders results and never filters",
            kind: SettingKind::F64,
            startup_effective: false,
            secret: false,
            apply: set_retired_weight,
            clear: clear_retired_weight,
            effective: retired_weight_effective,
        },
        SettingSpec {
            key: "index.files",
            doc: "Keep a generated index.md in every folder of a file domain, so the knowledge navigates statically without Crystalline (default true)",
            kind: SettingKind::Bool,
            startup_effective: false,
            secret: false,
            apply: set_index_files,
            clear: clear_index_files,
            effective: index_files_effective,
        },
        SettingSpec {
            key: "capture.similar",
            doc: "Attach the nearest existing engrams to every write_engram and content edit_engram receipt as a similar list with guidance to merge, supersede, link or ignore them (default true); false switches the advisory off everywhere, MCP and Fluid alike",
            kind: SettingKind::Bool,
            startup_effective: false,
            secret: false,
            apply: set_capture_similar,
            clear: clear_capture_similar,
            effective: capture_similar_effective,
        },
        SettingSpec {
            key: "identity.actor",
            doc: "Who is recorded as the writer of an engram (generated.by), for example team-bot/1.0 or human:jordi; unset means the connected client is used",
            kind: SettingKind::String,
            startup_effective: false,
            secret: false,
            apply: set_identity_actor,
            clear: clear_identity_actor,
            effective: identity_actor_effective,
        },
        SettingSpec {
            key: "auth.trusted_header",
            doc: "The request header a trusted reverse proxy sets to name the authenticated user, for example X-Forwarded-User; unset means no header is believed (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_trusted_header,
            clear: clear_trusted_header,
            effective: trusted_header_effective,
        },
        SettingSpec {
            key: "auth.anonymous",
            doc: "Serve requests that carry no identity at all (default false) (applies at the next daemon start)",
            kind: SettingKind::Bool,
            startup_effective: true,
            secret: false,
            apply: set_anonymous,
            clear: clear_anonymous,
            effective: anonymous_effective,
        },
        SettingSpec {
            key: "auth.mcp",
            doc: "Require every MCP connection over HTTP to authenticate with a personal MCP token (issue one in Fluid under profile > Agent access); off means the legacy open HTTP tier (applies at the next daemon start)",
            kind: SettingKind::Bool,
            startup_effective: true,
            secret: false,
            apply: set_mcp,
            clear: clear_mcp,
            effective: mcp_effective,
        },
        SettingSpec {
            key: "auth.oauth",
            doc: "Serve OAuth for MCP clients: the well-known metadata, dynamic client registration, authorization with a consent page and a token endpoint, so a hosted client such as Claude.ai connects without a pasted token; requires auth.mcp, since the tokens it issues are checked at that gate (applies at the next daemon start)",
            kind: SettingKind::Bool,
            startup_effective: true,
            secret: false,
            apply: set_oauth,
            clear: clear_oauth,
            effective: oauth_effective,
        },
        SettingSpec {
            key: "auth.max_users",
            doc: "How many accounts trusted-header provisioning may mint in total (default 100); the crystalline users CLI is never capped (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_max_users,
            clear: clear_max_users,
            effective: max_users_effective,
        },
        SettingSpec {
            key: "auth.proxy_headers",
            doc: "Trust the reverse proxy's Remote-User, Remote-Name, Remote-Email and Remote-Groups headers to name the signed-in user - ONLY safe when crystalline is unreachable except through that proxy and the proxy strips client-supplied copies of those headers (default false); an account is provisioned on first sight at the auth.oidc.default_role role, viewer when that is unset (applies at the next daemon start)",
            kind: SettingKind::Bool,
            startup_effective: true,
            secret: false,
            apply: set_proxy_headers,
            clear: clear_proxy_headers,
            effective: proxy_headers_effective,
        },
        SettingSpec {
            key: "auth.oidc.issuer",
            doc: "The single sign-on provider's issuer url, the one discovery appends /.well-known/openid-configuration to, for example https://login.microsoftonline.com/<your-tenant-id>/v2.0; unset means SSO is off and only the local accounts sign in (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_oidc_issuer,
            clear: clear_oidc_issuer,
            effective: oidc_issuer_effective,
        },
        SettingSpec {
            key: "auth.oidc.client_id",
            doc: "The client id (application id) the single sign-on provider issued for this Crystalline instance (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_oidc_client_id,
            clear: clear_oidc_client_id,
            effective: oidc_client_id_effective,
        },
        SettingSpec {
            key: "auth.oidc.client_secret",
            doc: "The client secret that goes with auth.oidc.client_id; a credential, so it is only ever shown as (set) and never echoed back - prefer supplying it through CRYSTALLINE_AUTH_OIDC_CLIENT_SECRET, which keeps it out of the config file and out of the shell history a value typed at config set lands in (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: true,
            apply: set_oidc_client_secret,
            clear: clear_oidc_client_secret,
            effective: oidc_client_secret_effective,
        },
        SettingSpec {
            key: "auth.oidc.name",
            doc: "The single sign-on provider's display name, the label on the sign-in button; unset means the generic wording (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_oidc_name,
            clear: clear_oidc_name,
            effective: oidc_name_effective,
        },
        SettingSpec {
            key: "auth.oidc.scopes",
            doc: "The scopes requested at authorization, space separated; unset means the standard set (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_oidc_scopes,
            clear: clear_oidc_scopes,
            effective: oidc_scopes_effective,
        },
        SettingSpec {
            key: "auth.oidc.default_role",
            doc: "The role an account provisioned through single sign-on is created at: viewer, editor or admin; unset means viewer, the least privileged one (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_oidc_default_role,
            clear: clear_oidc_default_role,
            effective: oidc_default_role_effective,
        },
        SettingSpec {
            key: "auth.oidc.redirect_uri",
            doc: "The address the single sign-on provider sends the browser back to, used verbatim instead of the one derived from a request's Host and forwarded scheme; an absolute https url (http only on loopback) ending in /api/v1/auth/oidc/callback, registered with the provider in exactly that spelling - set it where a proxy rewrites the Host, unset it to derive the address per request (applies at the next daemon start)",
            kind: SettingKind::String,
            startup_effective: true,
            secret: false,
            apply: set_oidc_redirect_uri,
            clear: clear_oidc_redirect_uri,
            effective: oidc_redirect_uri_effective,
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
            let (value, _) = spec.display(&effective);
            let source = if overlay.overrides_key(spec.key) {
                SettingSource::Env
            } else if spec.display(file).1 {
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

/// Drop the `skills` block entirely once every field in it has been cleared,
/// so an unset config round-trips to exactly the pre-feature shape (no empty
/// `skills: {}` line).
fn drop_skills_if_empty(config: &mut GlobalConfig) {
    if config.skills.as_ref() == Some(&SkillsConfig::default()) {
        config.skills = None;
    }
}

/// Drop the `search` block entirely once every field in it has been cleared,
/// so an unset config round-trips to exactly the pre-feature shape (no empty
/// `search: {}` line).
fn drop_search_if_empty(config: &mut GlobalConfig) {
    if config.search.as_ref() == Some(&SearchConfig::default()) {
        config.search = None;
    }
}

/// Drop the `index` block entirely once every field in it has been cleared,
/// so an unset config round-trips to exactly the pre-feature shape (no empty
/// `index: {}` line).
fn drop_index_if_empty(config: &mut GlobalConfig) {
    if config.index.as_ref() == Some(&IndexConfig::default()) {
        config.index = None;
    }
}

/// Drop the `capture` block once every field in it has been cleared, so an
/// unset config round-trips to exactly the pre-feature shape (no empty
/// `capture: {}` line).
fn drop_capture_if_empty(config: &mut GlobalConfig) {
    if config.capture.as_ref() == Some(&CaptureConfig::default()) {
        config.capture = None;
    }
}

/// Drop the `identity` block entirely once every field in it has been cleared,
/// so an unset config round-trips to exactly the pre-feature shape (no empty
/// `identity: {}` line).
fn drop_identity_if_empty(config: &mut GlobalConfig) {
    if config.identity.as_ref() == Some(&IdentityConfig::default()) {
        config.identity = None;
    }
}

/// Drop the `auth` block entirely once every field in it has been cleared, so
/// an unset config round-trips to exactly the pre-feature shape (no empty
/// `auth: {}` line).
fn drop_auth_if_empty(config: &mut GlobalConfig) {
    if config.auth.as_ref() == Some(&AuthConfig::default()) {
        config.auth = None;
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

// --- github.stacks ------------------------------------------------------------

fn set_stacks(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: bool = value.parse().map_err(|_| {
        SettingsError(format!(
            "github.stacks must be true or false, got '{value}'"
        ))
    })?;
    config
        .github
        .get_or_insert_with(GitHubConfig::default)
        .stacks = Some(parsed);
    Ok(())
}

fn clear_stacks(config: &mut GlobalConfig) {
    if let Some(g) = config.github.as_mut() {
        g.stacks = None;
    }
    drop_github_if_empty(config);
}

/// Unlike every other GitHub toggle this one defaults ON, so the effective
/// value an unset key shows is `true`.
fn stacks_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.github.as_ref().and_then(|g| g.stacks).is_none();
    (config.github_stacks().to_string(), is_default)
}

// --- github.share_identity ----------------------------------------------------

/// The strict half of the share-identity pair. Only the two words this
/// registry knows are ever stored, so
/// [`GlobalConfig::github_share_identity`] - which tolerates anything else by
/// reading it as `instance`, because a config file is hand-editable - always
/// reads back exactly what was set here.
fn set_share_identity(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let mode = match value.trim() {
        "instance" => ShareIdentityMode::Instance,
        "personal" => ShareIdentityMode::Personal,
        _ => {
            return Err(SettingsError(format!(
                "github.share_identity must be instance or personal, got '{value}'"
            )));
        }
    };
    config
        .github
        .get_or_insert_with(GitHubConfig::default)
        .share_identity = Some(mode.as_str().to_string());
    Ok(())
}

fn clear_share_identity(config: &mut GlobalConfig) {
    if let Some(g) = config.github.as_mut() {
        g.share_identity = None;
    }
    drop_github_if_empty(config);
}

fn share_identity_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config
        .github
        .as_ref()
        .and_then(|g| g.share_identity.as_deref())
        .is_none();
    (
        config.github_share_identity().as_str().to_string(),
        is_default,
    )
}

// --- github.agent_identity ----------------------------------------------------

/// The value names a Crystalline account whose credential the token store
/// addresses by name, so it is held to that store's own predicate -
/// [`valid_identity_name`], the allowlist `[a-z0-9._-]` up to
/// [`MAX_IDENTITY_NAME_BYTES`] - rather than to "anything without whitespace",
/// and by CALLING it rather than mirroring it: a mirror would drift, and the
/// drift would show up as a setting that saves and then cannot be resolved. An
/// empty value clears the setting instead of storing a name nothing can
/// resolve: the REST admin API sets without an unset verb, so the empty string
/// has to be the way back there (`configure` also carries Unset;
/// `set_allowed_hosts` is the same trade).
fn set_agent_identity(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        clear_agent_identity(config);
        return Ok(());
    }
    if !valid_identity_name(trimmed) {
        return Err(SettingsError(format!(
            "github.agent_identity must be a crystalline account name of at most {MAX_IDENTITY_NAME_BYTES} bytes, drawn from lowercase letters, digits, '.', '_' and '-'"
        )));
    }
    config
        .github
        .get_or_insert_with(GitHubConfig::default)
        .agent_identity = Some(trimmed.to_string());
    Ok(())
}

fn clear_agent_identity(config: &mut GlobalConfig) {
    if let Some(g) = config.github.as_mut() {
        g.agent_identity = None;
    }
    drop_github_if_empty(config);
}

fn agent_identity_effective(config: &GlobalConfig) -> (String, bool) {
    match config.github_agent_identity() {
        Some(name) => (name.to_string(), false),
        None => (String::new(), true),
    }
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

/// The effective value is what the daemon will actually do, so an unset setting
/// reports the loopback address the endpoint now opens by default rather than
/// the old `false`. It is still the default (nobody set it), which is what the
/// second element says. `true` reports that same address for the same reason:
/// it is a spelling of the default bind, not a different one, and reporting the
/// word back would leave a reader guessing which address it means.
fn http_effective(config: &GlobalConfig) -> (String, bool) {
    match config.service.as_ref().and_then(|s| s.http.as_ref()) {
        Some(HttpSetting::Enabled(true)) => (crate::daemon::DEFAULT_HTTP_ADDR.to_string(), false),
        Some(HttpSetting::Enabled(false)) => ("false".to_string(), false),
        Some(HttpSetting::Address(a)) => (a.clone(), false),
        None => (crate::daemon::DEFAULT_HTTP_ADDR.to_string(), true),
    }
}

// --- service.ui ------------------------------------------------------------

fn set_ui(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: bool = value
        .parse()
        .map_err(|_| SettingsError(format!("service.ui must be true or false, got '{value}'")))?;
    config.service.get_or_insert_with(ServiceConfig::default).ui = Some(parsed);
    Ok(())
}

fn clear_ui(config: &mut GlobalConfig) {
    if let Some(s) = config.service.as_mut() {
        s.ui = None;
    }
    drop_service_if_empty(config);
}

/// The effective value is [`GlobalConfig::ui_enabled`], so an operator who
/// turned the API off sees the UI reported off too - the coupling is visible in
/// `config show` rather than only at startup. `is_default` still tracks the
/// `service.ui` key alone: the value shown may come from `service.api`, but the
/// source of this row is whether this key was set.
fn ui_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.service.as_ref().and_then(|s| s.ui).is_none();
    (config.ui_enabled().to_string(), is_default)
}

// --- service.api -----------------------------------------------------------

fn set_api(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: bool = value
        .parse()
        .map_err(|_| SettingsError(format!("service.api must be true or false, got '{value}'")))?;
    config
        .service
        .get_or_insert_with(ServiceConfig::default)
        .api = Some(parsed);
    Ok(())
}

fn clear_api(config: &mut GlobalConfig) {
    if let Some(s) = config.service.as_mut() {
        s.api = None;
    }
    drop_service_if_empty(config);
}

fn api_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.service.as_ref().and_then(|s| s.api).is_none();
    (config.api_enabled().to_string(), is_default)
}

// --- service.allowed_hosts -------------------------------------------------

/// Split a comma-separated Host list into trimmed, non-empty entries. Shared by
/// the settings apply path and the daemon's flag resolver so both normalize the
/// same way.
pub(crate) fn parse_allowed_hosts(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn set_allowed_hosts(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let hosts = parse_allowed_hosts(value);
    if let Some(bad) = hosts.iter().find(|h| h.contains(char::is_whitespace)) {
        return Err(SettingsError(format!(
            "service.allowed_hosts entries must not contain whitespace, got '{bad}'"
        )));
    }
    // An empty list means "no extra hosts": clear the setting (and drop the
    // service block if it is now empty) rather than persist an empty vec.
    if hosts.is_empty() {
        clear_allowed_hosts(config);
        return Ok(());
    }
    config
        .service
        .get_or_insert_with(ServiceConfig::default)
        .allowed_hosts = Some(hosts);
    Ok(())
}

fn clear_allowed_hosts(config: &mut GlobalConfig) {
    if let Some(s) = config.service.as_mut() {
        s.allowed_hosts = None;
    }
    drop_service_if_empty(config);
}

fn allowed_hosts_effective(config: &GlobalConfig) -> (String, bool) {
    match config
        .service
        .as_ref()
        .and_then(|s| s.allowed_hosts.as_ref())
    {
        Some(hosts) if !hosts.is_empty() => (hosts.join(","), false),
        _ => (String::new(), true),
    }
}

// --- skills.serve -------------------------------------------------------------

/// Accepts the tri-state `auto`, `true` and `false`. The two booleans are the
/// setting's own spelling rather than a compatibility shim, so a config, a
/// `configure set` call and `CRYSTALLINE_SKILLS_SERVE` written before the
/// setting grew its third value all keep working unchanged.
fn set_skills_serve(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed = SkillsServe::parse(value.trim()).ok_or_else(|| {
        SettingsError(format!(
            "skills.serve must be auto, true or false, got '{value}'"
        ))
    })?;
    config
        .skills
        .get_or_insert_with(SkillsConfig::default)
        .serve = Some(parsed);
    Ok(())
}

fn clear_skills_serve(config: &mut GlobalConfig) {
    if let Some(s) = config.skills.as_mut() {
        s.serve = None;
    }
    drop_skills_if_empty(config);
}

fn skills_serve_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.skills.as_ref().and_then(|s| s.serve).is_none();
    (config.skills_serve().as_str().to_string(), is_default)
}

// --- service.response_format ------------------------------------------------

fn set_response_format(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed = match value {
        "toon" => ResponseFormat::Toon,
        "json" => ResponseFormat::Json,
        _ => {
            return Err(SettingsError(format!(
                "service.response_format must be toon or json, got '{value}'"
            )));
        }
    };
    config
        .service
        .get_or_insert_with(ServiceConfig::default)
        .response_format = Some(parsed);
    Ok(())
}

fn clear_response_format(config: &mut GlobalConfig) {
    if let Some(s) = config.service.as_mut() {
        s.response_format = None;
    }
    drop_service_if_empty(config);
}

fn response_format_effective(config: &GlobalConfig) -> (String, bool) {
    let stored = config.service.as_ref().and_then(|s| s.response_format);
    let value = match stored.unwrap_or_default() {
        ResponseFormat::Toon => "toon",
        ResponseFormat::Json => "json",
    };
    (value.to_string(), stored.is_none())
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

// --- search.salience_weight --------------------------------------------------

fn set_salience_weight(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let w: f64 = value.parse().map_err(|_| {
        SettingsError(format!(
            "search.salience_weight must be a number, got '{value}'"
        ))
    })?;
    if !(0.0..=1.0).contains(&w) {
        return Err(SettingsError(format!(
            "search.salience_weight must be between 0.0 and 1.0, got {w}"
        )));
    }
    config
        .search
        .get_or_insert_with(SearchConfig::default)
        .salience_weight = Some(w);
    Ok(())
}

fn clear_salience_weight(config: &mut GlobalConfig) {
    if let Some(s) = config.search.as_mut() {
        s.salience_weight = None;
    }
    drop_search_if_empty(config);
}

fn salience_weight_effective(config: &GlobalConfig) -> (String, bool) {
    match config.salience_weight() {
        Some(w) => (format!("{w}"), false),
        None => (DEFAULT_SALIENCE_WEIGHT.to_string(), true),
    }
}

// --- search.retired_weight ----------------------------------------------------

fn set_retired_weight(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let w: f64 = value.parse().map_err(|_| {
        SettingsError(format!(
            "search.retired_weight must be a number, got '{value}'"
        ))
    })?;
    if !(0.0..=1.0).contains(&w) {
        return Err(SettingsError(format!(
            "search.retired_weight must be between 0.0 and 1.0, got {w}"
        )));
    }
    config
        .search
        .get_or_insert_with(SearchConfig::default)
        .retired_weight = Some(w);
    Ok(())
}

fn clear_retired_weight(config: &mut GlobalConfig) {
    if let Some(s) = config.search.as_mut() {
        s.retired_weight = None;
    }
    drop_search_if_empty(config);
}

fn retired_weight_effective(config: &GlobalConfig) -> (String, bool) {
    match config.retired_weight() {
        Some(w) => (format!("{w}"), false),
        None => (DEFAULT_RETIRED_WEIGHT.to_string(), true),
    }
}

// --- index.files ---------------------------------------------------------------

fn set_index_files(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: bool = value
        .parse()
        .map_err(|_| SettingsError(format!("index.files must be true or false, got '{value}'")))?;
    config.index.get_or_insert_with(IndexConfig::default).files = Some(parsed);
    Ok(())
}

fn clear_index_files(config: &mut GlobalConfig) {
    if let Some(i) = config.index.as_mut() {
        i.files = None;
    }
    drop_index_if_empty(config);
}

fn index_files_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.index.as_ref().and_then(|i| i.files).is_none();
    (config.index_files().to_string(), is_default)
}

// --- capture.similar --------------------------------------------------------

fn set_capture_similar(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: bool = value.parse().map_err(|_| {
        SettingsError(format!(
            "capture.similar must be true or false, got '{value}'"
        ))
    })?;
    config
        .capture
        .get_or_insert_with(CaptureConfig::default)
        .similar = Some(parsed);
    Ok(())
}

fn clear_capture_similar(config: &mut GlobalConfig) {
    if let Some(c) = config.capture.as_mut() {
        c.similar = None;
    }
    drop_capture_if_empty(config);
}

fn capture_similar_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.capture.as_ref().and_then(|c| c.similar).is_none();
    (config.capture_similar().to_string(), is_default)
}

// --- identity.actor ---------------------------------------------------------

fn set_identity_actor(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SettingsError(
            "identity.actor must not be empty".to_string(),
        ));
    }
    if trimmed.contains(char::is_whitespace) {
        return Err(SettingsError(format!(
            "identity.actor must not contain whitespace, got '{value}'"
        )));
    }
    config
        .identity
        .get_or_insert_with(IdentityConfig::default)
        .actor = Some(trimmed.to_string());
    Ok(())
}

fn clear_identity_actor(config: &mut GlobalConfig) {
    if let Some(i) = config.identity.as_mut() {
        i.actor = None;
    }
    drop_identity_if_empty(config);
}

fn identity_actor_effective(config: &GlobalConfig) -> (String, bool) {
    match config.identity_actor() {
        Some(actor) => (actor.to_string(), false),
        None => (String::new(), true),
    }
}

// --- auth.trusted_header ------------------------------------------------------

fn set_trusted_header(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SettingsError(
            "auth.trusted_header must not be empty".to_string(),
        ));
    }
    if trimmed.contains(char::is_whitespace) {
        return Err(SettingsError(format!(
            "auth.trusted_header must not contain whitespace, got '{value}'"
        )));
    }
    // The served API parses this into an `axum` `HeaderName` when it builds the
    // HTTP surface, and a value that will not parse leaves that surface
    // refusing to come up. Parsing here means a typo is refused at the moment
    // it is typed, rather than accepted now and discovered as a dead endpoint
    // at the next daemon start. The parse over there stays as the second layer.
    if axum::http::HeaderName::try_from(trimmed.to_ascii_lowercase()).is_err() {
        return Err(SettingsError(format!(
            "auth.trusted_header must be a valid HTTP header name - letters, digits or any of \
             !#$%&'*+-.^_`|~, and nothing else - got '{value}'"
        )));
    }
    config
        .auth
        .get_or_insert_with(AuthConfig::default)
        .trusted_header = Some(trimmed.to_string());
    Ok(())
}

fn clear_trusted_header(config: &mut GlobalConfig) {
    if let Some(a) = config.auth.as_mut() {
        a.trusted_header = None;
    }
    drop_auth_if_empty(config);
}

fn trusted_header_effective(config: &GlobalConfig) -> (String, bool) {
    match config.auth_trusted_header() {
        Some(header) => (header.to_string(), false),
        None => (String::new(), true),
    }
}

// --- auth.anonymous -----------------------------------------------------------

fn set_anonymous(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: bool = value.parse().map_err(|_| {
        SettingsError(format!(
            "auth.anonymous must be true or false, got '{value}'"
        ))
    })?;
    config
        .auth
        .get_or_insert_with(AuthConfig::default)
        .anonymous = Some(parsed);
    Ok(())
}

fn clear_anonymous(config: &mut GlobalConfig) {
    if let Some(a) = config.auth.as_mut() {
        a.anonymous = None;
    }
    drop_auth_if_empty(config);
}

fn anonymous_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.auth.as_ref().and_then(|a| a.anonymous).is_none();
    (config.auth_anonymous().to_string(), is_default)
}

// --- auth.mcp -----------------------------------------------------------

fn set_mcp(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: bool = value
        .parse()
        .map_err(|_| SettingsError(format!("auth.mcp must be true or false, got '{value}'")))?;
    config.auth.get_or_insert_with(AuthConfig::default).mcp = Some(parsed);
    Ok(())
}

fn clear_mcp(config: &mut GlobalConfig) {
    if let Some(a) = config.auth.as_mut() {
        a.mcp = None;
    }
    drop_auth_if_empty(config);
}

fn mcp_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.auth.as_ref().and_then(|a| a.mcp).is_none();
    (config.auth_mcp().to_string(), is_default)
}

// --- auth.oauth -----------------------------------------------------------

fn set_oauth(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: bool = value
        .parse()
        .map_err(|_| SettingsError(format!("auth.oauth must be true or false, got '{value}'")))?;
    config.auth.get_or_insert_with(AuthConfig::default).oauth = Some(parsed);
    Ok(())
}

fn clear_oauth(config: &mut GlobalConfig) {
    if let Some(a) = config.auth.as_mut() {
        a.oauth = None;
    }
    drop_auth_if_empty(config);
}

fn oauth_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.auth.as_ref().and_then(|a| a.oauth).is_none();
    (config.auth_oauth().to_string(), is_default)
}

// --- auth.proxy_headers -------------------------------------------------------

fn set_proxy_headers(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: bool = value.parse().map_err(|_| {
        SettingsError(format!(
            "auth.proxy_headers must be true or false, got '{value}'"
        ))
    })?;
    config
        .auth
        .get_or_insert_with(AuthConfig::default)
        .proxy_headers = Some(parsed);
    Ok(())
}

fn clear_proxy_headers(config: &mut GlobalConfig) {
    if let Some(a) = config.auth.as_mut() {
        a.proxy_headers = None;
    }
    drop_auth_if_empty(config);
}

fn proxy_headers_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.auth.as_ref().and_then(|a| a.proxy_headers).is_none();
    (config.auth_proxy_headers().to_string(), is_default)
}

// --- auth.max_users -----------------------------------------------------------

fn set_max_users(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let parsed: u32 = value.trim().parse().map_err(|_| {
        SettingsError(format!(
            "auth.max_users must be a positive integer, got '{value}'"
        ))
    })?;
    if parsed == 0 {
        return Err(SettingsError(
            "auth.max_users must be at least 1: zero would refuse every trusted-header identity"
                .to_string(),
        ));
    }
    config
        .auth
        .get_or_insert_with(AuthConfig::default)
        .max_users = Some(parsed);
    Ok(())
}

fn clear_max_users(config: &mut GlobalConfig) {
    if let Some(a) = config.auth.as_mut() {
        a.max_users = None;
    }
    drop_auth_if_empty(config);
}

fn max_users_effective(config: &GlobalConfig) -> (String, bool) {
    let is_default = config.auth.as_ref().and_then(|a| a.max_users).is_none();
    (config.auth_max_users().to_string(), is_default)
}

// --- auth.oidc.* --------------------------------------------------------------

/// The placeholder in the tenant-independent Entra discovery url, lowercased
/// so a `{tenantId}` copied off a portal page matches too.
///
/// Shared with the relying party rather than spelled twice: `set_oidc_issuer`
/// is one way the issuer reaches the config and `CRYSTALLINE_AUTH_OIDC_ISSUER`
/// is another, and a guard that only one of them applies is a guard that does
/// not hold.
pub const ENTRA_TEMPLATE_MARKER: &str = "{tenantid}";

/// What to say when the template turns up, wherever it turns up.
pub const ENTRA_TEMPLATE_HELP: &str = "this is the tenant-independent Entra discovery template, not your issuer - use the \
     tenant-specific URL https://login.microsoftonline.com/<your-tenant-id>/v2.0";

/// What to say when one of the tenant-independent Entra endpoints turns up.
/// They are worse than the template: they discover cleanly and then refuse
/// every token as an issuer mismatch, which nobody can read back to this.
pub const ENTRA_TENANT_INDEPENDENT_HELP: &str = "common, organizations and consumers are the tenant-independent Entra endpoints, not \
     your issuer - they discover fine and then refuse every token as an issuer mismatch; use \
     the tenant-specific URL https://login.microsoftonline.com/<your-tenant-id>/v2.0";

/// Why `issuer` cannot be an issuer, when it is one of the Entra values an
/// operator pastes off a portal page: the `{tenantid}` template, or the
/// `common`, `organizations` and `consumers` endpoints. `None` means it may
/// still be wrong, but not in a way this guard knows about.
///
/// One function for both places the issuer can arrive through - the settings
/// registry and the environment overlay - so the two guards cannot drift.
pub fn entra_issuer_problem(issuer: &str) -> Option<&'static str> {
    let lower = issuer.to_ascii_lowercase();
    if lower.contains(ENTRA_TEMPLATE_MARKER) {
        return Some(ENTRA_TEMPLATE_HELP);
    }
    let (_, after_host) = lower.split_once("login.microsoftonline.com/")?;
    let first_segment = after_host.split('/').next().unwrap_or("");
    matches!(first_segment, "common" | "organizations" | "consumers")
        .then_some(ENTRA_TENANT_INDEPENDENT_HELP)
}

/// The `auth.oidc` block, created on demand so the first `auth.oidc.*` write
/// materializes it and every later one reuses it.
fn oidc_mut(config: &mut GlobalConfig) -> &mut OidcConfig {
    config
        .auth
        .get_or_insert_with(AuthConfig::default)
        .oidc
        .get_or_insert_with(OidcConfig::default)
}

/// Drop an emptied `auth.oidc` block, then an `auth` block emptied by that.
/// Unsetting the last oidc key must leave the config exactly as it was before
/// the first one was set, not a file carrying two empty maps.
fn drop_oidc_if_empty(config: &mut GlobalConfig) {
    if let Some(a) = config.auth.as_mut()
        && a.oidc.as_ref() == Some(&OidcConfig::default())
    {
        a.oidc = None;
    }
    drop_auth_if_empty(config);
}

/// Trim and reject an empty `auth.oidc.*` string, the shape every key in the
/// block shares. `unset` is how a key is removed; an empty string would
/// otherwise write a present-but-blank value the relying party would have to
/// second-guess.
fn oidc_value(key: &str, value: &str) -> Result<String, SettingsError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SettingsError(format!(
            "{key} must not be empty - unset it instead to turn it off"
        )));
    }
    Ok(trimmed.to_string())
}

/// One `auth.oidc.*` string field's effective value: the configured value, or
/// empty and flagged as a default when the block or the field is absent.
fn oidc_effective(
    config: &GlobalConfig,
    field: fn(&OidcConfig) -> Option<&String>,
) -> (String, bool) {
    match config.auth_oidc().and_then(field) {
        Some(v) => (v.clone(), false),
        None => (String::new(), true),
    }
}

/// Clear one `auth.oidc.*` field, then drop the block (and the `auth` block
/// above it) if that emptied it. The write-side twin of [`oidc_effective`].
fn clear_oidc(config: &mut GlobalConfig, field: fn(&mut OidcConfig) -> &mut Option<String>) {
    if let Some(o) = config.auth.as_mut().and_then(|a| a.oidc.as_mut()) {
        *field(o) = None;
    }
    drop_oidc_if_empty(config);
}

fn set_oidc_issuer(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let issuer = oidc_value("auth.oidc.issuer", value)?;
    // The tenant-independent Entra discovery template is the one wrong value
    // people paste from a portal page, and it fails much later as an issuer
    // mismatch on a token nobody can read. Refuse it here, where the fix is a
    // sentence away.
    if let Some(help) = entra_issuer_problem(&issuer) {
        return Err(SettingsError(help.to_string()));
    }
    oidc_mut(config).issuer = Some(issuer);
    Ok(())
}

fn clear_oidc_issuer(config: &mut GlobalConfig) {
    clear_oidc(config, |o| &mut o.issuer);
}

fn oidc_issuer_effective(config: &GlobalConfig) -> (String, bool) {
    oidc_effective(config, |o| o.issuer.as_ref())
}

fn set_oidc_client_id(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let id = oidc_value("auth.oidc.client_id", value)?;
    oidc_mut(config).client_id = Some(id);
    Ok(())
}

fn clear_oidc_client_id(config: &mut GlobalConfig) {
    clear_oidc(config, |o| &mut o.client_id);
}

fn oidc_client_id_effective(config: &GlobalConfig) -> (String, bool) {
    oidc_effective(config, |o| o.client_id.as_ref())
}

fn set_oidc_client_secret(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let secret = oidc_value("auth.oidc.client_secret", value)?;
    oidc_mut(config).client_secret = Some(secret);
    Ok(())
}

fn clear_oidc_client_secret(config: &mut GlobalConfig) {
    clear_oidc(config, |o| &mut o.client_secret);
}

/// Raw like every sibling: the registry entry is `secret: true`, and
/// [`SettingSpec::display`] is what turns this into [`SECRET_DISPLAY`]
/// before it reaches `config show`, the `configure` tool result or `doctor`.
fn oidc_client_secret_effective(config: &GlobalConfig) -> (String, bool) {
    oidc_effective(config, |o| o.client_secret.as_ref())
}

fn set_oidc_name(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let name = oidc_value("auth.oidc.name", value)?;
    oidc_mut(config).name = Some(name);
    Ok(())
}

fn clear_oidc_name(config: &mut GlobalConfig) {
    clear_oidc(config, |o| &mut o.name);
}

fn oidc_name_effective(config: &GlobalConfig) -> (String, bool) {
    oidc_effective(config, |o| o.name.as_ref())
}

fn set_oidc_scopes(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    // Scopes are space separated on the wire, so the stored form is the wire
    // form with its internal spacing normalized: a value pasted with newlines
    // or double spaces still produces one valid scope parameter.
    let scopes = oidc_value("auth.oidc.scopes", value)?;
    let normalized = scopes.split_whitespace().collect::<Vec<_>>().join(" ");
    oidc_mut(config).scopes = Some(normalized);
    Ok(())
}

fn clear_oidc_scopes(config: &mut GlobalConfig) {
    clear_oidc(config, |o| &mut o.scopes);
}

fn oidc_scopes_effective(config: &GlobalConfig) -> (String, bool) {
    oidc_effective(config, |o| o.scopes.as_ref())
}

fn set_oidc_default_role(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let raw = oidc_value("auth.oidc.default_role", value)?;
    // Parsed through the one role type the accounts database uses, so this key
    // can never name a role that does not exist, and stored in that type's own
    // spelling so casing is canonical on the way in.
    let role: Role = raw.parse().map_err(|_| {
        SettingsError(format!(
            "auth.oidc.default_role must be viewer, editor or admin, got '{value}'"
        ))
    })?;
    oidc_mut(config).default_role = Some(role.as_str().to_string());
    Ok(())
}

fn clear_oidc_default_role(config: &mut GlobalConfig) {
    clear_oidc(config, |o| &mut o.default_role);
}

fn oidc_default_role_effective(config: &GlobalConfig) -> (String, bool) {
    match config.auth_oidc().and_then(|o| o.default_role.as_ref()) {
        Some(role) => (role.clone(), false),
        // The same constant the relying party provisions at, so what this key
        // says it does when unset and what a sign-in then does cannot drift.
        None => (crate::rest::DEFAULT_OIDC_ROLE.as_str().to_string(), true),
    }
}

/// The path the callback is served at, absolute on this instance's HTTP
/// surface. A configured `auth.oidc.redirect_uri` has to end here, because
/// that is where the browser the provider redirects actually lands.
///
/// Spelled once, and pinned against the router's own constant by
/// `rest::oidc`'s `the_validated_callback_path_is_the_one_this_router_serves`.
pub const OIDC_CALLBACK_PATH: &str = "/api/v1/auth/oidc/callback";

/// Why `value` cannot be the address a provider sends a browser back to, or
/// `None` when it can.
///
/// One function for both places the key arrives through - the settings
/// registry and the environment overlay - so the two guards cannot drift,
/// exactly as [`entra_issuer_problem`] does for the issuer.
///
/// Three rules, and each one is a mistake an operator makes rather than a
/// theoretical shape: the value has to be an absolute url (a path alone is
/// what somebody writes who expects the host to be filled in), it has to be
/// https unless it is a development server on loopback (an identity crossing
/// plaintext is the one thing this whole flow exists to avoid), and it has to
/// end at the callback this instance actually serves (anything else registers
/// an address the browser never reaches).
pub fn oidc_redirect_uri_problem(value: &str) -> Option<String> {
    let key = "auth.oidc.redirect_uri";
    let Ok(url) = openidconnect::url::Url::parse(value.trim()) else {
        return Some(format!(
            "{key} must be an absolute url, for example \
             https://knowledge.example.com{OIDC_CALLBACK_PATH}, got '{value}'"
        ));
    };
    let loopback = match url.host() {
        Some(openidconnect::url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(openidconnect::url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(openidconnect::url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => {
            return Some(format!(
                "{key} must be an https url - an identity is what comes back to it; \
                 http is allowed only on loopback, for a development server"
            ));
        }
    }
    if url.path() != OIDC_CALLBACK_PATH || url.query().is_some() || url.fragment().is_some() {
        return Some(format!(
            "{key} must end in {OIDC_CALLBACK_PATH}, with no query and no fragment - \
             that is the one address this instance serves the callback at, and the \
             provider has to have it registered in exactly that spelling"
        ));
    }
    None
}

fn set_oidc_redirect_uri(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    let uri = oidc_value("auth.oidc.redirect_uri", value)?;
    if let Some(problem) = oidc_redirect_uri_problem(&uri) {
        return Err(SettingsError(problem));
    }
    oidc_mut(config).redirect_uri = Some(uri);
    Ok(())
}

fn clear_oidc_redirect_uri(config: &mut GlobalConfig) {
    clear_oidc(config, |o| &mut o.redirect_uri);
}

fn oidc_redirect_uri_effective(config: &GlobalConfig) -> (String, bool) {
    oidc_effective(config, |o| o.redirect_uri.as_ref())
}

// --- domains_root ----------------------------------------------------------

fn set_domains_root(config: &mut GlobalConfig, value: &str) -> Result<(), SettingsError> {
    if value.trim().is_empty() {
        return Err(SettingsError("domains_root must not be empty".to_string()));
    }
    config.domains_root = Some(PathBuf::from(value.trim()));
    Ok(())
}

fn clear_domains_root(config: &mut GlobalConfig) {
    config.domains_root = None;
}

fn domains_root_effective(config: &GlobalConfig) -> (String, bool) {
    // The shown value is always the resolved root (the configured path or the
    // built-in default), so an operator sees where domains actually land; the
    // default flag reflects whether it was explicitly set.
    (
        config.domains_root().display().to_string(),
        config.domains_root.is_none(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known_keys() -> Vec<&'static str> {
        registry().iter().map(|s| s.key).collect()
    }

    #[test]
    fn registry_lists_exactly_the_thirty_five_keys_in_order() {
        assert_eq!(
            known_keys(),
            vec![
                "domains_root",
                "github.enabled",
                "github.stacks",
                "github.share_identity",
                "github.agent_identity",
                "github.poll_secs",
                "github.api_url",
                "github.oauth_client_id",
                "service.read_only",
                "service.http",
                "service.ui",
                "service.api",
                "service.allowed_hosts",
                "service.response_format",
                "skills.serve",
                "database.backend",
                "database.url",
                "search.salience_weight",
                "search.retired_weight",
                "index.files",
                "capture.similar",
                "identity.actor",
                "auth.trusted_header",
                "auth.anonymous",
                "auth.mcp",
                "auth.oauth",
                "auth.max_users",
                "auth.proxy_headers",
                "auth.oidc.issuer",
                "auth.oidc.client_id",
                "auth.oidc.client_secret",
                "auth.oidc.name",
                "auth.oidc.scopes",
                "auth.oidc.default_role",
                "auth.oidc.redirect_uri",
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
                ("domains_root", "CRYSTALLINE_DOMAINS_ROOT".to_string()),
                ("github.enabled", "CRYSTALLINE_GITHUB_ENABLED".to_string()),
                ("github.stacks", "CRYSTALLINE_GITHUB_STACKS".to_string()),
                (
                    "github.share_identity",
                    "CRYSTALLINE_GITHUB_SHARE_IDENTITY".to_string()
                ),
                (
                    "github.agent_identity",
                    "CRYSTALLINE_GITHUB_AGENT_IDENTITY".to_string()
                ),
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
                ("service.ui", "CRYSTALLINE_SERVICE_UI".to_string()),
                ("service.api", "CRYSTALLINE_SERVICE_API".to_string()),
                (
                    "service.allowed_hosts",
                    "CRYSTALLINE_SERVICE_ALLOWED_HOSTS".to_string()
                ),
                (
                    "service.response_format",
                    "CRYSTALLINE_SERVICE_RESPONSE_FORMAT".to_string()
                ),
                ("skills.serve", "CRYSTALLINE_SKILLS_SERVE".to_string()),
                (
                    "database.backend",
                    "CRYSTALLINE_DATABASE_BACKEND".to_string()
                ),
                ("database.url", "CRYSTALLINE_DATABASE_URL".to_string()),
                (
                    "search.salience_weight",
                    "CRYSTALLINE_SEARCH_SALIENCE_WEIGHT".to_string()
                ),
                (
                    "search.retired_weight",
                    "CRYSTALLINE_SEARCH_RETIRED_WEIGHT".to_string()
                ),
                ("index.files", "CRYSTALLINE_INDEX_FILES".to_string()),
                ("capture.similar", "CRYSTALLINE_CAPTURE_SIMILAR".to_string()),
                ("identity.actor", "CRYSTALLINE_IDENTITY_ACTOR".to_string()),
                (
                    "auth.trusted_header",
                    "CRYSTALLINE_AUTH_TRUSTED_HEADER".to_string()
                ),
                ("auth.anonymous", "CRYSTALLINE_AUTH_ANONYMOUS".to_string()),
                ("auth.mcp", "CRYSTALLINE_AUTH_MCP".to_string()),
                ("auth.oauth", "CRYSTALLINE_AUTH_OAUTH".to_string()),
                ("auth.max_users", "CRYSTALLINE_AUTH_MAX_USERS".to_string()),
                (
                    "auth.proxy_headers",
                    "CRYSTALLINE_AUTH_PROXY_HEADERS".to_string()
                ),
                (
                    "auth.oidc.issuer",
                    "CRYSTALLINE_AUTH_OIDC_ISSUER".to_string()
                ),
                (
                    "auth.oidc.client_id",
                    "CRYSTALLINE_AUTH_OIDC_CLIENT_ID".to_string()
                ),
                (
                    "auth.oidc.client_secret",
                    "CRYSTALLINE_AUTH_OIDC_CLIENT_SECRET".to_string()
                ),
                ("auth.oidc.name", "CRYSTALLINE_AUTH_OIDC_NAME".to_string()),
                (
                    "auth.oidc.scopes",
                    "CRYSTALLINE_AUTH_OIDC_SCOPES".to_string()
                ),
                (
                    "auth.oidc.default_role",
                    "CRYSTALLINE_AUTH_OIDC_DEFAULT_ROLE".to_string()
                ),
                (
                    "auth.oidc.redirect_uri",
                    "CRYSTALLINE_AUTH_OIDC_REDIRECT_URI".to_string()
                ),
            ]
        );
    }

    #[test]
    fn change_note_is_present_only_for_startup_effective_keys() {
        let no_env = EnvOverlay::default();
        assert!(change_note("domains_root", &no_env).is_none());
        assert!(change_note("github.enabled", &no_env).is_none());
        assert!(change_note("github.poll_secs", &no_env).is_none());
        assert!(change_note("github.api_url", &no_env).is_none());
        assert!(change_note("github.oauth_client_id", &no_env).is_none());
        assert!(change_note("service.read_only", &no_env).is_some());
        assert!(change_note("service.http", &no_env).is_some());
        assert!(change_note("service.ui", &no_env).is_some());
        assert!(change_note("service.api", &no_env).is_some());
        assert!(change_note("service.allowed_hosts", &no_env).is_some());
        assert!(change_note("service.response_format", &no_env).is_none());
        assert!(change_note("database.backend", &no_env).is_some());
        assert!(change_note("database.url", &no_env).is_some());
        assert!(change_note("search.salience_weight", &no_env).is_none());
        assert!(change_note("search.retired_weight", &no_env).is_none());
        assert!(change_note("index.files", &no_env).is_none());
        assert!(change_note("capture.similar", &no_env).is_none());
        assert!(change_note("identity.actor", &no_env).is_none());
        // The effective value is snapshotted at engine construction
        // (`Engine::skills_serve`), so a write really does wait for the next
        // start and the note is the honest label on that.
        assert!(change_note("skills.serve", &no_env).is_some());
        assert!(change_note("auth.trusted_header", &no_env).is_some());
        assert!(change_note("auth.anonymous", &no_env).is_some());
        assert!(change_note("auth.max_users", &no_env).is_some());
        assert!(change_note("auth.oidc.issuer", &no_env).is_some());
        assert!(change_note("auth.oidc.client_id", &no_env).is_some());
        assert!(change_note("auth.oidc.client_secret", &no_env).is_some());
        assert!(change_note("auth.oidc.name", &no_env).is_some());
        assert!(change_note("auth.oidc.scopes", &no_env).is_some());
        assert!(change_note("auth.oidc.default_role", &no_env).is_some());
        assert!(change_note("auth.oidc.redirect_uri", &no_env).is_some());
        assert!(change_note("auth.proxy_headers", &no_env).is_some());
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
    fn apply_allowed_hosts_parses_and_round_trips() {
        let mut cfg = GlobalConfig::default();
        apply(
            &mut cfg,
            "service.allowed_hosts",
            " muthur.lan, mcp.example.com ",
        )
        .unwrap();
        assert_eq!(
            cfg.service
                .as_ref()
                .and_then(|s| s.allowed_hosts.as_deref()),
            Some(["muthur.lan".to_string(), "mcp.example.com".to_string()].as_slice())
        );
        // effective() joins the list back into the comma-separated form.
        assert_eq!(
            allowed_hosts_effective(&cfg),
            ("muthur.lan,mcp.example.com".to_string(), false)
        );
    }

    #[test]
    fn apply_allowed_hosts_rejects_whitespace_in_a_host() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "service.allowed_hosts", "bad host").unwrap_err();
        assert!(err.0.contains("whitespace"), "{}", err.0);
    }

    #[test]
    fn apply_allowed_hosts_empty_clears_and_drops_the_service_block() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "service.allowed_hosts", "muthur.lan").unwrap();
        assert!(cfg.service.is_some());
        // Clearing the only set field drops the empty service block entirely,
        // and effective() reports the default (loopback-only).
        apply(&mut cfg, "service.allowed_hosts", "").unwrap();
        assert!(cfg.service.is_none());
        assert_eq!(allowed_hosts_effective(&cfg), (String::new(), true));
    }

    /// The tri-state applies, reports and clears through the registry, and the
    /// two boolean spellings stay accepted so a config, a `configure set` call
    /// or an env override written before the third value existed keeps working.
    #[test]
    fn skills_serve_applies_all_three_values_clears_and_rejects() {
        let mut config = GlobalConfig::default();
        assert_eq!(
            skills_serve_effective(&config),
            ("auto".to_string(), true),
            "the default decides per connection"
        );

        for (value, expected) in [
            ("false", SkillsServe::Never),
            ("true", SkillsServe::Always),
            ("auto", SkillsServe::Auto),
        ] {
            apply(&mut config, "skills.serve", value).unwrap();
            assert_eq!(config.skills_serve(), expected, "set {value}");
            assert_eq!(
                skills_serve_effective(&config),
                (value.to_string(), false),
                "reported back as {value}, no longer the default"
            );
        }

        let err = apply(&mut config, "skills.serve", "yes").unwrap_err();
        assert!(err.to_string().contains("auto, true or false"), "{err}");

        unset(&mut config, "skills.serve").unwrap();
        assert_eq!(skills_serve_effective(&config), ("auto".to_string(), true));
        assert!(
            config.skills.is_none(),
            "clearing the only key in the block drops the block"
        );
    }

    #[test]
    fn response_format_applies_clears_and_rejects() {
        let mut config = GlobalConfig::default();
        // The default is toon, reported as such.
        assert_eq!(
            response_format_effective(&config),
            ("toon".to_string(), true)
        );
        apply(&mut config, "service.response_format", "json").unwrap();
        assert_eq!(config.response_format(), ResponseFormat::Json);
        assert_eq!(
            response_format_effective(&config),
            ("json".to_string(), false)
        );
        apply(&mut config, "service.response_format", "toon").unwrap();
        assert_eq!(config.response_format(), ResponseFormat::Toon);
        assert_eq!(
            response_format_effective(&config),
            ("toon".to_string(), false)
        );
        let err = apply(&mut config, "service.response_format", "yaml").unwrap_err();
        assert!(err.to_string().contains("toon or json"), "{err}");
        unset(&mut config, "service.response_format").unwrap();
        assert_eq!(
            response_format_effective(&config),
            ("toon".to_string(), true)
        );
        // The service block is dropped once its last field clears.
        assert!(config.service.is_none());
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

    /// `github.stacks` is the one collaboration toggle that defaults ON, so
    /// the round trip a test has to pin is the way back: setting it false has
    /// to survive, and clearing it has to land back on true rather than on
    /// the bool default.
    #[test]
    fn apply_github_stacks_round_trips_and_defaults_on() {
        let mut cfg = GlobalConfig::default();
        assert!(cfg.github_stacks(), "stacking is the default");

        apply(&mut cfg, "github.stacks", "false").unwrap();
        assert!(!cfg.github_stacks());
        assert_eq!(cfg.github.as_ref().unwrap().stacks, Some(false));

        apply(&mut cfg, "github.stacks", "true").unwrap();
        assert!(cfg.github_stacks());

        unset(&mut cfg, "github.stacks").unwrap();
        assert!(cfg.github_stacks(), "back to the on default");
        assert!(cfg.github.is_none(), "and the emptied block goes with it");
    }

    #[test]
    fn apply_github_stacks_rejects_non_bool() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "github.stacks", "sometimes").unwrap_err();
        assert!(err.to_string().contains("github.stacks"), "{err}");
        assert!(err.to_string().contains("sometimes"), "{err}");
        assert!(cfg.github.is_none(), "a rejected value must not be written");
    }

    /// The registry is the strict half of the share-identity pair: it stores
    /// only the two words it knows, so a value Crystalline wrote always reads
    /// back as what was set.
    #[test]
    fn apply_github_share_identity_round_trips_and_defaults_to_instance() {
        let mut cfg = GlobalConfig::default();
        assert_eq!(
            cfg.github_share_identity(),
            ShareIdentityMode::Instance,
            "the instance token shares until someone says otherwise"
        );

        apply(&mut cfg, "github.share_identity", "personal").unwrap();
        assert_eq!(cfg.github_share_identity(), ShareIdentityMode::Personal);
        assert_eq!(
            cfg.github.as_ref().unwrap().share_identity.as_deref(),
            Some("personal")
        );

        apply(&mut cfg, "github.share_identity", "instance").unwrap();
        assert_eq!(cfg.github_share_identity(), ShareIdentityMode::Instance);

        unset(&mut cfg, "github.share_identity").unwrap();
        assert_eq!(cfg.github_share_identity(), ShareIdentityMode::Instance);
        assert!(cfg.github.is_none(), "and the emptied block goes with it");
    }

    /// The accessor tolerates a hand-edited typo by falling back to
    /// `instance`; a write through the registry does not, and the refusal
    /// names both accepted words so the fix is in the message.
    #[test]
    fn apply_github_share_identity_rejects_anything_but_the_two_words() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "github.share_identity", "sideways").unwrap_err();
        assert!(err.to_string().contains("github.share_identity"), "{err}");
        assert!(err.to_string().contains("instance"), "{err}");
        assert!(err.to_string().contains("personal"), "{err}");
        assert!(err.to_string().contains("sideways"), "{err}");
        assert!(cfg.github.is_none(), "a rejected value must not be written");
    }

    #[test]
    fn apply_github_agent_identity_round_trips_and_empty_clears_it() {
        let mut cfg = GlobalConfig::default();
        assert_eq!(cfg.github_agent_identity(), None);

        apply(&mut cfg, "github.agent_identity", "share-bot").unwrap();
        assert_eq!(cfg.github_agent_identity(), Some("share-bot"));

        // No unset verb reaches every surface, so the empty string is how an
        // agent identity is taken away again.
        apply(&mut cfg, "github.agent_identity", "").unwrap();
        assert_eq!(cfg.github_agent_identity(), None);
        assert!(cfg.github.is_none(), "and the emptied block goes with it");

        apply(&mut cfg, "github.agent_identity", "  share-bot  ").unwrap();
        assert_eq!(
            cfg.github_agent_identity(),
            Some("share-bot"),
            "the stored value is trimmed"
        );

        unset(&mut cfg, "github.agent_identity").unwrap();
        assert_eq!(cfg.github_agent_identity(), None);
        assert!(cfg.github.is_none());
    }

    /// The value names an account whose credential is addressed by name, so
    /// it is held to the same allowlist the token store applies rather than to
    /// "anything without spaces".
    #[test]
    fn apply_github_agent_identity_rejects_a_name_outside_the_account_class() {
        for bad in [
            "Share-Bot",
            "share bot",
            "team/bot",
            "bot:ghes.example",
            "bot\\name",
            &"b".repeat(MAX_IDENTITY_NAME_BYTES + 1),
        ] {
            let mut cfg = GlobalConfig::default();
            let err = apply(&mut cfg, "github.agent_identity", bad).unwrap_err();
            assert!(
                err.to_string().contains("github.agent_identity"),
                "{bad}: {err}"
            );
            assert!(cfg.github.is_none(), "a rejected value must not be written");
        }
    }

    /// The cross-crate agreement this setting rests on: `github.agent_identity`
    /// names an account the token store will have to address, so the setting
    /// accepts a value if and only if
    /// [`crystalline_remote::valid_identity_name`] does. One predicate decides
    /// the character class AND the ceiling; a value that saves here and then
    /// cannot be resolved there is the failure this pins shut.
    #[test]
    fn the_agent_identity_class_is_the_token_stores_own_predicate() {
        let names = [
            "bot",
            "share-bot",
            "share_bot.2",
            "Share-Bot",
            "share bot",
            "team/bot",
            "bot:ghes.example",
            "bot\\name",
            "bot\u{1f}name",
            "",
            "   ",
            &"b".repeat(MAX_IDENTITY_NAME_BYTES),
            &"b".repeat(MAX_IDENTITY_NAME_BYTES + 1),
        ];
        for name in names {
            let mut cfg = GlobalConfig::default();
            let accepted = apply(&mut cfg, "github.agent_identity", name).is_ok();
            // The empty value is the documented way to clear the setting, so it
            // is accepted here while the predicate rejects it; every other value
            // must agree exactly.
            let expected = valid_identity_name(name.trim()) || name.trim().is_empty();
            assert_eq!(accepted, expected, "{name:?}");
        }
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
        assert_eq!(views.len(), 35);
        assert_eq!(
            views.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(),
            vec![
                "domains_root",
                "github.enabled",
                "github.stacks",
                "github.share_identity",
                "github.agent_identity",
                "github.poll_secs",
                "github.api_url",
                "github.oauth_client_id",
                "service.read_only",
                "service.http",
                "service.ui",
                "service.api",
                "service.allowed_hosts",
                "service.response_format",
                "skills.serve",
                "database.backend",
                "database.url",
                "search.salience_weight",
                "search.retired_weight",
                "index.files",
                "capture.similar",
                "identity.actor",
                "auth.trusted_header",
                "auth.anonymous",
                "auth.mcp",
                "auth.oauth",
                "auth.max_users",
                "auth.proxy_headers",
                "auth.oidc.issuer",
                "auth.oidc.client_id",
                "auth.oidc.client_secret",
                "auth.oidc.name",
                "auth.oidc.scopes",
                "auth.oidc.default_role",
                "auth.oidc.redirect_uri",
            ]
        );

        let domains_root = &views[0];
        assert!(
            domains_root.value.ends_with("Documents/Crystalline"),
            "{}",
            domains_root.value
        );
        assert_eq!(domains_root.source, SettingSource::Default);

        let enabled = &views[1];
        assert_eq!(enabled.value, "true");
        assert_eq!(enabled.source, SettingSource::Config);
        assert!(!enabled.doc.is_empty());

        // The one GitHub setting whose unset default is on.
        let stacks = &views[2];
        assert_eq!(stacks.value, "true");
        assert_eq!(stacks.source, SettingSource::Default);

        // The default mode is the pre-feature one: the instance token shares.
        let share_identity = &views[3];
        assert_eq!(share_identity.value, "instance");
        assert_eq!(share_identity.source, SettingSource::Default);

        // No agent identity is configured out of the box, so an HTTP-MCP share
        // in personal mode has nothing to run under until someone names one.
        let agent_identity = &views[4];
        assert_eq!(agent_identity.value, "");
        assert_eq!(agent_identity.source, SettingSource::Default);

        let poll_secs = &views[5];
        assert_eq!(poll_secs.value, "300");
        assert_eq!(poll_secs.source, SettingSource::Default);

        let api_url = &views[6];
        assert_eq!(api_url.value, "https://api.github.com");
        assert_eq!(api_url.source, SettingSource::Default);

        let oauth = &views[7];
        assert_eq!(oauth.value, crystalline_remote::GITHUB_CLIENT_ID);
        assert_eq!(oauth.source, SettingSource::Default);

        let read_only = &views[8];
        assert_eq!(read_only.value, "false");
        assert_eq!(read_only.source, SettingSource::Default);

        // The endpoint is on by default, so the effective value nobody set is the
        // loopback address the daemon will actually bind.
        let http = &views[9];
        assert_eq!(http.value, "127.0.0.1:7411");
        assert_eq!(http.source, SettingSource::Default);

        // Both HTTP-surface toggles default to on, so an unconfigured install
        // gets the web UI and the JSON API on that endpoint.
        let ui = &views[10];
        assert_eq!(ui.value, "true");
        assert_eq!(ui.source, SettingSource::Default);

        let api = &views[11];
        assert_eq!(api.value, "true");
        assert_eq!(api.source, SettingSource::Default);

        let allowed_hosts = &views[12];
        assert_eq!(allowed_hosts.value, "");
        assert_eq!(allowed_hosts.source, SettingSource::Default);

        let response_format = &views[13];
        assert_eq!(response_format.value, "toon");
        assert_eq!(response_format.source, SettingSource::Default);

        let skills_serve = &views[14];
        assert_eq!(skills_serve.value, "auto");
        assert_eq!(skills_serve.source, SettingSource::Default);

        let backend = &views[15];
        assert_eq!(backend.value, "turso");
        assert_eq!(backend.source, SettingSource::Default);

        let url = &views[16];
        assert_eq!(url.value, "");
        assert_eq!(url.source, SettingSource::Default);

        let salience_weight = &views[17];
        assert_eq!(salience_weight.value, "0.15");
        assert_eq!(salience_weight.source, SettingSource::Default);

        let retired_weight = &views[18];
        assert_eq!(retired_weight.value, "0.6");
        assert_eq!(retired_weight.source, SettingSource::Default);

        let index_files = &views[19];
        assert_eq!(index_files.value, "true");
        assert_eq!(index_files.source, SettingSource::Default);

        let capture_similar = &views[20];
        assert_eq!(capture_similar.value, "true");
        assert_eq!(capture_similar.source, SettingSource::Default);

        let identity_actor = &views[21];
        assert_eq!(identity_actor.value, "");
        assert_eq!(identity_actor.source, SettingSource::Default);

        let trusted_header = &views[22];
        assert_eq!(trusted_header.value, "");
        assert_eq!(trusted_header.source, SettingSource::Default);

        let anonymous = &views[23];
        assert_eq!(anonymous.value, "false");
        assert_eq!(anonymous.source, SettingSource::Default);
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

    // --- service.ui and service.api ------------------------------------------

    #[test]
    fn apply_service_ui_and_api_happy_path() {
        let mut cfg = GlobalConfig::default();
        assert!(cfg.ui_enabled(), "absent means the UI is served");
        assert!(cfg.api_enabled(), "absent means the API is served");

        apply(&mut cfg, "service.ui", "false").unwrap();
        assert!(!cfg.ui_enabled());
        assert!(cfg.api_enabled(), "the UI toggle leaves the API alone");

        apply(&mut cfg, "service.ui", "true").unwrap();
        apply(&mut cfg, "service.api", "false").unwrap();
        assert!(!cfg.api_enabled());
        assert!(
            !cfg.ui_enabled(),
            "a UI without its API is a dead shell, so the API toggle takes it down too"
        );
    }

    #[test]
    fn apply_service_ui_and_api_reject_non_bool() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "service.ui", "yes").unwrap_err();
        assert!(err.to_string().contains("service.ui must be true or false"));
        let err = apply(&mut cfg, "service.api", "on").unwrap_err();
        assert!(
            err.to_string()
                .contains("service.api must be true or false")
        );
    }

    #[test]
    fn effective_reports_the_ui_and_api_defaults_and_a_set_value() {
        let mut cfg = GlobalConfig::default();
        assert_eq!(ui_effective(&cfg), ("true".to_string(), true));
        assert_eq!(api_effective(&cfg), ("true".to_string(), true));

        apply(&mut cfg, "service.api", "false").unwrap();
        assert_eq!(api_effective(&cfg), ("false".to_string(), false));
        assert_eq!(
            ui_effective(&cfg),
            ("false".to_string(), true),
            "the UI is off because the API is, but the ui key itself is untouched"
        );
    }

    #[test]
    fn unset_service_ui_and_api_drop_an_emptied_service_block() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "service.ui", "false").unwrap();
        apply(&mut cfg, "service.api", "false").unwrap();
        assert!(cfg.service.is_some());

        unset(&mut cfg, "service.ui").unwrap();
        assert!(
            cfg.service.is_some(),
            "service.api is still set, so the block must survive"
        );
        unset(&mut cfg, "service.api").unwrap();
        assert!(
            cfg.service.is_none(),
            "the last set field was cleared, so the block should vanish"
        );
        assert!(cfg.ui_enabled());
        assert!(cfg.api_enabled());

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

    /// `config show` reports what the daemon will actually do, so an untouched
    /// config reads as the loopback address the endpoint now opens by default -
    /// still flagged as the default, because nobody set it. `false` is the
    /// opt-out and reads as itself.
    #[test]
    fn service_http_effective_reports_the_loopback_default_when_unset() {
        let cfg = GlobalConfig::default();
        assert_eq!(
            http_effective(&cfg),
            ("127.0.0.1:7411".to_string(), true),
            "an unset service.http serves the loopback default"
        );

        let mut off = GlobalConfig::default();
        apply(&mut off, "service.http", "false").unwrap();
        assert_eq!(http_effective(&off), ("false".to_string(), false));

        let mut addr = GlobalConfig::default();
        apply(&mut addr, "service.http", "0.0.0.0:7411").unwrap();
        assert_eq!(http_effective(&addr), ("0.0.0.0:7411".to_string(), false));

        // `true` binds the very address the unset case does, so it reports the
        // address rather than the word: same endpoint, one spelling of it.
        let mut on = GlobalConfig::default();
        apply(&mut on, "service.http", "true").unwrap();
        assert_eq!(http_effective(&on), ("127.0.0.1:7411".to_string(), false));
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

    // --- index.files ----------------------------------------------------------

    #[test]
    fn apply_index_files_happy_path_and_unset_drops_the_block() {
        let mut cfg = GlobalConfig::default();
        assert!(cfg.index_files(), "generation is on by default");
        apply(&mut cfg, "index.files", "false").unwrap();
        assert!(!cfg.index_files());
        assert_eq!(index_files_effective(&cfg), ("false".to_string(), false));

        unset(&mut cfg, "index.files").unwrap();
        assert!(cfg.index.is_none(), "the emptied index block must vanish");
        assert!(cfg.index_files());

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(!yaml.contains("index"), "{yaml}");
    }

    #[test]
    fn apply_index_files_rejects_non_bool() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "index.files", "sometimes").unwrap_err();
        assert!(err.to_string().contains("index.files"), "{err}");
        assert!(cfg.index.is_none(), "a rejected value must not be written");
    }

    // --- capture.similar --------------------------------------------------------

    #[test]
    fn capture_similar_round_trips_and_defaults_on() {
        let mut cfg = GlobalConfig::default();
        assert!(cfg.capture_similar(), "absent means on");
        apply(&mut cfg, "capture.similar", "false").unwrap();
        assert!(!cfg.capture_similar());
        let (value, is_default) = capture_similar_effective(&cfg);
        assert_eq!((value.as_str(), is_default), ("false", false));
        assert!(apply(&mut cfg, "capture.similar", "maybe").is_err());
        unset(&mut cfg, "capture.similar").unwrap();
        assert!(cfg.capture.is_none(), "an unset block is dropped whole");
        assert!(cfg.capture_similar());
    }

    // --- domains_root ----------------------------------------------------------

    #[test]
    fn apply_domains_root_happy_path() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "domains_root", "/srv/knowledge").unwrap();
        assert_eq!(
            cfg.domains_root.as_deref(),
            Some(std::path::Path::new("/srv/knowledge"))
        );
        assert_eq!(cfg.domains_root().display().to_string(), "/srv/knowledge");
    }

    #[test]
    fn apply_domains_root_rejects_empty() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "domains_root", "   ").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn unset_domains_root_returns_to_default() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "domains_root", "/srv/knowledge").unwrap();
        unset(&mut cfg, "domains_root").unwrap();
        assert!(cfg.domains_root.is_none());
        // Back to the built-in default, not empty.
        assert!(
            cfg.domains_root()
                .display()
                .to_string()
                .ends_with("Documents/Crystalline"),
            "{}",
            cfg.domains_root().display()
        );

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(!yaml.contains("domains_root"), "{yaml}");
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

    // --- auth.trusted_header ----------------------------------------------------

    #[test]
    fn apply_auth_trusted_header_happy_path() {
        let mut cfg = GlobalConfig::default();
        assert_eq!(
            cfg.auth_trusted_header(),
            None,
            "the feature is off by default"
        );
        apply(&mut cfg, "auth.trusted_header", " X-Forwarded-User ").unwrap();
        assert_eq!(cfg.auth_trusted_header(), Some("X-Forwarded-User"));
        assert_eq!(
            trusted_header_effective(&cfg),
            ("X-Forwarded-User".to_string(), false)
        );
    }

    #[test]
    fn apply_auth_trusted_header_rejects_empty_and_whitespace() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "auth.trusted_header", "   ").unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
        let err = apply(&mut cfg, "auth.trusted_header", "X-Forwarded User").unwrap_err();
        assert!(err.to_string().contains("whitespace"), "{err}");
        assert!(cfg.auth.is_none(), "a rejected value must not be written");
    }

    /// A value HTTP does not allow as a header name is refused here, where the
    /// operator typed it. Accepting it would store a setting the served API
    /// cannot parse, and the HTTP endpoint would then refuse to come up at the
    /// next start with nothing pointing back at this command.
    #[test]
    fn apply_auth_trusted_header_rejects_an_invalid_header_name() {
        let mut cfg = GlobalConfig::default();
        for bad in [
            "X-User:",
            "X-User@host",
            "user/name",
            "(remote-user)",
            "\"x\"",
        ] {
            let err = apply(&mut cfg, "auth.trusted_header", bad).unwrap_err();
            assert!(
                err.to_string().contains("valid HTTP header name"),
                "{bad:?} must be refused as a header name, got: {err}"
            );
            assert!(
                cfg.auth.is_none(),
                "a rejected value must not be written: {bad:?}"
            );
        }

        // The shapes a proxy actually sets, including the punctuation a token
        // does allow, are all accepted.
        for good in [
            "X-Forwarded-User",
            "remote-user",
            "Remote_User",
            "X-User.Id",
        ] {
            apply(&mut cfg, "auth.trusted_header", good).unwrap();
            assert_eq!(cfg.auth_trusted_header(), Some(good));
        }
    }

    #[test]
    fn unset_auth_trusted_header_drops_an_emptied_auth_block() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "auth.trusted_header", "X-Forwarded-User").unwrap();
        assert!(cfg.auth.is_some());

        unset(&mut cfg, "auth.trusted_header").unwrap();
        assert!(
            cfg.auth.is_none(),
            "the only set field was cleared, so the block should vanish"
        );
        assert_eq!(cfg.auth_trusted_header(), None);

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("auth"),
            "an emptied auth block must not round-trip into the yaml: {yaml}"
        );
    }

    // --- auth.anonymous ---------------------------------------------------------

    #[test]
    fn apply_auth_anonymous_happy_path() {
        let mut cfg = GlobalConfig::default();
        assert!(!cfg.auth_anonymous(), "anonymous access is off by default");
        apply(&mut cfg, "auth.anonymous", "true").unwrap();
        assert!(cfg.auth_anonymous());
        apply(&mut cfg, "auth.anonymous", "false").unwrap();
        assert!(!cfg.auth_anonymous());
        assert_eq!(anonymous_effective(&cfg), ("false".to_string(), false));
    }

    #[test]
    fn auth_anonymous_rejects_non_bool() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "auth.anonymous", "yes").unwrap_err();
        assert!(err.to_string().contains("must be true or false"), "{err}");
        assert!(cfg.auth.is_none(), "a rejected value must not be written");
    }

    #[test]
    fn unset_auth_anonymous_drops_an_emptied_auth_block() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "auth.anonymous", "true").unwrap();
        assert!(cfg.auth.is_some());

        unset(&mut cfg, "auth.anonymous").unwrap();
        assert!(
            cfg.auth.is_none(),
            "the only set field was cleared, so the block should vanish"
        );
        assert!(!cfg.auth_anonymous());

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("auth"),
            "an emptied auth block must not round-trip into the yaml: {yaml}"
        );
    }

    #[test]
    fn auth_block_survives_when_a_sibling_field_remains_set() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "auth.trusted_header", "X-Forwarded-User").unwrap();
        apply(&mut cfg, "auth.anonymous", "true").unwrap();

        unset(&mut cfg, "auth.anonymous").unwrap();
        assert!(
            cfg.auth.is_some(),
            "trusted_header is still set, so the block must survive"
        );
        assert_eq!(cfg.auth_trusted_header(), Some("X-Forwarded-User"));
        assert!(!cfg.auth_anonymous());
    }

    // --- auth.mcp -----------------------------------------------------------

    #[test]
    fn auth_mcp_round_trips_and_defaults_off() {
        let mut config = GlobalConfig::default();
        apply(&mut config, "auth.mcp", "true").unwrap();
        assert!(config.auth_mcp());
        unset(&mut config, "auth.mcp").unwrap();
        assert!(!config.auth_mcp());
    }

    // --- auth.oauth -----------------------------------------------------------

    #[test]
    fn auth_oauth_round_trips_and_needs_auth_mcp() {
        let mut config = GlobalConfig::default();
        assert!(!config.auth_oauth());
        apply(&mut config, "auth.oauth", "true").unwrap();
        assert!(config.auth_oauth());
        assert_eq!(oauth_effective(&config), ("true".to_string(), false));

        let no_env = EnvOverlay::default();
        assert!(change_note("auth.oauth", &no_env).is_some());

        let err = crate::rest::AuthCfg::resolve(&config)
            .expect_err("auth.oauth without auth.mcp must refuse to resolve");
        assert!(err.to_string().contains("auth.mcp"), "{err}");

        unset(&mut config, "auth.oauth").unwrap();
        assert!(!config.auth_oauth());
        assert!(
            config.auth.is_none(),
            "the block this key created goes with it"
        );
    }

    // --- auth.proxy_headers ---------------------------------------------------

    #[test]
    fn proxy_headers_round_trips_and_defaults_off() {
        let mut config = GlobalConfig::default();
        assert!(
            !config.auth_proxy_headers(),
            "trust-the-proxy is off unless an operator turns it on"
        );
        apply(&mut config, "auth.proxy_headers", "true").unwrap();
        assert!(config.auth_proxy_headers());
        assert_eq!(
            proxy_headers_effective(&config),
            ("true".to_string(), false)
        );
        unset(&mut config, "auth.proxy_headers").unwrap();
        assert!(!config.auth_proxy_headers());
        assert!(
            config.auth.is_none(),
            "the block this key created goes with it"
        );
    }

    #[test]
    fn proxy_headers_refuses_anything_but_a_boolean() {
        let mut config = GlobalConfig::default();
        let err = apply(&mut config, "auth.proxy_headers", "yes").unwrap_err();
        assert!(err.to_string().contains("true or false"), "{err}");
        assert!(config.auth.is_none(), "a rejected value is not written");
    }

    /// The teaching text is the feature here as much as the flag is: an
    /// operator who turns this on is trusting every header their proxy does
    /// not strip, and the doc string has to say so where they read it.
    #[test]
    fn proxy_headers_teaches_the_trust_boundary() {
        let spec = find("auth.proxy_headers").unwrap();
        assert!(
            spec.startup_effective,
            "resolved once, like the other modes"
        );
        assert!(!spec.secret);
        assert!(matches!(spec.kind, SettingKind::Bool));
        for phrase in [
            "Remote-User",
            "Remote-Name",
            "Remote-Email",
            "Remote-Groups",
            "ONLY safe",
            "unreachable except through",
            "strips",
        ] {
            assert!(
                spec.doc.contains(phrase),
                "the doc must teach the trust boundary, missing '{phrase}': {}",
                spec.doc
            );
        }
    }

    // --- auth.max_users -----------------------------------------------------------

    #[test]
    fn apply_auth_max_users_happy_path() {
        let mut cfg = GlobalConfig::default();
        assert_eq!(cfg.auth_max_users(), 100, "the default cap is 100");
        apply(&mut cfg, "auth.max_users", "50").unwrap();
        assert_eq!(cfg.auth_max_users(), 50);
        assert_eq!(max_users_effective(&cfg), ("50".to_string(), false));
    }

    #[test]
    fn auth_max_users_rejects_zero_and_non_numbers() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "auth.max_users", "0").unwrap_err();
        assert!(err.to_string().contains("at least 1"), "{err}");
        assert!(cfg.auth.is_none(), "a rejected value must not be written");

        let err = apply(&mut cfg, "auth.max_users", "many").unwrap_err();
        assert!(err.to_string().contains("positive integer"), "{err}");
        assert!(cfg.auth.is_none(), "a rejected value must not be written");
    }

    #[test]
    fn unset_auth_max_users_restores_the_default_and_drops_an_emptied_auth_block() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "auth.max_users", "50").unwrap();
        assert!(cfg.auth.is_some());

        unset(&mut cfg, "auth.max_users").unwrap();
        assert!(
            cfg.auth.is_none(),
            "the only set field was cleared, so the block should vanish"
        );
        assert_eq!(cfg.auth_max_users(), 100);
        assert_eq!(max_users_effective(&cfg), ("100".to_string(), true));

        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("auth"),
            "an emptied auth block must not round-trip into the yaml: {yaml}"
        );
    }

    // --- secrets ------------------------------------------------------------

    /// Every key the registry flags as a secret is invisible on every display
    /// path, and the flag is the only thing that makes it so: a value is
    /// applied, then looked for in the snapshot and in the overlay's doctor
    /// listing and `Debug`. Registry-driven, so the next credential-shaped
    /// key is covered by setting its flag and nothing else.
    #[test]
    fn every_secret_key_is_masked_on_every_display_path() {
        let secrets: Vec<&SettingSpec> = registry().iter().filter(|s| s.secret).collect();
        assert_eq!(
            secrets.iter().map(|s| s.key).collect::<Vec<_>>(),
            vec!["database.url", "auth.oidc.client_secret"],
        );

        for spec in secrets {
            assert!(is_secret_key(spec.key), "{}", spec.key);
            let marker = format!("marker-{}", spec.key.replace('.', "-"));

            // Unset: empty and a default, so `(set)` really means set.
            let unset = snapshot(&GlobalConfig::default(), &EnvOverlay::default())
                .into_iter()
                .find(|v| v.key == spec.key)
                .unwrap();
            assert_eq!(unset.value, "", "{}", spec.key);
            assert_eq!(unset.source, SettingSource::Default, "{}", spec.key);

            // Set in the file: masked, stored, and the marker is nowhere.
            let mut config = GlobalConfig::default();
            apply(&mut config, spec.key, &marker).unwrap();
            let views = snapshot(&config, &EnvOverlay::default());
            let view = views.iter().find(|v| v.key == spec.key).unwrap();
            assert_eq!(view.value, SECRET_DISPLAY, "{}", spec.key);
            assert_eq!(view.source, SettingSource::Config, "{}", spec.key);
            assert!(
                !views.iter().any(|v| v.value.contains(&marker)),
                "{} leaked into the snapshot",
                spec.key
            );
            assert_eq!(spec.display(&config).0, SECRET_DISPLAY);

            // Set by the environment: masked in the snapshot, in doctor's
            // override listing and in the overlay's own render.
            let overlay = EnvOverlay::from_vars([(spec.env_var(), marker.clone())]).unwrap();
            let view = snapshot(&GlobalConfig::default(), &overlay)
                .into_iter()
                .find(|v| v.key == spec.key)
                .unwrap();
            assert_eq!(view.value, SECRET_DISPLAY, "{}", spec.key);
            assert_eq!(view.source, SettingSource::Env, "{}", spec.key);
            let (_, _, shown) = overlay
                .active_overrides()
                .into_iter()
                .find(|(_, key, _)| key == spec.key)
                .unwrap();
            assert_eq!(shown, SECRET_DISPLAY, "{}", spec.key);
            let rendered = format!("{overlay:?}");
            assert!(!rendered.contains(&marker), "{rendered}");
        }
    }

    /// A key that is not flagged shows its value, so the flag is not
    /// accidentally masking everything.
    #[test]
    fn a_plain_key_is_not_a_secret_and_shows_its_value() {
        assert!(!is_secret_key("auth.oidc.client_id"));
        assert!(!is_secret_key("no.such.key"));
        let mut config = GlobalConfig::default();
        apply(&mut config, "auth.oidc.client_id", "app-1234").unwrap();
        let view = snapshot(&config, &EnvOverlay::default())
            .into_iter()
            .find(|v| v.key == "auth.oidc.client_id")
            .unwrap();
        assert_eq!(view.value, "app-1234");
    }

    // --- auth.oidc.* --------------------------------------------------------

    /// One key of the oidc block for the round-trip table: its setting key, a
    /// representative value and the field that value must land in.
    type OidcCase = (
        &'static str,
        &'static str,
        fn(&OidcConfig) -> Option<&String>,
    );

    /// Every key in the block round-trips through its own accessor and leaves
    /// no residue behind: unsetting the one key that was set drops the oidc
    /// block and the auth block that only existed to hold it.
    #[test]
    fn every_oidc_key_round_trips_and_unsets_back_to_nothing() {
        let cases: [OidcCase; 6] = [
            ("auth.oidc.issuer", "https://login.example.com/v2.0", |o| {
                o.issuer.as_ref()
            }),
            ("auth.oidc.client_id", "app-1234", |o| o.client_id.as_ref()),
            ("auth.oidc.client_secret", "hunter2", |o| {
                o.client_secret.as_ref()
            }),
            ("auth.oidc.name", "Example SSO", |o| o.name.as_ref()),
            ("auth.oidc.scopes", "openid profile email", |o| {
                o.scopes.as_ref()
            }),
            ("auth.oidc.default_role", "editor", |o| {
                o.default_role.as_ref()
            }),
        ];

        for (key, value, field) in cases {
            let mut cfg = GlobalConfig::default();
            apply(&mut cfg, key, value).unwrap();
            assert_eq!(
                cfg.auth_oidc().and_then(field).map(String::as_str),
                Some(value),
                "{key} should round-trip"
            );

            unset(&mut cfg, key).unwrap();
            assert!(
                cfg.auth.is_none(),
                "{key} was the only set field, so both blocks should vanish"
            );
            let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
            assert!(
                !yaml.contains("oidc"),
                "an emptied oidc block must not round-trip into the yaml: {yaml}"
            );
        }
    }

    /// Unsetting one key of several leaves the rest of the block standing:
    /// the collapse is about an emptied block, not about any unset.
    #[test]
    fn unsetting_one_oidc_key_keeps_the_rest_of_the_block() {
        let mut cfg = GlobalConfig::default();
        apply(
            &mut cfg,
            "auth.oidc.issuer",
            "https://login.example.com/v2.0",
        )
        .unwrap();
        apply(&mut cfg, "auth.oidc.client_id", "app-1234").unwrap();

        unset(&mut cfg, "auth.oidc.client_id").unwrap();
        assert_eq!(
            cfg.auth_oidc().and_then(|o| o.issuer.as_deref()),
            Some("https://login.example.com/v2.0")
        );
    }

    /// An absent block is SSO off, and that is what the accessor says.
    #[test]
    fn auth_oidc_is_absent_until_a_key_is_set() {
        let mut cfg = GlobalConfig::default();
        assert!(cfg.auth_oidc().is_none());
        apply(&mut cfg, "auth.mcp", "true").unwrap();
        assert!(
            cfg.auth_oidc().is_none(),
            "an auth block without oidc is still SSO off"
        );
    }

    /// `common`, `organizations` and `consumers` are the tenant-independent
    /// Entra endpoints. They discover fine and then fail as an issuer mismatch
    /// on the first token, so they are refused here with the same correction
    /// the template gets.
    #[test]
    fn oidc_issuer_refuses_the_tenant_independent_entra_endpoints() {
        for endpoint in ["common", "organizations", "consumers"] {
            let mut config = GlobalConfig::default();
            let err = apply(
                &mut config,
                "auth.oidc.issuer",
                &format!("https://login.microsoftonline.com/{endpoint}/v2.0"),
            )
            .unwrap_err();
            assert!(
                err.to_string().contains("tenant-specific"),
                "{endpoint}: {err}"
            );
            assert!(config.auth.is_none(), "{endpoint} must not be written");
            // Case and a trailing slash do not get it past the guard.
            let err = apply(
                &mut config,
                "auth.oidc.issuer",
                &format!(
                    "https://LOGIN.microsoftonline.com/{}/v2.0/",
                    endpoint.to_uppercase()
                ),
            )
            .unwrap_err();
            assert!(
                err.to_string().contains("tenant-specific"),
                "{endpoint}: {err}"
            );
        }
        // A tenant whose id merely starts with one of those words is a tenant.
        let mut config = GlobalConfig::default();
        apply(
            &mut config,
            "auth.oidc.issuer",
            "https://login.microsoftonline.com/commonwealth-corp/v2.0",
        )
        .unwrap();
    }

    #[test]
    fn oidc_issuer_refuses_the_entra_template() {
        let mut config = GlobalConfig::default();
        let err = apply(
            &mut config,
            "auth.oidc.issuer",
            "https://login.microsoftonline.com/{tenantid}/v2.0",
        )
        .unwrap_err();
        assert!(err.to_string().contains("tenant-specific"), "{err}");
        assert!(
            config.auth.is_none(),
            "a rejected issuer must not be written"
        );

        // The casing a portal page actually shows is refused too.
        let err = apply(
            &mut config,
            "auth.oidc.issuer",
            "https://login.microsoftonline.com/{tenantId}/v2.0",
        )
        .unwrap_err();
        assert!(err.to_string().contains("tenant-specific"), "{err}");

        // The tenant-specific url the message points at is accepted.
        apply(
            &mut config,
            "auth.oidc.issuer",
            "https://login.microsoftonline.com/9f1c-tenant/v2.0",
        )
        .unwrap();
    }

    #[test]
    fn oidc_client_secret_never_echoes() {
        let mut config = GlobalConfig::default();
        apply(&mut config, "auth.oidc.client_secret", "hunter2").unwrap();

        let view = snapshot(&config, &EnvOverlay::default())
            .into_iter()
            .find(|v| v.key == "auth.oidc.client_secret")
            .unwrap();
        assert!(!view.value.contains("hunter2"), "{}", view.value);
        assert_eq!(view.value, SECRET_DISPLAY);
        assert_eq!(view.source, SettingSource::Config);
        // The secret is still stored: only the display is redacted.
        assert_eq!(
            config.auth_oidc().and_then(|o| o.client_secret.as_deref()),
            Some("hunter2")
        );
    }

    /// An unset secret shows nothing at all, so `(set)` really does mean set.
    #[test]
    fn oidc_client_secret_shows_empty_when_unset() {
        let config = GlobalConfig::default();
        let view = snapshot(&config, &EnvOverlay::default())
            .into_iter()
            .find(|v| v.key == "auth.oidc.client_secret")
            .unwrap();
        assert_eq!(view.value, "");
        assert_eq!(view.source, SettingSource::Default);
    }

    /// A secret supplied by the environment is redacted on the same terms as
    /// one in the file, and the source still says where it came from.
    #[test]
    fn an_env_supplied_oidc_secret_is_redacted_too() {
        let overlay = EnvOverlay::from_vars([(
            "CRYSTALLINE_AUTH_OIDC_CLIENT_SECRET".to_string(),
            "env-hunter2".to_string(),
        )])
        .unwrap();
        let view = snapshot(&GlobalConfig::default(), &overlay)
            .into_iter()
            .find(|v| v.key == "auth.oidc.client_secret")
            .unwrap();
        assert_eq!(view.value, SECRET_DISPLAY);
        assert_eq!(view.source, SettingSource::Env);
    }

    #[test]
    fn oidc_default_role_accepts_the_three_roles_and_canonicalizes_casing() {
        let mut cfg = GlobalConfig::default();
        for (typed, stored) in [
            ("viewer", "viewer"),
            ("Editor", "editor"),
            ("ADMIN", "admin"),
        ] {
            apply(&mut cfg, "auth.oidc.default_role", typed).unwrap();
            assert_eq!(
                cfg.auth_oidc().and_then(|o| o.default_role.as_deref()),
                Some(stored)
            );
        }
    }

    // --- auth.oidc.redirect_uri -----------------------------------------------

    #[test]
    fn oidc_redirect_uri_round_trips_and_unsets_back_to_nothing() {
        let mut cfg = GlobalConfig::default();
        let configured = "https://kb.example.test/api/v1/auth/oidc/callback";
        apply(&mut cfg, "auth.oidc.redirect_uri", configured).unwrap();
        assert_eq!(
            cfg.auth_oidc().and_then(|o| o.redirect_uri.as_deref()),
            Some(configured)
        );
        assert_eq!(
            oidc_redirect_uri_effective(&cfg),
            (configured.to_string(), false)
        );
        unset(&mut cfg, "auth.oidc.redirect_uri").unwrap();
        assert!(cfg.auth.is_none(), "the emptied blocks go with it");
        assert_eq!(oidc_redirect_uri_effective(&cfg), (String::new(), true));
    }

    /// The address the provider sends the browser back to has to be one this
    /// instance actually serves the callback at, and one an identity may
    /// safely cross: https everywhere except a loopback development server.
    #[test]
    fn oidc_redirect_uri_is_validated_where_it_is_set() {
        for good in [
            "https://kb.example.test/api/v1/auth/oidc/callback",
            "https://kb.example.test:8443/api/v1/auth/oidc/callback",
            "http://localhost:8787/api/v1/auth/oidc/callback",
            "http://127.0.0.1:8787/api/v1/auth/oidc/callback",
        ] {
            let mut cfg = GlobalConfig::default();
            apply(&mut cfg, "auth.oidc.redirect_uri", good).unwrap_or_else(|e| {
                panic!("{good} should be accepted: {e}");
            });
            assert!(oidc_redirect_uri_problem(good).is_none());
        }

        let cases = [
            ("http://kb.example.test/api/v1/auth/oidc/callback", "https"),
            (
                "https://kb.example.test/callback",
                "/api/v1/auth/oidc/callback",
            ),
            ("/api/v1/auth/oidc/callback", "absolute"),
            ("not a url at all", "absolute"),
        ];
        for (bad, phrase) in cases {
            let mut cfg = GlobalConfig::default();
            let err = apply(&mut cfg, "auth.oidc.redirect_uri", bad)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains(phrase),
                "{bad} must be refused with teaching text naming '{phrase}', got: {err}"
            );
            assert!(cfg.auth.is_none(), "a rejected value is not written");
        }
    }

    #[test]
    fn oidc_redirect_uri_names_the_setting_in_every_refusal() {
        for bad in ["http://kb.example.test/api/v1/auth/oidc/callback", "junk"] {
            let problem = oidc_redirect_uri_problem(bad).unwrap();
            assert!(
                problem.contains("auth.oidc.redirect_uri"),
                "the refusal must name the key it is about: {problem}"
            );
        }
    }

    #[test]
    fn oidc_default_role_refuses_an_unknown_role() {
        let mut cfg = GlobalConfig::default();
        let err = apply(&mut cfg, "auth.oidc.default_role", "owner").unwrap_err();
        assert!(err.to_string().contains("viewer, editor or admin"), "{err}");
        assert!(cfg.auth.is_none(), "a rejected role must not be written");
    }

    /// Unset is how a key is turned off; an empty string would leave a
    /// present-but-blank value behind, so it is refused with that instruction.
    #[test]
    fn an_empty_oidc_value_is_refused_with_the_unset_instruction() {
        let mut cfg = GlobalConfig::default();
        for key in [
            "auth.oidc.issuer",
            "auth.oidc.client_id",
            "auth.oidc.client_secret",
            "auth.oidc.name",
            "auth.oidc.scopes",
            "auth.oidc.default_role",
        ] {
            let err = apply(&mut cfg, key, "   ").unwrap_err();
            assert!(err.to_string().contains("unset it instead"), "{key}: {err}");
        }
        assert!(cfg.auth.is_none());
    }

    #[test]
    fn oidc_scopes_normalize_to_one_space_separated_parameter() {
        let mut cfg = GlobalConfig::default();
        apply(&mut cfg, "auth.oidc.scopes", "  openid   profile\n email ").unwrap();
        assert_eq!(
            cfg.auth_oidc().and_then(|o| o.scopes.as_deref()),
            Some("openid profile email")
        );
    }

    /// Every oidc key is startup-effective: the relying party is built once
    /// when the HTTP surface comes up, so a change waits for the next start.
    #[test]
    fn every_oidc_key_is_startup_effective() {
        for spec in registry()
            .iter()
            .filter(|s| s.key.starts_with("auth.oidc."))
        {
            assert!(spec.startup_effective, "{}", spec.key);
        }
    }
}
