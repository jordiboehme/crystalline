//! The executor seam of the auth store.
//!
//! Every method of [`super::AuthStore`] speaks to its database through
//! [`AuthDb`], with one SQL text and `?N` placeholders. Today the only arm is
//! Turso (a local file); a Postgres arm runs the same text against a pool.
//! Nothing outside this file touches a connection.

use anyhow::{Context, Result};
use turso::{Connection, Database, Row, Value};

/// One row of a result, read cell by cell.
pub(super) struct AuthRow {
    row: Row,
}

impl AuthRow {
    /// The value of column `i`.
    pub(super) fn get_value(&self, i: usize) -> Result<Value> {
        Ok(self.row.get_value(i)?)
    }
}

/// The database behind an auth store.
pub(super) enum AuthDb {
    Turso {
        // Retained so the connection stays valid for as long as the store does.
        _db: Database,
        conn: Connection,
        /// How many `BEGIN IMMEDIATE` this store has run, so a test can pin
        /// that a method runs in a transaction.
        #[cfg(test)]
        begins: std::sync::atomic::AtomicUsize,
    },
}

impl AuthDb {
    /// Wrap an opened, migrated Turso database and its connection.
    pub(super) fn turso(db: Database, conn: Connection) -> AuthDb {
        AuthDb::Turso {
            _db: db,
            conn,
            #[cfg(test)]
            begins: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// How many transactions this store has begun.
    #[cfg(test)]
    pub(super) fn begins(&self) -> usize {
        match self {
            AuthDb::Turso { begins, .. } => begins.load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Run one statement and give back the number of rows it changed.
    pub(super) async fn execute(&self, sql: &str, p: Vec<Value>) -> Result<u64> {
        match self {
            AuthDb::Turso { conn, .. } => Ok(conn.execute(sql, p).await?),
        }
    }

    /// Run a query and give back every row. The cursor is drained so the
    /// statement finishes instead of rolling back when it drops.
    pub(super) async fn query(&self, sql: &str, p: Vec<Value>) -> Result<Vec<AuthRow>> {
        match self {
            AuthDb::Turso { conn, .. } => {
                let mut rows = conn.query(sql, p).await?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().await? {
                    out.push(AuthRow { row });
                }
                Ok(out)
            }
        }
    }

    /// First row of a query, if any.
    pub(super) async fn query_first(&self, sql: &str, p: Vec<Value>) -> Result<Option<AuthRow>> {
        Ok(self
            .query(sql, p)
            .await
            .context("querying the auth database")?
            .into_iter()
            .next())
    }

    /// Open a write transaction that takes the write lock immediately rather
    /// than on first write. Deferred would let two processes both start, both
    /// read, and only then discover they conflict; immediate makes the
    /// multi-statement operations serialize cleanly, which is the whole point
    /// of using one.
    pub(super) async fn begin_immediate(&self) -> Result<()> {
        match self {
            AuthDb::Turso { conn, .. } => {
                #[cfg(test)]
                self.count_begin();
                conn.execute("BEGIN IMMEDIATE", ())
                    .await
                    .context("opening an auth database transaction")?;
                Ok(())
            }
        }
    }

    #[cfg(test)]
    fn count_begin(&self) {
        match self {
            AuthDb::Turso { begins, .. } => {
                begins.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    /// Commit when the body succeeded, roll back when it did not. The rollback
    /// is best-effort: the body's error is what the caller needs to see, and
    /// an abandoned transaction is released when the connection drops anyway.
    pub(super) async fn finish<T>(&self, result: Result<T>) -> Result<T> {
        match self {
            AuthDb::Turso { conn, .. } => match result {
                Ok(value) => {
                    conn.execute("COMMIT", ())
                        .await
                        .context("committing an auth database transaction")?;
                    Ok(value)
                }
                Err(e) => {
                    let _ = conn.execute("ROLLBACK", ()).await;
                    Err(e)
                }
            },
        }
    }
}

/// What a test store keeps alive: the folder of its Turso file. Dropped with
/// the test.
#[cfg(test)]
pub(crate) struct TestGuard {
    _dir: Option<tempfile::TempDir>,
}

/// A fresh, empty auth store on the backend `CRYSTALLINE_TEST_AUTH_BACKEND`
/// names. `turso` (the default) is a file in a new temporary folder.
#[cfg(test)]
pub(crate) async fn test_store() -> (super::AuthStore, TestGuard) {
    let backend = std::env::var("CRYSTALLINE_TEST_AUTH_BACKEND").unwrap_or_default();
    match backend.as_str() {
        "" | "turso" => {
            let dir = tempfile::tempdir().unwrap();
            let store = super::AuthStore::open(&dir.path().join("web-auth.db"))
                .await
                .unwrap();
            (store, TestGuard { _dir: Some(dir) })
        }
        other => panic!("CRYSTALLINE_TEST_AUTH_BACKEND={other}: no such auth backend yet"),
    }
}
