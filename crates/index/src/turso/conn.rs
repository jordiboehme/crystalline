//! The store's connection, with one retry for a refused temp directory.
//!
//! The embedded engine answers many ordinary statements with an ephemeral
//! table: an `IN (...)` list or subquery, a `RETURNING` clause, a correlated
//! subquery, a `DISTINCT`. With the default temp store it backs each one with a
//! fresh directory plus file under the process temp location, created eagerly
//! by `tempfile::tempdir()` when the table is opened, even when it stays empty
//! (`TempFile::new` in turso_core's `io/mod.rs`). On Windows that create is
//! refused now and then with "permission denied", and the statement fails with
//! `I/O error (tempdir): permission denied` although nothing is wrong with the
//! database or the statement. The runtime keeps the default temp store on
//! purpose (a sorter that outgrows its buffer must spill to disk, see
//! `research/2026-07-28-turso-sorter-spill.md`), so the statement is retried
//! instead, once, and only on exactly that error.
//!
//! A retry is only sound when the failed attempt left nothing behind, and what
//! it leaves depends on where it ran (turso_core 0.8.1, `Program::abort` in
//! `vdbe/mod.rs`):
//!
//! - In autocommit mode the statement was its own transaction and the engine
//!   rolled it back, so the statement runs again.
//! - Inside an explicit transaction that is still open, the statement had a
//!   statement savepoint and only the statement was undone, so it runs again.
//! - Inside an explicit transaction that is gone, the engine rolled back the
//!   whole transaction (a read, or a write without a statement savepoint, takes
//!   that path). The statement alone cannot run again: the writes before it
//!   are lost. So the connection keeps a journal of every statement since the
//!   `BEGIN`, with what each one returned (its change count, or a hash of its
//!   rows). It opens the transaction again, replays the journal and then runs
//!   the statement. The replay is checked, not assumed: every replayed
//!   statement must return what it returned the first time, so the caller,
//!   which acted on those results, is still right about the transaction. Any
//!   difference (another writer got in between, say) ends the replay. A
//!   journal that grows past [`JOURNAL_CAP`] is dropped, and a failure there
//!   is not retried.
//!
//! When a replay cannot be done, fails or does not match, it is rolled back
//! and the caller gets the original refusal, with no explicit transaction
//! open: exactly what it got before this retry existed. A failed retry of the
//! statement itself is returned as it is.
//!
//! Only the directory create is retried. A failure to open the file inside
//! the new directory has another label and is returned as it is. When the
//! connection cannot say whether it is in a transaction (`is_autocommit`
//! errors), nothing is retried either: the refusal is returned as it is and
//! the journal is closed, because a statement run again outside a transaction
//! the refusal dropped would commit on its own.
//!
//! The replay is sound only under three assumptions, which the store keeps:
//!
//! - **One writer.** A replayed statement is checked against what it returned
//!   the first time, but its parameters are not recomputed: one derived from
//!   `last_insert_rowid` or from an earlier change count would replay
//!   "successfully" with other content if another connection wrote between
//!   the drop and the replay. The store owns its database file and writes
//!   through this one connection.
//! - **Reads through [`Conn::prepare_read`] are not journalled.** Their
//!   results are not checked again on a replay, so the caller must not base a
//!   write inside a transaction on one in a way a replay would get wrong; a
//!   read whose answer matters to the transaction goes through
//!   [`Conn::query_all`], which is journalled and checked.
//! - **No `SAVEPOINT`, `RELEASE` or `ROLLBACK TO`.** The journal is one flat
//!   list from the `BEGIN`; a statement rolled back to a savepoint would still
//!   be in it and be replayed. [`Conn::execute`] and [`Conn::query_all`]
//!   refuse those statements in a debug build, so a change that adds one
//!   breaks a test instead of the journal.

use std::hash::{Hash, Hasher};
use std::mem::size_of;
use std::sync::Mutex;

use turso::params::{IntoParams, Params};
use turso::{Connection, Row, Statement, Value};

/// The most bytes a transaction's journal holds before it is dropped. A full
/// reindex runs in one transaction and carries every body as a parameter;
/// past this size a replay is not worth the memory.
const JOURNAL_CAP: usize = 64 << 20;

/// The store's connection: the engine's connection plus the retry.
pub(crate) struct Conn {
    conn: Connection,
    journal: Mutex<Journal>,
    /// Whether transactions are journalled for a replay. A read connection
    /// keeps no journal: it never writes, and its caller runs the whole read
    /// again instead (see `ReaderBackend` in the `reader` module).
    journalled: bool,
}

/// What the connection knows about the explicit transaction it is in.
enum Journal {
    /// Autocommit mode.
    Idle,
    /// Inside an explicit transaction, with every statement since `begin`.
    Open {
        begin: String,
        entries: Vec<Entry>,
        bytes: usize,
    },
    /// Inside an explicit transaction whose journal outgrew [`JOURNAL_CAP`].
    TooLarge,
}

/// One statement of a journal, run again the way it ran the first time.
struct Entry {
    shape: Shape,
    sql: String,
    params: Params,
    /// [`fingerprint`] of what the statement returned the first time.
    result: u64,
}

/// How a statement is run: through `execute`, or as a query drained to the end.
#[derive(Clone, Copy)]
enum Shape {
    Execute,
    Query,
}

/// What one run of a statement returned.
enum Outcome {
    Changes(u64),
    Rows(Vec<Row>),
}

/// A statement runner: the store's [`Conn`], or the engine's own connection in
/// a migration, which runs with its temp store in memory and needs no retry.
pub(crate) trait Execute {
    async fn execute(&self, sql: &str, params: Vec<Value>) -> turso::Result<u64>;
}

impl Execute for Conn {
    async fn execute(&self, sql: &str, params: Vec<Value>) -> turso::Result<u64> {
        Conn::execute(self, sql, params).await
    }
}

impl Execute for Connection {
    async fn execute(&self, sql: &str, params: Vec<Value>) -> turso::Result<u64> {
        Connection::execute(self, sql, params).await
    }
}

/// Whether `err` is the engine failing to create the directory for an
/// ephemeral table, which is the only error this connection retries.
pub(crate) fn is_tempdir_refusal(err: &turso::Error) -> bool {
    matches!(
        err,
        turso::Error::IoError(std::io::ErrorKind::PermissionDenied, "tempdir")
    )
}

/// Whether `sql` opens, releases or rolls back to a savepoint, which the
/// journal cannot follow (see the module doc).
fn is_savepoint_statement(sql: &str) -> bool {
    let mut words = sql
        .split(|c: char| c.is_whitespace() || c == ';')
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_uppercase);
    match words.next().as_deref() {
        Some("SAVEPOINT" | "RELEASE") => true,
        Some("ROLLBACK") => match words.next().as_deref() {
            Some("TO") => true,
            Some("TRANSACTION") => words.next().as_deref() == Some("TO"),
            _ => false,
        },
        _ => false,
    }
}

/// Whether `sql` is a read: what [`Conn::prepare_read`] accepts.
fn is_read(sql: &str) -> bool {
    sql.trim_start()
        .get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case("SELECT"))
}

impl Conn {
    /// Wrap an open connection, which must be in autocommit mode.
    pub(crate) fn new(conn: Connection) -> Conn {
        Conn {
            conn,
            journal: Mutex::new(Journal::Idle),
            journalled: true,
        }
    }

    /// Wrap a read connection, which keeps no journal: a refusal that drops
    /// its read transaction is returned, and the caller runs the read again.
    pub(crate) fn reader(conn: Connection) -> Conn {
        Conn {
            conn,
            journal: Mutex::new(Journal::Idle),
            journalled: false,
        }
    }

    /// Whether the connection is in autocommit mode (no transaction open). An
    /// engine that cannot say counts as autocommit.
    pub(crate) fn is_autocommit(&self) -> bool {
        self.conn.is_autocommit().unwrap_or(true)
    }

    /// The engine's connection, for tests, and only outside a transaction:
    /// a write on it inside one would be missing from the journal, and a
    /// replay would come back without it.
    #[cfg(test)]
    pub(crate) fn raw(&self) -> &Connection {
        debug_assert!(
            self.conn.is_autocommit().unwrap_or(true),
            "the raw connection is for use outside a transaction only"
        );
        &self.conn
    }

    /// Prepare a read to run many times, in or out of a transaction. Only a
    /// `SELECT` is accepted: a read changes nothing a replay has to repeat.
    /// Its runs are neither journalled nor retried, so a refused read here
    /// fails as it did before the retry existed, and a journal left behind by
    /// a transaction it dropped is reset by the next statement through
    /// [`Conn::run`].
    pub(crate) async fn prepare_read(&self, sql: &str) -> turso::Result<Statement> {
        debug_assert!(is_read(sql), "prepare_read takes a SELECT: {sql}");
        if !is_read(sql) {
            return Err(turso::Error::Misuse(format!(
                "prepare_read takes a SELECT: {sql}"
            )));
        }
        self.conn.prepare(sql).await
    }

    /// The rowid of the last row this connection inserted.
    pub(crate) fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }

    /// Run a statement that returns no rows, as `Connection::execute` does.
    pub(crate) async fn execute(&self, sql: &str, params: impl IntoParams) -> turso::Result<u64> {
        let params = params.into_params()?;
        match self.run(Shape::Execute, sql, params).await? {
            Outcome::Changes(n) => Ok(n),
            Outcome::Rows(_) => Ok(0),
        }
    }

    /// Run a query and drain it to the end, every row kept.
    pub(crate) async fn query_all(&self, sql: &str, params: Vec<Value>) -> turso::Result<Vec<Row>> {
        match self
            .run(Shape::Query, sql, Params::Positional(params))
            .await?
        {
            Outcome::Rows(rows) => Ok(rows),
            Outcome::Changes(_) => Ok(Vec::new()),
        }
    }

    async fn run(&self, shape: Shape, sql: &str, params: Params) -> turso::Result<Outcome> {
        debug_assert!(
            !is_savepoint_statement(sql),
            "the journal cannot follow a savepoint: {sql}"
        );
        let before = self.autocommit().ok();
        let err = match self.once(shape, sql, params.clone()).await {
            Ok(outcome) => {
                self.record(shape, sql, params, &outcome, before)?;
                return Ok(outcome);
            }
            Err(err) if is_tempdir_refusal(&err) => err,
            Err(err) => {
                self.forget_if_closed();
                return Err(err);
            }
        };
        // Whether the refusal dropped a transaction decides between a retry
        // and a replay, and a guess either way is wrong somewhere: a statement
        // retried as if in autocommit mode inside a transaction the refusal
        // dropped commits on its own. So an unknown state is no retry at all:
        // the refusal goes back as it is, with the journal closed.
        let (Some(was_autocommit), Ok(now_autocommit)) = (before, self.autocommit()) else {
            hook::event("retry:unknown");
            *self.lock() = Journal::Idle;
            return Err(err);
        };
        let replayed = !was_autocommit && now_autocommit;
        if replayed {
            hook::event("replay");
            tracing::warn!(
                statement = sql,
                "the engine could not create a temp directory and dropped the transaction; \
                 replaying it"
            );
            self.replay(err).await?;
        } else {
            hook::event("retry");
            tracing::warn!(
                statement = sql,
                "the engine could not create a temp directory; running the statement again"
            );
        }
        match self.once(shape, sql, params.clone()).await {
            Ok(outcome) => {
                self.record(shape, sql, params, &outcome, Some(was_autocommit))?;
                Ok(outcome)
            }
            Err(err) if replayed => {
                self.abandon().await;
                Err(err)
            }
            Err(err) => {
                self.forget_if_closed();
                Err(err)
            }
        }
    }

    /// One run of a statement, drained to the end.
    async fn once(&self, shape: Shape, sql: &str, params: Params) -> turso::Result<Outcome> {
        match shape {
            Shape::Execute => self.conn.execute(sql, params).await.map(Outcome::Changes),
            Shape::Query => {
                let mut rows = self.conn.query(sql, params).await?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().await? {
                    out.push(row);
                }
                Ok(Outcome::Rows(out))
            }
        }
    }

    /// Open the dropped transaction again and run every statement it had run,
    /// each of which must return what it returned the first time. When that
    /// cannot be done, `err` (the refusal that dropped the transaction) is
    /// returned and no transaction is left open.
    async fn replay(&self, err: turso::Error) -> turso::Result<()> {
        let journal = std::mem::replace(&mut *self.lock(), Journal::Idle);
        let Journal::Open {
            begin,
            entries,
            bytes,
        } = journal
        else {
            hook::event("replay:unavailable");
            tracing::warn!("the transaction's journal was dropped; it cannot be replayed");
            return Err(err);
        };
        if self.conn.execute(&begin, ()).await.is_err() {
            hook::event("replay:failed");
            self.abandon().await;
            return Err(err);
        }
        for entry in &entries {
            match self
                .once(entry.shape, &entry.sql, entry.params.clone())
                .await
            {
                Ok(outcome) if fingerprint(&outcome) == entry.result => {}
                Ok(_) => {
                    hook::event("replay:mismatch");
                    tracing::warn!(
                        statement = entry.sql,
                        "the replayed statement returned something else than the first time; \
                         giving the transaction up"
                    );
                    self.abandon().await;
                    return Err(err);
                }
                Err(e) => {
                    hook::event("replay:failed");
                    tracing::warn!(statement = entry.sql, error = %e, "the replay failed");
                    self.abandon().await;
                    return Err(err);
                }
            }
        }
        *self.lock() = Journal::Open {
            begin,
            entries,
            bytes,
        };
        Ok(())
    }

    /// Bring the journal in line with the connection after a statement ran:
    /// a statement that left autocommit mode opened a transaction and starts a
    /// journal, one inside a transaction joins it with what it returned, and
    /// one that ended in autocommit mode leaves none. `was_autocommit` is
    /// `None` when the state before the statement was unknown, and then the
    /// journal is closed: it may be missing this statement, and a replay
    /// without it would be wrong.
    fn record(
        &self,
        shape: Shape,
        sql: &str,
        params: Params,
        outcome: &Outcome,
        was_autocommit: Option<bool>,
    ) -> turso::Result<()> {
        let autocommit = self.autocommit().unwrap_or(true);
        let mut journal = self.lock();
        let Some(was_autocommit) = was_autocommit.filter(|_| !autocommit && self.journalled) else {
            *journal = Journal::Idle;
            return Ok(());
        };
        if was_autocommit {
            *journal = Journal::Open {
                begin: sql.to_string(),
                entries: Vec::new(),
                bytes: 0,
            };
        } else if let Journal::Open { entries, bytes, .. } = &mut *journal {
            *bytes += entry_bytes(sql, &params);
            if *bytes > journal_cap() {
                *journal = Journal::TooLarge;
            } else {
                entries.push(Entry {
                    shape,
                    sql: sql.to_string(),
                    params,
                    result: fingerprint(outcome),
                });
            }
        }
        Ok(())
    }

    /// Whether the connection is in autocommit mode, as the engine says; a
    /// test can make it fail.
    fn autocommit(&self) -> turso::Result<bool> {
        #[cfg(test)]
        if hook::autocommit_fails() {
            return Err(turso::Error::Misuse("autocommit state unknown".into()));
        }
        self.conn.is_autocommit()
    }

    /// Drop the journal when a failed statement ended the transaction.
    fn forget_if_closed(&self) {
        if self.autocommit().unwrap_or(true) {
            *self.lock() = Journal::Idle;
        }
    }

    /// Leave no explicit transaction open after a replay or retry that failed,
    /// so the caller sees what it would have seen without the retry: the
    /// transaction gone and its own `ROLLBACK` refused or a no-op.
    async fn abandon(&self) {
        if !self.autocommit().unwrap_or(true) {
            let _ = self.conn.execute("ROLLBACK", ()).await;
        }
        *self.lock() = Journal::Idle;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Journal> {
        self.journal.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// A hash of what a statement returned: its change count, or every value of
/// every row in order. A replayed statement must give the same one.
fn fingerprint(outcome: &Outcome) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match outcome {
        Outcome::Changes(n) => {
            0u8.hash(&mut h);
            n.hash(&mut h);
        }
        Outcome::Rows(rows) => {
            1u8.hash(&mut h);
            rows.len().hash(&mut h);
            for row in rows {
                let width = row.column_count();
                width.hash(&mut h);
                for i in 0..width {
                    match row.get_value(i) {
                        Ok(Value::Null) => 0u8.hash(&mut h),
                        Ok(Value::Integer(n)) => (1u8, n).hash(&mut h),
                        Ok(Value::Real(f)) => (2u8, f.to_bits()).hash(&mut h),
                        Ok(Value::Text(t)) => (3u8, t).hash(&mut h),
                        Ok(Value::Blob(b)) => (4u8, b).hash(&mut h),
                        Err(_) => 5u8.hash(&mut h),
                    }
                }
            }
        }
    }
    h.finish()
}

/// What a journal entry for `sql` with `params` costs in memory.
fn entry_bytes(sql: &str, params: &Params) -> usize {
    fn value(v: &Value) -> usize {
        size_of::<Value>()
            + match v {
                Value::Text(s) => s.len(),
                Value::Blob(b) => b.len(),
                _ => 0,
            }
    }
    size_of::<Entry>()
        + sql.len()
        + match params {
            Params::None => 0,
            Params::Positional(values) => values.iter().map(value).sum(),
            Params::Named(values) => values.iter().map(|(k, v)| k.len() + value(v)).sum(),
        }
}

/// [`JOURNAL_CAP`], which a test can lower.
fn journal_cap() -> usize {
    #[cfg(test)]
    if let Some(cap) = hook::journal_cap() {
        return cap;
    }
    JOURNAL_CAP
}

/// Test seams: the retry reports each step here, and a test can act on it.
mod hook {
    #[cfg(test)]
    use std::sync::Mutex;

    #[cfg(test)]
    type Hook = Box<dyn Fn(&str) + Send>;

    /// Called with "retry" or "replay" between a refused attempt and its
    /// retry, with "retry:unknown" when the connection's state was unknown and
    /// nothing is retried, and with "replay:unavailable", "replay:failed" or
    /// "replay:mismatch" when a replay is given up.
    #[cfg(test)]
    static EVENT: Mutex<Option<Hook>> = Mutex::new(None);

    #[cfg(test)]
    static JOURNAL_CAP: Mutex<Option<usize>> = Mutex::new(None);

    #[cfg(test)]
    static AUTOCOMMIT_FAILS: Mutex<bool> = Mutex::new(false);

    #[cfg(test)]
    pub(super) fn autocommit_fails() -> bool {
        *AUTOCOMMIT_FAILS.lock().unwrap()
    }

    #[cfg(all(test, unix))]
    pub(super) fn set_autocommit_fails(on: bool) {
        *AUTOCOMMIT_FAILS.lock().unwrap() = on;
    }

    pub(super) fn event(_what: &str) {
        #[cfg(test)]
        if let Some(hook) = EVENT.lock().unwrap().as_ref() {
            hook(_what);
        }
    }

    #[cfg(test)]
    pub(super) fn journal_cap() -> Option<usize> {
        *JOURNAL_CAP.lock().unwrap()
    }

    /// Used by the unix-only test below; Windows cannot refuse a create the
    /// same way, so it has no caller there.
    #[cfg(all(test, unix))]
    pub(super) fn set_event(hook: impl Fn(&str) + Send + 'static) {
        *EVENT.lock().unwrap() = Some(Box::new(hook));
    }

    #[cfg(all(test, unix))]
    pub(super) fn set_journal_cap(cap: Option<usize>) {
        *JOURNAL_CAP.lock().unwrap() = cap;
    }
}

#[cfg(test)]
mod savepoint_tests {
    use super::is_savepoint_statement;

    #[test]
    fn the_statements_the_journal_cannot_follow_are_recognised() {
        for sql in [
            "SAVEPOINT a",
            "  savepoint a;",
            "RELEASE a",
            "RELEASE SAVEPOINT a",
            "ROLLBACK TO a",
            "rollback transaction to savepoint a",
        ] {
            assert!(is_savepoint_statement(sql), "{sql}");
        }
        for sql in [
            "ROLLBACK",
            "ROLLBACK TRANSACTION",
            "BEGIN",
            "COMMIT",
            "SELECT 'SAVEPOINT'",
            "INSERT INTO release_notes(v) VALUES (1)",
        ] {
            assert!(!is_savepoint_statement(sql), "{sql}");
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use turso::{Builder, Connection, Value};

    use super::{Conn, hook, is_tempdir_refusal};
    use crate::store::ReadStore;
    use crate::store::{DomainKind, Store};

    /// Set in the child run of
    /// [`a_refused_temp_directory_is_retried_once_and_leaves_no_trace`],
    /// holding the folder the child's temp location points at.
    const CHILD: &str = "CRYSTALLINE_TEST_TEMPDIR_REFUSED";
    /// Set beside [`CHILD`]: a folder for the file-backed databases, outside
    /// the locked temp location.
    const CHILD_DB: &str = "CRYSTALLINE_TEST_TEMPDIR_DB";

    fn lock(dir: &Path) {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    }

    fn unlock(dir: &Path) {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    type Action = Box<dyn FnOnce() + Send>;

    /// What the hook saw, and what it does on the next "retry" or "replay".
    #[derive(Clone)]
    struct Watch {
        dir: PathBuf,
        seen: Arc<Mutex<Vec<String>>>,
        /// Unlock the temp location before the retry.
        heal: Arc<Mutex<bool>>,
        /// Run once before the next replay, after the transaction dropped.
        before_replay: Arc<Mutex<Option<Action>>>,
    }

    impl Watch {
        fn install(dir: &Path) -> Watch {
            let watch = Watch {
                dir: dir.to_path_buf(),
                seen: Arc::default(),
                heal: Arc::new(Mutex::new(true)),
                before_replay: Arc::default(),
            };
            let w = watch.clone();
            hook::set_event(move |what| {
                w.seen.lock().unwrap().push(what.to_string());
                if what == "replay"
                    && let Some(action) = w.before_replay.lock().unwrap().take()
                {
                    action();
                }
                if (what == "retry" || what == "replay") && *w.heal.lock().unwrap() {
                    unlock(&w.dir);
                }
            });
            watch
        }

        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.seen.lock().unwrap())
        }

        fn heal(&self, on: bool) {
            *self.heal.lock().unwrap() = on;
        }

        fn lock(&self) {
            lock(&self.dir);
        }

        fn unlock(&self) {
            unlock(&self.dir);
        }
    }

    async fn texts(conn: &Conn, sql: &str) -> Vec<String> {
        conn.query_all(sql, vec![])
            .await
            .unwrap()
            .iter()
            .map(|r| match r.get_value(0).unwrap() {
                Value::Text(t) => t,
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn in_txn(conn: &Conn) -> bool {
        !conn.conn.is_autocommit().unwrap()
    }

    /// An `IN (SELECT ...)` is answered with an ephemeral table.
    const PICK: &str = "SELECT v FROM t WHERE id IN (SELECT id FROM u) ORDER BY id";

    async fn tables(conn: &Conn) {
        conn.execute(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)",
            (),
        )
        .await
        .unwrap();
        conn.execute("CREATE TABLE u (id INTEGER PRIMARY KEY)", ())
            .await
            .unwrap();
        conn.execute(
            "INSERT INTO t(id, v) VALUES (1, 'one'), (2, 'two'), (3, 'three')",
            (),
        )
        .await
        .unwrap();
        conn.execute("INSERT INTO u(id) VALUES (1), (3)", ())
            .await
            .unwrap();
    }

    /// The scenarios every backing (memory, file with WAL) runs.
    async fn scenarios(conn: &Conn, w: &Watch) {
        tables(conn).await;

        // A read in autocommit mode runs again.
        w.lock();
        assert_eq!(texts(conn, PICK).await, ["one", "three"]);
        assert_eq!(w.take(), ["retry"]);

        // A write in autocommit mode was rolled back and runs again, once.
        w.lock();
        let changed = conn
            .execute(
                "UPDATE t SET v = v || '!' WHERE id IN (SELECT id FROM u)",
                (),
            )
            .await
            .unwrap();
        assert_eq!(changed, 2);
        assert_eq!(w.take(), ["retry"]);
        assert_eq!(
            texts(conn, "SELECT v FROM t ORDER BY id").await,
            ["one!", "two", "three!"],
            "the update applied exactly once"
        );

        // A read inside a transaction makes the engine drop the transaction,
        // so the transaction is replayed: the insert before the read is kept
        // and the one after it lands in the same transaction.
        conn.execute("BEGIN", ()).await.unwrap();
        conn.execute("INSERT INTO t(id, v) VALUES (4, 'four')", ())
            .await
            .unwrap();
        conn.execute("INSERT INTO u(id) VALUES (4)", ())
            .await
            .unwrap();
        w.lock();
        assert_eq!(
            texts(conn, PICK).await,
            ["one!", "three!", "four"],
            "the replayed transaction is the one the read sees"
        );
        assert_eq!(w.take(), ["replay"]);
        conn.execute("INSERT INTO t(id, v) VALUES (5, 'five')", ())
            .await
            .unwrap();
        assert!(in_txn(conn), "still in the transaction");
        conn.execute("COMMIT", ()).await.unwrap();
        assert_eq!(
            texts(conn, "SELECT v FROM t ORDER BY id").await,
            ["one!", "two", "three!", "four", "five"]
        );

        // A write with a statement savepoint inside a transaction: the engine
        // undid only the statement and kept the transaction, so the statement
        // runs again and the transaction's earlier write is kept.
        conn.execute("BEGIN", ()).await.unwrap();
        conn.execute("INSERT INTO u(id) VALUES (2)", ())
            .await
            .unwrap();
        w.lock();
        let changed = conn
            .execute(
                "UPDATE t SET v = v || '?' WHERE id IN (SELECT id FROM u)",
                (),
            )
            .await
            .unwrap();
        assert_eq!(changed, 4);
        assert_eq!(w.take(), ["retry"], "only the statement was undone");
        conn.execute("COMMIT", ()).await.unwrap();
        assert_eq!(
            texts(conn, "SELECT v FROM t ORDER BY id").await,
            ["one!?", "two?", "three!?", "four?", "five"]
        );

        // A single-row write with no statement savepoint drops the
        // transaction: the transaction is replayed and the write runs again.
        // A `RETURNING` is what gives it an ephemeral table.
        conn.execute("BEGIN", ()).await.unwrap();
        conn.execute("INSERT INTO t(id, v) VALUES (10, 'ten')", ())
            .await
            .unwrap();
        w.lock();
        let rows = conn
            .query_all("INSERT INTO t(v) VALUES ('eleven') RETURNING id", vec![])
            .await
            .unwrap();
        assert_eq!(rows[0].get_value(0).unwrap(), Value::Integer(11));
        assert_eq!(w.take(), ["replay"], "the single-row write dropped it");
        conn.execute("COMMIT", ()).await.unwrap();
        assert_eq!(
            texts(conn, "SELECT v FROM t WHERE id >= 10 ORDER BY id").await,
            ["ten", "eleven"]
        );

        // A replayed `INSERT ... RETURNING` gives the id it gave the first
        // time, and the caller's later use of that id stays right.
        conn.execute("BEGIN", ()).await.unwrap();
        let rows = conn
            .query_all("INSERT INTO t(v) VALUES ('kept') RETURNING id", vec![])
            .await
            .unwrap();
        let Value::Integer(kept) = rows[0].get_value(0).unwrap() else {
            panic!("an integer id");
        };
        w.lock();
        assert_eq!(texts(conn, PICK).await.len(), 4);
        assert_eq!(w.take(), ["replay"]);
        conn.execute("INSERT INTO u(id) VALUES (?1)", vec![Value::Integer(kept)])
            .await
            .unwrap();
        conn.execute("COMMIT", ()).await.unwrap();
        assert_eq!(
            texts(conn, PICK).await.last().map(String::as_str),
            Some("kept"),
            "the id the caller kept points at the replayed row"
        );

        // A rolled back replay leaves nothing behind either.
        conn.execute("BEGIN", ()).await.unwrap();
        conn.execute("INSERT INTO t(id, v) VALUES (20, 'twenty')", ())
            .await
            .unwrap();
        w.lock();
        assert_eq!(texts(conn, PICK).await.len(), 5);
        assert_eq!(w.take(), ["replay"]);
        conn.execute("ROLLBACK", ()).await.unwrap();
        assert!(
            texts(conn, "SELECT v FROM t WHERE id = 20")
                .await
                .is_empty()
        );

        // A refusal that does not go away is returned after one retry, and a
        // transaction it dropped stays dropped.
        w.heal(false);
        w.lock();
        let err = conn.query_all(PICK, vec![]).await.unwrap_err();
        assert!(is_tempdir_refusal(&err), "{err}");
        assert_eq!(w.take(), ["retry"]);
        w.unlock();
        conn.execute("BEGIN", ()).await.unwrap();
        conn.execute("INSERT INTO t(id, v) VALUES (21, 'x')", ())
            .await
            .unwrap();
        w.lock();
        let err = conn.query_all(PICK, vec![]).await.unwrap_err();
        assert!(is_tempdir_refusal(&err), "{err}");
        assert_eq!(w.take(), ["replay"]);
        assert!(!in_txn(conn), "no transaction left open");
        w.unlock();
        assert!(
            texts(conn, "SELECT v FROM t WHERE id = 21")
                .await
                .is_empty()
        );

        // A refusal during the replay itself: the journal holds a statement
        // with an ephemeral table, and the temp location stays locked. The
        // caller gets the original refusal and nothing commits.
        conn.execute("BEGIN", ()).await.unwrap();
        conn.query_all("INSERT INTO t(id, v) VALUES (22, 'x') RETURNING id", vec![])
            .await
            .unwrap();
        w.lock();
        let err = conn.query_all(PICK, vec![]).await.unwrap_err();
        assert!(is_tempdir_refusal(&err), "{err}");
        assert_eq!(w.take(), ["replay", "replay:failed"]);
        assert!(!in_txn(conn), "no transaction left open");
        w.unlock();
        assert!(
            texts(conn, "SELECT v FROM t WHERE id = 22")
                .await
                .is_empty()
        );

        // A journal past the cap is dropped: the caller gets the original
        // refusal and nothing commits.
        w.heal(true);
        hook::set_journal_cap(Some(1024));
        conn.execute("BEGIN", ()).await.unwrap();
        conn.execute(
            "INSERT INTO t(id, v) VALUES (23, ?1)",
            vec![Value::Text("x".repeat(2048))],
        )
        .await
        .unwrap();
        w.lock();
        let err = conn.query_all(PICK, vec![]).await.unwrap_err();
        assert!(is_tempdir_refusal(&err), "{err}");
        assert_eq!(w.take(), ["replay", "replay:unavailable"]);
        assert!(!in_txn(conn), "no transaction left open");
        hook::set_journal_cap(None);
        w.unlock();
        assert!(
            texts(conn, "SELECT v FROM t WHERE id = 23")
                .await
                .is_empty()
        );

        // An unknown connection state is no retry at all, in autocommit mode
        // or in a transaction the refusal dropped: the refusal goes back as it
        // is, nothing commits and no transaction is left open.
        hook::set_autocommit_fails(true);
        w.lock();
        let err = conn.query_all(PICK, vec![]).await.unwrap_err();
        assert!(is_tempdir_refusal(&err), "{err}");
        assert_eq!(w.take(), ["retry:unknown"]);
        hook::set_autocommit_fails(false);
        w.unlock();
        conn.execute("BEGIN", ()).await.unwrap();
        conn.execute("INSERT INTO t(id, v) VALUES (24, 'x')", ())
            .await
            .unwrap();
        hook::set_autocommit_fails(true);
        w.lock();
        let err = conn.query_all(PICK, vec![]).await.unwrap_err();
        assert!(is_tempdir_refusal(&err), "the original refusal: {err}");
        assert_eq!(w.take(), ["retry:unknown"], "neither retried nor replayed");
        hook::set_autocommit_fails(false);
        w.unlock();
        assert!(!in_txn(conn), "no transaction left open");
        assert!(
            texts(conn, "SELECT v FROM t WHERE id = 24")
                .await
                .is_empty(),
            "nothing of the transaction committed"
        );
    }

    /// A replay that does not reproduce the first run is given up: another
    /// connection changes a row the transaction had read, between the drop
    /// and the replay.
    async fn mismatch(conn: &Conn, other: Connection, w: &Watch) {
        conn.execute("BEGIN", ()).await.unwrap();
        conn.execute("INSERT INTO t(id, v) VALUES (30, 'thirty')", ())
            .await
            .unwrap();
        assert_eq!(texts(conn, "SELECT v FROM t WHERE id = 1").await, ["one!?"]);
        *w.before_replay.lock().unwrap() = Some(Box::new(move || {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    other
                        .execute("UPDATE t SET v = 'changed' WHERE id = 1", ())
                        .await
                        .unwrap();
                })
            })
        }));
        w.lock();
        let err = conn.query_all(PICK, vec![]).await.unwrap_err();
        assert!(is_tempdir_refusal(&err), "the original refusal: {err}");
        assert_eq!(w.take(), ["replay", "replay:mismatch"]);
        assert!(!in_txn(conn), "no transaction left open");
        w.unlock();
        assert!(
            texts(conn, "SELECT v FROM t WHERE id = 30")
                .await
                .is_empty(),
            "nothing of the transaction committed"
        );
        assert_eq!(
            texts(conn, "SELECT v FROM t WHERE id = 1").await,
            ["changed"]
        );
    }

    /// The child: runs where the temp location is a folder this test can
    /// lock (no directory can be created in it) and unlock.
    async fn child(dir: PathBuf, db_dir: PathBuf) {
        let w = Watch::install(&dir);

        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = Conn::new(db.connect().unwrap());
        scenarios(&conn, &w).await;

        let path = db_dir.join("wal.db");
        let db = Builder::new_local(path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let conn = Conn::new(db.connect().unwrap());
        let mut rows = conn.raw().query("PRAGMA journal_mode", ()).await.unwrap();
        let mode = rows.next().await.unwrap().unwrap().get_value(0).unwrap();
        drop(rows);
        assert_eq!(mode, Value::Text("wal".to_string()));
        scenarios(&conn, &w).await;
        mismatch(&conn, db.connect().unwrap(), &w).await;

        // Through the store: the domain listing reads with subqueries the
        // engine answers with ephemeral tables.
        let store = crate::turso::TursoStore::open_in_memory().await.unwrap();
        store
            .upsert_domain("notes", None, DomainKind::Virtual)
            .await
            .unwrap();
        w.lock();
        assert_eq!(store.domain_stats().await.unwrap().len(), 1);
        assert_eq!(w.take(), ["retry"]);
        w.unlock();
    }

    /// A statement whose ephemeral table cannot get its temp directory is run
    /// again once, and what it leaves behind is what a clean run leaves.
    ///
    /// On Windows the engine's create of that directory is refused now and
    /// then with "permission denied" (see the module doc). A folder with no
    /// write permission refuses it the same way, every time, on unix; this
    /// runs the scenarios in a child process whose temp location is such a
    /// folder, and a hook unlocks it between the refused attempt and the
    /// retry. The environment can only be changed for a whole process, hence
    /// the child.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refused_temp_directory_is_retried_once_and_leaves_no_trace() {
        if let (Some(dir), Some(db_dir)) = (std::env::var_os(CHILD), std::env::var_os(CHILD_DB)) {
            child(PathBuf::from(dir), PathBuf::from(db_dir)).await;
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("temp");
        let db_dir = root.path().join("db");
        std::fs::create_dir(&dir).unwrap();
        std::fs::create_dir(&db_dir).unwrap();
        lock(&dir);
        if std::fs::create_dir(dir.join("probe")).is_ok() {
            // Root ignores the permission bits, so nothing can be refused.
            unlock(&dir);
            eprintln!("skipped: a folder without write permission accepts a create here");
            return;
        }
        unlock(&dir);
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "turso::conn::tests::a_refused_temp_directory_is_retried_once_and_leaves_no_trace",
                "--exact",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(CHILD, &dir)
            .env(CHILD_DB, &db_dir)
            .env("TMPDIR", &dir)
            .env_remove("TURSO_TMPDIR")
            .env_remove("SQLITE_TMPDIR")
            .output()
            .unwrap();
        unlock(&dir);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "the child failed:\n{stdout}\n{stderr}"
        );
        assert!(
            stdout.contains("1 passed"),
            "the child ran the test rather than filtering it out:\n{stdout}"
        );
    }
}
