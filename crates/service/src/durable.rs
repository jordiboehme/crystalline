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
/// this process has a key and how many rows the database holds. Never a row
/// name, and a missing key fails nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretsState {
    /// Whether `CRYSTALLINE_SECRET_KEY` or `CRYSTALLINE_SECRET_KEY_FILE` is
    /// set for this process. The key is not read.
    pub key_configured: bool,
    /// How many sealed secrets the database holds, `None` when not counted.
    pub sealed: Option<u64>,
    /// Why they could not be counted.
    pub error: Option<String>,
}

impl SecretsState {
    fn line(&self) -> String {
        let rows = match (&self.sealed, &self.error) {
            (Some(n), _) => format!("{n} sealed in the database"),
            (None, Some(e)) => format!("not counted ({e})"),
            (None, None) => "not counted".to_string(),
        };
        let key = if self.key_configured {
            "a key is configured".to_string()
        } else {
            format!(
                "no key is configured here ({}), so a GitHub token changes only on the active instance",
                crystalline_remote::sealed::KEY_ENV
            )
        };
        format!("Secrets: {rows}; {key}")
    }

    fn to_json(&self) -> Value {
        json!({
            "key_configured": self.key_configured,
            "sealed": self.sealed,
            "error": self.error,
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
            "durable state: {owner}; accounts in {}; {}; {}",
            self.accounts.as_str(),
            self.config.document_words(),
            self.config.base_words()
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
    Some(report)
}

/// The sealed secrets as a command sees them: counted on a lock-free probe
/// connection, which reads no row and no name, and whether this process has
/// a key. A count that fails is said in the state and fails nothing.
#[cfg(feature = "postgres")]
async fn secrets_state_for_command(cfg: &GlobalConfig) -> SecretsState {
    let key_configured = crate::secret_start::key_variable_set();
    let counted = match crystalline_index::postgres::PgTarget::from_config(&cfg.database()) {
        Ok(target) => crystalline_index::postgres::count_sealed_secrets(&target)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let (sealed, error) = match counted {
        Ok(n) => (Some(n), None),
        Err(e) => (None, Some(e)),
    };
    SecretsState {
        key_configured,
        sealed,
        error,
    }
}

#[cfg(not(feature = "postgres"))]
async fn secrets_state_for_command(_cfg: &GlobalConfig) -> SecretsState {
    SecretsState {
        key_configured: crate::secret_start::key_variable_set(),
        sealed: None,
        error: Some("this build has no Postgres backend".to_string()),
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
/// owner row it just wrote, read back, and `document`, what the takeover read
/// of the configuration found (it is not read again). A row that cannot be
/// read leaves the line without a time.
#[cfg(feature = "postgres")]
pub(crate) async fn durable_state_of_owner(
    loaded: &crate::overlay::LoadedConfig,
    database: &str,
    document: DocumentState,
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
    report_of(cfg, "active", database.to_string(), state, row, config)
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
    DurableStateReport {
        role: role.to_string(),
        database,
        owner_state,
        owner,
        accounts,
        config,
        secrets: SecretsState::default(),
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
            secrets: SecretsState::default(),
            warnings: vec!["the accounts file /x is not read".to_string()],
        }
    }

    /// The start line names the owner, the schema, the time and where the
    /// accounts live, and never a credential.
    #[test]
    fn startup_line_names_owner_and_accounts() {
        assert_eq!(
            owned().startup_line(),
            "durable state: owner of postgres db.internal:5432/c, schema team_a since 2026-11-16T09:30:00Z; accounts in postgres; configuration document (updated 2026-11-17T08:00:00Z); config.yaml /etc/crystalline/config.yaml read as base"
        );
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
                .ends_with("configuration document (not saved yet); config.yaml none")
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
