//! The Postgres arm of the auth store's executor seam.
//!
//! One bare `PgConnection` in the role `auth`, built by the index's shared
//! session builder ([`connect_one`]), so it carries the instance tag, the
//! keepalives, the schema's `search_path` and, in the owner's process, the
//! fence check. It connects lazily and, after a lost connection, reconnects
//! on the next call, never inside a transaction: a transaction whose
//! connection died fails, it is never carried on over a new session.
//!
//! The SQL text is the one the Turso arm runs. Placeholders `?N` become `$N`
//! (cached per SQL string), a `NULL` is bound as a text-typed null, and the
//! cells come back as [`turso::Value`]s ([`decode_cell`]): INT2, INT4 and INT8
//! as `Integer`, BOOL as `Integer(0 or 1)`, the text types as `Text`. Any other
//! column type is an error, so a dialect slip fails loudly instead of being
//! read as something else.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use crystalline_index::postgres::{
    PgRole, PgSessionOptions, PgTarget, apply_locked_set, connect_one, lock_key,
};
use sqlx::postgres::PgRow;
use sqlx::{AssertSqlSafe, Connection, PgConnection, Row, TypeInfo, ValueRef};
use turso::Value;

use super::AuthOpenError;
use super::pg_migrations::AUTH_MIGRATIONS;

/// The advisory lock every auth write transaction takes on Postgres, mixed
/// with the schema so two schemas never serialize each other's writes.
pub fn auth_write_lock(schema: Option<&str>) -> i64 {
    lock_key(schema, "auth-write")
}

/// How often the orphan prune runs on Postgres after the one at open.
pub(super) const PRUNE_EVERY: Duration = Duration::from_secs(3600);

/// The connection and what it is doing.
struct PgState {
    /// `None` before the first statement and after a lost connection.
    conn: Option<PgConnection>,
    /// A transaction is open (between `begin` and `finish`).
    in_tx: bool,
    /// A statement of the open transaction failed: Postgres has aborted it,
    /// so it can only be rolled back.
    failed: bool,
}

/// The auth store's Postgres executor.
pub(super) struct PgAuth {
    target: PgTarget,
    opts: PgSessionOptions,
    /// [`auth_write_lock`] of the target's schema.
    write_lock: i64,
    state: tokio::sync::Mutex<PgState>,
    /// `?N` SQL text to its `$N` form.
    rewritten: Mutex<HashMap<String, Arc<str>>>,
    /// When the orphan prune last ran.
    pruned_at: Mutex<Option<Instant>>,
    /// How many transactions this store has begun.
    #[cfg(test)]
    pub(super) begins: std::sync::atomic::AtomicUsize,
}

/// Whether `e` means the session is gone (or unusable), so the next call
/// must connect again: an I/O, TLS or protocol failure, or a server error of
/// the connection-exception (`08`) or operator-intervention (`57P`, which a
/// `pg_terminate_backend` sends) classes.
fn is_connection_lost(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::Protocol(_)
        | sqlx::Error::WorkerCrashed => true,
        sqlx::Error::Database(db) => db
            .code()
            .is_some_and(|c| c.starts_with("08") || c.starts_with("57P")),
        _ => false,
    }
}

/// `sql` with every `?N` placeholder outside a quoted literal or identifier
/// written `$N`.
pub(super) fn rewrite_placeholders(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut quote: Option<char> = None;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                out.push(c);
            }
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                out.push(c);
            }
            None if c == '?' && chars.peek().is_some_and(char::is_ascii_digit) => out.push('$'),
            None => out.push(c),
        }
    }
    out
}

/// `text` with `p` bound in order. A `NULL` is bound as a text-typed null:
/// every `NULL` the shared SQL binds goes into a text column, and a `NULL`
/// meant for a `BIGINT` column is written `CAST(?N AS BIGINT)` in the shared
/// text, which both engines accept.
fn bound(
    text: Arc<str>,
    p: Vec<Value>,
) -> sqlx::query::Query<'static, sqlx::Postgres, sqlx::postgres::PgArguments> {
    let mut q = sqlx::query(AssertSqlSafe(text));
    for v in p {
        q = match v {
            Value::Null => q.bind(None::<String>),
            Value::Integer(n) => q.bind(n),
            Value::Real(f) => q.bind(f),
            Value::Text(s) => q.bind(s),
            Value::Blob(b) => q.bind(b),
        };
    }
    q
}

/// One cell of a Postgres row as a [`Value`].
fn decode_cell(row: &PgRow, i: usize) -> Result<Value> {
    let raw = row.try_get_raw(i)?;
    if raw.is_null() {
        return Ok(Value::Null);
    }
    let ty = raw.type_info().name().to_string();
    Ok(match ty.as_str() {
        "BOOL" => Value::Integer(i64::from(row.try_get::<bool, _>(i)?)),
        "INT2" => Value::Integer(i64::from(row.try_get::<i16, _>(i)?)),
        "INT4" => Value::Integer(i64::from(row.try_get::<i32, _>(i)?)),
        "INT8" => Value::Integer(row.try_get::<i64, _>(i)?),
        "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" => Value::Text(row.try_get::<String, _>(i)?),
        "FLOAT4" => Value::Real(f64::from(row.try_get::<f32, _>(i)?)),
        "FLOAT8" => Value::Real(row.try_get::<f64, _>(i)?),
        "BYTEA" => Value::Blob(row.try_get::<Vec<u8>, _>(i)?),
        other => {
            bail!("the auth store cannot read a {other} column (column {i}); cast it in the SQL")
        }
    })
}

/// Every cell of a row.
pub(super) fn decode_row(row: &PgRow) -> Result<Vec<Value>> {
    (0..row.len()).map(|i| decode_cell(row, i)).collect()
}

/// The version the ledger of [`AUTH_MIGRATIONS`] records in `schema` (the
/// connection's current schema when `None`), 0 when there is no ledger.
async fn ledger_version(conn: &mut PgConnection, schema: Option<&str>) -> Result<i64> {
    let ledger = AUTH_MIGRATIONS.ledger;
    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = COALESCE($1::text, current_schema()) AND table_name = $2)",
    )
    .bind(schema)
    .bind(ledger)
    .fetch_one(&mut *conn)
    .await
    .context("reading the accounts tables' version")?;
    if !present {
        return Ok(0);
    }
    let version: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT COALESCE(MAX(version), 0) FROM {ledger}"
    )))
    .fetch_one(&mut *conn)
    .await
    .context("reading the accounts tables' version")?;
    Ok(version)
}

impl PgAuth {
    /// Connect to `target` in the role `auth` and, with `migrate`, apply
    /// [`AUTH_MIGRATIONS`] under the migration lock. Without `migrate` the
    /// ledger must be exactly at this binary's version: behind is
    /// [`AuthOpenError::Behind`], ahead is [`AuthOpenError::TooNew`].
    pub(super) async fn open(
        target: PgTarget,
        opts: PgSessionOptions,
        migrate: bool,
    ) -> std::result::Result<PgAuth, AuthOpenError> {
        let schema = target.schema.clone();
        let mut conn = connect_one(&target, PgRole::Auth, &opts)
            .await
            .context("connecting to the accounts database")?;
        let known = AUTH_MIGRATIONS.steps.last().map_or(0, |m| m.version);
        if migrate {
            let run = apply_locked_set(&mut conn, schema.as_deref(), &AUTH_MIGRATIONS).await;
            if run.tainted {
                // The session may still hold the migration lock: close it,
                // the next statement connects again.
                let _ = conn.close().await;
                return match run.version {
                    Ok(_) => Err(anyhow!("the migration lock was not released").into()),
                    Err(e) => Err(migration_error(e)),
                };
            }
            run.version.map_err(migration_error)?;
        } else {
            let found = ledger_version(&mut conn, schema.as_deref()).await?;
            if found < known {
                let _ = conn.close().await;
                return Err(AuthOpenError::Behind { found, need: known });
            }
            if found > known {
                let _ = conn.close().await;
                return Err(AuthOpenError::TooNew { found, known });
            }
        }
        Ok(PgAuth {
            write_lock: auth_write_lock(schema.as_deref()),
            target,
            opts,
            state: tokio::sync::Mutex::new(PgState {
                conn: Some(conn),
                in_tx: false,
                failed: false,
            }),
            rewritten: Mutex::new(HashMap::new()),
            pruned_at: Mutex::new(None),
            #[cfg(test)]
            begins: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// The `$N` form of `sql`, rewritten once per SQL string.
    fn rewritten(&self, sql: &str) -> Arc<str> {
        let mut cache = self.rewritten.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(done) = cache.get(sql) {
            return done.clone();
        }
        let done: Arc<str> = rewrite_placeholders(sql).into();
        cache.insert(sql.to_string(), done.clone());
        done
    }

    /// Whether the orphan prune is due (it never ran, or ran
    /// [`PRUNE_EVERY`] ago), marking it as run now when it is.
    pub(super) fn prune_due(&self) -> bool {
        let mut at = self.pruned_at.lock().unwrap_or_else(|e| e.into_inner());
        match *at {
            Some(last) if last.elapsed() < PRUNE_EVERY => false,
            _ => {
                *at = Some(Instant::now());
                true
            }
        }
    }

    /// The live connection, connecting first when there is none. Refused
    /// inside a transaction whose connection was lost.
    async fn connection<'a>(&self, st: &'a mut PgState) -> Result<&'a mut PgConnection> {
        if st.conn.is_none() {
            if st.in_tx {
                bail!("the accounts database connection was lost inside a transaction");
            }
            let conn = connect_one(&self.target, PgRole::Auth, &self.opts)
                .await
                .context("connecting to the accounts database")?;
            st.conn = Some(conn);
        }
        Ok(st.conn.as_mut().expect("connected just above"))
    }

    /// Note a failed statement: drop a lost connection, mark an open
    /// transaction as aborted.
    fn failed(st: &mut PgState, e: &sqlx::Error) {
        if is_connection_lost(e) {
            st.conn = None;
        }
        if st.in_tx {
            st.failed = true;
        }
    }

    /// Run `sql` with `p` and give back every row.
    async fn run(&self, sql: &str, p: Vec<Value>) -> Result<Vec<PgRow>> {
        let text = self.rewritten(sql);
        let mut st = self.state.lock().await;
        let conn = self.connection(&mut st).await?;
        let q = bound(text, p);
        match q.fetch_all(&mut *conn).await {
            Ok(rows) => Ok(rows),
            Err(e) => {
                Self::failed(&mut st, &e);
                Err(e.into())
            }
        }
    }

    /// Run one statement and give back the number of rows it changed.
    pub(super) async fn execute(&self, sql: &str, p: Vec<Value>) -> Result<u64> {
        let text = self.rewritten(sql);
        let mut st = self.state.lock().await;
        let conn = self.connection(&mut st).await?;
        let q = bound(text, p);
        match q.execute(&mut *conn).await {
            Ok(done) => Ok(done.rows_affected()),
            Err(e) => {
                Self::failed(&mut st, &e);
                Err(e.into())
            }
        }
    }

    /// Run a query and give back every row, decoded.
    pub(super) async fn query(&self, sql: &str, p: Vec<Value>) -> Result<Vec<Vec<Value>>> {
        self.run(sql, p).await?.iter().map(decode_row).collect()
    }

    /// Run one statement of the transaction protocol (no parameters).
    async fn control(&self, st: &mut PgState, sql: &'static str) -> Result<()> {
        let conn = self.connection(st).await?;
        match sqlx::raw_sql(sql).execute(&mut *conn).await {
            Ok(_) => Ok(()),
            Err(e) => {
                Self::failed(st, &e);
                Err(e.into())
            }
        }
    }

    /// `BEGIN`, then the schema's auth write lock for the rest of the
    /// transaction. Read committed, so every statement after the lock sees
    /// what the writer before it committed. A transaction a cancelled caller
    /// left open is rolled back first; a begin that fails leaves none open.
    pub(super) async fn begin(&self) -> Result<()> {
        #[cfg(test)]
        self.begins
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut st = self.state.lock().await;
        if st.in_tx {
            self.abandon(&mut st).await;
        }
        self.control(&mut st, "BEGIN").await?;
        st.in_tx = true;
        st.failed = false;
        let conn = match self.connection(&mut st).await {
            Ok(conn) => conn,
            Err(e) => {
                self.abandon(&mut st).await;
                return Err(e);
            }
        };
        let locked = sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(self.write_lock)
            .execute(&mut *conn)
            .await;
        if let Err(e) = locked {
            Self::failed(&mut st, &e);
            self.abandon(&mut st).await;
            return Err(anyhow::Error::from(e).context("taking the accounts write lock"));
        }
        Ok(())
    }

    /// Roll the open transaction back, best effort, and forget it. A
    /// rollback that fails drops the connection, which ends the transaction
    /// on the server.
    async fn abandon(&self, st: &mut PgState) {
        if let Some(conn) = st.conn.as_mut()
            && sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await.is_err()
        {
            st.conn = None;
        }
        st.in_tx = false;
        st.failed = false;
    }

    /// Commit (`ok`) or roll back the open transaction. A commit of a
    /// transaction one of whose statements failed is a rollback and an
    /// error, never a silent `ROLLBACK` reported as success.
    pub(super) async fn finish(&self, ok: bool) -> Result<()> {
        let mut st = self.state.lock().await;
        if !ok {
            self.abandon(&mut st).await;
            return Ok(());
        }
        if st.failed || st.conn.is_none() {
            self.abandon(&mut st).await;
            bail!("an accounts database transaction failed and was rolled back");
        }
        let committed = self.control(&mut st, "COMMIT").await;
        if committed.is_err() && st.conn.is_some() {
            self.abandon(&mut st).await;
        }
        st.in_tx = false;
        st.failed = false;
        committed
    }
}

/// An error of the migration run as an open error: a ledger ahead of this
/// binary is [`AuthOpenError::TooNew`].
fn migration_error(e: crystalline_index::IndexError) -> AuthOpenError {
    match e {
        crystalline_index::IndexError::SchemaTooNew { found, known } => {
            AuthOpenError::TooNew { found, known }
        }
        other => {
            AuthOpenError::Other(anyhow::Error::new(other).context("migrating the accounts tables"))
        }
    }
}

/// A test-only schema for an auth store, dropped (with `CASCADE`) when the
/// guard goes, on a thread of its own and bounded by a timeout, like the
/// index's `ScratchSchema` but without opening an index store first.
#[cfg(test)]
pub(crate) struct AuthScratch {
    url: String,
    schema: String,
}

#[cfg(test)]
impl AuthScratch {
    /// A fresh schema name in the database at `url`.
    pub(crate) fn new(url: &str, prefix: &str) -> AuthScratch {
        use std::hash::{BuildHasher, RandomState};
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        AuthScratch {
            url: url.to_string(),
            schema: format!(
                "{prefix}_{}_{n}_{:x}",
                std::process::id(),
                RandomState::new().hash_one(n)
            ),
        }
    }

    /// The database block of this schema.
    pub(crate) fn config(&self) -> crystalline_core::config::DatabaseConfig {
        crystalline_core::config::DatabaseConfig {
            backend: crystalline_core::config::DatabaseBackend::Postgres,
            url: Some(self.url.clone()),
            schema: Some(self.schema.clone()),
        }
    }

    /// The schema's name.
    pub(crate) fn name(&self) -> &str {
        &self.schema
    }
}

#[cfg(test)]
impl Drop for AuthScratch {
    fn drop(&mut self) {
        let (url, schema) = (self.url.clone(), self.schema.clone());
        let cleanup = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                eprintln!("could not drop the scratch schema {schema}: no runtime");
                return;
            };
            let dropped = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(30), async {
                    let target = PgTarget { url, schema: None };
                    let mut conn =
                        connect_one(&target, PgRole::Cli, &PgSessionOptions::default()).await?;
                    sqlx::raw_sql(AssertSqlSafe(format!(
                        "DROP SCHEMA IF EXISTS \"{schema}\" CASCADE"
                    )))
                    .execute(&mut conn)
                    .await?;
                    let _ = conn.close().await;
                    Ok::<(), anyhow::Error>(())
                })
                .await
            });
            match dropped {
                Ok(Ok(())) => {}
                Ok(Err(e)) => eprintln!("could not drop the scratch schema {schema}: {e}"),
                Err(_) => eprintln!("gave up dropping the scratch schema {schema}: timed out"),
            }
        });
        let _ = cleanup.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `?N` becomes `$N` outside quotes only, and a bare `?` stays.
    #[test]
    fn placeholders_are_rewritten_outside_quotes() {
        assert_eq!(
            rewrite_placeholders("SELECT ?1, ?12 FROM t WHERE a = '?3' AND \"?4\" = ?2 OR b = ?"),
            "SELECT $1, $12 FROM t WHERE a = '?3' AND \"?4\" = $2 OR b = ?"
        );
        assert_eq!(
            rewrite_placeholders("x = 'it''s ?1' AND y = ?1"),
            "x = 'it''s ?1' AND y = $1"
        );
    }

    /// Every connection-loss class drops the session; an ordinary statement
    /// error keeps it.
    #[test]
    fn connection_loss_is_told_apart_from_a_statement_error() {
        assert!(is_connection_lost(&sqlx::Error::Io(std::io::Error::other(
            "gone"
        ))));
        assert!(is_connection_lost(&sqlx::Error::Protocol("bad".into())));
        assert!(!is_connection_lost(&sqlx::Error::RowNotFound));
    }
}
