//! The sealed vault over Postgres: [`PgVault`] keeps the credential stores'
//! secrets in the `instance_secret` table of the database this instance
//! owns, through the owner's side channel.
//!
//! This is the first crate that names both the index (the side channel) and
//! remote (the [`SecretVault`] seam), so the implementation lives here. The
//! daemon installs it with [`crystalline_remote::vault::install`] after the
//! takeover and the start checks, on its runtime and never on the owner
//! thread; a call made on the owner thread is refused by the side channel
//! instead of waiting for itself.
//!
//! A CLI command that seals a secret while a daemon of its machine owns the
//! database reaches the same table on a connection of its own instead
//! ([`PgVault::beside_owner`], a short-lived second writer), with the same
//! statements.
//!
//! Every call waits at most [`VAULT_TIMEOUT`]. A save and a delete carry the
//! key id in their one statement, so a row sealed with another key is never
//! replaced or deleted, even by a writer that raced the check: such a row is
//! answered with [`RemoteError::VaultKeyMismatch`] and kept.

use crystalline_index::IndexError;
use crystalline_index::postgres::{SecretRow, SecretSummary, SecretTable, SideChannel};
use crystalline_remote::{RemoteError, SealedRow, SecretVault, VaultSummary};

pub use crystalline_index::postgres::VAULT_TIMEOUT;

/// The sealed secrets in the owner's database.
pub struct PgVault {
    side: Rows,
    /// The id of the key this process seals with: a delete removes only a
    /// row sealed with it.
    key_id: String,
}

impl PgVault {
    /// The vault over `side`, for a process that holds the key `key_id`.
    pub fn new(side: SideChannel, key_id: impl Into<String>) -> PgVault {
        PgVault {
            side: Rows::Side(side),
            key_id: key_id.into(),
        }
    }

    /// The vault over `table`, a connection of its own beside the instance
    /// that owns the database, for a CLI process that holds the key
    /// `key_id`.
    pub fn beside_owner(table: SecretTable, key_id: impl Into<String>) -> PgVault {
        PgVault {
            side: Rows::Table(table),
            key_id: key_id.into(),
        }
    }

    fn mismatch(name: &str, row_key_id: String, key_id: &str) -> RemoteError {
        RemoteError::VaultKeyMismatch {
            name: name.to_string(),
            row_key_id,
            key_id: key_id.to_string(),
        }
    }
}

/// Where the rows are reached: the owner's side channel, or a connection
/// of its own beside the owner.
enum Rows {
    Side(SideChannel),
    Table(SecretTable),
}

impl Rows {
    fn vault_load(&self, name: &str) -> Result<Option<SecretRow>, IndexError> {
        match self {
            Rows::Side(side) => side.vault_load(name),
            Rows::Table(table) => table.vault_load(name),
        }
    }

    fn vault_save(&self, name: &str, sealed: &[u8], key_id: &str) -> Result<bool, IndexError> {
        match self {
            Rows::Side(side) => side.vault_save(name, sealed, key_id),
            Rows::Table(table) => table.vault_save(name, sealed, key_id),
        }
    }

    fn vault_delete(&self, name: &str, key_id: &str) -> Result<bool, IndexError> {
        match self {
            Rows::Side(side) => side.vault_delete(name, key_id),
            Rows::Table(table) => table.vault_delete(name, key_id),
        }
    }

    fn vault_summary(&self) -> Result<SecretSummary, IndexError> {
        match self {
            Rows::Side(side) => side.vault_summary(),
            Rows::Table(table) => table.vault_summary(),
        }
    }
}

/// A database failure of a vault call, as the stores report it. Never
/// carries a secret: the side channel's errors name tables and statements.
fn failed(what: &str, name: Option<&str>, e: IndexError) -> RemoteError {
    let subject = match name {
        Some(name) => format!("the sealed secret {name}"),
        None => "the sealed secrets".to_string(),
    };
    RemoteError::Refused(format!("could not {what} {subject} in the database: {e}"))
}

fn sealed_row(row: SecretRow) -> SealedRow {
    SealedRow {
        sealed: row.sealed,
        key_id: row.key_id,
    }
}

impl SecretVault for PgVault {
    fn load(&self, name: &str) -> Result<Option<SealedRow>, RemoteError> {
        self.side
            .vault_load(name)
            .map(|row| row.map(sealed_row))
            .map_err(|e| failed("read", Some(name), e))
    }

    fn save(&self, name: &str, sealed: &[u8], key_id: &str) -> Result<(), RemoteError> {
        // Twice at most: a row of another key that went away between the
        // refused write and the look is written on the second turn.
        for _ in 0..2 {
            let written = self
                .side
                .vault_save(name, sealed, key_id)
                .map_err(|e| failed("save", Some(name), e))?;
            if written {
                return Ok(());
            }
            if let Some(row) = self.load(name)?
                && row.key_id != key_id
            {
                return Err(PgVault::mismatch(name, row.key_id, key_id));
            }
        }
        Err(RemoteError::Refused(format!(
            "could not save the sealed secret {name} in the database: it changed while it was saved"
        )))
    }

    fn delete(&self, name: &str) -> Result<bool, RemoteError> {
        let deleted = self
            .side
            .vault_delete(name, &self.key_id)
            .map_err(|e| failed("delete", Some(name), e))?;
        if deleted {
            return Ok(true);
        }
        match self.load(name)? {
            Some(row) if row.key_id != self.key_id => {
                Err(PgVault::mismatch(name, row.key_id, &self.key_id))
            }
            _ => Ok(false),
        }
    }

    fn summary(&self) -> Result<VaultSummary, RemoteError> {
        let summary = self
            .side
            .vault_summary()
            .map_err(|e| failed("count", None, e))?;
        Ok(VaultSummary {
            rows: usize::try_from(summary.rows).unwrap_or(usize::MAX),
            key_ids: summary.key_ids,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use chrono::Utc;
    use crystalline_index::postgres::{
        Acquired, OwnerIdentity, PgOwner, PgTarget, PostgresStore, ScratchSchema,
    };
    use crystalline_remote::{StoredToken, TokenIdentity, TokenStore, vault};

    use super::*;
    use crate::test_vault::{guard, key};

    fn identity() -> OwnerIdentity {
        OwnerIdentity {
            instance_id: "vvvvvvvv-0000-4000-8000-000000000000".to_string(),
            label: "vault-test".to_string(),
            command: None,
            version: "0.33.0-test".to_string(),
        }
    }

    /// Without the lock the side channel answers nothing, and a vault call
    /// says so as a store error at once rather than waiting. Nothing
    /// connects, so no database is needed.
    #[tokio::test]
    async fn pg_vault_without_the_lock_is_refused_at_once() {
        let owner = PgOwner::start(
            PgTarget {
                url: "postgres://u@127.0.0.1:1/none".to_string(),
                schema: Some("vault_engine_unit".to_string()),
            },
            identity(),
        )
        .unwrap();
        let pg = PgVault::new(owner.side(), key(1).key_id());
        let started = Instant::now();
        let answer = tokio::task::spawn_blocking(move || pg.load("github"))
            .await
            .unwrap();
        let Err(RemoteError::Refused(text)) = answer else {
            panic!("not refused: {answer:?}");
        };
        assert!(text.contains("the sealed secret github"), "{text}");
        assert!(text.contains("does not hold"), "{text}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// The credential stores seal into Postgres through an installed
    /// `PgVault`, and a vault holding another key never replaces or deletes
    /// a row: it answers `VaultKeyMismatch` and the row stays as it was.
    #[tokio::test]
    async fn pg_vault_seals_store_rows_and_refuses_another_key_id() {
        let Some(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL")
            .ok()
            .filter(|u| !u.is_empty())
        else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "pgvault");
        drop(
            PostgresStore::open_in_schema_with_readers(&url, scratch.name(), 0)
                .await
                .unwrap(),
        );
        let owner = PgOwner::start(
            PgTarget {
                url: url.clone(),
                schema: Some(scratch.name().to_string()),
            },
            identity(),
        )
        .unwrap();
        assert!(matches!(
            owner.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        let side = owner.side();
        let _guard = guard();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("origins");
        tokio::task::spawn_blocking(move || {
            let mine = Arc::new(PgVault::new(side.clone(), key(1).key_id()));
            vault::install(mine.clone(), key(1));
            let token = StoredToken {
                access_token: "ghp_sealed_secret".to_string(),
                host: "github.com".to_string(),
                user: "keeper".to_string(),
                created_at: Utc::now(),
                refresh_token: None,
                expires_at: None,
                refresh_expires_at: None,
            };
            let store =
                TokenStore::save_resolving_for(&TokenIdentity::Instance, None, &dir, &token)
                    .unwrap();
            assert_eq!(store.kind(), "vault");
            let row = mine.load("github").unwrap().expect("the row");
            assert_eq!(row.key_id, key(1).key_id());
            assert!(
                !row.sealed.windows(9).any(|w| w == b"ghp_seale"),
                "the row is sealed"
            );
            let (_, back) =
                TokenStore::resolve_and_load_for(&TokenIdentity::Instance, None, &dir).unwrap();
            assert_eq!(back.unwrap().access_token, "ghp_sealed_secret");

            let other = PgVault::new(side, key(2).key_id());
            for refused in [
                other.save("github", b"other", &key(2).key_id()),
                other.delete("github").map(|_| ()),
            ] {
                let Err(RemoteError::VaultKeyMismatch {
                    name,
                    row_key_id,
                    key_id,
                }) = refused
                else {
                    panic!("not a mismatch: {refused:?}");
                };
                assert_eq!(name, "github");
                assert_eq!(row_key_id, key(1).key_id());
                assert_eq!(key_id, key(2).key_id());
            }
            assert_eq!(mine.load("github").unwrap(), Some(row));
            assert!(!other.delete("github-app").unwrap(), "a missing row");
            assert_eq!(
                mine.summary().unwrap(),
                VaultSummary {
                    rows: 1,
                    key_ids: vec![key(1).key_id()],
                }
            );
            store.delete().unwrap();
            assert_eq!(mine.summary().unwrap().rows, 0);
            assert!(!dir.exists(), "nothing was written to the state folder");
        })
        .await
        .unwrap();
        owner.release().await.unwrap();
    }
}
