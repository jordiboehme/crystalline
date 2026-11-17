//! One active instance per database: the owner lock.
//!
//! The active instance holds a session advisory lock on [`owner_key`] of its
//! schema, taken with `pg_try_advisory_lock` on one bare connection (the lock
//! connection), never on a pool connection, whose recycling would drop the
//! lock unnoticed. [`PgOwner`] keeps that connection on a std thread of its
//! own with a current-thread runtime, so it works the same under the daemon
//! and under any CLI runtime. The thread only answers requests and probes;
//! it never calls the engine or a secret store.
//!
//! While it holds the lock, the thread probes every [`PROBE_PERIOD`] that the
//! lock session still holds the key. A probe that answers false, or a lock
//! connection that broke, fences at once; a probe that timed out or failed on
//! an open connection is retried once at once ([`ProbeState`]). Fencing flips
//! the [`FenceState`] watch and drops the connection; the lock connection
//! never reconnects.
//!
//! A new owner ends its predecessor's sessions ([`PgOwner::takeover`]): every
//! session tagged with the owner key whose instance tag is not its own,
//! except the `standby` and `probe` roles. Sessions tagged `none` count as
//! predecessors, because they are unfenced writers.
//!
//! The owner thread also serves the side channel ([`SideChannel`]): a second
//! connection, in the `side` role and fenced like the pool, on which the
//! active owner loads and saves the configuration document
//! (`instance_config`) and reads and writes the sealed secrets
//! (`instance_secret`). It runs as its own task on the owner runtime, so a
//! slow side statement never holds up a probe. The side channel never
//! migrates; the owner migrates when it opens the store, before the first
//! side call.
//!
//! The timing constants are the shipped values; [`OwnerTimings`] scales them
//! down by [`timing_scale`] for tests and derives the fence detection bound
//! and the grace from them.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::{AssertSqlSafe, Connection, PgConnection};
use tokio::sync::{mpsc, oneshot, watch};

use super::session::{
    HOLDS_KEY_SQL, OwnerFence, PgRole, PgSessionOptions, PgTarget, application_name,
    application_prefix, connect_one, owner_key, short_tag,
};
use crate::error::{IndexError, Result};

/// How often the owner probes that its lock session still holds the lock.
pub const PROBE_PERIOD: Duration = Duration::from_secs(5);

/// How long the owner waits for one probe, the client side. The server side
/// is the lock connection's 3 s `statement_timeout`.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

/// How often a probe that timed out or failed on an open connection is
/// retried, at once, before the owner fences.
pub const PROBE_RETRIES: u32 = 1;

/// The longest an owner takes to notice it lost the lock: one period, then
/// the probe and its retries all running into the timeout.
pub const FENCE_DETECT_MAX: Duration =
    PROBE_PERIOD.saturating_add(PROBE_TIMEOUT.saturating_mul(1 + PROBE_RETRIES));

/// The time a fenced owner takes from noticing to its exit.
pub const EXIT_MARGIN: Duration = Duration::from_secs(2);

/// How long a new owner waits after ending its predecessor's sessions when
/// the predecessor did not release cleanly: by then a predecessor that was
/// merely paused has noticed and stopped.
pub const GRACE: Duration = FENCE_DETECT_MAX.saturating_add(EXIT_MARGIN);

/// How often a standby tries the lock.
pub const STANDBY_POLL: Duration = Duration::from_secs(2);

/// The lock connection's `idle_session_timeout`: a lock session that nobody
/// probes for this long is ended by the server, and the lock with it.
pub const LOCK_IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// The lock connection's `statement_timeout`, the server side of a probe:
/// below [`PROBE_TIMEOUT`], so the server gives up on a probe before the
/// client does.
pub const LOCK_STATEMENT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long `pg_terminate_backend` waits for each predecessor session to be
/// gone, in milliseconds.
pub const TERMINATE_WAIT_MS: i64 = 5000;

/// The side channel's `statement_timeout`, set at connect. Not scaled.
pub const SIDE_STATEMENT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a caller of a sealed-secret side-channel call
/// ([`SideChannel::vault_load`] and its siblings) waits for the owner
/// thread. No worse than the keychain's own bound. Not scaled.
pub const VAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the caller of a blocking side-channel call waits past its own
/// timeout for the owner thread's answer. The owner thread gives up the
/// request at the timeout itself, so within this margin the caller learns
/// what happened instead of guessing.
const SIDE_REPLY_MARGIN: Duration = Duration::from_millis(500);

/// The test-only divisor of the timings, honored in debug builds only.
pub const TIMING_SCALE_ENV: &str = "CRYSTALLINE_TEST_OWNER_TIMING_SCALE";

/// The divisor of the owner timings: [`TIMING_SCALE_ENV`] in a debug build
/// when it holds a positive integer, else 1. A release build always uses 1.
pub fn timing_scale() -> u32 {
    if !cfg!(debug_assertions) {
        return 1;
    }
    std::env::var(TIMING_SCALE_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1)
}

/// The owner timings at one scale. The base values are divided by the scale,
/// and the fence detection bound and the grace are derived from the divided
/// values, so the inequalities between them hold at any scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnerTimings {
    /// [`PROBE_PERIOD`], scaled.
    pub probe_period: Duration,
    /// [`PROBE_TIMEOUT`], scaled.
    pub probe_timeout: Duration,
    /// [`FENCE_DETECT_MAX`], derived from the scaled values.
    pub fence_detect_max: Duration,
    /// [`EXIT_MARGIN`], scaled.
    pub exit_margin: Duration,
    /// [`GRACE`], derived from the scaled values.
    pub grace: Duration,
    /// [`STANDBY_POLL`], scaled.
    pub standby_poll: Duration,
    /// [`LOCK_IDLE_TIMEOUT`], scaled.
    pub lock_idle_timeout: Duration,
    /// [`LOCK_STATEMENT_TIMEOUT`], scaled.
    pub lock_statement_timeout: Duration,
}

impl OwnerTimings {
    /// The timings divided by `scale` (0 counts as 1).
    pub fn at(scale: u32) -> OwnerTimings {
        let scale = scale.max(1);
        let probe_period = PROBE_PERIOD / scale;
        let probe_timeout = PROBE_TIMEOUT / scale;
        let exit_margin = EXIT_MARGIN / scale;
        let fence_detect_max = probe_period + probe_timeout * (1 + PROBE_RETRIES);
        OwnerTimings {
            probe_period,
            probe_timeout,
            fence_detect_max,
            exit_margin,
            grace: fence_detect_max + exit_margin,
            standby_poll: STANDBY_POLL / scale,
            lock_idle_timeout: LOCK_IDLE_TIMEOUT / scale,
            lock_statement_timeout: LOCK_STATEMENT_TIMEOUT / scale,
        }
    }

    /// The timings at [`timing_scale`].
    pub fn current() -> OwnerTimings {
        OwnerTimings::at(timing_scale())
    }
}

/// A duration as a Postgres setting: whole seconds as `Ns`, else `Nms`, and
/// never below 1 ms, because 0 turns a timeout off.
fn pg_duration(d: Duration) -> String {
    let ms = d.as_millis().max(1);
    if ms.is_multiple_of(1000) {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    }
}

/// The lock connection's `idle_session_timeout` and `statement_timeout` at
/// `t`: set at connect for a `lock` connection and again on a session that
/// wins the lock, so a paused owner's lock session is ended after the scaled
/// [`LOCK_IDLE_TIMEOUT`] in a test too.
pub(crate) fn lock_session_settings(t: &OwnerTimings) -> [(&'static str, String); 2] {
    [
        ("idle_session_timeout", pg_duration(t.lock_idle_timeout)),
        ("statement_timeout", pg_duration(t.lock_statement_timeout)),
    ]
}

/// The side channel's `statement_timeout` as a startup option.
pub(crate) fn side_session_setting() -> (&'static str, String) {
    ("statement_timeout", pg_duration(SIDE_STATEMENT_TIMEOUT))
}

/// What one probe of the lock found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The lock session holds the lock.
    Held,
    /// The lock session answered and does not hold the lock.
    NotHeld,
    /// The probe timed out or failed while the connection stayed open.
    Timeout,
    /// The lock connection is gone, and its lock with it.
    Broken,
}

/// What the owner does after a probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeAction {
    /// Probe again at the next period.
    Continue,
    /// Probe again at once, on the same connection.
    RetryNow,
    /// Stop: the instance no longer owns the database, for the reason given.
    Fence(String),
}

/// The probe verdict: false or a broken connection fences at once, a timeout
/// is retried [`PROBE_RETRIES`] times before it fences, and a held answer
/// starts the count again.
#[derive(Clone, Debug, Default)]
pub struct ProbeState {
    failures: u32,
}

impl ProbeState {
    /// A state with no failure counted.
    pub fn new() -> ProbeState {
        ProbeState::default()
    }

    /// The action after the probe outcome `o`.
    pub fn next(&mut self, o: ProbeOutcome) -> ProbeAction {
        match o {
            ProbeOutcome::Held => {
                self.failures = 0;
                ProbeAction::Continue
            }
            ProbeOutcome::NotHeld => {
                ProbeAction::Fence("the lock session no longer holds the owner lock".to_string())
            }
            ProbeOutcome::Broken => ProbeAction::Fence("the lock connection was lost".to_string()),
            ProbeOutcome::Timeout if self.failures < PROBE_RETRIES => {
                self.failures += 1;
                ProbeAction::RetryNow
            }
            ProbeOutcome::Timeout => ProbeAction::Fence(format!(
                "the ownership probe failed {} times in a row",
                PROBE_RETRIES + 1
            )),
        }
    }
}

/// Who this instance is, as the owner row records it.
#[derive(Clone, Debug)]
pub struct OwnerIdentity {
    /// The instance id; its first 8 characters tag every connection.
    pub instance_id: String,
    /// The instance's label, for people reading the owner row.
    pub label: String,
    /// The command that holds the lock, for a CLI process or an embedded
    /// session; `None` for a daemon.
    pub command: Option<String>,
    /// The Crystalline version.
    pub version: String,
}

/// The `instance_owner` row: who holds, or last held, the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerRow {
    /// The holder's instance id.
    pub instance_id: String,
    /// The holder's label.
    pub label: String,
    /// The command that held the lock, if a command did.
    pub command: Option<String>,
    /// The holder's process id.
    pub pid: i64,
    /// The backend pid of the holder's lock session.
    pub backend_pid: i64,
    /// The holder's Crystalline version.
    pub version: String,
    /// When the holder took the lock.
    pub since: DateTime<Utc>,
    /// When the holder released the lock cleanly; `None` while it holds it
    /// or when it stopped without releasing.
    pub released_at: Option<DateTime<Utc>>,
}

/// The answer to a lock attempt.
#[derive(Clone, Debug)]
pub enum Acquired {
    /// This instance holds the lock now, on the session `lock_pid`.
    /// `previous` is the owner row as the predecessor left it.
    Held {
        /// The backend pid of the lock session.
        lock_pid: i32,
        /// The owner row before this instance announced itself.
        previous: Option<OwnerRow>,
    },
    /// Another session holds the lock; the owner row, if there is one.
    Busy(Option<OwnerRow>),
}

/// Whether the instance still owns the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FenceState {
    /// No loss was seen.
    Holding,
    /// The lock was lost, for the reason given. Final.
    Fenced(String),
}

/// What a waiting standby reports while it polls.
#[derive(Clone, Debug, Default)]
pub struct StandbyStatus {
    /// When the wait started.
    pub waiting_since: DateTime<Utc>,
    /// The owner row the last poll read; `None` when the holder has not
    /// announced itself or the table does not exist.
    pub owner: Option<OwnerRow>,
    /// How many polls ran.
    pub polls: u64,
    /// Why the last poll failed, when it did; cleared by the next poll that
    /// reaches the database.
    pub last_error: Option<String>,
}

impl StandbyStatus {
    /// A status whose wait starts now.
    pub fn new() -> StandbyStatus {
        StandbyStatus {
            waiting_since: Utc::now(),
            ..StandbyStatus::default()
        }
    }
}

/// What [`PgOwner::takeover`] did.
#[derive(Clone, Copy, Debug)]
pub struct TakeoverReport {
    /// How many predecessor sessions were ended.
    pub terminated: u32,
    /// How long the takeover waited after ending them.
    pub waited: Duration,
}

/// Is any session holding the owner key `$1` in this database?
const HELD_BY_ANYONE_SQL: &str = "SELECT EXISTS (SELECT 1 FROM pg_locks \
     WHERE locktype = 'advisory' AND granted \
     AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
     AND classid::bigint = (($1 >> 32) & 4294967295) \
     AND objid::bigint = ($1 & 4294967295) AND objsubid = 1)";

/// End every session of this database whose `application_name` starts with
/// the key prefix `$1`, whose instance tag is not `$2` and whose role is not
/// `standby` or `probe`, waiting `$3` ms for each to be gone. The targets
/// are materialized first, so the filter runs before any termination.
/// Answers how many were ended.
const TERMINATE_SQL: &str = "WITH targets AS MATERIALIZED (\
       SELECT pid FROM pg_stat_activity \
       WHERE datname = current_database() AND pid <> pg_backend_pid() \
       AND starts_with(application_name, $1) \
       AND split_part(application_name, '/', 3) <> $2 \
       AND split_part(application_name, '/', 4) NOT IN ('standby', 'probe')) \
     SELECT count(*) FILTER (WHERE ended) FROM \
       (SELECT pg_terminate_backend(pid, $3) AS ended FROM targets) t";

/// The error of the takeover's termination statement. A refused termination
/// (SQLSTATE 42501: the role may not end the predecessor's sessions) becomes
/// a sentence that names the database and the remedy, so the start stops
/// with it; every other error stays as it is.
fn terminate_error(e: sqlx::Error, database: &str) -> IndexError {
    let refused = matches!(&e, sqlx::Error::Database(db) if db.code().as_deref() == Some("42501"));
    if refused {
        return terminate_refused(database);
    }
    IndexError::from(e)
}

/// The sentence of a refused takeover termination on `database`.
fn terminate_refused(database: &str) -> IndexError {
    IndexError::Invalid(format!(
        "this instance took the database {database} but may not end the sessions of the instance \
         before it: grant its database role pg_signal_backend, or run every instance on this \
         database as the same role (when the instance before ran as a superuser, only the \
         same role or a superuser may end its sessions)"
    ))
}

/// The instance table `name` of `schema`, qualified so a missing table in
/// the schema never resolves to one in `public`.
fn instance_table(schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(s) => format!("{}.{name}", super::quote_ident(s)),
        None => name.to_string(),
    }
}

/// The `instance_owner` table of `schema`.
fn owner_table(schema: Option<&str>) -> String {
    instance_table(schema, "instance_owner")
}

/// The `instance_config` table of `schema`.
fn config_table(schema: Option<&str>) -> String {
    instance_table(schema, "instance_config")
}

/// The `instance_secret` table of `schema`.
fn secret_table(schema: Option<&str>) -> String {
    instance_table(schema, "instance_secret")
}

/// Whether `table` exists. Never creates anything.
async fn table_exists(conn: &mut PgConnection, table: &str) -> Result<bool> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(table)
        .fetch_one(&mut *conn)
        .await?;
    Ok(exists)
}

/// The microseconds since the epoch as a time.
fn from_micros(us: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp_micros(us)
        .ok_or_else(|| IndexError::Db(format!("a stored time is out of range: {us}")))
}

/// The owner row from `table`, when the table exists and has it.
async fn read_row(conn: &mut PgConnection, table: &str) -> Result<Option<OwnerRow>> {
    if !table_exists(conn, table).await? {
        return Ok(None);
    }
    type Raw = (
        String,
        String,
        Option<String>,
        i64,
        i64,
        String,
        i64,
        Option<i64>,
    );
    let sql = format!(
        "SELECT instance_id, label, command, pid, backend_pid, version, \
         (EXTRACT(EPOCH FROM since) * 1000000)::int8, \
         (EXTRACT(EPOCH FROM released_at) * 1000000)::int8 \
         FROM {table} WHERE id = 1"
    );
    let raw: Option<Raw> = sqlx::query_as(AssertSqlSafe(sql))
        .fetch_optional(&mut *conn)
        .await?;
    let Some((instance_id, label, command, pid, backend_pid, version, since, released)) = raw
    else {
        return Ok(None);
    };
    Ok(Some(OwnerRow {
        instance_id,
        label,
        command,
        pid,
        backend_pid,
        version,
        since: from_micros(since)?,
        released_at: released.map(from_micros).transpose()?,
    }))
}

/// Whether `e` means the connection itself is gone: a socket or protocol
/// failure, a connection exception (class 08) or a session the server ended
/// (class 57P, for example `pg_terminate_backend` or `idle_session_timeout`).
fn is_broken(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Database(db) => db
            .code()
            .is_some_and(|c| c.starts_with("08") || c.starts_with("57P")),
        sqlx::Error::Io(_)
        | sqlx::Error::Protocol(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::WorkerCrashed => true,
        _ => false,
    }
}

/// Whether `e` is the server's cancel of a statement (SQLSTATE 57014), as its
/// `statement_timeout` raises it.
fn is_query_canceled(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.code().as_deref() == Some("57014"))
}

/// Take a stale cancel off `conn` before a probe is retried. A probe given up
/// on the client side leaves its answer on the wire, and sqlx hands a stale
/// error to the next statement: when the server's own `statement_timeout`
/// ended that probe, its 57014 would fail the retry at once and count as a
/// second failure. One `SELECT 1` takes it; a 57014 here (the stale one, or
/// this statement's own on a server that is still slow) is ignored, so the
/// retry reads the lock and not the old error.
async fn drain_stale_cancel(conn: &mut PgConnection) -> std::result::Result<(), sqlx::Error> {
    match sqlx::query("SELECT 1").execute(conn).await {
        Ok(_) => Ok(()),
        Err(e) if is_query_canceled(&e) => Ok(()),
        Err(e) => Err(e),
    }
}

/// The lock-free view of `target`: `None` when no session holds the owner
/// lock, else the owner row (`None` inside when the holder has not announced
/// itself). Uses a `probe` connection and creates nothing.
pub async fn probe_owner(target: &PgTarget) -> Result<Option<Option<OwnerRow>>> {
    let mut conn = connect_one(target, PgRole::Probe, &PgSessionOptions::default()).await?;
    let key = owner_key(target.schema.as_deref());
    let table = owner_table(target.schema.as_deref());
    let found = async {
        let held: bool = sqlx::query_scalar(HELD_BY_ANYONE_SQL)
            .bind(key)
            .fetch_one(&mut conn)
            .await?;
        if !held {
            return Ok(None);
        }
        Ok(Some(read_row(&mut conn, &table).await?))
    }
    .await;
    let _ = conn.close().await;
    found
}

/// The owner row of `target`, read on a `probe` connection without the lock.
/// `None` when the table or the row does not exist.
pub async fn read_owner_row(target: &PgTarget) -> Result<Option<OwnerRow>> {
    let mut conn = connect_one(target, PgRole::Probe, &PgSessionOptions::default()).await?;
    let row = read_row(&mut conn, &owner_table(target.schema.as_deref())).await;
    let _ = conn.close().await;
    row
}

/// The configuration document: the instance's settings and domain registry,
/// one row of `instance_config`.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigDoc {
    /// The document itself.
    pub body: serde_json::Value,
    /// When it was last saved.
    pub updated_at: DateTime<Utc>,
    /// Who saved it last.
    pub updated_by: String,
}

/// The configuration document from `table`, when it has one. A missing
/// table is `None` for a lock-free reader (`required` off) and an error for
/// the owner's side channel, whose owner migrated before it serves. Never
/// creates anything.
async fn read_config(
    conn: &mut PgConnection,
    table: &str,
    required: bool,
) -> Result<Option<ConfigDoc>> {
    if !table_exists(conn, table).await? {
        if required {
            return Err(IndexError::Db(format!(
                "the configuration table {table} does not exist: the active instance migrates \
                 the database before it reads its configuration"
            )));
        }
        return Ok(None);
    }
    let sql = format!(
        "SELECT body::text, (EXTRACT(EPOCH FROM updated_at) * 1000000)::int8, updated_by \
         FROM {table} WHERE id = 1"
    );
    let raw: Option<(String, i64, String)> = sqlx::query_as(AssertSqlSafe(sql))
        .fetch_optional(&mut *conn)
        .await?;
    let Some((body, updated_at, updated_by)) = raw else {
        return Ok(None);
    };
    Ok(Some(ConfigDoc {
        body: serde_json::from_str(&body)?,
        updated_at: from_micros(updated_at)?,
        updated_by,
    }))
}

/// Open a transaction on `conn` and replace the configuration document in
/// `table` with `body` (JSON text), saved by `by`, without committing. A
/// connection dropped before its `COMMIT` leaves nothing written.
async fn stage_config(conn: &mut PgConnection, table: &str, body: &str, by: &str) -> Result<()> {
    let sql = format!(
        "INSERT INTO {table} (id, body, updated_at, updated_by) \
         VALUES (1, $1::jsonb, now(), $2) \
         ON CONFLICT (id) DO UPDATE SET body = EXCLUDED.body, \
         updated_at = EXCLUDED.updated_at, updated_by = EXCLUDED.updated_by"
    );
    sqlx::raw_sql("BEGIN").execute(&mut *conn).await?;
    sqlx::query(AssertSqlSafe(sql))
        .bind(body)
        .bind(by)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// The result of a `COMMIT` that was sent: `None` when it did not answer
/// within [`SIDE_STATEMENT_TIMEOUT`]. The server's own error (a deferred
/// constraint, a serialization failure) means the transaction was rolled
/// back, so the write is known not to be stored. A timeout, a broken
/// connection or any other client-side failure leaves the outcome unknown:
/// [`IndexError::MaybeStored`].
fn commit_outcome(answer: Option<std::result::Result<(), sqlx::Error>>) -> Result<()> {
    match answer {
        Some(Ok(())) => Ok(()),
        Some(Err(e)) if matches!(e, sqlx::Error::Database(_)) && !is_broken(&e) => Err(e.into()),
        Some(Err(e)) => Err(maybe_stored(&e.to_string())),
        None => Err(maybe_stored("the commit did not answer in time")),
    }
}

/// The error of a save whose `COMMIT` went out with no known outcome.
fn maybe_stored(why: &str) -> IndexError {
    IndexError::MaybeStored(format!(
        "the save may have been stored: {why}; read the configuration again to see whether it was"
    ))
}

/// The configuration document of `target`, read on a `probe` connection
/// without the lock, for a view that runs without the active instance.
/// `None` when the table or the row does not exist.
pub async fn read_config_document(target: &PgTarget) -> Result<Option<ConfigDoc>> {
    let mut conn = connect_one(target, PgRole::Probe, &PgSessionOptions::default()).await?;
    let doc = read_config(&mut conn, &config_table(target.schema.as_deref()), false).await;
    let _ = conn.close().await;
    doc
}

/// How many sealed secrets `instance_secret` of `target` holds, counted on a
/// `probe` connection without the lock, for `status` and `doctor`: never a
/// row name, never a sealed byte. `0` when the table does not exist yet.
pub async fn count_sealed_secrets(target: &PgTarget) -> Result<u64> {
    let mut conn = connect_one(target, PgRole::Probe, &PgSessionOptions::default()).await?;
    let table = secret_table(target.schema.as_deref());
    let counted = async {
        if !table_exists(&mut conn, &table).await? {
            return Ok(0);
        }
        let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*)::int8 FROM {table}"
        )))
        .fetch_one(&mut conn)
        .await?;
        Ok(u64::try_from(rows).unwrap_or(0))
    }
    .await;
    let _ = conn.close().await;
    counted
}

/// One row of `instance_secret`: a sealed secret and the id of the key that
/// sealed it. The index never opens it.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretRow {
    /// The sealed bytes.
    pub sealed: Vec<u8>,
    /// The id of the key the row was sealed with.
    pub key_id: String,
}

/// Keeps the sealed bytes out of `Debug` output.
impl std::fmt::Debug for SecretRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretRow")
            .field("sealed", &format_args!("<{} bytes>", self.sealed.len()))
            .field("key_id", &self.key_id)
            .finish()
    }
}

/// What `instance_secret` holds, without a row name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretSummary {
    /// How many rows there are.
    pub rows: u64,
    /// The distinct key ids that sealed them, sorted.
    pub key_ids: Vec<String>,
}

async fn load_secret(
    conn: &mut PgConnection,
    table: &str,
    name: &str,
) -> Result<Option<SecretRow>> {
    let sql = format!("SELECT sealed, key_id FROM {table} WHERE name = $1");
    let row: Option<(Vec<u8>, String)> = sqlx::query_as(AssertSqlSafe(sql))
        .bind(name)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.map(|(sealed, key_id)| SecretRow { sealed, key_id }))
}

/// Write the row `name`, unless a row of that name was sealed with another
/// key: one statement, so no other writer slips in between the check and the
/// write. True when the row was written.
async fn save_secret(
    conn: &mut PgConnection,
    table: &str,
    name: &str,
    sealed: &[u8],
    key_id: &str,
) -> Result<bool> {
    let sql = format!(
        "INSERT INTO {table} AS s (name, sealed, key_id, updated_at) \
         VALUES ($1, $2, $3, now()) \
         ON CONFLICT (name) DO UPDATE SET sealed = EXCLUDED.sealed, \
         key_id = EXCLUDED.key_id, updated_at = EXCLUDED.updated_at \
         WHERE s.key_id = EXCLUDED.key_id"
    );
    let done = sqlx::query(AssertSqlSafe(sql))
        .bind(name)
        .bind(sealed)
        .bind(key_id)
        .execute(&mut *conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// Delete the row `name` when it was sealed with `key_id`. True when a row
/// was deleted.
async fn delete_secret(
    conn: &mut PgConnection,
    table: &str,
    name: &str,
    key_id: &str,
) -> Result<bool> {
    let sql = format!("DELETE FROM {table} WHERE name = $1 AND key_id = $2");
    let done = sqlx::query(AssertSqlSafe(sql))
        .bind(name)
        .bind(key_id)
        .execute(&mut *conn)
        .await?;
    Ok(done.rows_affected() > 0)
}

async fn summarize_secrets(conn: &mut PgConnection, table: &str) -> Result<SecretSummary> {
    let sql = format!("SELECT key_id, count(*)::int8 FROM {table} GROUP BY key_id ORDER BY key_id");
    let groups: Vec<(String, i64)> = sqlx::query_as(AssertSqlSafe(sql))
        .fetch_all(&mut *conn)
        .await?;
    Ok(SecretSummary {
        rows: groups
            .iter()
            .map(|(_, n)| u64::try_from(*n).unwrap_or(0))
            .sum(),
        key_ids: groups.into_iter().map(|(id, _)| id).collect(),
    })
}

/// Where the side task sends its answer: a std channel for a blocking
/// caller, which may sit inside any runtime, or a oneshot for an async one.
enum SideReply<T> {
    Blocking(std::sync::mpsc::SyncSender<Result<T>>),
    Async(oneshot::Sender<Result<T>>),
}

impl<T> SideReply<T> {
    fn send(self, answer: Result<T>) {
        match self {
            SideReply::Blocking(tx) => {
                let _ = tx.try_send(answer);
            }
            SideReply::Async(tx) => {
                let _ = tx.send(answer);
            }
        }
    }
}

/// What a side-channel request asks for.
enum SideOp {
    /// Read the configuration document.
    Load { reply: SideReply<Option<ConfigDoc>> },
    /// Replace the configuration document.
    Save {
        body: String,
        by: String,
        gate: CommitGate,
        reply: SideReply<()>,
    },
    /// Read one sealed secret.
    SecretLoad {
        name: String,
        reply: SideReply<Option<SecretRow>>,
    },
    /// Write one sealed secret unless another key sealed the row.
    SecretSave {
        name: String,
        sealed: Vec<u8>,
        key_id: String,
        reply: SideReply<bool>,
    },
    /// Delete one sealed secret sealed with `key_id`.
    SecretDelete {
        name: String,
        key_id: String,
        reply: SideReply<bool>,
    },
    /// Count the sealed secrets by key.
    SecretSummary { reply: SideReply<SecretSummary> },
}

/// Who decides about a save's `COMMIT` first: the side task, which sends it,
/// or the caller, who stops waiting. Exactly one of them wins, so a caller
/// that gave up is told "not stored" only when the commit was never sent.
#[derive(Clone, Default)]
struct CommitGate(Arc<AtomicU8>);

impl CommitGate {
    const PENDING: u8 = 0;
    const SENT: u8 = 1;
    const ABANDONED: u8 = 2;

    /// The side task's claim before it sends `COMMIT`: false when the caller
    /// already gave up, and then no commit is sent.
    fn send_commit(&self) -> bool {
        self.0
            .compare_exchange(
                Self::PENDING,
                Self::SENT,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// The caller's claim when it stops waiting: true when no commit was
    /// sent and none will be, false when the commit is already out.
    fn abandon(&self) -> bool {
        self.0
            .compare_exchange(
                Self::PENDING,
                Self::ABANDONED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

/// A side-channel request and the moment its caller stops waiting.
struct SideRequest {
    deadline: Instant,
    op: SideOp,
}

/// `timeout` from now, or a day from now when that overflows.
fn deadline_after(timeout: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(timeout)
        .unwrap_or_else(|| now + Duration::from_secs(86_400))
}

/// The error of a side-channel request that ran out of time.
fn side_timed_out() -> IndexError {
    IndexError::Db("the database did not answer the side channel in time".to_string())
}

/// The active owner's way to the configuration document and the sealed
/// secrets, served on the owner
/// thread by a second connection in the `side` role: fenced, tagged with the
/// instance and with a 5 s `statement_timeout`
/// ([`SIDE_STATEMENT_TIMEOUT`]). It answers only while the owner holds the
/// lock; the connection is dropped when the lock is released or lost and
/// after any failed or timed-out request. The owner's `instance_config`
/// table must exist: the side channel reports a missing one as an error.
///
/// The handle is `Clone` and `Send`. The blocking calls work from any thread
/// and inside any runtime, except on the owner thread itself: there they
/// return an error at once, because the owner thread would wait for its own
/// answer forever. The owner thread never calls the side channel.
///
/// The side channel never migrates: the table comes from the owner's
/// migrations, which run before its first call.
#[derive(Clone)]
pub struct SideChannel {
    requests: mpsc::UnboundedSender<SideRequest>,
    owner_thread: ThreadId,
}

impl SideChannel {
    /// Send `op` and wait at most `timeout` plus [`SIDE_REPLY_MARGIN`] for
    /// the owner thread's verdict on this thread. A save's `gate` decides
    /// what giving up means: nothing stored, or, once its commit is out,
    /// [`IndexError::MaybeStored`].
    fn blocking<T>(
        &self,
        timeout: Duration,
        gate: Option<&CommitGate>,
        make: impl FnOnce(SideReply<T>) -> SideOp,
    ) -> Result<T> {
        if std::thread::current().id() == self.owner_thread {
            return Err(IndexError::Invalid(
                "side channel called from the owner thread".to_string(),
            ));
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let request = SideRequest {
            deadline: deadline_after(timeout),
            op: make(SideReply::Blocking(tx)),
        };
        self.requests.send(request).map_err(|_| thread_gone())?;
        let gave_up = match rx.recv_timeout(timeout.saturating_add(SIDE_REPLY_MARGIN)) {
            Ok(answer) => return answer,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => side_timed_out(),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => thread_gone(),
        };
        match gate {
            Some(gate) if !gate.abandon() => Err(maybe_stored(
                "the commit was sent and did not answer in time",
            )),
            _ => Err(gave_up),
        }
    }

    /// The configuration document, waiting at most `timeout` on this thread.
    /// `None` when there is none yet.
    pub fn load_config_blocking(&self, timeout: Duration) -> Result<Option<ConfigDoc>> {
        self.blocking(timeout, None, |reply| SideOp::Load { reply })
    }

    /// The configuration document, bounded by [`SIDE_STATEMENT_TIMEOUT`] on
    /// the owner thread and that plus a 500 ms reply margin here. `None` when
    /// there is none yet.
    pub async fn load_config(&self) -> Result<Option<ConfigDoc>> {
        let (tx, rx) = oneshot::channel();
        let request = SideRequest {
            deadline: deadline_after(SIDE_STATEMENT_TIMEOUT),
            op: SideOp::Load {
                reply: SideReply::Async(tx),
            },
        };
        self.requests.send(request).map_err(|_| thread_gone())?;
        match tokio::time::timeout(SIDE_STATEMENT_TIMEOUT.saturating_add(SIDE_REPLY_MARGIN), rx)
            .await
        {
            Ok(answer) => answer.map_err(|_| thread_gone())?,
            Err(_) => Err(side_timed_out()),
        }
    }

    /// Replace the configuration document with `body`, saved by `by`, in one
    /// transaction.
    ///
    /// `timeout` bounds the connect, the `BEGIN` and the write. Once the
    /// `COMMIT` is sent it runs to its end under [`SIDE_STATEMENT_TIMEOUT`],
    /// and this thread waits for it at most a 500 ms reply margin past
    /// `timeout`. Every error leaves the stored document as it was, except
    /// [`IndexError::MaybeStored`]: the commit went out and its outcome is
    /// unknown (no answer in time, or the connection broke), so the caller
    /// reads the document again instead of assuming either outcome.
    pub fn save_config_blocking(
        &self,
        body: &serde_json::Value,
        by: &str,
        timeout: Duration,
    ) -> Result<()> {
        let body = serde_json::to_string(body)?;
        let by = by.to_string();
        let gate = CommitGate::default();
        self.blocking(timeout, Some(&gate.clone()), |reply| SideOp::Save {
            body,
            by,
            gate,
            reply,
        })
    }

    /// The sealed secret `name`, waiting at most [`VAULT_TIMEOUT`] on this
    /// thread. `None` when there is no such row.
    pub fn vault_load(&self, name: &str) -> Result<Option<SecretRow>> {
        let name = name.to_string();
        self.blocking(VAULT_TIMEOUT, None, |reply| SideOp::SecretLoad {
            name,
            reply,
        })
    }

    /// Write the sealed secret `name`, sealed with `key_id`, waiting at most
    /// [`VAULT_TIMEOUT`]. False, and nothing written, when a row of that
    /// name was sealed with another key. A timeout leaves the outcome
    /// unknown: the statement may still have landed.
    pub fn vault_save(&self, name: &str, sealed: &[u8], key_id: &str) -> Result<bool> {
        let (name, sealed, key_id) = (name.to_string(), sealed.to_vec(), key_id.to_string());
        self.blocking(VAULT_TIMEOUT, None, |reply| SideOp::SecretSave {
            name,
            sealed,
            key_id,
            reply,
        })
    }

    /// Delete the sealed secret `name` when it was sealed with `key_id`,
    /// waiting at most [`VAULT_TIMEOUT`]. True when a row was deleted; false
    /// when there was none, or one sealed with another key, which is kept.
    pub fn vault_delete(&self, name: &str, key_id: &str) -> Result<bool> {
        let (name, key_id) = (name.to_string(), key_id.to_string());
        self.blocking(VAULT_TIMEOUT, None, |reply| SideOp::SecretDelete {
            name,
            key_id,
            reply,
        })
    }

    /// How many sealed secrets there are and which keys sealed them, waiting
    /// at most [`VAULT_TIMEOUT`].
    pub fn vault_summary(&self) -> Result<SecretSummary> {
        self.blocking(VAULT_TIMEOUT, None, |reply| SideOp::SecretSummary { reply })
    }
}

/// The side task on the owner thread: one connection, opened on the first
/// request while the owner holds the lock.
struct Side {
    target: PgTarget,
    instance_id: String,
    key: i64,
    table: String,
    /// The `instance_secret` table.
    secrets: String,
    lock_pid: Arc<AtomicI32>,
    /// The open side connection and the lock session it was fenced on.
    conn: Option<(PgConnection, i32)>,
}

impl Side {
    async fn run(mut self, mut inbox: mpsc::UnboundedReceiver<SideRequest>) {
        while let Some(request) = inbox.recv().await {
            self.serve(request).await;
        }
    }

    async fn serve(&mut self, request: SideRequest) {
        let SideRequest { deadline, op } = request;
        match op {
            SideOp::Load { reply } => {
                let answer = bounded(deadline, self.load()).await;
                self.forget_after(&answer);
                reply.send(answer);
            }
            SideOp::Save {
                body,
                by,
                gate,
                reply,
            } => {
                // The caller's deadline bounds the write up to the commit;
                // the commit itself is never cut off by the client.
                let answer = match bounded(deadline, self.stage(&body, &by)).await {
                    Ok(()) => self.commit(&gate).await,
                    Err(e) => Err(e),
                };
                self.forget_after(&answer);
                reply.send(answer);
            }
            SideOp::SecretLoad { name, reply } => {
                let table = self.secrets.clone();
                let answer = bounded(deadline, async {
                    load_secret(self.connection().await?, &table, &name).await
                })
                .await;
                self.forget_after(&answer);
                reply.send(answer);
            }
            SideOp::SecretSave {
                name,
                sealed,
                key_id,
                reply,
            } => {
                let table = self.secrets.clone();
                let answer = bounded(deadline, async {
                    save_secret(self.connection().await?, &table, &name, &sealed, &key_id).await
                })
                .await;
                self.forget_after(&answer);
                reply.send(answer);
            }
            SideOp::SecretDelete {
                name,
                key_id,
                reply,
            } => {
                let table = self.secrets.clone();
                let answer = bounded(deadline, async {
                    delete_secret(self.connection().await?, &table, &name, &key_id).await
                })
                .await;
                self.forget_after(&answer);
                reply.send(answer);
            }
            SideOp::SecretSummary { reply } => {
                let table = self.secrets.clone();
                let answer = bounded(deadline, async {
                    summarize_secrets(self.connection().await?, &table).await
                })
                .await;
                self.forget_after(&answer);
                reply.send(answer);
            }
        }
    }

    /// Drop the connection after a failed or timed-out request: its state is
    /// unknown, and a statement given up on may still answer on the wire.
    fn forget_after<T>(&mut self, answer: &Result<T>) {
        if answer.is_err() {
            self.conn = None;
        }
    }

    /// The side connection, fenced on the current lock session. A connection
    /// opened under another lock session (before a release or a fence) is
    /// dropped; without the lock there is none.
    async fn connection(&mut self) -> Result<&mut PgConnection> {
        let lock_pid = self.lock_pid.load(Ordering::Acquire);
        let kept = self.conn.take().filter(|(_, pid)| *pid == lock_pid);
        if lock_pid == 0 {
            return Err(IndexError::Invalid(
                "this instance does not hold the database".to_string(),
            ));
        }
        let conn = match kept {
            Some((conn, _)) => conn,
            None => {
                let options = PgSessionOptions {
                    instance_tag: Some(self.instance_id.clone()),
                    fence: Some(OwnerFence {
                        key: self.key,
                        lock_pid,
                    }),
                };
                connect_one(&self.target, PgRole::Side, &options).await?
            }
        };
        Ok(&mut self.conn.insert((conn, lock_pid)).0)
    }

    async fn load(&mut self) -> Result<Option<ConfigDoc>> {
        let table = self.table.clone();
        read_config(self.connection().await?, &table, true).await
    }

    async fn stage(&mut self, body: &str, by: &str) -> Result<()> {
        let table = self.table.clone();
        stage_config(self.connection().await?, &table, body, by).await
    }

    /// Commit the staged save, unless its caller already gave up: then the
    /// transaction is left open, and dropping the connection rolls it back.
    async fn commit(&mut self, gate: &CommitGate) -> Result<()> {
        let Some((conn, _)) = self.conn.as_mut() else {
            return Err(IndexError::Invalid("no side connection".to_string()));
        };
        if !gate.send_commit() {
            return Err(side_timed_out());
        }
        let answer = tokio::time::timeout(
            SIDE_STATEMENT_TIMEOUT,
            sqlx::raw_sql("COMMIT").execute(&mut *conn),
        )
        .await
        .ok()
        .map(|r| r.map(|_| ()));
        commit_outcome(answer)
    }
}

/// `work`, given up at `deadline`.
async fn bounded<T>(deadline: Instant, work: impl Future<Output = Result<T>>) -> Result<T> {
    let Some(left) = deadline.checked_duration_since(Instant::now()) else {
        return Err(side_timed_out());
    };
    tokio::time::timeout(left, work)
        .await
        .unwrap_or_else(|_| Err(side_timed_out()))
}

/// A request to the owner thread.
enum Request {
    /// Try the lock once on a `role` connection; keep the connection after a
    /// busy answer when `keep` is set (a polling standby).
    Acquire {
        role: PgRole,
        keep: bool,
        reply: oneshot::Sender<Result<Acquired>>,
    },
    /// End the predecessor's sessions.
    Terminate { reply: oneshot::Sender<Result<u32>> },
    /// Write the owner row.
    Announce { reply: oneshot::Sender<Result<()>> },
    /// Record the release and unlock.
    Release { reply: oneshot::Sender<Result<()>> },
    /// Run one statement answering one text value on the current
    /// connection, behind the owner's back.
    #[cfg(test)]
    Query {
        sql: String,
        reply: oneshot::Sender<Result<String>>,
    },
    /// Run a function on the owner thread.
    #[cfg(test)]
    Run { f: Box<dyn FnOnce() + Send> },
}

/// The instance-ownership lock of one database schema.
///
/// [`PgOwner::start`] spawns the owner thread; nothing connects until the
/// first lock attempt. Dropping the handle ends the thread, which closes the
/// lock connection and so frees the lock without recording a release, and
/// ends the side channel with it.
///
/// The owner thread never calls a secret store, the engine or the side
/// channel; it only answers requests. A blocking [`SideChannel`] call made on
/// the owner thread returns an error instead of waiting for itself.
pub struct PgOwner {
    requests: mpsc::UnboundedSender<Request>,
    side: SideChannel,
    fenced: watch::Receiver<FenceState>,
    lock_pid: Arc<AtomicI32>,
    key: i64,
    timings: OwnerTimings,
}

impl PgOwner {
    /// Spawn the owner thread for `target` as `me`, with the timings at
    /// [`timing_scale`].
    pub fn start(target: PgTarget, me: OwnerIdentity) -> Result<PgOwner> {
        PgOwner::start_with(target, me, OwnerTimings::current())
    }

    /// [`PgOwner::start`] with explicit timings.
    pub(crate) fn start_with(
        target: PgTarget,
        me: OwnerIdentity,
        timings: OwnerTimings,
    ) -> Result<PgOwner> {
        let (requests, inbox) = mpsc::unbounded_channel();
        let (fence_tx, fenced) = watch::channel(FenceState::Holding);
        let lock_pid = Arc::new(AtomicI32::new(0));
        let key = owner_key(target.schema.as_deref());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| IndexError::Db(format!("could not start the owner thread: {e}")))?;
        let side = Side {
            target: target.clone(),
            instance_id: me.instance_id.clone(),
            key,
            table: config_table(target.schema.as_deref()),
            secrets: secret_table(target.schema.as_deref()),
            lock_pid: lock_pid.clone(),
            conn: None,
        };
        let (side_requests, side_inbox) = mpsc::unbounded_channel();
        let worker = Worker {
            table: owner_table(target.schema.as_deref()),
            target,
            me,
            key,
            timings,
            conn: None,
            held: None,
            fenced: false,
            fence_tx,
            lock_pid: lock_pid.clone(),
            probe: ProbeState::new(),
            drain_before_probe: false,
        };
        // The side task lives on the owner runtime and ends with it, when
        // every owner handle is gone.
        let thread = std::thread::Builder::new()
            .name("crystalline-pg-owner".to_string())
            .spawn(move || {
                runtime.block_on(async move {
                    tokio::spawn(side.run(side_inbox));
                    worker.run(inbox).await;
                })
            })
            .map_err(|e| IndexError::Db(format!("could not start the owner thread: {e}")))?;
        Ok(PgOwner {
            requests,
            side: SideChannel {
                requests: side_requests,
                owner_thread: thread.thread().id(),
            },
            fenced,
            lock_pid,
            key,
            timings,
        })
    }

    /// Send a request and wait for its answer.
    async fn call<T>(&self, make: impl FnOnce(oneshot::Sender<Result<T>>) -> Request) -> Result<T> {
        let (reply, answer) = oneshot::channel();
        self.requests.send(make(reply)).map_err(|_| thread_gone())?;
        answer.await.map_err(|_| thread_gone())?
    }

    /// Try the lock once, on a `lock` connection. A busy answer closes the
    /// connection again.
    pub async fn try_acquire(&self) -> Result<Acquired> {
        self.call(|reply| Request::Acquire {
            role: PgRole::Lock,
            keep: false,
            reply,
        })
        .await
    }

    /// Wait until the lock is free and take it: try it every
    /// [`STANDBY_POLL`] on one `standby` connection, reconnecting with a
    /// growing backoff when the database is unreachable, and report each poll
    /// through `status`. Never creates a schema or a table. The won session
    /// becomes the lock connection, with the lock connection's name and
    /// timeouts. Dropping the future ends the wait.
    ///
    /// Returns an error only when the owner cannot wait: it already holds the
    /// lock, it was fenced, or its thread is gone.
    pub async fn wait_acquire(&self, status: watch::Sender<StandbyStatus>) -> Result<Acquired> {
        status.send_modify(|s| {
            s.waiting_since = Utc::now();
            s.polls = 0;
            s.last_error = None;
        });
        let mut failures = 0u32;
        loop {
            let tried = self
                .call(|reply| Request::Acquire {
                    role: PgRole::Standby,
                    keep: true,
                    reply,
                })
                .await;
            match tried {
                Ok(held @ Acquired::Held { .. }) => return Ok(held),
                Ok(Acquired::Busy(owner)) => {
                    failures = 0;
                    status.send_modify(|s| {
                        s.polls += 1;
                        s.owner = owner;
                        s.last_error = None;
                    });
                }
                Err(e) if matches!(e, IndexError::Invalid(_)) || self.requests.is_closed() => {
                    return Err(e);
                }
                Err(e) => {
                    failures = failures.saturating_add(1);
                    status.send_modify(|s| {
                        s.polls += 1;
                        s.last_error = Some(e.to_string());
                    });
                }
            }
            tokio::time::sleep(standby_delay(self.timings.standby_poll, failures)).await;
        }
    }

    /// End the predecessor's sessions, then wait [`GRACE`] unless `previous`
    /// records a clean release. The owner must hold the lock.
    pub async fn takeover(&self, previous: Option<&OwnerRow>) -> Result<TakeoverReport> {
        let terminated = self.call(|reply| Request::Terminate { reply }).await?;
        let started = Instant::now();
        if !previous.is_some_and(|p| p.released_at.is_some()) {
            tokio::time::sleep(self.timings.grace).await;
        }
        Ok(TakeoverReport {
            terminated,
            waited: started.elapsed(),
        })
    }

    /// Write the owner row, when the `instance_owner` table exists. The owner
    /// must hold the lock. Run it again after migrations that created the
    /// table.
    pub async fn announce(&self) -> Result<()> {
        self.call(|reply| Request::Announce { reply }).await
    }

    /// The fence the owner-process connections check at connect. While the
    /// owner does not hold the lock (before the first acquire, after a
    /// release or a fence) its `lock_pid` is 0, which no session has, so
    /// every fenced connect is refused.
    pub fn fence(&self) -> OwnerFence {
        OwnerFence {
            key: self.key,
            lock_pid: self.lock_pid.load(Ordering::Acquire),
        }
    }

    /// The fence watch: it turns [`FenceState::Fenced`] once the lock is lost.
    /// A release does not fence.
    pub fn fenced(&self) -> watch::Receiver<FenceState> {
        self.fenced.clone()
    }

    /// Record `released_at` in the owner row and unlock. Probing stops
    /// first, so a release never reads as a fence. Does nothing when the
    /// owner does not hold the lock.
    pub async fn release(&self) -> Result<()> {
        self.call(|reply| Request::Release { reply }).await
    }

    /// The side channel to the configuration document. It answers while
    /// this owner holds the lock.
    pub fn side(&self) -> SideChannel {
        self.side.clone()
    }

    /// Run `f` on the owner thread and wait for it to return.
    #[cfg(test)]
    async fn run_on_thread(&self, f: impl FnOnce() + Send + 'static) -> Result<()> {
        let (done, finished) = oneshot::channel::<Result<()>>();
        self.requests
            .send(Request::Run {
                f: Box::new(move || {
                    f();
                    let _ = done.send(Ok(()));
                }),
            })
            .map_err(|_| thread_gone())?;
        finished.await.map_err(|_| thread_gone())?
    }

    /// The text answer of `sql` on the owner thread's current connection.
    #[cfg(test)]
    async fn query(&self, sql: String) -> Result<String> {
        self.call(|reply| Request::Query { sql, reply }).await
    }

    /// A setting of the owner thread's current connection.
    #[cfg(test)]
    async fn show(&self, setting: &str) -> Result<String> {
        self.query(format!("SHOW {setting}")).await
    }
}

/// The error of a request the owner thread could not take or answer.
fn thread_gone() -> IndexError {
    IndexError::Db("the database owner thread has stopped".to_string())
}

/// The wait before the next standby poll: the poll period, doubled for each
/// failure in a row up to 16 times the period.
fn standby_delay(poll: Duration, failures: u32) -> Duration {
    poll * 2u32.pow(failures.min(4))
}

/// The lock this thread holds.
#[derive(Clone, Copy)]
struct Held {
    lock_pid: i32,
    since: DateTime<Utc>,
}

/// The owner thread's state.
struct Worker {
    target: PgTarget,
    me: OwnerIdentity,
    key: i64,
    table: String,
    timings: OwnerTimings,
    /// The open connection and the role it was opened in; a won standby
    /// connection is renamed to [`PgRole::Lock`].
    conn: Option<(PgConnection, PgRole)>,
    held: Option<Held>,
    fenced: bool,
    fence_tx: watch::Sender<FenceState>,
    lock_pid: Arc<AtomicI32>,
    probe: ProbeState,
    /// The last probe was given up on the client side, so its answer may
    /// still be on the wire: the next probe drains it first.
    drain_before_probe: bool,
}

impl Worker {
    /// Answer requests and, while the lock is held, probe it, until every
    /// handle is dropped.
    async fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Request>) {
        let mut ticks = tokio::time::interval(self.timings.probe_period);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                request = inbox.recv() => match request {
                    Some(request) => self.handle(request, &mut ticks).await,
                    None => break,
                },
                _ = ticks.tick(), if self.held.is_some() => self.probe_until_settled().await,
            }
        }
        self.close_conn().await;
    }

    async fn handle(&mut self, request: Request, ticks: &mut tokio::time::Interval) {
        match request {
            Request::Acquire { role, keep, reply } => {
                let answer = self.acquire(role, keep).await;
                if matches!(answer, Ok(Acquired::Held { .. })) {
                    ticks.reset();
                }
                // A caller that went away before the answer (a cancelled
                // wait) does not leave the lock held behind its back.
                if let Err(Ok(Acquired::Held { .. })) = reply.send(answer) {
                    self.held = None;
                    self.lock_pid.store(0, Ordering::Release);
                    self.close_conn().await;
                }
            }
            Request::Terminate { reply } => {
                let _ = reply.send(self.terminate().await);
            }
            Request::Announce { reply } => {
                let _ = reply.send(self.announce().await);
            }
            Request::Release { reply } => {
                let _ = reply.send(self.release().await);
            }
            #[cfg(test)]
            Request::Query { sql, reply } => {
                let answer = match self.conn.as_mut() {
                    Some((conn, _)) => sqlx::query_scalar(AssertSqlSafe(sql))
                        .fetch_one(&mut *conn)
                        .await
                        .map_err(IndexError::from),
                    None => Err(IndexError::Invalid("no connection".to_string())),
                };
                let _ = reply.send(answer);
            }
            #[cfg(test)]
            Request::Run { f } => f(),
        }
    }

    fn session_options(&self) -> PgSessionOptions {
        PgSessionOptions {
            instance_tag: Some(self.me.instance_id.clone()),
            fence: None,
        }
    }

    async fn close_conn(&mut self) {
        if let Some((conn, _)) = self.conn.take() {
            let _ = tokio::time::timeout(Duration::from_secs(1), conn.close()).await;
        }
    }

    async fn acquire(&mut self, role: PgRole, keep: bool) -> Result<Acquired> {
        if self.fenced {
            return Err(IndexError::Invalid(
                "this instance lost the database and cannot take it again".to_string(),
            ));
        }
        if self.held.is_some() {
            return Err(IndexError::Invalid(
                "this instance already holds the database".to_string(),
            ));
        }
        if !matches!(&self.conn, Some((_, r)) if *r == role) {
            self.close_conn().await;
            let conn = connect_one(&self.target, role, &self.session_options()).await?;
            self.conn = Some((conn, role));
        }
        let attempt = self.try_lock().await;
        match &attempt {
            Err(_) => {
                // A connection in an unknown state is never reused; closing
                // it also frees a lock it may have taken.
                self.conn = None;
            }
            Ok(Acquired::Busy(_)) if !keep => self.close_conn().await,
            _ => {}
        }
        attempt
    }

    async fn try_lock(&mut self) -> Result<Acquired> {
        let Some((conn, role)) = self.conn.as_mut() else {
            return Err(IndexError::Invalid("no connection".to_string()));
        };
        let (taken, pid): (bool, i32) =
            sqlx::query_as("SELECT pg_try_advisory_lock($1), pg_backend_pid()")
                .bind(self.key)
                .fetch_one(&mut *conn)
                .await?;
        if !taken {
            return Ok(Acquired::Busy(read_row(conn, &self.table).await?));
        }
        // The won session is the lock connection: its name and this owner's
        // lock timeouts, also on a `lock` connection, whose startup options
        // carry the process timings rather than this owner's. A plain `SET`,
        // not `SET LOCAL`, because no transaction is open.
        let name = application_name(self.key, &self.me.instance_id, PgRole::Lock);
        let [(_, idle), (_, statement)] = lock_session_settings(&self.timings);
        sqlx::query(
            "SELECT set_config('application_name', $1, false), \
             set_config('idle_session_timeout', $2, false), \
             set_config('statement_timeout', $3, false)",
        )
        .bind(name)
        .bind(idle)
        .bind(statement)
        .execute(&mut *conn)
        .await?;
        *role = PgRole::Lock;
        let previous = read_row(conn, &self.table).await?;
        self.held = Some(Held {
            lock_pid: pid,
            since: Utc::now(),
        });
        self.lock_pid.store(pid, Ordering::Release);
        self.probe = ProbeState::new();
        Ok(Acquired::Held {
            lock_pid: pid,
            previous,
        })
    }

    /// The held lock and its connection, or the error of a request that
    /// needs them.
    fn holding(&mut self) -> Result<(Held, &mut PgConnection)> {
        match (self.held, self.conn.as_mut()) {
            (Some(held), Some((conn, _))) => Ok((held, conn)),
            _ => Err(IndexError::Invalid(
                "this instance does not hold the database".to_string(),
            )),
        }
    }

    async fn terminate(&mut self) -> Result<u32> {
        let prefix = application_prefix(self.key);
        let tag = short_tag(&self.me.instance_id);
        let database = self.target.describe();
        let (_, conn) = self.holding()?;
        // Each termination may wait up to TERMINATE_WAIT_MS, longer than the
        // lock connection's statement timeout, so the statement runs in a
        // transaction that lifts it.
        let mut tx = conn.begin().await?;
        sqlx::query("SET LOCAL statement_timeout = 0")
            .execute(&mut *tx)
            .await?;
        let ended: i64 = sqlx::query_scalar(TERMINATE_SQL)
            .bind(prefix)
            .bind(tag)
            .bind(TERMINATE_WAIT_MS)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| terminate_error(e, &database))?;
        tx.commit().await?;
        Ok(u32::try_from(ended).unwrap_or(u32::MAX))
    }

    async fn announce(&mut self) -> Result<()> {
        let table = self.table.clone();
        let me = self.me.clone();
        let (held, conn) = self.holding()?;
        if !table_exists(conn, &table).await? {
            return Ok(());
        }
        let sql = format!(
            "INSERT INTO {table} \
             (id, instance_id, label, command, pid, backend_pid, version, since, released_at) \
             VALUES (1, $1, $2, $3, $4, $5, $6, \
                     TIMESTAMPTZ 'epoch' + $7 * INTERVAL '1 microsecond', NULL) \
             ON CONFLICT (id) DO UPDATE SET instance_id = EXCLUDED.instance_id, \
             label = EXCLUDED.label, command = EXCLUDED.command, pid = EXCLUDED.pid, \
             backend_pid = EXCLUDED.backend_pid, version = EXCLUDED.version, \
             since = EXCLUDED.since, released_at = NULL"
        );
        sqlx::query(AssertSqlSafe(sql))
            .bind(&me.instance_id)
            .bind(&me.label)
            .bind(&me.command)
            .bind(i64::from(std::process::id()))
            .bind(i64::from(held.lock_pid))
            .bind(&me.version)
            .bind(held.since.timestamp_micros())
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    async fn release(&mut self) -> Result<()> {
        // Forgetting the lock first stops the probe, so the unlock below is
        // never read as a loss.
        let Some(held) = self.held.take() else {
            return Ok(());
        };
        let mut outcome = Ok(());
        if let Some((conn, _)) = self.conn.as_mut() {
            let table = self.table.clone();
            let marked = async {
                if table_exists(conn, &table).await? {
                    let sql = format!(
                        "UPDATE {table} SET released_at = now() \
                         WHERE id = 1 AND instance_id = $1 AND backend_pid = $2"
                    );
                    sqlx::query(AssertSqlSafe(sql))
                        .bind(&self.me.instance_id)
                        .bind(i64::from(held.lock_pid))
                        .execute(&mut *conn)
                        .await?;
                }
                Ok::<(), IndexError>(())
            }
            .await;
            let unlocked = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
                .bind(self.key)
                .fetch_one(&mut *conn)
                .await
                .map(|_| ())
                .map_err(IndexError::from);
            outcome = marked.and(unlocked);
        }
        // Closing the session frees the lock even when the unlock failed.
        self.close_conn().await;
        self.lock_pid.store(0, Ordering::Release);
        outcome
    }

    async fn probe_once(&mut self) -> ProbeOutcome {
        let (key, limit) = (self.key, self.timings.probe_timeout);
        let drain = std::mem::take(&mut self.drain_before_probe);
        let Ok((held, conn)) = self.holding() else {
            return ProbeOutcome::Broken;
        };
        // The drain and the probe share the one timeout, so a retry still
        // fits the `FENCE_DETECT_MAX` the grace is built on.
        let probe = async {
            if drain {
                drain_stale_cancel(&mut *conn).await?;
            }
            sqlx::query_scalar::<_, bool>(HOLDS_KEY_SQL)
                .bind(key)
                .bind(held.lock_pid)
                .fetch_one(&mut *conn)
                .await
        };
        match tokio::time::timeout(limit, probe).await {
            Err(_) => {
                self.drain_before_probe = true;
                ProbeOutcome::Timeout
            }
            Ok(Ok(true)) => ProbeOutcome::Held,
            Ok(Ok(false)) => ProbeOutcome::NotHeld,
            Ok(Err(e)) if is_broken(&e) => ProbeOutcome::Broken,
            Ok(Err(e)) => {
                tracing::debug!("the ownership probe failed on an open connection: {e}");
                ProbeOutcome::Timeout
            }
        }
    }

    async fn probe_until_settled(&mut self) {
        loop {
            let outcome = self.probe_once().await;
            match self.probe.next(outcome) {
                ProbeAction::Continue => return,
                ProbeAction::RetryNow => continue,
                ProbeAction::Fence(reason) => {
                    self.fence(reason);
                    return;
                }
            }
        }
    }

    /// Give up the database for good: forget the lock, drop the connection
    /// without reconnecting and tell every watcher.
    fn fence(&mut self, reason: String) {
        tracing::debug!("fenced: {reason}");
        self.held = None;
        self.fenced = true;
        self.conn = None;
        self.lock_pid.store(0, Ordering::Release);
        self.fence_tx.send_replace(FenceState::Fenced(reason));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::postgres::{PostgresStore, ScratchSchema};

    /// A refused takeover termination names the database and the remedy;
    /// any other error is passed on unchanged.
    #[test]
    fn a_refused_termination_says_what_to_grant() {
        let IndexError::Invalid(text) = terminate_refused("db:5432/c, schema a") else {
            panic!("not an Invalid error");
        };
        assert!(text.contains("db:5432/c, schema a"), "{text}");
        assert!(text.contains("pg_signal_backend"), "{text}");
        assert!(text.contains("ran as a superuser"), "{text}");
        assert!(
            matches!(
                terminate_error(sqlx::Error::RowNotFound, "x"),
                IndexError::Db(_)
            ),
            "another error stays a database error"
        );
    }

    fn pg_url() -> Option<String> {
        std::env::var("CRYSTALLINE_TEST_POSTGRES_URL")
            .ok()
            .filter(|u| !u.is_empty())
    }

    /// The timings the PG tests run at: a tenth of the shipped ones.
    fn fast() -> OwnerTimings {
        OwnerTimings::at(10)
    }

    fn identity(tag: &str, label: &str) -> OwnerIdentity {
        OwnerIdentity {
            instance_id: format!("{tag}-0000-4000-8000-000000000000"),
            label: label.to_string(),
            command: None,
            version: "0.33.0-test".to_string(),
        }
    }

    fn target(url: &str, schema: &ScratchSchema) -> PgTarget {
        PgTarget {
            url: url.to_string(),
            schema: Some(schema.name().to_string()),
        }
    }

    /// Create the scratch schema with its tables, `instance_owner` among
    /// them, and close the store's sessions again so no `none` session of
    /// the test stays behind.
    async fn migrate(url: &str, schema: &ScratchSchema) {
        let store = PostgresStore::open_in_schema_with_readers(url, schema.name(), 0)
            .await
            .unwrap();
        store.pool.close().await;
    }

    async fn plain(t: &PgTarget) -> PgConnection {
        connect_one(t, PgRole::Probe, &PgSessionOptions::default())
            .await
            .unwrap()
    }

    /// Try the lock until it is free, for a predecessor whose session is
    /// still going away.
    async fn acquire_eventually(o: &PgOwner) -> (i32, Option<OwnerRow>) {
        for _ in 0..100 {
            if let Acquired::Held { lock_pid, previous } = o.try_acquire().await.unwrap() {
                return (lock_pid, previous);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the lock never came free");
    }

    async fn alive(conn: &mut PgConnection, pid: i32) -> bool {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE pid = $1)")
            .bind(pid)
            .fetch_one(conn)
            .await
            .unwrap()
    }

    async fn backend_pid(conn: &mut PgConnection) -> i32 {
        sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(conn)
            .await
            .unwrap()
    }

    /// The grace outlasts the worst case of fence detection and the lock
    /// session's idle timeout outlasts a probe cycle, at the shipped scale and
    /// at the test scale.
    #[test]
    fn grace_exceeds_worst_case_fence_detection() {
        for scale in [1, 10] {
            let t = OwnerTimings::at(scale);
            let worst = t.probe_period + t.probe_timeout * (1 + PROBE_RETRIES);
            assert_eq!(t.fence_detect_max, worst, "scale {scale}");
            assert!(t.grace > worst, "scale {scale}: {t:?}");
            assert!(
                t.lock_idle_timeout > t.probe_period + t.probe_timeout,
                "scale {scale}: {t:?}"
            );
            assert!(
                t.lock_statement_timeout < t.probe_timeout,
                "scale {scale}: {t:?}"
            );
        }
        let shipped = OwnerTimings::at(1);
        assert_eq!(shipped.fence_detect_max, FENCE_DETECT_MAX);
        assert_eq!(shipped.grace, GRACE);
        assert_eq!(
            (FENCE_DETECT_MAX, GRACE),
            (Duration::from_secs(13), Duration::from_secs(15))
        );
        // The lock session's settings at both scales.
        let settings = |scale| lock_session_settings(&OwnerTimings::at(scale)).map(|(_, v)| v);
        assert_eq!(settings(1), ["15s".to_string(), "3s".to_string()]);
        assert_eq!(settings(10), ["1500ms".to_string(), "300ms".to_string()]);
        assert_eq!(pg_duration(Duration::ZERO), "1ms");
    }

    #[test]
    fn probe_state_fences_at_once_on_not_held_and_broken() {
        for outcome in [ProbeOutcome::NotHeld, ProbeOutcome::Broken] {
            let mut s = ProbeState::new();
            assert!(
                matches!(s.next(outcome), ProbeAction::Fence(_)),
                "{outcome:?}"
            );
            // Even right after a retried timeout.
            let mut s = ProbeState::new();
            assert_eq!(s.next(ProbeOutcome::Timeout), ProbeAction::RetryNow);
            assert!(
                matches!(s.next(outcome), ProbeAction::Fence(_)),
                "{outcome:?}"
            );
        }
    }

    #[test]
    fn probe_state_retries_once_on_timeout_then_fences() {
        let mut s = ProbeState::new();
        assert_eq!(s.next(ProbeOutcome::Timeout), ProbeAction::RetryNow);
        let ProbeAction::Fence(reason) = s.next(ProbeOutcome::Timeout) else {
            panic!("a second timeout did not fence");
        };
        assert!(reason.contains("2 times"), "{reason}");
    }

    #[test]
    fn probe_state_resets_after_a_held_answer() {
        let mut s = ProbeState::new();
        for _ in 0..3 {
            assert_eq!(s.next(ProbeOutcome::Timeout), ProbeAction::RetryNow);
            assert_eq!(s.next(ProbeOutcome::Held), ProbeAction::Continue);
        }
    }

    #[test]
    fn timing_scale_divides_and_defaults_to_one() {
        assert_eq!(OwnerTimings::at(0), OwnerTimings::at(1));
        let t = OwnerTimings::at(10);
        assert_eq!(t.probe_period, Duration::from_millis(500));
        assert_eq!(t.grace, Duration::from_millis(1500));
        assert_eq!(standby_delay(STANDBY_POLL, 0), STANDBY_POLL);
        assert_eq!(standby_delay(STANDBY_POLL, 9), STANDBY_POLL * 16);
    }

    #[tokio::test]
    async fn two_owners_on_one_schema_exactly_one_holds() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "own2");
        let t = target(&url, &scratch);
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), fast()).unwrap();
        let b = PgOwner::start_with(t.clone(), identity("bbbbbbbb", "b"), fast()).unwrap();
        let Acquired::Held { lock_pid, previous } = a.try_acquire().await.unwrap() else {
            panic!("the first owner did not get the lock");
        };
        assert!(previous.is_none());
        assert_eq!(a.fence().lock_pid, lock_pid);
        assert!(matches!(
            b.try_acquire().await.unwrap(),
            Acquired::Busy(None)
        ));
        assert_eq!(b.fence().lock_pid, 0);
        assert!(probe_owner(&t).await.unwrap().is_some());
        // A busy try leaves no connection of B behind. A closed session can
        // linger in pg_stat_activity for a moment, so look a few times.
        let mut c = plain(&t).await;
        let b_prefix = format!(
            "{}bbbbbbbb/",
            application_prefix(owner_key(t.schema.as_deref()))
        );
        let mut b_sessions = -1i64;
        for _ in 0..50 {
            b_sessions = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE starts_with(application_name, $1)",
            )
            .bind(&b_prefix)
            .fetch_one(&mut c)
            .await
            .unwrap();
            if b_sessions == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(b_sessions, 0);
        // Once A is gone, B gets it.
        drop(a);
        acquire_eventually(&b).await;
        assert!(matches!(a_second_try(&t).await, Acquired::Busy(_)));
        b.release().await.unwrap();
        assert!(probe_owner(&t).await.unwrap().is_none());
    }

    /// A third owner's single try on `t`.
    async fn a_second_try(t: &PgTarget) -> Acquired {
        let c = PgOwner::start_with(t.clone(), identity("cccccccc", "c"), fast()).unwrap();
        c.try_acquire().await.unwrap()
    }

    #[tokio::test]
    async fn two_schemas_on_one_database_both_hold() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let one = ScratchSchema::new(&url, "owns1");
        let two = ScratchSchema::new(&url, "owns2");
        let a = PgOwner::start_with(target(&url, &one), identity("aaaaaaaa", "a"), fast()).unwrap();
        let b = PgOwner::start_with(target(&url, &two), identity("bbbbbbbb", "b"), fast()).unwrap();
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        assert!(matches!(
            b.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        assert_ne!(a.fence().key, b.fence().key);
        a.release().await.unwrap();
        b.release().await.unwrap();
    }

    /// A statement given up on the client side whose server-side
    /// `statement_timeout` fires later leaves its 57014 on the wire, and sqlx
    /// hands it to the next statement: without the drain a probe retry would
    /// fail at once on that stale error. The drain takes it, and the next
    /// statement reads its own answer.
    #[tokio::test]
    async fn a_stale_cancel_is_drained_before_the_probe_retry() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "ownd");
        let t = target(&url, &scratch);
        async fn leave_a_stale_cancel(c: &mut PgConnection) {
            sqlx::query("SET statement_timeout = '200ms'")
                .execute(&mut *c)
                .await
                .unwrap();
            let given_up = tokio::time::timeout(
                Duration::from_millis(50),
                sqlx::query("SELECT pg_sleep(2)").execute(&mut *c),
            )
            .await;
            assert!(
                given_up.is_err(),
                "the slow statement answered: {given_up:?}"
            );
            // The server cancels it at 200ms; its error waits on the wire.
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        // Without the drain the next statement reads the stale cancel.
        let mut c = plain(&t).await;
        leave_a_stale_cancel(&mut c).await;
        let stale = sqlx::query_scalar::<_, bool>("SELECT true")
            .fetch_one(&mut c)
            .await;
        assert!(
            stale.as_ref().is_err_and(is_query_canceled),
            "no stale cancel: {stale:?}"
        );
        // With it the next statement reads its own answer.
        let mut c = plain(&t).await;
        leave_a_stale_cancel(&mut c).await;
        drain_stale_cancel(&mut c).await.unwrap();
        let fresh: bool = sqlx::query_scalar("SELECT true")
            .fetch_one(&mut c)
            .await
            .unwrap();
        assert!(fresh);
    }

    #[tokio::test]
    async fn terminating_the_lock_backend_fences_within_detect_max() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "ownk");
        let t = target(&url, &scratch);
        let timings = fast();
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), timings).unwrap();
        let Acquired::Held { lock_pid, .. } = a.try_acquire().await.unwrap() else {
            panic!("no lock");
        };
        let mut fenced = a.fenced();
        assert_eq!(*fenced.borrow(), FenceState::Holding);
        let mut c = plain(&t).await;
        let ended: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1, 5000)")
            .bind(lock_pid)
            .fetch_one(&mut c)
            .await
            .unwrap();
        assert!(ended);
        let started = Instant::now();
        tokio::time::timeout(timings.fence_detect_max, fenced.changed())
            .await
            .expect("no fence within FENCE_DETECT_MAX")
            .unwrap();
        assert!(started.elapsed() < timings.fence_detect_max);
        let FenceState::Fenced(reason) = fenced.borrow().clone() else {
            panic!("the watch did not fence");
        };
        assert!(reason.contains("connection was lost"), "{reason}");
        assert_eq!(a.fence().lock_pid, 0);
        // A fenced owner never takes the database again.
        let Err(err) = a.try_acquire().await else {
            panic!("a fenced owner took the lock again");
        };
        assert!(err.to_string().contains("lost the database"), "{err}");
    }

    /// A probe that reads false (the lock session lost the lock while the
    /// session stayed) fences, and a release does not.
    #[tokio::test]
    async fn a_probe_that_reads_not_held_fences_the_owner() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "ownn");
        let t = target(&url, &scratch);
        let timings = fast();
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), timings).unwrap();
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        // A release is not a fence.
        a.release().await.unwrap();
        tokio::time::sleep(timings.probe_period * 3).await;
        assert_eq!(*a.fenced().borrow(), FenceState::Holding);
        // Unlocked behind the owner's back: the session stays, the lock goes.
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        let mut fenced = a.fenced();
        let unlocked = a
            .query(format!(
                "SELECT pg_advisory_unlock({})::text",
                a.fence().key
            ))
            .await
            .unwrap();
        assert_eq!(unlocked, "true");
        tokio::time::timeout(timings.fence_detect_max, fenced.changed())
            .await
            .expect("no fence within FENCE_DETECT_MAX")
            .unwrap();
        let FenceState::Fenced(reason) = fenced.borrow().clone() else {
            panic!("the watch did not fence");
        };
        assert!(reason.contains("no longer holds"), "{reason}");
    }

    #[tokio::test]
    async fn takeover_terminates_predecessor_pool_sessions() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "ownt");
        let t = target(&url, &scratch);
        let a_id = identity("aaaaaaaa", "a");
        let a = PgOwner::start_with(t.clone(), a_id.clone(), fast()).unwrap();
        let Acquired::Held { lock_pid, .. } = a.try_acquire().await.unwrap() else {
            panic!("no lock");
        };
        let a_opts = PgSessionOptions {
            instance_tag: Some(a_id.instance_id.clone()),
            fence: Some(a.fence()),
        };
        let mut a_pool = connect_one(&t, PgRole::Pool, &a_opts).await.unwrap();
        let mut a_standby = connect_one(&t, PgRole::Standby, &a_opts).await.unwrap();
        let mut untagged = connect_one(&t, PgRole::Pool, &PgSessionOptions::default())
            .await
            .unwrap();
        let (a_pool_pid, a_standby_pid, untagged_pid) = (
            backend_pid(&mut a_pool).await,
            backend_pid(&mut a_standby).await,
            backend_pid(&mut untagged).await,
        );
        // A's lock goes, A's pool session stays.
        let mut c = plain(&t).await;
        sqlx::query("SELECT pg_terminate_backend($1, 5000)")
            .bind(lock_pid)
            .execute(&mut c)
            .await
            .unwrap();
        let b = PgOwner::start_with(t.clone(), identity("bbbbbbbb", "b"), fast()).unwrap();
        let (_, previous) = acquire_eventually(&b).await;
        let b_opts = PgSessionOptions {
            instance_tag: Some("bbbbbbbb".to_string()),
            fence: Some(b.fence()),
        };
        let mut b_pool = connect_one(&t, PgRole::Pool, &b_opts).await.unwrap();
        let b_pool_pid = backend_pid(&mut b_pool).await;
        assert!(alive(&mut c, a_pool_pid).await);
        let report = b.takeover(previous.as_ref()).await.unwrap();
        assert_eq!(report.terminated, 2, "{report:?}");
        assert!(
            !alive(&mut c, a_pool_pid).await,
            "A's pool session survived"
        );
        assert!(
            !alive(&mut c, untagged_pid).await,
            "the untagged session survived"
        );
        assert!(
            alive(&mut c, a_standby_pid).await,
            "A's standby session was ended"
        );
        assert!(alive(&mut c, b_pool_pid).await, "B ended its own session");
        // B still owns the database after its own takeover.
        assert_eq!(*b.fenced().borrow(), FenceState::Holding);
        b.release().await.unwrap();
    }

    #[tokio::test]
    async fn takeover_skips_grace_after_a_clean_release() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "owng");
        migrate(&url, &scratch).await;
        let t = target(&url, &scratch);
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), fast()).unwrap();
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        a.announce().await.unwrap();
        a.release().await.unwrap();
        let b = PgOwner::start_with(t.clone(), identity("bbbbbbbb", "b"), fast()).unwrap();
        let (_, previous) = acquire_eventually(&b).await;
        let previous = previous.expect("A's row");
        assert_eq!(previous.label, "a");
        assert!(previous.released_at.is_some());
        let report = b.takeover(Some(&previous)).await.unwrap();
        assert!(report.waited < Duration::from_secs(1), "{report:?}");
        b.release().await.unwrap();
    }

    #[tokio::test]
    async fn takeover_waits_grace_without_release_row() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "ownw");
        migrate(&url, &scratch).await;
        let t = target(&url, &scratch);
        let timings = fast();
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), timings).unwrap();
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        a.announce().await.unwrap();
        // A stops without a release: its session closes, its row stays.
        drop(a);
        let b = PgOwner::start_with(t.clone(), identity("bbbbbbbb", "b"), timings).unwrap();
        let (_, previous) = acquire_eventually(&b).await;
        let previous = previous.expect("A's row");
        assert!(previous.released_at.is_none());
        let report = b.takeover(Some(&previous)).await.unwrap();
        assert!(report.waited >= timings.grace, "{report:?}");
        // No row at all waits the grace too.
        b.release().await.unwrap();
        let report = {
            let c = PgOwner::start_with(t.clone(), identity("cccccccc", "c"), timings).unwrap();
            acquire_eventually(&c).await;
            let r = c.takeover(None).await.unwrap();
            c.release().await.unwrap();
            r
        };
        assert!(report.waited >= timings.grace, "{report:?}");
    }

    #[tokio::test]
    async fn standby_on_an_empty_schema_creates_nothing() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "owne");
        let t = target(&url, &scratch);
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), fast()).unwrap();
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        a.announce().await.unwrap();
        let b = PgOwner::start_with(t.clone(), identity("bbbbbbbb", "b"), fast()).unwrap();
        let (tx, mut rx) = watch::channel(StandbyStatus::new());
        let wait = b.wait_acquire(tx);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                r = &mut wait => panic!("the standby got the lock: {r:?}"),
                changed = rx.changed() => {
                    changed.unwrap();
                    if rx.borrow().polls >= 3 {
                        break;
                    }
                }
            }
        }
        let status = rx.borrow().clone();
        assert!(status.owner.is_none(), "{status:?}");
        assert!(status.last_error.is_none(), "{status:?}");
        let mut c = plain(&t).await;
        let schemas: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pg_namespace WHERE nspname = $1")
                .bind(scratch.name())
                .fetch_one(&mut c)
                .await
                .unwrap();
        assert_eq!(schemas, 0, "the schema was created");
        let table: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(owner_table(Some(scratch.name())))
            .fetch_one(&mut c)
            .await
            .unwrap();
        assert!(!table, "instance_owner was created");
    }

    /// A standby that wins the lock turns its session into the lock
    /// connection: the lock role's name and both timeouts.
    #[tokio::test]
    async fn a_standby_that_wins_becomes_the_lock_session() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "ownp");
        migrate(&url, &scratch).await;
        let t = target(&url, &scratch);
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), fast()).unwrap();
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        a.announce().await.unwrap();
        let b = PgOwner::start_with(t.clone(), identity("bbbbbbbb", "b"), fast()).unwrap();
        let (tx, mut rx) = watch::channel(StandbyStatus::new());
        let wait = b.wait_acquire(tx);
        tokio::pin!(wait);
        while rx.borrow().polls == 0 {
            tokio::select! {
                r = &mut wait => panic!("the standby got the lock: {r:?}"),
                changed = rx.changed() => changed.unwrap(),
            }
        }
        assert_eq!(
            rx.borrow().owner.as_ref().map(|o| o.label.clone()),
            Some("a".to_string())
        );
        a.release().await.unwrap();
        let won = tokio::time::timeout(Duration::from_secs(10), &mut wait)
            .await
            .expect("the standby never took the lock")
            .unwrap();
        let Acquired::Held { previous, .. } = won else {
            panic!("not held");
        };
        assert!(previous.unwrap().released_at.is_some());
        assert_eq!(
            b.show("application_name").await.unwrap(),
            application_name(b.fence().key, "bbbbbbbb", PgRole::Lock)
        );
        assert_eq!(b.show("idle_session_timeout").await.unwrap(), "1500ms");
        assert_eq!(b.show("statement_timeout").await.unwrap(), "300ms");
        // The probe keeps holding on the renamed session.
        tokio::time::sleep(fast().probe_period * 3).await;
        assert_eq!(*b.fenced().borrow(), FenceState::Holding);
        b.release().await.unwrap();
    }

    /// A blocking side-channel call made on the owner thread returns an
    /// error at once instead of waiting for its own answer; the same call on
    /// another thread is served (here refused only because the owner holds
    /// nothing). Nothing connects, so no database is needed.
    #[tokio::test]
    async fn side_channel_refuses_calls_from_owner_thread() {
        let t = PgTarget {
            url: "postgres://u@127.0.0.1:1/none".to_string(),
            schema: Some("side_unit".to_string()),
        };
        let o = PgOwner::start_with(t, identity("aaaaaaaa", "a"), fast()).unwrap();
        let side = o.side();
        let (tx, rx) = std::sync::mpsc::channel();
        let on_thread = side.clone();
        tokio::time::timeout(
            Duration::from_secs(5),
            o.run_on_thread(move || {
                let started = Instant::now();
                let load = on_thread.load_config_blocking(Duration::from_secs(30));
                let save = on_thread.save_config_blocking(
                    &serde_json::json!({}),
                    "test",
                    Duration::from_secs(30),
                );
                let _ = tx.send((load, save, started.elapsed()));
            }),
        )
        .await
        .expect("the owner thread waited for itself")
        .unwrap();
        let (load, save, took) = rx.recv().unwrap();
        for answer in [load.map(|_| ()), save] {
            let Err(IndexError::Invalid(text)) = answer else {
                panic!("not refused: {answer:?}");
            };
            assert_eq!(text, "side channel called from the owner thread");
        }
        assert!(took < Duration::from_secs(1), "{took:?}");
        // Another thread reaches the side task, which refuses for want of
        // the lock.
        let answer = std::thread::spawn(move || side.load_config_blocking(Duration::from_secs(5)))
            .join()
            .unwrap();
        let Err(IndexError::Invalid(text)) = answer else {
            panic!("not refused: {answer:?}");
        };
        assert!(text.contains("does not hold"), "{text}");
    }

    /// Every sealed-secret call made on the owner thread itself is refused
    /// at once instead of waiting for its own answer, so a secret store
    /// reached from there can never deadlock it. Nothing connects.
    #[tokio::test]
    async fn pg_vault_never_called_from_owner_thread() {
        let t = PgTarget {
            url: "postgres://u@127.0.0.1:1/none".to_string(),
            schema: Some("vault_unit".to_string()),
        };
        let o = PgOwner::start_with(t, identity("aaaaaaaa", "a"), fast()).unwrap();
        let on_thread = o.side();
        let (tx, rx) = std::sync::mpsc::channel();
        tokio::time::timeout(
            Duration::from_secs(5),
            o.run_on_thread(move || {
                let started = Instant::now();
                let answers = vec![
                    on_thread.vault_load("github").map(|_| ()),
                    on_thread.vault_save("github", b"x", "k").map(|_| ()),
                    on_thread.vault_delete("github", "k").map(|_| ()),
                    on_thread.vault_summary().map(|_| ()),
                ];
                let _ = tx.send((answers, started.elapsed()));
            }),
        )
        .await
        .expect("the owner thread waited for itself")
        .unwrap();
        let (answers, took) = rx.recv().unwrap();
        for answer in answers {
            let Err(IndexError::Invalid(text)) = answer else {
                panic!("not refused: {answer:?}");
            };
            assert_eq!(text, "side channel called from the owner thread");
        }
        assert!(took < Duration::from_secs(1), "{took:?}");
    }

    /// The sealed secrets round-trip over the side channel while the owner
    /// holds the lock. A save or a delete never touches a row sealed with
    /// another key, in one statement each, and without the lock nothing
    /// answers.
    #[tokio::test]
    async fn vault_side_channel_round_trip_keeps_rows_of_another_key_id() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "vaultr");
        migrate(&url, &scratch).await;
        let t = target(&url, &scratch);
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), fast()).unwrap();
        let side = a.side();
        let run = |f: Box<dyn FnOnce(&SideChannel) + Send>| {
            let side = side.clone();
            async move {
                tokio::task::spawn_blocking(move || f(&side)).await.unwrap();
            }
        };
        run(Box::new(|s| {
            let refused = s.vault_load("github").unwrap_err();
            assert!(refused.to_string().contains("does not hold"), "{refused}");
        }))
        .await;
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        run(Box::new(|s| {
            assert_eq!(s.vault_load("github").unwrap(), None);
            assert_eq!(s.vault_summary().unwrap(), SecretSummary::default());
            assert!(s.vault_save("github", b"one", "key_a").unwrap());
            assert!(s.vault_save("github", b"two", "key_a").unwrap());
            assert_eq!(
                s.vault_load("github").unwrap(),
                Some(SecretRow {
                    sealed: b"two".to_vec(),
                    key_id: "key_a".to_string()
                })
            );
            // Another key neither replaces nor deletes the row.
            assert!(!s.vault_save("github", b"three", "key_b").unwrap());
            assert!(!s.vault_delete("github", "key_b").unwrap());
            assert_eq!(s.vault_load("github").unwrap().unwrap().sealed, b"two");
            assert!(s.vault_save("github-app", b"app", "key_b").unwrap());
            assert_eq!(
                s.vault_summary().unwrap(),
                SecretSummary {
                    rows: 2,
                    key_ids: vec!["key_a".to_string(), "key_b".to_string()],
                }
            );
            assert!(s.vault_delete("github", "key_a").unwrap());
            assert!(!s.vault_delete("github", "key_a").unwrap(), "already gone");
            assert_eq!(s.vault_load("github").unwrap(), None);
        }))
        .await;
        // The name is compared byte for byte.
        run(Box::new(|s| {
            assert!(s.vault_save("GitHub-App", b"other", "key_b").unwrap());
            assert_eq!(s.vault_summary().unwrap().rows, 2);
        }))
        .await;

        a.release().await.unwrap();
        run(Box::new(|s| {
            let refused = s.vault_summary().unwrap_err();
            assert!(refused.to_string().contains("does not hold"), "{refused}");
        }))
        .await;
    }

    /// The side channel saves and loads the document while the owner holds
    /// the lock, the lock-free read sees the same row, and without the lock
    /// the side channel answers nothing.
    #[tokio::test]
    async fn config_round_trip() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        // Before any migration the lock-free read finds no table and creates
        // none.
        let bare = ScratchSchema::new(&url, "cfgb");
        assert_eq!(
            read_config_document(&target(&url, &bare)).await.unwrap(),
            None
        );
        let scratch = ScratchSchema::new(&url, "cfgr");
        migrate(&url, &scratch).await;
        let t = target(&url, &scratch);
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), fast()).unwrap();
        let side = a.side();
        let refused = side.load_config().await.unwrap_err();
        assert!(refused.to_string().contains("does not hold"), "{refused}");
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        assert_eq!(side.load_config().await.unwrap(), None);
        assert_eq!(read_config_document(&t).await.unwrap(), None);

        let first = serde_json::json!({
            "settings": {"search": {"limit": 7}},
            "domains": [{"name": "notes", "path": "/srv/notes"}],
        });
        let blocking = side.clone();
        let body = first.clone();
        tokio::task::spawn_blocking(move || {
            blocking.save_config_blocking(&body, "test-a", Duration::from_secs(5))
        })
        .await
        .unwrap()
        .unwrap();
        let doc = side.load_config().await.unwrap().expect("the document");
        assert_eq!(doc.body, first);
        assert_eq!(doc.updated_by, "test-a");
        assert!((Utc::now() - doc.updated_at).num_seconds().abs() < 60);
        assert_eq!(read_config_document(&t).await.unwrap(), Some(doc.clone()));
        let blocking = side.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            blocking.load_config_blocking(Duration::from_secs(5))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(loaded, Some(doc.clone()));

        // A second save replaces the whole document.
        let second = serde_json::json!({"settings": {}});
        let blocking = side.clone();
        let body = second.clone();
        tokio::task::spawn_blocking(move || {
            blocking.save_config_blocking(&body, "test-b", Duration::from_secs(5))
        })
        .await
        .unwrap()
        .unwrap();
        let doc = read_config_document(&t)
            .await
            .unwrap()
            .expect("the document");
        assert_eq!((doc.body, doc.updated_by.as_str()), (second, "test-b"));

        // The side connection is tagged with the instance and the side role.
        let mut c = plain(&t).await;
        let sides: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name = $1")
                .bind(application_name(a.fence().key, "aaaaaaaa", PgRole::Side))
                .fetch_one(&mut c)
                .await
                .unwrap();
        assert_eq!(sides, 1);

        // After a release the side channel answers nothing.
        a.release().await.unwrap();
        let refused = side.load_config().await.unwrap_err();
        assert!(refused.to_string().contains("does not hold"), "{refused}");
    }

    /// A save that waits on a row another session holds gives up at its
    /// timeout, and the document stays as it was once the other session
    /// lets go; the next save goes through on a fresh connection.
    #[tokio::test]
    async fn config_save_times_out_under_a_held_row_lock() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "cfgl");
        migrate(&url, &scratch).await;
        let t = target(&url, &scratch);
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), fast()).unwrap();
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        let side = a.side();
        let save = |body: serde_json::Value, timeout: Duration| {
            let side = side.clone();
            tokio::task::spawn_blocking(move || {
                let started = Instant::now();
                let answer = side.save_config_blocking(&body, "test", timeout);
                (answer, started.elapsed())
            })
        };
        let first = serde_json::json!({"n": 1});
        save(first.clone(), Duration::from_secs(5))
            .await
            .unwrap()
            .0
            .unwrap();

        let mut blocker = plain(&t).await;
        let mut held = blocker.begin().await.unwrap();
        sqlx::query(AssertSqlSafe(format!(
            "SELECT id FROM {} WHERE id = 1 FOR UPDATE",
            config_table(t.schema.as_deref())
        )))
        .execute(&mut *held)
        .await
        .unwrap();
        let timeout = Duration::from_secs(1);
        let (answer, took) = save(serde_json::json!({"n": 2}), timeout).await.unwrap();
        assert!(answer.is_err(), "the save went through a held row");
        assert!(took < timeout + Duration::from_secs(1), "{took:?}");
        held.commit().await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let doc = read_config_document(&t)
            .await
            .unwrap()
            .expect("the document");
        assert_eq!(doc.body, first, "the timed-out save landed");

        let third = serde_json::json!({"n": 3});
        save(third.clone(), Duration::from_secs(5))
            .await
            .unwrap()
            .0
            .unwrap();
        assert_eq!(read_config_document(&t).await.unwrap().unwrap().body, third);
        a.release().await.unwrap();
    }

    /// An owner whose database is never reached: nothing connects until a
    /// lock attempt, so its side channel answers without a server.
    fn unreachable_owner() -> PgOwner {
        let t = PgTarget {
            url: "postgres://u@127.0.0.1:1/none".to_string(),
            schema: Some("side_unit".to_string()),
        };
        PgOwner::start_with(t, identity("aaaaaaaa", "a"), fast()).unwrap()
    }

    /// The answer of a blocking side call made straight from a runtime's
    /// own thread: the side task's refusal, not a panic or a hang.
    fn assert_blocking_call_answers(o: &PgOwner) {
        let started = Instant::now();
        let answer = o.side().load_config_blocking(Duration::from_secs(5));
        let Err(IndexError::Invalid(text)) = answer else {
            panic!("not refused: {answer:?}");
        };
        assert!(text.contains("does not hold"), "{text}");
        let answer =
            o.side()
                .save_config_blocking(&serde_json::json!({}), "t", Duration::from_secs(5));
        assert!(
            matches!(&answer, Err(IndexError::Invalid(t)) if t.contains("does not hold")),
            "{answer:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// A blocking side call inside a current-thread runtime's body.
    #[tokio::test]
    async fn side_channel_blocking_call_in_a_current_thread_runtime() {
        let o = unreachable_owner();
        assert_blocking_call_answers(&o);
    }

    /// A blocking side call inside a multi-thread runtime's worker.
    #[tokio::test(flavor = "multi_thread")]
    async fn side_channel_blocking_call_in_a_multi_thread_runtime() {
        let o = unreachable_owner();
        assert_blocking_call_answers(&o);
    }

    /// The gate lets exactly one side decide: a caller that gave up first
    /// stops the commit, and a commit sent first makes the caller's give-up
    /// a "may have been stored".
    #[test]
    fn config_commit_gate_decides_once() {
        let g = CommitGate::default();
        assert!(g.abandon());
        assert!(!g.send_commit());
        let g = CommitGate::default();
        assert!(g.send_commit());
        assert!(!g.abandon());
    }

    /// A commit that answered is stored, one that timed out or lost its
    /// connection may be stored.
    #[test]
    fn config_commit_outcome_classifies_the_unknown() {
        assert!(commit_outcome(Some(Ok(()))).is_ok());
        for unknown in [
            None,
            Some(Err(sqlx::Error::Io(std::io::Error::other("reset")))),
            Some(Err(sqlx::Error::WorkerCrashed)),
        ] {
            let answer = commit_outcome(unknown);
            let Err(IndexError::MaybeStored(text)) = answer else {
                panic!("not uncertain: {answer:?}");
            };
            assert!(text.contains("may have been stored"), "{text}");
        }
    }

    /// A commit that outlives its caller's patience is reported as maybe
    /// stored, and it was in fact stored; a commit the server refuses is a
    /// plain error and stores nothing. A deferred constraint trigger makes
    /// the commit slow or failing.
    #[tokio::test]
    async fn config_commit_outcome_after_the_commit_went_out() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "cfgc");
        migrate(&url, &scratch).await;
        let t = target(&url, &scratch);
        let table = config_table(t.schema.as_deref());
        let mut c = plain(&t).await;
        sqlx::raw_sql(AssertSqlSafe(format!(
            "CREATE FUNCTION {schema}.cfg_at_commit() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN \
               IF NEW.body ? 'slow' THEN PERFORM pg_sleep(2); END IF; \
               IF NEW.body ? 'refuse' THEN RAISE EXCEPTION 'refused at commit'; END IF; \
               RETURN NULL; \
             END $$; \
             CREATE CONSTRAINT TRIGGER cfg_at_commit AFTER INSERT OR UPDATE ON {table} \
             DEFERRABLE INITIALLY DEFERRED FOR EACH ROW \
             EXECUTE FUNCTION {schema}.cfg_at_commit();",
            schema = scratch.name()
        )))
        .execute(&mut c)
        .await
        .unwrap();
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), fast()).unwrap();
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        let save = |body: serde_json::Value, timeout: Duration| {
            let side = a.side();
            tokio::task::spawn_blocking(move || side.save_config_blocking(&body, "test", timeout))
        };
        let first = serde_json::json!({"n": 1});
        save(first.clone(), Duration::from_secs(5))
            .await
            .unwrap()
            .unwrap();

        // The server refuses at commit: a plain error, nothing stored.
        let refused = save(serde_json::json!({"refuse": true}), Duration::from_secs(5))
            .await
            .unwrap();
        let Err(e) = refused else {
            panic!("the refused commit succeeded");
        };
        assert!(!matches!(e, IndexError::MaybeStored(_)), "{e}");
        assert!(e.to_string().contains("refused at commit"), "{e}");
        assert_eq!(read_config_document(&t).await.unwrap().unwrap().body, first);

        // The commit takes 2 s, longer than the caller waits.
        let slow = serde_json::json!({"slow": true});
        let started = Instant::now();
        let answer = save(slow.clone(), Duration::from_millis(300))
            .await
            .unwrap();
        assert!(
            matches!(&answer, Err(IndexError::MaybeStored(_))),
            "{answer:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        let mut stored = None;
        for _ in 0..50 {
            stored = read_config_document(&t).await.unwrap().map(|d| d.body);
            if stored.as_ref() == Some(&slow) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(stored, Some(slow), "the commit that went out was stored");
        a.release().await.unwrap();
    }

    /// The owner's side channel refuses a schema without `instance_config`,
    /// where the lock-free read answers `None`.
    #[tokio::test]
    async fn side_channel_reports_a_missing_config_table() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let bare = ScratchSchema::new(&url, "cfgm");
        let t = target(&url, &bare);
        let a = PgOwner::start_with(t.clone(), identity("aaaaaaaa", "a"), fast()).unwrap();
        assert!(matches!(
            a.try_acquire().await.unwrap(),
            Acquired::Held { .. }
        ));
        let err = a.side().load_config().await.unwrap_err();
        assert!(err.to_string().contains("instance_config"), "{err}");
        assert_eq!(read_config_document(&t).await.unwrap(), None);
        a.release().await.unwrap();
    }

    #[tokio::test]
    async fn owner_row_survives_until_release_and_records_released_at() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "ownr");
        migrate(&url, &scratch).await;
        let t = target(&url, &scratch);
        let mut me = identity("aaaaaaaa", "laptop");
        me.command = Some("crystalline reindex".to_string());
        let a = PgOwner::start_with(t.clone(), me.clone(), fast()).unwrap();
        let Acquired::Held { lock_pid, .. } = a.try_acquire().await.unwrap() else {
            panic!("no lock");
        };
        assert_eq!(read_owner_row(&t).await.unwrap(), None);
        assert_eq!(probe_owner(&t).await.unwrap(), Some(None));
        a.announce().await.unwrap();
        let row = read_owner_row(&t).await.unwrap().expect("the row");
        assert_eq!(row.instance_id, me.instance_id);
        assert_eq!(row.label, "laptop");
        assert_eq!(row.command.as_deref(), Some("crystalline reindex"));
        assert_eq!(row.pid, i64::from(std::process::id()));
        assert_eq!(row.backend_pid, i64::from(lock_pid));
        assert_eq!(row.version, "0.33.0-test");
        assert!(row.released_at.is_none());
        assert!((Utc::now() - row.since).num_seconds().abs() < 60);
        // A second announce keeps the time the lock was taken.
        tokio::time::sleep(Duration::from_millis(20)).await;
        a.announce().await.unwrap();
        let again = read_owner_row(&t).await.unwrap().unwrap();
        assert_eq!(again, row);
        assert_eq!(probe_owner(&t).await.unwrap(), Some(Some(row.clone())));
        // Still there after a few probe periods.
        tokio::time::sleep(fast().probe_period * 2).await;
        assert_eq!(read_owner_row(&t).await.unwrap(), Some(row.clone()));
        a.release().await.unwrap();
        let released = read_owner_row(&t).await.unwrap().expect("the row stays");
        let at = released.released_at.expect("released_at");
        assert!(at >= row.since);
        assert_eq!(
            OwnerRow {
                released_at: None,
                ..released
            },
            row
        );
        assert_eq!(probe_owner(&t).await.unwrap(), None);
        assert_eq!(a.fence().lock_pid, 0);
        // A release that was already done does nothing.
        a.release().await.unwrap();
        assert_eq!(*a.fenced().borrow(), FenceState::Holding);
    }
}
