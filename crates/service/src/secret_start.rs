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
            Ok(None) => Err(anyhow::anyhow!(missing_key_sentence())),
            Err(e) => Err(anyhow::anyhow!("{e}")),
        },
        DatabaseBackend::Turso => Ok((None, set.then(turso_ignores_key_sentence))),
    }
}

/// Whether either key variable is set to something other than blank, the
/// way [`SealKey::from_env`] tells set from unset, without reading a file.
fn key_variable_set() -> bool {
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
        "the database {database} holds {} sealed secrets, and some are sealed with key {}, but {KEY_ENV} is key {key_id}. The secrets are kept as they are. Set {KEY_ENV} to the key they were sealed with",
        summary.rows,
        others.join(", ")
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
    use std::sync::Arc;

    use crystalline_remote::SecretVault;

    let key_id = key.key_id();
    let pg_vault = Arc::new(crystalline_engine::vault::PgVault::new(pg.side(), &key_id));
    let reading = pg_vault.clone();
    let summary = tokio::task::spawn_blocking(move || reading.summary())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .map_err(|e| {
            anyhow::anyhow!(
                "could not read the sealed secrets in the database {} ({e}); on Postgres the secrets live in the database, so this instance does not start without them",
                pg.database()
            )
        })?;
    check_key_ids(&summary, &key_id, pg.database())?;
    vault::install(pg_vault, key);
    tracing::info!(
        "secrets sealed in postgres with key {key_id} ({} rows)",
        summary.rows
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
