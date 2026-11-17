//! The secret key at the start of `serve` (spec D, owner decision of
//! 2026-11-16).
//!
//! The backend decides where secrets live, never the key. On Postgres the
//! GitHub tokens, a saved GitHub App and the server sign-ins are sealed in
//! the database, so the key is required: a start without it stops before it
//! takes the lock, and nothing falls back to the keychain or the state
//! folder. On Turso the key is ignored, with one warning.
//!
//! After the takeover and the migrations, [`open_vault`] compares the key
//! with the rows the database holds: rows sealed with another key stop the
//! start and are kept. Only then is the vault installed, before the engine
//! is built and before any credential is read.

use crystalline_core::config::DatabaseBackend;
#[cfg(feature = "postgres")]
use crystalline_remote::VaultSummary;
use crystalline_remote::sealed::{KEY_ENV, KEY_FILE_ENV, KeyError, SealKey};
use crystalline_remote::vault;

/// Why a start on Postgres without a key stops.
pub(crate) fn missing_key_sentence() -> String {
    format!(
        "this instance keeps its index in Postgres, and there its GitHub tokens, a saved GitHub App and its server sign-ins are sealed in the database with a secret key. Set {KEY_ENV} (or {KEY_FILE_ENV}) to a 32 byte key, for example one made with `openssl rand -base64 32`. Keep the key safe: the sealed secrets cannot be read without it"
    )
}

/// A session or start refused for its secret key: missing on Postgres, or
/// not one that parses. Typed so the embedded MCP stack can send the
/// sentence as the initialize error, as it does for an owned database,
/// instead of serving a status server that cannot help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyRefusal(pub(crate) String);

impl std::fmt::Display for KeyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KeyRefusal {}

/// The one warning of a start on Turso with a key set.
pub(crate) fn turso_ignores_key_sentence() -> String {
    format!(
        "{KEY_ENV} or {KEY_FILE_ENV} is set, but this instance keeps its index in a local file. The key is ignored: secrets stay in the keychain or the state folder as before"
    )
}

/// What the start does with the secret key: on Postgres the key to seal
/// with (read only there, through `read`), or the error that stops the
/// start; on Turso nothing, with a warning when either variable is set
/// (`set`): the key, or the file it names, is never read there.
pub(crate) fn plan(
    backend: DatabaseBackend,
    set: bool,
    read: impl FnOnce() -> Result<Option<SealKey>, KeyError>,
) -> anyhow::Result<(Option<SealKey>, Option<String>)> {
    match backend {
        DatabaseBackend::Postgres => match read() {
            Ok(Some(key)) => Ok((Some(key), None)),
            Ok(None) => Err(anyhow::Error::new(KeyRefusal(missing_key_sentence()))),
            Err(e) => Err(anyhow::Error::new(KeyRefusal(e.to_string()))),
        },
        DatabaseBackend::Turso => Ok((None, set.then(turso_ignores_key_sentence))),
    }
}

/// The variable this process's key comes from, for a sentence about it:
/// [`KEY_FILE_ENV`] when that one is set (not blank), else [`KEY_ENV`].
#[cfg(feature = "postgres")]
pub(crate) fn key_variable_in_use() -> &'static str {
    let set = |var: &str| {
        std::env::var_os(var).is_some_and(|v| v.to_str().is_none_or(|s| !s.trim().is_empty()))
    };
    if set(KEY_FILE_ENV) {
        KEY_FILE_ENV
    } else {
        KEY_ENV
    }
}

/// Whether either key variable is set to something other than blank, the
/// way [`SealKey::from_env`] tells set from unset, without reading a file.
pub(crate) fn key_variable_set() -> bool {
    [KEY_ENV, KEY_FILE_ENV].iter().any(|var| {
        std::env::var_os(var).is_some_and(|v| v.to_str().is_none_or(|s| !s.trim().is_empty()))
    })
}

/// Read the key for `backend` and act on it: on Postgres this process keeps
/// its secrets in the vault from now on ([`vault::require`]), so no store
/// falls back to the keychain or a file before the vault is installed. The
/// key comes back for [`open_vault`].
pub(crate) fn read_for_start(backend: DatabaseBackend) -> anyhow::Result<Option<SealKey>> {
    let (key, warning) = plan(backend, key_variable_set(), SealKey::from_env)?;
    if let Some(line) = warning {
        tracing::warn!("{line}");
    }
    if key.is_some() {
        vault::require();
    }
    Ok(key)
}

/// Whether the rows the vault holds were all sealed with `key_id`. A row
/// sealed with another key stops the start; the rows are never deleted.
#[cfg(feature = "postgres")]
pub(crate) fn check_key_ids(
    summary: &VaultSummary,
    key_id: &str,
    database: &str,
) -> anyhow::Result<()> {
    let others: Vec<&str> = summary
        .key_ids
        .iter()
        .map(String::as_str)
        .filter(|id| *id != key_id)
        .collect();
    if others.is_empty() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "the database {database} holds {} sealed secrets, and some are sealed with key {}, but {var} is key {key_id}. The secrets are kept as they are. Set {var} to the key they were sealed with",
        summary.rows,
        others.join(", "),
        var = key_variable_in_use()
    ))
}

/// Check the key against the sealed rows of the database `pg` owns and
/// install the vault over its side channel. Called on the runtime after the
/// store open migrated the database, and before the engine is built, so
/// every credential this daemon reads comes from the vault. Never on the
/// owner thread: the summary runs on a blocking thread of the runtime.
#[cfg(feature = "postgres")]
pub(crate) async fn open_vault(
    pg: &crate::standby::PgStart,
    key: SealKey,
) -> anyhow::Result<VaultSummary> {
    open_vault_over(pg.side(), pg.database(), key).await
}

/// [`open_vault`] over the side channel of any owner of `database`: the
/// daemon's, or a command's that claimed the database
/// ([`crate::claim::CommandDbOwner`]), which then seals and opens its secrets
/// as the daemon does.
#[cfg(feature = "postgres")]
pub(crate) async fn open_vault_over(
    side: crystalline_index::postgres::SideChannel,
    database: &str,
    key: SealKey,
) -> anyhow::Result<VaultSummary> {
    let pg_vault = crystalline_engine::vault::PgVault::new(side, key.key_id());
    install_checked(pg_vault, database, key).await
}

/// [`open_vault`] for a CLI command that seals a secret itself while a
/// daemon of its machine owns `database`: the vault reaches the rows on a
/// connection of its own, beside the owner, after the same key check, so a
/// key other than the rows' is refused before anything is written.
#[cfg(feature = "postgres")]
pub(crate) async fn open_vault_beside_owner(
    target: crystalline_index::postgres::PgTarget,
    database: &str,
    key: SealKey,
) -> anyhow::Result<VaultSummary> {
    let tag = crystalline_core::config::read_or_create_instance_id().ok();
    let table = crystalline_index::postgres::SecretTable::new(target, tag);
    let pg_vault = crystalline_engine::vault::PgVault::beside_owner(table, key.key_id());
    install_checked(pg_vault, database, key).await
}

/// Check `key` against the rows `pg_vault` reaches and install it.
#[cfg(feature = "postgres")]
async fn install_checked(
    pg_vault: crystalline_engine::vault::PgVault,
    database: &str,
    key: SealKey,
) -> anyhow::Result<VaultSummary> {
    use std::sync::Arc;

    use crystalline_remote::SecretVault;

    let key_id = key.key_id();
    let pg_vault = Arc::new(pg_vault);
    let reading = pg_vault.clone();
    let summary = tokio::task::spawn_blocking(move || reading.summary())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .map_err(|e| {
            anyhow::anyhow!(
                "could not read the sealed secrets in the database {database} ({e}); on Postgres the secrets live in the database, so nothing goes on without them"
            )
        })?;
    check_key_ids(&summary, &key_id, database)?;
    vault::install(pg_vault, key);
    tracing::info!(
        "secrets sealed in postgres with key {key_id} ({})",
        crate::durable::rows(summary.rows as u64)
    );
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SealKey {
        SealKey::from_bytes([7; 32])
    }

    /// The owner decision: on Postgres a start without the key stops, with
    /// a sentence naming the variable and how to make a key. Nothing falls
    /// back to the keychain or a file.
    #[test]
    fn postgres_without_key_fails_start_naming_the_variable() {
        let err = plan(DatabaseBackend::Postgres, false, || Ok(None)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("CRYSTALLINE_SECRET_KEY"), "{text}");
        assert!(text.contains("CRYSTALLINE_SECRET_KEY_FILE"), "{text}");
        assert!(text.contains("openssl rand -base64 32"), "{text}");
        // A key that does not parse stops the start too, naming its variable.
        let bad = plan(DatabaseBackend::Postgres, true, || {
            Err(KeyError::Malformed { var: KEY_FILE_ENV })
        })
        .unwrap_err();
        assert!(bad.to_string().contains(KEY_FILE_ENV), "{bad}");
        let (key, warning) = plan(DatabaseBackend::Postgres, true, || Ok(Some(key()))).unwrap();
        assert!(key.is_some() && warning.is_none());
    }

    /// On Turso the key is ignored with one warning when a variable is set,
    /// and it is never read (not even the file); without one there is no
    /// warning.
    #[test]
    fn turso_warns_and_ignores_secret_key() {
        let never = || -> Result<Option<SealKey>, KeyError> { panic!("the key was read on Turso") };
        let (key, warning) = plan(DatabaseBackend::Turso, true, never).unwrap();
        assert!(key.is_none(), "the key is not used on Turso");
        let warning = warning.expect("one warning");
        assert!(warning.contains("CRYSTALLINE_SECRET_KEY"), "{warning}");
        assert!(warning.contains("ignored"), "{warning}");
        let (key, warning) = plan(DatabaseBackend::Turso, false, never).unwrap();
        assert!(key.is_none() && warning.is_none());
    }

    /// Rows sealed with another key stop the start, naming both key ids;
    /// rows of this key alone, or none, pass.
    #[cfg(feature = "postgres")]
    #[test]
    fn key_id_mismatch_names_both_ids() {
        let mine = key().key_id();
        let summary = |ids: &[&str]| VaultSummary {
            rows: 3,
            key_ids: ids.iter().map(|s| s.to_string()).collect(),
        };
        check_key_ids(&VaultSummary::default(), &mine, "db").unwrap();
        check_key_ids(&summary(&[&mine]), &mine, "db").unwrap();
        let err = check_key_ids(&summary(&["0123456789abcdef", &mine]), &mine, "db")
            .unwrap_err()
            .to_string();
        assert!(err.contains("0123456789abcdef"), "{err}");
        assert!(err.contains(&mine), "{err}");
        assert!(err.contains("kept"), "{err}");
    }
}
