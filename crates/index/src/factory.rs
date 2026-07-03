//! The store factory: open the configured backend as a [`Store`] trait object.
//!
//! The engine and the CLI hold `Arc<Mutex<dyn Store>>` rather than a concrete
//! type, so which backend they talk to is a runtime decision driven by the
//! `database` config block. This module is the single place that decision is
//! made. The Turso arm reproduces the historical open behaviour exactly (the
//! `--db` override, the default `index.db` path, the corruption-recovery
//! reopen); the Postgres arm opens an sqlx pool at `database.url` and migrates,
//! behind the `postgres` cargo feature.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crystalline_core::config::{self, DatabaseBackend, DatabaseConfig};
use tokio::sync::Mutex;

use crate::error::{IndexError, Result};
use crate::store::Store;
use crate::turso::TursoStore;

/// Open (creating if needed) the storage backend named by `cfg` as a boxed
/// [`Store`], behind the same `tokio::sync::Mutex` the engine serializes all
/// access through.
///
/// - `db_override` is the `--db` flag: when present it wins for the Turso
///   backend, ahead of a `database.url` file-path override and the default
///   `index.db` path. It is ignored by the Postgres backend, whose location
///   comes from `database.url`.
/// - `resilient` selects the corruption-recovery open for the Turso backend
///   (discard an unreadable database file and start fresh); it is the
///   `reindex --full` recovery path and has no effect on Postgres.
///
/// The backend/url combination is validated here (never at config parse time),
/// so `verify` and `prompt` never trip on a database block they do not use.
pub async fn open_store(
    cfg: &DatabaseConfig,
    db_override: Option<&Path>,
    resilient: bool,
) -> Result<Arc<Mutex<dyn Store>>> {
    cfg.validate()
        .map_err(|e| IndexError::Invalid(e.to_string()))?;
    match cfg.backend {
        DatabaseBackend::Turso => {
            let path = turso_path(cfg, db_override)?;
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent).map_err(|source| IndexError::Io {
                    path: parent.display().to_string(),
                    source,
                })?;
            }
            let store = if resilient {
                TursoStore::open_resilient(&path).await?
            } else {
                TursoStore::open(&path).await?
            };
            // Unsize the concrete store into the trait object the callers hold.
            let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
            Ok(store)
        }
        // Boxed so the caller's future is not sized for the sqlx open and
        // migration machinery, whose inlined debug-build state machine is
        // large enough to overflow the smaller default stacks on Windows
        // main threads and tokio workers.
        DatabaseBackend::Postgres => Box::pin(open_postgres(cfg, resilient)).await,
    }
}

/// Open the Postgres backend. `validate()` already guaranteed a good `url`, so
/// the pool open is the only failure point here. `resilient` is a no-op for
/// Postgres: corruption recovery is a Turso file concern, so the branch is kept
/// for a uniform signature but ignored.
#[cfg(feature = "postgres")]
async fn open_postgres(cfg: &DatabaseConfig, resilient: bool) -> Result<Arc<Mutex<dyn Store>>> {
    let _ = resilient;
    let url = cfg
        .url
        .as_deref()
        .ok_or_else(|| IndexError::Invalid("the postgres backend requires a url".to_string()))?;
    let store = crate::postgres::PostgresStore::open(url).await?;
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    Ok(store)
}

/// Without the `postgres` feature the backend code is not compiled in, so a
/// `backend: postgres` config on a Turso-only build is an actionable error
/// rather than a silent fallback.
#[cfg(not(feature = "postgres"))]
async fn open_postgres(_cfg: &DatabaseConfig, _resilient: bool) -> Result<Arc<Mutex<dyn Store>>> {
    Err(IndexError::Unsupported(
        "this build has no postgres backend; rebuild with the postgres feature \
         or use backend: turso"
            .to_string(),
    ))
}

/// Resolve the Turso database file path: the `--db` override first, then a
/// `database.url` file-path override, then the default `index.db` path.
fn turso_path(cfg: &DatabaseConfig, db_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = db_override {
        return Ok(p.to_path_buf());
    }
    if let Some(url) = cfg.url.as_deref().filter(|u| !u.is_empty()) {
        return Ok(config::expand_tilde(url));
    }
    config::index_db_path().map_err(|e| IndexError::Invalid(e.to_string()))
}
