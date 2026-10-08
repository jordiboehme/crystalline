//! The `CRYSTALLINE_*` variables that shape what a daemon serves, and what a
//! client says when the running daemon was started without one it sets.
//!
//! A daemon the Windows sign-in task starts runs with Task Scheduler's
//! environment, so a variable set only in a shell never reaches it. Outside
//! Claude Desktop's package the task stands in for a refused spawn only
//! when no shaping variable is set here (`instance::after_refused_breakaway`),
//! and `status` and `doctor` name a variable that explains a difference
//! between the running daemon and what this shell would have started.

use std::ffi::OsString;
use std::path::Path;

use serde::Serialize;

use crate::instance::{HolderFacts, HttpBinding};

/// Shaping variables read outside the settings registry. Every registry
/// key's variable is shaping too, see [`is_shaping`].
const SHAPING_NAMES: &[&str] = &[
    crate::overlay::CONFIG_PATH_ENV,
    crate::overlay::GITHUB_TOKEN_ENV,
    "CRYSTALLINE_MODELS_DIR",
    "CRYSTALLINE_HEARTBEAT_SECS",
    "CRYSTALLINE_STALE_SECS",
    "CRYSTALLINE_ADMIN_NAME",
    "CRYSTALLINE_ADMIN_NAME_FILE",
    "CRYSTALLINE_ADMIN_PASSWORD",
    "CRYSTALLINE_ADMIN_PASSWORD_FILE",
];

/// Every name under these prefixes is shaping: env-defined domains and the
/// connected server from the environment.
const SHAPING_PREFIXES: &[&str] = &[crate::overlay::DOMAIN_ENV_PREFIX, "CRYSTALLINE_REMOTE_"];

/// Variables only a client reads for itself: the install channel marker and
/// the Apple Silicon acceleration switch.
const CLIENT_ONLY: &[&str] = &[
    crate::overlay::CHANNEL_ENV,
    crystalline_index::ACCELERATION_ENV,
];

/// Test seams, never shaping.
const CLIENT_ONLY_PREFIX: &str = "CRYSTALLINE_TEST_";

/// Whether `name` is a variable only a client reads.
pub fn is_client_only(name: &str) -> bool {
    name.starts_with(CLIENT_ONLY_PREFIX) || CLIENT_ONLY.contains(&name)
}

/// Whether `name` changes what a daemon serves when it is set where the
/// daemon starts.
pub fn is_shaping(name: &str) -> bool {
    if is_client_only(name) {
        return false;
    }
    SHAPING_NAMES.contains(&name)
        || SHAPING_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        || crate::settings::registry()
            .iter()
            .any(|spec| spec.env_var() == name)
}

/// The shaping variables among `vars` that carry a value, by name, sorted
/// and each once. An empty value is unset, as everywhere in the overlay.
pub fn shaping_set_in(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Vec<String> {
    let mut names: Vec<String> = vars
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| is_shaping(name))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// [`shaping_set_in`] of this process's environment.
pub fn shaping_set_here() -> Vec<String> {
    shaping_set_in(std::env::vars_os())
}

/// `path` as both sides of the comparison spell it.
pub fn canonical_config_path(path: &Path) -> String {
    crystalline_engine::canonical_path_text(path)
}

/// What this client would have started: its config file (through the same
/// resolution a daemon uses, so a packaged client compares the real
/// Roaming folder and not the package's view), its HTTP binding and the
/// shaping variables set here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientView {
    pub config_path: String,
    pub http: HttpBinding,
    pub set: Vec<String>,
}

impl ClientView {
    pub fn of(loaded: &crate::overlay::LoadedConfig, set: Vec<String>) -> ClientView {
        ClientView {
            config_path: canonical_config_path(&loaded.path),
            http: match crate::daemon::resolve_http(None, &loaded.effective) {
                Some(addr) => HttpBinding::Bound(addr),
                None => HttpBinding::Off,
            },
            set,
        }
    }
}

/// One way the running daemon differs from what this client would have
/// started, and the variable set here that explains it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigMismatch {
    pub variable: String,
    /// What the daemon serves, as a person reads it.
    pub daemon: String,
    /// What this client's variable says.
    pub here: String,
}

impl ConfigMismatch {
    pub fn line(&self) -> String {
        format!(
            "The daemon serves {}, not {} from {}: a daemon the sign-in task started reads the system environment. Set {} as a user environment variable, or stop the daemon (`crystalline ctl shutdown`) and start it from this shell.",
            self.daemon, self.here, self.variable, self.variable
        )
    }
}

/// A canonical Windows path without its verbatim prefix.
fn without_verbatim(path: &str) -> &str {
    path.strip_prefix(r"\\?\").unwrap_or(path)
}

fn same_path(a: &str, b: &str, fold_case: bool) -> bool {
    let (a, b) = (without_verbatim(a), without_verbatim(b));
    if fold_case {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn describe_http(binding: &HttpBinding) -> String {
    match binding {
        HttpBinding::Bound(addr) => addr.clone(),
        _ => "no HTTP endpoint".to_string(),
    }
}

/// The differences between the holder and this client that a shaping
/// variable set here explains. A fact the holder did not give is unknown
/// and says nothing; a difference no variable here explains says nothing
/// either. The MCP session is never refused over any of it.
pub fn config_mismatches(facts: &HolderFacts, here: &ClientView) -> Vec<ConfigMismatch> {
    mismatches_with(facts, here, cfg!(windows))
}

pub(crate) fn mismatches_with(
    facts: &HolderFacts,
    here: &ClientView,
    fold_case: bool,
) -> Vec<ConfigMismatch> {
    let set = |name: &str| here.set.iter().any(|s| s == name);
    let mut found = Vec::new();
    if let Some(daemon) = facts.config_path.as_deref()
        && set(crate::overlay::CONFIG_PATH_ENV)
        && !same_path(daemon, &here.config_path, fold_case)
    {
        found.push(ConfigMismatch {
            variable: crate::overlay::CONFIG_PATH_ENV.to_string(),
            daemon: without_verbatim(daemon).to_string(),
            here: without_verbatim(&here.config_path).to_string(),
        });
    }
    let http_var = "CRYSTALLINE_SERVICE_HTTP";
    if let Some(daemon) = facts.http.as_ref()
        && *daemon != HttpBinding::Unrecorded
        && set(http_var)
        && *daemon != here.http
    {
        found.push(ConfigMismatch {
            variable: http_var.to_string(),
            daemon: describe_http(daemon),
            here: describe_http(&here.http),
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pairs: &[(&str, &str)]) -> Vec<String> {
        shaping_set_in(
            pairs
                .iter()
                .map(|(k, v)| (std::ffi::OsString::from(k), std::ffi::OsString::from(v))),
        )
    }

    /// A new settings key cannot be forgotten: its variable is shaping the
    /// moment it is in the registry.
    #[test]
    fn every_settings_key_variable_shapes_the_daemon() {
        for spec in crate::settings::registry() {
            assert!(
                is_shaping(&spec.env_var()),
                "{} is not shaping",
                spec.env_var()
            );
        }
    }

    /// Every variable the overlay skips as read elsewhere is either shaping
    /// or client only, so a new one has to be put in one of the two lists.
    #[test]
    fn every_reserved_variable_is_classified() {
        for name in crate::overlay::RESERVED_VARS {
            assert!(
                is_shaping(name) || is_client_only(name),
                "{name} is neither shaping nor client only: add it to SHAPING_NAMES or CLIENT_ONLY in shaping.rs"
            );
        }
    }

    #[test]
    fn the_shaping_set_names_what_a_daemon_reads_at_start() {
        for name in [
            "CRYSTALLINE_CONFIG",
            "CRYSTALLINE_DOMAINS_ROOT",
            "CRYSTALLINE_MODELS_DIR",
            "CRYSTALLINE_SERVICE_HTTP",
            "CRYSTALLINE_DATABASE_URL",
            "CRYSTALLINE_AUTH_ANONYMOUS",
            "CRYSTALLINE_GITHUB_ENABLED",
            "CRYSTALLINE_GITHUB_TOKEN",
            "CRYSTALLINE_DOMAIN_NOTES",
            "CRYSTALLINE_DOMAIN_NOTES_ORIGIN",
            "CRYSTALLINE_REMOTE_URL",
            "CRYSTALLINE_REMOTE_TOKEN",
            "CRYSTALLINE_ADMIN_PASSWORD_FILE",
            "CRYSTALLINE_HEARTBEAT_SECS",
        ] {
            assert_eq!(set(&[(name, "x")]), [name.to_string()], "{name}");
        }
        for name in [
            "CRYSTALLINE_TEST_DAEMON_TASK",
            "CRYSTALLINE_TEST_NO_KEYCHAIN",
            "CRYSTALLINE_CHANNEL",
            "CRYSTALLINE_ACCELERATION",
            "CRYSTALLINE_NOT_A_SETTING",
            "PATH",
        ] {
            assert!(set(&[(name, "x")]).is_empty(), "{name} is client only");
        }
        assert!(
            set(&[("CRYSTALLINE_SERVICE_HTTP", "")]).is_empty(),
            "an empty value is unset"
        );
        assert_eq!(
            set(&[
                ("CRYSTALLINE_SERVICE_HTTP", "false"),
                ("CRYSTALLINE_CONFIG", "/c.yaml"),
                ("CRYSTALLINE_SERVICE_HTTP", "false"),
            ]),
            [
                "CRYSTALLINE_CONFIG".to_string(),
                "CRYSTALLINE_SERVICE_HTTP".to_string()
            ],
            "sorted, each once"
        );
    }

    fn facts(config_path: Option<&str>, http: Option<HttpBinding>) -> HolderFacts {
        HolderFacts {
            pid: 1,
            version: crystalline_core::VERSION.to_string(),
            mcp_line_options: true,
            runs_in: None,
            start: None,
            state_dir: None,
            config_path: config_path.map(str::to_string),
            http,
        }
    }

    fn here(config_path: &str, http: HttpBinding, set: &[&str]) -> ClientView {
        ClientView {
            config_path: config_path.to_string(),
            http,
            set: set.iter().map(|s| s.to_string()).collect(),
        }
    }

    const LINE_HTTP: &str = "The daemon serves no HTTP endpoint, not 127.0.0.1:7499 from CRYSTALLINE_SERVICE_HTTP: a daemon the sign-in task started reads the system environment. Set CRYSTALLINE_SERVICE_HTTP as a user environment variable, or stop the daemon (`crystalline ctl shutdown`) and start it from this shell.";

    #[test]
    fn a_difference_a_variable_set_here_explains_is_one_line() {
        let found = mismatches_with(
            &facts(
                Some("/home/ada/.config/crystalline/config.yaml"),
                Some(HttpBinding::Off),
            ),
            &here(
                "/home/ada/.config/crystalline/config.yaml",
                HttpBinding::Bound("127.0.0.1:7499".to_string()),
                &["CRYSTALLINE_SERVICE_HTTP"],
            ),
            false,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].variable, "CRYSTALLINE_SERVICE_HTTP");
        assert_eq!(found[0].line(), LINE_HTTP);

        let config = mismatches_with(
            &facts(
                Some("/home/ada/.config/crystalline/config.yaml"),
                Some(HttpBinding::Bound("127.0.0.1:7411".to_string())),
            ),
            &here(
                "/work/team.yaml",
                HttpBinding::Bound("127.0.0.1:7411".to_string()),
                &["CRYSTALLINE_CONFIG"],
            ),
            false,
        );
        assert_eq!(
            config[0].line(),
            "The daemon serves /home/ada/.config/crystalline/config.yaml, not /work/team.yaml from CRYSTALLINE_CONFIG: a daemon the sign-in task started reads the system environment. Set CRYSTALLINE_CONFIG as a user environment variable, or stop the daemon (`crystalline ctl shutdown`) and start it from this shell."
        );
    }

    /// No false positives: a difference no variable set here explains, and a
    /// holder that did not say (a 0.24.0 daemon, or `unrecorded`), are
    /// silent.
    #[test]
    fn a_difference_nothing_here_explains_is_not_reported() {
        let http_here = HttpBinding::Bound("127.0.0.1:7499".to_string());
        let unexplained = here(
            "/a/config.yaml",
            http_here.clone(),
            &["CRYSTALLINE_DATABASE_URL"],
        );
        assert!(
            mismatches_with(
                &facts(Some("/b/config.yaml"), Some(HttpBinding::Off)),
                &unexplained,
                false
            )
            .is_empty()
        );
        let explained = here(
            "/a/config.yaml",
            http_here,
            &["CRYSTALLINE_CONFIG", "CRYSTALLINE_SERVICE_HTTP"],
        );
        assert!(
            mismatches_with(&facts(None, None), &explained, false).is_empty(),
            "a 0.24.0 daemon"
        );
        assert!(
            mismatches_with(
                &facts(None, Some(HttpBinding::Unrecorded)),
                &explained,
                false
            )
            .is_empty(),
            "a holder that recorded nothing"
        );
    }

    /// A packaged client with no shaping variable says nothing, however
    /// differently the two sides spell the config path: case, the verbatim
    /// prefix, the package's redirected Roaming folder.
    #[test]
    fn a_client_with_no_variable_set_says_nothing_whatever_the_spelling() {
        let daemon = facts(
            Some(r"\\?\C:\Users\Ada\AppData\Roaming\crystalline\config.yaml"),
            Some(HttpBinding::Bound("127.0.0.1:7411".to_string())),
        );
        for spelled in [
            r"C:\Users\Ada\AppData\Roaming\crystalline\config.yaml",
            r"c:\users\ada\appdata\roaming\crystalline\CONFIG.YAML",
            r"C:\Users\Ada\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\crystalline\config.yaml",
        ] {
            let client = here(spelled, HttpBinding::Off, &[]);
            assert!(
                mismatches_with(&daemon, &client, true).is_empty(),
                "{spelled}"
            );
            assert!(
                mismatches_with(&daemon, &client, false).is_empty(),
                "{spelled}"
            );
        }
    }

    /// `CRYSTALLINE_CONFIG` naming the same file in another spelling is no
    /// difference: the verbatim prefix always, the case on Windows.
    #[test]
    fn the_same_file_in_another_spelling_is_the_same_config() {
        let daemon = facts(
            Some(r"\\?\C:\Users\Ada\AppData\Roaming\crystalline\config.yaml"),
            Some(HttpBinding::Off),
        );
        let prefix_only = here(
            r"C:\Users\Ada\AppData\Roaming\crystalline\config.yaml",
            HttpBinding::Off,
            &["CRYSTALLINE_CONFIG"],
        );
        assert!(mismatches_with(&daemon, &prefix_only, false).is_empty());
        let case = here(
            r"c:\users\ada\appdata\roaming\crystalline\config.yaml",
            HttpBinding::Off,
            &["CRYSTALLINE_CONFIG"],
        );
        assert!(
            mismatches_with(&daemon, &case, true).is_empty(),
            "Windows folds case"
        );
        assert_eq!(
            mismatches_with(&daemon, &case, false).len(),
            1,
            "elsewhere case is a difference"
        );
        let shown = &mismatches_with(&daemon, &case, false)[0];
        assert!(
            !shown.daemon.starts_with(r"\\?\"),
            "shown without the verbatim prefix: {shown:?}"
        );
    }
}
