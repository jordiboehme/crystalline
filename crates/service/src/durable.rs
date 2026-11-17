//! The "ownership and durable state" report: who owns the database, where
//! the accounts live and what is left behind on this machine's disk (spec
//! section F). The daemon logs its one-line form at start; `status` and
//! `doctor` print the block, without a daemon from the owner row alone (a
//! report never opens the store, rule R6).
//!
//! It exists on Postgres only. On Turso there is no block.

use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use crystalline_core::config::{AccountsLocation, GlobalConfig};
use crystalline_index::postgres::OwnerRow;
use serde_json::{Value, json};

/// Where the accounts live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountsLocationKind {
    /// In the Postgres database.
    Postgres,
    /// In a `web-auth.db` file.
    File,
}

impl AccountsLocationKind {
    fn as_str(self) -> &'static str {
        match self {
            AccountsLocationKind::Postgres => "postgres",
            AccountsLocationKind::File => "file",
        }
    }
}

/// What a look at the owner found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerState {
    /// An instance holds the database and its row names it.
    Owned,
    /// An instance holds the database and has not announced itself yet.
    Unannounced,
    /// No instance holds the database.
    Free,
    /// The look failed or ran out of time.
    Unknown(String),
}

/// What a look at the configuration document found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentState {
    /// The document was saved, last at this time.
    Saved(DateTime<Utc>),
    /// No document is stored yet: the database runs on config.yaml and the
    /// defaults.
    NotSaved,
    /// The document could not be read.
    Unreadable(String),
}

/// Where the settings and the domain registry come from (spec section F).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigState {
    /// The document in the database.
    pub document: DocumentState,
    /// The config.yaml read as the read-only base, `None` when there is no
    /// such file.
    pub base_path: Option<PathBuf>,
    /// The domains config.yaml registers. They cannot be removed, purged or
    /// renamed while the configuration lives in the database.
    pub base_domains: Vec<String>,
}

impl ConfigState {
    /// The state for a base `config` read from `path`, with what the look
    /// at the document found. A path that is no file is no base.
    pub fn new(document: DocumentState, path: &Path, config: Option<&GlobalConfig>) -> Self {
        let base_path = path.is_file().then(|| path.to_path_buf());
        let base_domains = match (&base_path, config) {
            (Some(_), Some(config)) => config.domains.keys().cloned().collect(),
            _ => Vec::new(),
        };
        ConfigState {
            document,
            base_path,
            base_domains,
        }
    }

    fn document_words(&self) -> String {
        match &self.document {
            DocumentState::Saved(at) => format!("configuration document (updated {})", stamp(*at)),
            DocumentState::NotSaved => "configuration document (not saved yet)".to_string(),
            DocumentState::Unreadable(e) => format!("configuration document (not read: {e})"),
        }
    }

    fn base_words(&self) -> String {
        match &self.base_path {
            Some(path) => format!("config.yaml {} read as base", path.display()),
            None => "config.yaml none".to_string(),
        }
    }

    /// The info line about base domains, `None` when config.yaml registers
    /// none.
    pub fn base_domains_line(&self) -> Option<String> {
        let path = self.base_path.as_ref()?;
        (!self.base_domains.is_empty()).then(|| {
            format!(
                "config.yaml {} registers {} ({}): they cannot be removed, purged or renamed while the configuration lives in the database, so edit that file",
                path.display(),
                match self.base_domains.len() {
                    1 => "1 domain".to_string(),
                    n => format!("{n} domains"),
                },
                self.base_domains.join(", ")
            )
        })
    }
}

/// What a report says about the sealed secrets (spec section D): whether
/// this process has a key, the ids of the keys and how many rows the
/// database holds. Never a row name, and a missing key fails nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretsState {
    /// Whether `CRYSTALLINE_SECRET_KEY` or `CRYSTALLINE_SECRET_KEY_FILE` is
    /// set for this process. A report does not read the key.
    pub key_configured: bool,
    /// The ids of the keys the secrets are sealed with: the key of the
    /// instance that installed its vault, or for a report the ids the
    /// sealed rows record. An id, never the key.
    pub key_ids: Vec<String>,
    /// How many sealed secrets the database holds, `None` when not counted.
    pub sealed: Option<u64>,
    /// Why they could not be counted.
    pub error: Option<String>,
}

impl SecretsState {
    fn rows_words(&self) -> String {
        let keys = match self.key_ids.as_slice() {
            [] => String::new(),
            [one] => format!(" with key {one}"),
            many => format!(" with keys {}", many.join(", ")),
        };
        match (&self.sealed, &self.error) {
            (Some(n), _) => format!("{n} sealed in the database{keys}"),
            (None, Some(e)) => format!("not counted ({e})"),
            (None, None) => "not counted".to_string(),
        }
    }

    fn line(&self) -> String {
        let key = if self.key_configured {
            "a key is configured".to_string()
        } else {
            format!(
                "no key is configured here ({}), so a GitHub token changes only on the active instance",
                crystalline_remote::sealed::KEY_ENV
            )
        };
        format!("Secrets: {}; {key}", self.rows_words())
    }

    /// The clause of the start line: the key this instance seals with and
    /// how many rows the database holds.
    fn startup_words(&self) -> String {
        match (self.key_ids.as_slice(), self.sealed) {
            ([id], Some(n)) => format!("secrets sealed in postgres with key {id} ({})", rows(n)),
            ([id], None) => format!("secrets sealed in postgres with key {id}"),
            _ => format!("secrets {}", self.rows_words()),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "key_configured": self.key_configured,
            "key_ids": self.key_ids,
            "sealed": self.sealed,
            "error": self.error,
        })
    }
}

/// `n` rows, in words.
pub(crate) fn rows(n: u64) -> String {
    match n {
        1 => "1 row".to_string(),
        n => format!("{n} rows"),
    }
}

/// Where the GitHub App of an instance on Postgres comes from (spec section
/// F). There is no App file there: a saved App is sealed in the database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AppSource {
    /// The `CRYSTALLINE_GITHUB_APP_*` variables.
    Env,
    /// A row of the sealed vault, read by the instance that has the key.
    Vault,
    /// No App is configured.
    #[default]
    None,
    /// A report without the vault: a saved App would be sealed in the
    /// database, which the report does not read.
    NotChecked,
    /// The vault could not be read.
    Unread(String),
}

impl AppSource {
    fn as_str(&self) -> &'static str {
        match self {
            AppSource::Env => "env",
            AppSource::Vault => "vault",
            AppSource::None => "none",
            AppSource::NotChecked => "not_checked",
            AppSource::Unread(_) => "unread",
        }
    }

    fn words(&self) -> String {
        match self {
            AppSource::Env => "from the environment".to_string(),
            AppSource::Vault => "sealed in the database".to_string(),
            AppSource::None => "none".to_string(),
            AppSource::NotChecked => "sealed in the database (not checked)".to_string(),
            AppSource::Unread(e) => format!("not read ({e})"),
        }
    }

    /// The clause of the start line.
    fn startup_words(&self) -> String {
        match self {
            AppSource::Env => "GitHub App from env".to_string(),
            AppSource::Vault => "GitHub App from vault".to_string(),
            AppSource::None => "GitHub App none".to_string(),
            AppSource::NotChecked => "GitHub App not checked".to_string(),
            AppSource::Unread(e) => format!("GitHub App not read ({e})"),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "source": self.as_str(),
            "error": match self {
                AppSource::Unread(e) => Some(e.as_str()),
                _ => None,
            },
        })
    }
}

/// The ownership and durable state of one Postgres database.
#[derive(Debug, Clone)]
pub struct DurableStateReport {
    /// This process's role: `active`, `standby`, or `none` for a command
    /// that is no instance.
    pub role: String,
    /// The database for people to read: `host:port/db` and the schema, with
    /// no credentials.
    pub database: String,
    /// What the look at the owner found.
    pub owner_state: OwnerState,
    /// The owner row, when an instance holds the database and announced
    /// itself.
    pub owner: Option<OwnerRow>,
    /// Where the accounts live.
    pub accounts: AccountsLocationKind,
    /// Where the settings and the domain registry come from.
    pub config: ConfigState,
    /// The key and the sealed secrets.
    pub secrets: SecretsState,
    /// Where the GitHub App comes from.
    pub github_app: AppSource,
    /// One sentence per thing left behind or lost, in the order of the spec.
    pub warnings: Vec<String>,
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The sentence for an accounts file left in the state directory, `None`
/// when there is none.
pub(crate) fn leftover_accounts_warning(database: &str) -> Option<String> {
    let state_dir = crystalline_core::config::state_dir().ok()?;
    let leftover = state_dir.join("web-auth.db");
    leftover.exists().then(|| {
        format!(
            "the accounts file {} is not read: on Postgres the accounts live in the database {database}. Move it away once you no longer need it",
            leftover.display()
        )
    })
}

/// The sentence for credential files an earlier run on this disk left in
/// the state directory, `None` when there are none: GitHub token files and
/// the App file or marker under `origins/`, and the sign-in of each
/// connected server under `remote/`. Only their names are looked at; their
/// content is never read. On Postgres every secret is sealed in the
/// database, so none of these is read.
pub(crate) fn leftover_credentials_warning(database: &str) -> Option<String> {
    let state_dir = crystalline_core::config::state_dir().ok()?;
    leftover_credential_files(&state_dir).map(|files| {
        format!(
            "the credential files {} are not read while Postgres is configured: the GitHub tokens, a saved GitHub App and the server sign-ins are sealed in the database {database}. Link GitHub again and sign in to the servers again where needed, then move the files away",
            files.join(", ")
        )
    })
}

/// The credential files under `state_dir`, sorted, `None` when there are
/// none. Read from the directory entries alone.
fn leftover_credential_files(state_dir: &Path) -> Option<Vec<String>> {
    let mut found = Vec::new();
    let origins = state_dir.join("origins");
    for entry in std::fs::read_dir(&origins).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let credential = (name.starts_with("github-token") && name.ends_with(".json"))
            || name == "github-app.json"
            || name == "github-app.present";
        if credential && entry.path().is_file() {
            found.push(entry.path().display().to_string());
        }
    }
    let remote = state_dir.join("remote");
    for entry in std::fs::read_dir(&remote).into_iter().flatten().flatten() {
        let file = entry
            .path()
            .join(crystalline_remote::server_token::CREDENTIAL_FILE);
        if file.is_file() {
            found.push(file.display().to_string());
        }
    }
    found.sort();
    (!found.is_empty()).then_some(found)
}

impl DurableStateReport {
    /// The one info line a Postgres start logs.
    pub fn startup_line(&self) -> String {
        let owner = match &self.owner {
            Some(o) => format!(
                "owner of postgres {} since {}",
                self.database,
                stamp(o.since)
            ),
            None => format!("owner of postgres {}", self.database),
        };
        format!(
            "durable state: {owner}; accounts in {}; {}; {}; {}; {}",
            self.accounts.as_str(),
            self.config.document_words(),
            self.config.base_words(),
            self.secrets.startup_words(),
            self.github_app.startup_words()
        )
    }

    /// The block `status` and `doctor` print, one line each.
    pub fn block_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("Database: postgres {}", self.database),
            format!("Role: {}", self.role),
        ];
        lines.push(match (&self.owner_state, &self.owner) {
            (OwnerState::Owned, Some(o)) => format!(
                "Owner: {} ({}pid {}, version {}, since {})",
                o.label,
                o.command
                    .as_deref()
                    .map(|c| format!("running `{c}`, "))
                    .unwrap_or_default(),
                o.pid,
                o.version,
                stamp(o.since)
            ),
            (OwnerState::Owned | OwnerState::Unannounced, _) => {
                "Owner: an instance that has not announced itself yet".to_string()
            }
            (OwnerState::Free, _) => format!(
                "Owner: none. The database {} has no active instance. Start one with `crystalline serve`",
                self.database
            ),
            (OwnerState::Unknown(error), _) => format!(
                "Owner: could not read who owns the database {} ({error})",
                self.database
            ),
        });
        lines.push(format!("Accounts: in {}", self.accounts.as_str()));
        lines.push(format!(
            "Configuration: {}; {}",
            self.config.document_words(),
            self.config.base_words()
        ));
        lines.push(self.secrets.line());
        lines.push(format!("GitHub App: {}", self.github_app.words()));
        lines.extend(
            self.config
                .base_domains_line()
                .map(|l| format!("Note: {l}")),
        );
        lines.extend(self.warnings.iter().map(|w| format!("Warning: {w}")));
        lines
    }

    /// The block as JSON, for `status --json` and `doctor --json`.
    pub fn to_json(&self) -> Value {
        let owner = self.owner.as_ref().map(|o| {
            json!({
                "label": o.label,
                "instance_id": o.instance_id,
                "command": o.command,
                "pid": o.pid,
                "version": o.version,
                "since": stamp(o.since),
            })
        });
        let (state, error) = match &self.owner_state {
            OwnerState::Owned => ("owned", None),
            OwnerState::Unannounced => ("unannounced", None),
            OwnerState::Free => ("free", None),
            OwnerState::Unknown(e) => ("unknown", Some(e.as_str())),
        };
        let (document, updated, document_error) = match &self.config.document {
            DocumentState::Saved(at) => ("saved", Some(stamp(*at)), None),
            DocumentState::NotSaved => ("none", None, None),
            DocumentState::Unreadable(e) => ("unreadable", None, Some(e.as_str())),
        };
        json!({
            "role": self.role,
            "database": self.database,
            "owner_state": state,
            "owner_error": error,
            "owner": owner,
            "accounts": self.accounts.as_str(),
            "config": {
                "document": document,
                "document_updated": updated,
                "document_error": document_error,
                "base_path": self.config.base_path.as_ref().map(|p| p.display().to_string()),
                "base_domains": self.config.base_domains,
            },
            "secrets": self.secrets.to_json(),
            "github_app": self.github_app.to_json(),
            "warnings": self.warnings,
        })
    }
}

fn accounts_kind(cfg: &GlobalConfig) -> AccountsLocationKind {
    match crate::claim::accounts_location(cfg) {
        Ok(AccountsLocation::File(_)) | Err(_) => AccountsLocationKind::File,
        Ok(AccountsLocation::Postgres(_)) => AccountsLocationKind::Postgres,
    }
}

/// The report of a command that is no instance (`status`, `doctor`): the
/// owner is read without the lock and without opening the store. `None` on
/// Turso, where there is no block. `role` is what the caller knows of the
/// instance it talked to (`active`, `standby`) or `none`.
pub async fn durable_state_for_command(
    cfg: &GlobalConfig,
    role: &str,
    config_path: Option<&Path>,
) -> Option<DurableStateReport> {
    use crate::claim::Holder;
    let (database, owner_state, owner) = match crate::claim::holder(cfg).await {
        Holder::NotPostgres => return None,
        Holder::Free { database } => (database, OwnerState::Free, None),
        Holder::Here { database, owner } | Holder::Elsewhere { database, owner } => {
            match owner.filter(|o| o.released_at.is_none()) {
                Some(row) => (database, OwnerState::Owned, Some(row)),
                None => (database, OwnerState::Unannounced, None),
            }
        }
        Holder::Unknown { database, error } => (database, OwnerState::Unknown(error), None),
    };
    let config = config_state_for_command(cfg, config_path).await;
    let mut report = report_of(cfg, role, database, owner_state, owner, config);
    report.secrets = secrets_state_for_command(cfg).await;
    report.github_app = app_source_for_command(cfg);
    Some(report)
}

/// The sealed secrets as a command sees them: summarized on a lock-free
/// probe connection, which reads no name and no sealed byte, and whether
/// this process has a key. A count that fails is said in the state and
/// fails nothing.
#[cfg(feature = "postgres")]
async fn secrets_state_for_command(cfg: &GlobalConfig) -> SecretsState {
    let key_configured = crate::secret_start::key_variable_set();
    let summary = match crystalline_index::postgres::PgTarget::from_config(&cfg.database()) {
        Ok(target) => crystalline_index::postgres::summarize_sealed_secrets(&target)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    match summary {
        Ok(summary) => SecretsState {
            key_configured,
            key_ids: summary.key_ids,
            sealed: Some(summary.rows),
            error: None,
        },
        Err(e) => SecretsState {
            key_configured,
            key_ids: Vec::new(),
            sealed: None,
            error: Some(e),
        },
    }
}

#[cfg(not(feature = "postgres"))]
async fn secrets_state_for_command(_cfg: &GlobalConfig) -> SecretsState {
    SecretsState {
        key_configured: crate::secret_start::key_variable_set(),
        key_ids: Vec::new(),
        sealed: None,
        error: Some("this build has no Postgres backend".to_string()),
    }
}

/// Where the App comes from, as a command sees it: the environment, or
/// else not checked. A report has no vault, and the owner decision keeps a
/// process without the key off the vault, so a saved App is not looked up.
fn app_source_for_command(cfg: &GlobalConfig) -> AppSource {
    let api_url = cfg.github.as_ref().and_then(|g| g.api_url.as_deref());
    let env_app = crate::EnvOverlay::from_process_env()
        .ok()
        .and_then(|overlay| overlay.github_app(api_url));
    if env_app.is_some() {
        AppSource::Env
    } else {
        AppSource::NotChecked
    }
}

/// The configuration state a command sees: the document read on a
/// lock-free probe connection, config.yaml as the base. A document that
/// cannot be read is said in the state and fails nothing.
async fn config_state_for_command(cfg: &GlobalConfig, config_path: Option<&Path>) -> ConfigState {
    let document = read_document_state(cfg).await;
    match crate::overlay::load(config_path) {
        Ok(loaded) => ConfigState::new(document, &loaded.path, Some(&loaded.file)),
        Err(_) => ConfigState {
            document,
            base_path: None,
            base_domains: Vec::new(),
        },
    }
}

/// What the document of `cfg`'s database holds, read without the lock.
#[cfg(feature = "postgres")]
async fn read_document_state(cfg: &GlobalConfig) -> DocumentState {
    let target = match crystalline_index::postgres::PgTarget::from_config(&cfg.database()) {
        Ok(target) => target,
        Err(e) => return DocumentState::Unreadable(e.to_string()),
    };
    match crystalline_index::postgres::read_config_document(&target).await {
        Ok(Some(doc)) => DocumentState::Saved(doc.updated_at),
        Ok(None) => DocumentState::NotSaved,
        Err(e) => DocumentState::Unreadable(e.to_string()),
    }
}

#[cfg(not(feature = "postgres"))]
async fn read_document_state(_cfg: &GlobalConfig) -> DocumentState {
    DocumentState::Unreadable("this build has no Postgres backend".to_string())
}

/// The report of the instance that owns `database`, for its start: the
/// owner row it just wrote, read back, `document`, what the takeover read
/// of the configuration found (it is not read again), and `secrets`, what
/// the vault install found. A row that cannot be read leaves the line
/// without a time.
#[cfg(feature = "postgres")]
pub(crate) async fn durable_state_of_owner(
    loaded: &crate::overlay::LoadedConfig,
    database: &str,
    document: DocumentState,
    secrets: SecretsState,
) -> DurableStateReport {
    let cfg = &loaded.effective;
    let row = match crystalline_index::postgres::PgTarget::from_config(&cfg.database()) {
        Ok(target) => crystalline_index::postgres::read_owner_row(&target)
            .await
            .ok()
            .flatten(),
        Err(_) => None,
    };
    let state = if row.is_some() {
        OwnerState::Owned
    } else {
        OwnerState::Unannounced
    };
    let config = ConfigState::new(document, &loaded.path, loaded.base.as_ref());
    let mut report = report_of(cfg, "active", database.to_string(), state, row, config);
    report.secrets = secrets;
    report.github_app = app_source_of_owner(loaded).await;
    report
}

/// Where the App of the instance that installed its vault comes from: the
/// environment, else the vault, read on a blocking thread of the runtime
/// (never the owner thread). Only the source is said, never the row.
#[cfg(feature = "postgres")]
async fn app_source_of_owner(loaded: &crate::overlay::LoadedConfig) -> AppSource {
    let api_url = loaded
        .effective
        .github
        .as_ref()
        .and_then(|g| g.api_url.clone());
    if loaded.overlay.github_app(api_url.as_deref()).is_some() {
        return AppSource::Env;
    }
    // The folder is not read in vault mode: the lookup goes to the vault.
    let dir = crystalline_core::config::origins_state_dir().unwrap_or_default();
    let home = crate::engine::AppHome::new(api_url, dir, false);
    match tokio::task::spawn_blocking(move || home.lookup()).await {
        Ok(Ok(lookup)) if lookup.credentials.is_some() => AppSource::Vault,
        Ok(Ok(_)) => AppSource::None,
        Ok(Err(e)) => AppSource::Unread(e.to_string()),
        Err(e) => AppSource::Unread(e.to_string()),
    }
}

fn report_of(
    cfg: &GlobalConfig,
    role: &str,
    database: String,
    owner_state: OwnerState,
    owner: Option<OwnerRow>,
    config: ConfigState,
) -> DurableStateReport {
    let accounts = accounts_kind(cfg);
    let mut warnings = Vec::new();
    if accounts == AccountsLocationKind::Postgres {
        warnings.extend(leftover_accounts_warning(&database));
    }
    // On Postgres the secrets are sealed in the database whatever holds
    // the accounts, so a credential file on this disk is never read.
    warnings.extend(leftover_credentials_warning(&database));
    DurableStateReport {
        role: role.to_string(),
        database,
        owner_state,
        owner,
        accounts,
        config,
        secrets: SecretsState::default(),
        github_app: AppSource::default(),
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn owned() -> DurableStateReport {
        DurableStateReport {
            role: "active".to_string(),
            database: "db.internal:5432/c, schema team_a".to_string(),
            owner_state: OwnerState::Owned,
            owner: Some(OwnerRow {
                instance_id: "abc".to_string(),
                label: "prod-a".to_string(),
                command: None,
                pid: 42,
                backend_pid: 7,
                version: "0.33.0".to_string(),
                since: Utc.with_ymd_and_hms(2026, 11, 16, 9, 30, 0).unwrap(),
                released_at: None,
            }),
            accounts: AccountsLocationKind::Postgres,
            config: ConfigState {
                document: DocumentState::Saved(
                    Utc.with_ymd_and_hms(2026, 11, 17, 8, 0, 0).unwrap(),
                ),
                base_path: Some(PathBuf::from("/etc/crystalline/config.yaml")),
                base_domains: vec!["team".to_string(), "docs".to_string()],
            },
            secrets: SecretsState {
                key_configured: true,
                key_ids: vec!["0123456789abcdef".to_string()],
                sealed: Some(3),
                error: None,
            },
            github_app: AppSource::Vault,
            warnings: vec!["the accounts file /x is not read".to_string()],
        }
    }

    /// The start line names the owner, the schema, the time and where the
    /// accounts live, and never a credential.
    #[test]
    fn startup_line_names_owner_and_accounts() {
        assert_eq!(
            owned().startup_line(),
            "durable state: owner of postgres db.internal:5432/c, schema team_a since 2026-11-16T09:30:00Z; accounts in postgres; configuration document (updated 2026-11-17T08:00:00Z); config.yaml /etc/crystalline/config.yaml read as base; secrets sealed in postgres with key 0123456789abcdef (3 rows); GitHub App from vault"
        );
    }

    /// The secrets row names the key id and the number of rows, never a row
    /// name, and the App row names where the App comes from.
    #[test]
    fn the_secrets_and_app_rows_name_the_key_and_the_source() {
        let mut report = owned();
        let lines = report.block_lines();
        assert!(
            lines.iter().any(|l| l
                == "Secrets: 3 sealed in the database with key 0123456789abcdef; a key is configured"),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l == "GitHub App: sealed in the database"),
            "{lines:?}"
        );
        let json = report.to_json();
        assert_eq!(json["secrets"]["key_ids"][0], "0123456789abcdef");
        assert_eq!(json["secrets"]["sealed"], 3);
        assert_eq!(json["github_app"]["source"], "vault");

        report.github_app = AppSource::Env;
        assert!(report.startup_line().ends_with("; GitHub App from env"));
        report.github_app = AppSource::None;
        assert!(report.startup_line().ends_with("; GitHub App none"));
        report.github_app = AppSource::NotChecked;
        assert!(
            report
                .block_lines()
                .iter()
                .any(|l| l == "GitHub App: sealed in the database (not checked)")
        );
        assert_eq!(report.to_json()["github_app"]["source"], "not_checked");

        // A report reads the ids the rows record; with no rows there is none.
        report.secrets = SecretsState {
            key_configured: false,
            key_ids: Vec::new(),
            sealed: Some(0),
            error: None,
        };
        assert!(report.block_lines().iter().any(|l| {
            l.starts_with("Secrets: 0 sealed in the database; no key is configured here")
        }));
        report.secrets.key_ids = vec!["aaaa".to_string(), "bbbb".to_string()];
        report.secrets.sealed = Some(2);
        assert!(
            report
                .block_lines()
                .iter()
                .any(|l| l.starts_with("Secrets: 2 sealed in the database with keys aaaa, bbbb;"))
        );
    }

    /// Credential files an earlier run left are found by their names alone
    /// (the content is never read), and nothing else in those folders is
    /// named.
    #[test]
    fn leftover_credential_files_are_found_by_name() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(leftover_credential_files(dir.path()), None);
        let origins = dir.path().join("origins");
        std::fs::create_dir_all(origins.join("team")).unwrap();
        for name in [
            "github-token.json",
            "github-token-personal-ada.json",
            "github-app.json",
            "github-app.present",
            "state.json",
        ] {
            std::fs::write(origins.join(name), "not json at all").unwrap();
        }
        let host = dir.path().join("remote").join("kb.example");
        std::fs::create_dir_all(&host).unwrap();
        std::fs::write(host.join("credential.json"), "x").unwrap();
        std::fs::write(dir.path().join("remote").join("sources.json"), "x").unwrap();
        let found = leftover_credential_files(dir.path()).unwrap();
        let expected: Vec<String> = [
            origins.join("github-app.json"),
            origins.join("github-app.present"),
            origins.join("github-token-personal-ada.json"),
            origins.join("github-token.json"),
            host.join("credential.json"),
        ]
        .iter()
        .map(|p| p.display().to_string())
        .collect();
        assert_eq!(found, expected);
    }

    /// The line names the document and the base, says when there is no
    /// document or no base, and the base domains get their own sentence.
    #[test]
    fn startup_line_names_document_and_base() {
        let mut report = owned();
        assert!(report.startup_line().contains(
            "configuration document (updated 2026-11-17T08:00:00Z); config.yaml /etc/crystalline/config.yaml read as base"
        ));
        let note = report.config.base_domains_line().unwrap();
        assert!(note.contains("registers 2 domains (team, docs)"), "{note}");
        assert!(
            note.contains("cannot be removed, purged or renamed"),
            "{note}"
        );
        report.config = ConfigState {
            document: DocumentState::NotSaved,
            base_path: None,
            base_domains: Vec::new(),
        };
        assert!(
            report
                .startup_line()
                .contains("; configuration document (not saved yet); config.yaml none; ")
        );
        assert!(report.config.base_domains_line().is_none());
        report.config.document = DocumentState::Unreadable("timed out".to_string());
        assert!(
            report
                .startup_line()
                .contains("configuration document (not read: timed out)")
        );
        let json = report.to_json();
        assert_eq!(json["config"]["document"], "unreadable");
        assert_eq!(json["config"]["document_error"], "timed out");
        assert_eq!(owned().to_json()["config"]["base_domains"][1], "docs");
        let lines = owned().block_lines();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("Configuration: configuration document (updated"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("Note: config.yaml /etc"))
        );
    }

    /// The block carries the owner row and each warning.
    #[test]
    fn the_block_names_the_owner_and_the_warnings() {
        let lines = owned().block_lines();
        assert!(lines.iter().any(|l| l.starts_with("Owner: prod-a (pid 42")));
        assert!(lines.iter().any(|l| l == "Accounts: in postgres"));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("Warning: the accounts file"))
        );
        let json = owned().to_json();
        assert_eq!(json["owner"]["label"], "prod-a");
        assert_eq!(json["accounts"], "postgres");
    }
}
