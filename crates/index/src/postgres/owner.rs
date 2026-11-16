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
//! The timing constants are the shipped values; [`OwnerTimings`] scales them
//! down by [`timing_scale`] for tests and derives the fence detection bound
//! and the grace from them.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
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

/// The `instance_owner` table of `schema`, qualified so a missing table in
/// the schema never resolves to one in `public`.
fn owner_table(schema: Option<&str>) -> String {
    match schema {
        Some(s) => format!("{}.instance_owner", super::quote_ident(s)),
        None => "instance_owner".to_string(),
    }
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
        .ok_or_else(|| IndexError::Db(format!("an owner row time is out of range: {us}")))
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
}

/// The instance-ownership lock of one database schema.
///
/// [`PgOwner::start`] spawns the owner thread; nothing connects until the
/// first lock attempt. Dropping the handle ends the thread, which closes the
/// lock connection and so frees the lock without recording a release.
pub struct PgOwner {
    requests: mpsc::UnboundedSender<Request>,
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
        };
        std::thread::Builder::new()
            .name("crystalline-pg-owner".to_string())
            .spawn(move || runtime.block_on(worker.run(inbox)))
            .map_err(|e| IndexError::Db(format!("could not start the owner thread: {e}")))?;
        Ok(PgOwner {
            requests,
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
            .await?;
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
        let Ok((held, conn)) = self.holding() else {
            return ProbeOutcome::Broken;
        };
        let probe = sqlx::query_scalar::<_, bool>(HOLDS_KEY_SQL)
            .bind(key)
            .bind(held.lock_pid)
            .fetch_one(&mut *conn);
        match tokio::time::timeout(limit, probe).await {
            Err(_) => ProbeOutcome::Timeout,
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
