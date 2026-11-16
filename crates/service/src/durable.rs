//! The "ownership and durable state" report: who owns the database, where
//! the accounts live and what is left behind on this machine's disk (spec
//! section F). The daemon logs its one-line form at start; `status` and
//! `doctor` print the block, without a daemon from the owner row alone (a
//! report never opens the store, rule R6).
//!
//! It exists on Postgres only. On Turso there is no block.

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
            "durable state: {owner}; accounts in {}",
            self.accounts.as_str()
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
        json!({
            "role": self.role,
            "database": self.database,
            "owner_state": state,
            "owner_error": error,
            "owner": owner,
            "accounts": self.accounts.as_str(),
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
    Some(report_of(cfg, role, database, owner_state, owner))
}

/// The report of the instance that owns `database`, for its start: the
/// owner row it just wrote, read back. A row that cannot be read leaves the
/// line without a time.
#[cfg(feature = "postgres")]
pub(crate) async fn durable_state_of_owner(
    cfg: &GlobalConfig,
    database: &str,
) -> DurableStateReport {
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
    report_of(cfg, "active", database.to_string(), state, row)
}

fn report_of(
    cfg: &GlobalConfig,
    role: &str,
    database: String,
    owner_state: OwnerState,
    owner: Option<OwnerRow>,
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
            warnings: vec!["the accounts file /x is not read".to_string()],
        }
    }

    /// The start line names the owner, the schema, the time and where the
    /// accounts live, and never a credential.
    #[test]
    fn startup_line_names_owner_and_accounts() {
        assert_eq!(
            owned().startup_line(),
            "durable state: owner of postgres db.internal:5432/c, schema team_a since 2026-11-16T09:30:00Z; accounts in postgres"
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
