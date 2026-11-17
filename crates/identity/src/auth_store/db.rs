//! The executor seam of the auth store.
//!
//! Every method of [`super::AuthStore`] speaks to its database through
//! [`AuthDb`], with one SQL text and `?N` placeholders: the Turso arm (a
//! local file) and, with the `postgres` feature, the Postgres arm
//! (`pg.rs`), which runs the same text on one connection. Nothing outside
//! this file and `pg.rs` touches a connection.

use anyhow::{Context, Result};
use turso::{Connection, Database, Row, Value};

/// One row of a result, read cell by cell: a Turso row, or the decoded
/// cells of a Postgres row.
pub(super) struct AuthRow {
    row: RowCells,
}

enum RowCells {
    Turso(Row),
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    Values(Vec<Value>),
}

impl AuthRow {
    /// The value of column `i`.
    pub(super) fn get_value(&self, i: usize) -> Result<Value> {
        match &self.row {
            RowCells::Turso(row) => Ok(row.get_value(i)?),
            RowCells::Values(cells) => cells
                .get(i)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no column {i} in the row")),
        }
    }
}

/// The database behind an auth store.
///
/// The Postgres arm's futures are boxed: inline, their sqlx state machines
/// deepen every async caller's layout past the compiler's recursion limit
/// in the service crate.
pub(super) enum AuthDb {
    Turso {
        // Retained so the connection stays valid for as long as the store does.
        _db: Database,
        conn: Connection,
        /// A transaction may be open: set before `BEGIN IMMEDIATE` is sent
        /// and cleared when it is finished, so one a cancelled caller left
        /// open is rolled back by the next guard
        /// ([`AuthDb::reset_stale_transaction`]).
        in_tx: std::sync::atomic::AtomicBool,
        /// How many `BEGIN IMMEDIATE` this store has run, so a test can pin
        /// that a method runs in a transaction.
        #[cfg(test)]
        begins: std::sync::atomic::AtomicUsize,
    },
    /// The Postgres database (and schema) the index uses.
    #[cfg(feature = "postgres")]
    Postgres(Box<super::pg::PgAuth>),
}

impl AuthDb {
    /// Wrap an opened, migrated Turso database and its connection.
    pub(super) fn turso(db: Database, conn: Connection) -> AuthDb {
        AuthDb::Turso {
            _db: db,
            conn,
            in_tx: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            begins: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Whether this is the Postgres arm, for the few places where the two
    /// differ (the hot path's stamps and prunes, the write lock on guarded
    /// writes).
    pub(super) fn is_postgres(&self) -> bool {
        match self {
            AuthDb::Turso { .. } => false,
            #[cfg(feature = "postgres")]
            AuthDb::Postgres(_) => true,
        }
    }

    /// Whether the Postgres orphan prune is due. Never on Turso, which
    /// prunes on every lookup.
    pub(super) fn prune_due(&self) -> bool {
        match self {
            AuthDb::Turso { .. } => false,
            #[cfg(feature = "postgres")]
            AuthDb::Postgres(pg) => pg.prune_due(),
        }
    }

    /// Mark the Postgres orphan prune as run, once it succeeded.
    pub(super) fn prune_done(&self) {
        #[cfg(feature = "postgres")]
        if let AuthDb::Postgres(pg) = self {
            pg.prune_done();
        }
    }

    /// Roll back a transaction a cancelled caller left open: a future
    /// dropped between `begin_immediate` and `finish` (a handler whose
    /// client went away) sends neither `COMMIT` nor `ROLLBACK`, and on Turso
    /// the open `BEGIN IMMEDIATE` would keep the file's write lock and take
    /// in every later statement of this connection. The caller has just
    /// taken the store's guard. The rollback is best effort: a transaction
    /// that never began, or a commit that went through before the drop,
    /// leaves nothing to roll back.
    pub(super) async fn reset_stale_transaction(&self) {
        match self {
            AuthDb::Turso { conn, in_tx, .. } => {
                if in_tx.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    let _ = conn.execute("ROLLBACK", ()).await;
                }
            }
            #[cfg(feature = "postgres")]
            AuthDb::Postgres(pg) => Box::pin(pg.reset_stale()).await,
        }
    }

    /// How many transactions this store has begun.
    #[cfg(test)]
    pub(super) fn begins(&self) -> usize {
        match self {
            AuthDb::Turso { begins, .. } => begins.load(std::sync::atomic::Ordering::Relaxed),
            #[cfg(feature = "postgres")]
            AuthDb::Postgres(pg) => pg.begins.load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Run one statement and give back the number of rows it changed.
    pub(super) async fn execute(&self, sql: &str, p: Vec<Value>) -> Result<u64> {
        match self {
            AuthDb::Turso { conn, .. } => Ok(conn.execute(sql, p).await?),
            #[cfg(feature = "postgres")]
            AuthDb::Postgres(pg) => Box::pin(pg.execute(sql, p)).await,
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
                    out.push(AuthRow {
                        row: RowCells::Turso(row),
                    });
                }
                Ok(out)
            }
            #[cfg(feature = "postgres")]
            AuthDb::Postgres(pg) => Ok(Box::pin(pg.query(sql, p))
                .await?
                .into_iter()
                .map(|cells| AuthRow {
                    row: RowCells::Values(cells),
                })
                .collect()),
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
    ///
    /// On Postgres: `BEGIN`, then `pg_advisory_xact_lock` on the schema's
    /// auth write lock, which serializes every writer of the accounts the
    /// same way.
    pub(super) async fn begin_immediate(&self) -> Result<()> {
        match self {
            AuthDb::Turso {
                conn,
                in_tx,
                #[cfg(test)]
                begins,
                ..
            } => {
                #[cfg(test)]
                begins.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // Open from before `BEGIN` is sent, so a caller cancelled
                // inside this call leaves one the next guard rolls back.
                in_tx.store(true, std::sync::atomic::Ordering::SeqCst);
                if let Err(e) = conn.execute("BEGIN IMMEDIATE", ()).await {
                    in_tx.store(false, std::sync::atomic::Ordering::SeqCst);
                    return Err(
                        anyhow::Error::from(e).context("opening an auth database transaction")
                    );
                }
                Ok(())
            }
            #[cfg(feature = "postgres")]
            AuthDb::Postgres(pg) => Box::pin(pg.begin())
                .await
                .context("opening an auth database transaction"),
        }
    }

    /// Commit when the body succeeded, roll back when it did not. The rollback
    /// is best-effort: the body's error is what the caller needs to see, and
    /// an abandoned transaction is released when the connection drops anyway.
    pub(super) async fn finish<T>(&self, result: Result<T>) -> Result<T> {
        match self {
            AuthDb::Turso { conn, in_tx, .. } => match result {
                Ok(value) => {
                    // A commit that fails leaves the transaction open and
                    // `in_tx` set: the next guard rolls it back.
                    conn.execute("COMMIT", ())
                        .await
                        .context("committing an auth database transaction")?;
                    in_tx.store(false, std::sync::atomic::Ordering::SeqCst);
                    Ok(value)
                }
                Err(e) => {
                    let _ = conn.execute("ROLLBACK", ()).await;
                    in_tx.store(false, std::sync::atomic::Ordering::SeqCst);
                    Err(e)
                }
            },
            #[cfg(feature = "postgres")]
            AuthDb::Postgres(pg) => match result {
                Ok(value) => {
                    Box::pin(pg.finish(true))
                        .await
                        .context("committing an auth database transaction")?;
                    Ok(value)
                }
                Err(e) => {
                    let _ = Box::pin(pg.finish(false)).await;
                    Err(e)
                }
            },
        }
    }
}

/// What a test store keeps alive: the folder of its Turso file, or its
/// Postgres scratch schema. Dropped with the test.
#[cfg(test)]
pub(crate) struct TestGuard {
    loc: crystalline_core::config::AccountsLocation,
    _dir: Option<tempfile::TempDir>,
    #[cfg(feature = "postgres")]
    _schema: Option<super::pg::AuthScratch>,
}

#[cfg(test)]
impl TestGuard {
    /// A second store on the same file or schema, as another process (the
    /// daemon and a `users` command) would open it.
    pub(crate) async fn second(&self) -> super::AuthStore {
        open_test_store(&self.loc).await
    }
}

/// The backend `CRYSTALLINE_TEST_AUTH_BACKEND` names: `turso` (the default)
/// or `postgres`.
#[cfg(test)]
pub(crate) fn test_backend() -> String {
    std::env::var("CRYSTALLINE_TEST_AUTH_BACKEND").unwrap_or_default()
}

#[cfg(test)]
async fn open_test_store(loc: &crystalline_core::config::AccountsLocation) -> super::AuthStore {
    match loc {
        crystalline_core::config::AccountsLocation::File(path) => {
            super::AuthStore::open(path).await.unwrap()
        }
        #[cfg(feature = "postgres")]
        crystalline_core::config::AccountsLocation::Postgres(_) => super::AuthStore::open_at(
            loc,
            &crystalline_index::postgres::PgSessionOptions::default(),
        )
        .await
        .unwrap(),
        #[cfg(not(feature = "postgres"))]
        crystalline_core::config::AccountsLocation::Postgres(_) => {
            panic!("this build has no postgres support")
        }
    }
}

/// A fresh, empty auth store on the backend `CRYSTALLINE_TEST_AUTH_BACKEND`
/// names. `turso` (the default) is a file in a new temporary folder;
/// `postgres` is a scratch schema in the database
/// `CRYSTALLINE_TEST_POSTGRES_URL` names.
#[cfg(test)]
pub(crate) async fn test_store() -> (super::AuthStore, TestGuard) {
    match test_backend().as_str() {
        "" | "turso" => {
            let dir = tempfile::tempdir().unwrap();
            let loc =
                crystalline_core::config::AccountsLocation::File(dir.path().join("web-auth.db"));
            let store = open_test_store(&loc).await;
            let guard = TestGuard {
                loc,
                _dir: Some(dir),
                #[cfg(feature = "postgres")]
                _schema: None,
            };
            (store, guard)
        }
        #[cfg(feature = "postgres")]
        "postgres" => {
            let url = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL")
                .ok()
                .filter(|u| !u.is_empty())
                .expect(
                    "CRYSTALLINE_TEST_AUTH_BACKEND=postgres needs CRYSTALLINE_TEST_POSTGRES_URL",
                );
            let (store, guard) = pg_test_store(&url, "auth").await;
            (store, guard)
        }
        other => panic!("CRYSTALLINE_TEST_AUTH_BACKEND={other}: no such auth backend"),
    }
}

/// A fresh auth store in a new scratch schema of the database at `url`.
#[cfg(all(test, feature = "postgres"))]
pub(crate) async fn pg_test_store(url: &str, prefix: &str) -> (super::AuthStore, TestGuard) {
    let schema = super::pg::AuthScratch::new(url, prefix);
    let loc = crystalline_core::config::AccountsLocation::Postgres(schema.config());
    let store = open_test_store(&loc).await;
    let guard = TestGuard {
        loc,
        _dir: None,
        _schema: Some(schema),
    };
    (store, guard)
}
