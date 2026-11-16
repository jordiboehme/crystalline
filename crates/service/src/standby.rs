//! The standby phase of `crystalline serve` on Postgres.
//!
//! One instance owns a Postgres database (or schema) at a time, through the
//! owner lock ([`crystalline_index::postgres::PgOwner`]). A `serve` that finds
//! the lock held waits as a standby until it is free, with no time limit:
//!
//! - It binds the control socket and publishes its record before the store
//!   opens, so a client probing the socket always gets an answer and never
//!   dislodges it as unresponsive ([`standby_ctl`]).
//! - It binds the HTTP endpoint early too and serves [`standby_router`]
//!   behind an [`HttpSlot`], so the full router replaces it later without a
//!   rebind.
//! - It never opens the store and never migrates: a newer standby that
//!   migrated under a live older owner would break that owner, and on an
//!   empty schema it creates nothing.
//! - It logs who owns the database once a minute.
//!
//! The Turso path never comes here: [`start_on_postgres`] answers `None` for
//! it, and `run_serve` keeps its order.

use std::sync::Arc;

use interprocess::local_socket::tokio::Listener as IpcListener;
use tokio::sync::watch;

#[cfg(not(feature = "postgres"))]
use crate::daemon::HeldOwnership;
use crate::daemon::{HttpSlot, Shared, StopFuture};

/// What the Postgres path set up before the store opens: the control socket
/// (served by [`standby_ctl`] until the daemon takes it over), the HTTP slot
/// and the held owner lock.
#[cfg_attr(
    not(feature = "postgres"),
    allow(dead_code, reason = "only the postgres backend builds one")
)]
pub(crate) struct PgStart {
    /// The early responder, which hands the listener back once the daemon
    /// sends its shared state on `handover`.
    ctl: tokio::task::JoinHandle<IpcListener>,
    handover: watch::Sender<Option<Arc<Shared>>>,
    /// The early HTTP slot; `None` when the endpoint is off or its bind
    /// failed (said in the log already).
    http: Option<HttpSlot>,
    /// The owner lock, held from here to the exit. Shared with the start
    /// guard, which releases it when a stop comes during the start.
    #[cfg(feature = "postgres")]
    owner: Arc<crystalline_index::postgres::PgOwner>,
    /// The start guard: it ends the process on a stop until the handover,
    /// then gives the stop future back.
    guard: tokio::task::JoinHandle<Option<StopFuture>>,
    guard_done: tokio::sync::oneshot::Sender<()>,
    /// The instance id every owner connection is tagged with.
    #[cfg(feature = "postgres")]
    instance_id: String,
    /// The database, as [`crystalline_index::postgres::PgTarget::describe`]
    /// says it, for the log.
    #[cfg(feature = "postgres")]
    database: String,
}

/// What the daemon keeps after the handover: the owner lock, held until the
/// last step of the shutdown gives it back ([`PgHeld::release`]) or the
/// process exits.
pub(crate) struct PgHeld {
    #[cfg(feature = "postgres")]
    owner: Arc<crystalline_index::postgres::PgOwner>,
}

impl PgHeld {
    /// Give the database back: record `released_at` in the owner row and
    /// unlock, so a standby takes over without waiting out the grace. The
    /// last step of a graceful stop; a failure is said in the log, and the
    /// exit right after frees the lock anyway.
    pub(crate) async fn release(&self) {
        #[cfg(feature = "postgres")]
        if let Err(e) = self.owner.release().await {
            tracing::warn!("could not release the database ({e}); the exit frees it");
        }
    }
}

impl PgStart {
    /// The settings every Postgres connection of this owner is built with:
    /// tagged with the instance id and fenced on the held lock, so a
    /// connection made after the lock is lost is refused.
    #[cfg(feature = "postgres")]
    pub(crate) fn session_options(&self) -> crystalline_index::postgres::PgSessionOptions {
        crystalline_index::postgres::PgSessionOptions {
            instance_tag: Some(self.instance_id.clone()),
            fence: Some(self.owner.fence()),
        }
    }

    /// Write the owner row again: on a schema the store open just created
    /// the table in, the announce right after the lock found none.
    #[cfg(feature = "postgres")]
    pub(crate) async fn announce(&self) -> anyhow::Result<()> {
        self.owner.announce().await.map_err(|e| {
            anyhow::anyhow!(
                "could not record this instance as the owner of the database {}: {e}",
                self.database
            )
        })
    }

    /// Open the accounts store in the database this instance owns, with the
    /// owner's connection settings.
    #[cfg(feature = "postgres")]
    pub(crate) async fn open_accounts(
        &self,
        cfg: &crystalline_core::config::GlobalConfig,
    ) -> anyhow::Result<crate::rest::AuthStore> {
        crate::rest::AuthStore::open_at(
            &crystalline_core::config::AccountsLocation::Postgres(cfg.database()),
            &self.session_options(),
        )
        .await
    }

    /// The early HTTP slot: the full router goes into it once built.
    pub(crate) fn http_slot(&self) -> Option<HttpSlot> {
        self.http.clone()
    }

    /// Hand the control socket to the daemon: every connection the early
    /// responder holds for the start is served on `shared` from now on, and
    /// the listener comes back for the accept loop, with the stop future for
    /// the daemon's own wait.
    pub(crate) async fn hand_over(
        self,
        shared: &Arc<Shared>,
    ) -> anyhow::Result<(IpcListener, PgHeld, StopFuture)> {
        let _ = self.guard_done.send(());
        let stop =
            self.guard.await.ok().flatten().ok_or_else(|| {
                anyhow::anyhow!("the start guard did not give the stop signals back")
            })?;
        self.handover.send_replace(Some(shared.clone()));
        let listener = self
            .ctl
            .await
            .map_err(|e| anyhow::anyhow!("the early control socket responder failed: {e}"))?;
        Ok((
            listener,
            PgHeld {
                #[cfg(feature = "postgres")]
                owner: self.owner,
            },
            stop,
        ))
    }
}

/// The router left behind in the early slot when the full router could not
/// be built: the probes only, so nothing says standby any more.
pub(crate) fn probes_router(base: crystalline_core::base::BasePath) -> axum::Router {
    let router = axum::Router::new()
        .route("/health", axum::routing::get(crate::daemon::health))
        .route("/ready", axum::routing::get(crate::daemon::ready));
    crate::daemon::under_base_path(router, base)
}

/// Without the postgres backend there is no Postgres path: the order of
/// `run_serve` stays the Turso one.
#[cfg(not(feature = "postgres"))]
pub(crate) async fn start_on_postgres(
    _cfg: &crystalline_core::config::GlobalConfig,
    _ownership: &HeldOwnership,
    _instance_id: &str,
    _http_addr: Option<&str>,
    _shutdown: &watch::Sender<bool>,
    stop: StopFuture,
    _farewell: Option<&'static str>,
) -> anyhow::Result<(Option<PgStart>, Option<StopFuture>)> {
    Ok((None, Some(stop)))
}

#[cfg(feature = "postgres")]
pub(crate) use pg::*;

#[cfg(feature = "postgres")]
mod pg {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crystalline_core::base::BasePath;
    use crystalline_core::config::{DatabaseBackend, GlobalConfig};
    use crystalline_index::IndexError;
    use crystalline_index::postgres::{
        Acquired, FenceState, OwnerIdentity, OwnerRow, PgOwner, PgTarget, StandbyStatus,
        TakeoverReport,
    };
    use interprocess::local_socket::tokio::{Listener as IpcListener, Stream as IpcStream};
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::watch;

    use super::PgStart;
    use crate::control::{envelope_err, envelope_ok};
    use crate::daemon::{HeldOwnership, HttpSlot, Role, Shared, StopFuture};

    /// The `Retry-After` the standby's HTTP answers carry, in seconds.
    pub(crate) const RETRY_AFTER_SECS: u64 = 5;

    /// The body of every HTTP answer of a standby but `/health` and `/ready`.
    /// It never names the active instance: this surface is unauthenticated.
    pub(crate) const STANDBY_HTTP: &str =
        "this instance is a standby; another instance is active on this database";

    /// The same answer while the instance that just took the database still
    /// opens its store.
    pub(crate) const STARTING_HTTP: &str = "this instance is starting; try again in a moment";

    /// What a connection parked for the start hears when the start failed
    /// after the lock was taken.
    pub(crate) const START_FAILED: &str = "this instance could not start; see its log";

    /// The test-only variable that holds a Postgres start for this many
    /// milliseconds right after the lock is taken, so a test can reach the
    /// window between the lock and the handover. Read by a debug build only.
    pub(crate) const START_HOLD_ENV: &str = "CRYSTALLINE_TEST_START_HOLD_MS";

    /// How long [`START_HOLD_ENV`] holds the start, in a debug build.
    fn start_hold() -> Option<Duration> {
        if !cfg!(debug_assertions) {
            return None;
        }
        std::env::var(START_HOLD_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|ms| *ms > 0)
            .map(Duration::from_millis)
    }

    /// How long a stop during the start waits for the release.
    const RELEASE_WAIT: Duration = Duration::from_secs(2);

    /// How often a standby says in the log who owns the database.
    pub(crate) const STANDBY_LOG_EVERY: Duration = Duration::from_secs(60);

    /// The longest session opener the standby reads before it answers an
    /// MCP client; a longer one is answered by a closed connection.
    const OPENER_LIMIT: u64 = 1024 * 1024;

    /// The owner row, unless it records a release: a released row is the
    /// predecessor's, and the session that holds the lock now has not
    /// announced itself yet.
    fn current_owner(owner: Option<&OwnerRow>) -> Option<&OwnerRow> {
        owner.filter(|o| o.released_at.is_none())
    }

    fn since(owner: &OwnerRow) -> String {
        owner
            .since
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    /// Who owns the database, for the log and the control socket.
    pub(crate) fn owner_phrase(owner: Option<&OwnerRow>) -> String {
        match current_owner(owner) {
            Some(o) => format!(
                "{} (pid {}, version {}, since {})",
                o.label,
                o.pid,
                o.version,
                since(o)
            ),
            None => "an instance that has not announced itself yet".to_string(),
        }
    }

    /// The line a standby logs at start and once a minute.
    pub(crate) fn standby_line(database: &str, owner: Option<&OwnerRow>) -> String {
        format!(
            "standby: the database {database} is owned by {}; waiting",
            owner_phrase(owner)
        )
    }

    /// The sentence a standby refuses every command and session with.
    pub(crate) fn standby_refusal(owner: Option<&OwnerRow>) -> String {
        match current_owner(owner) {
            Some(o) => format!(
                "this instance is a standby; the active instance is {} (since {}); run the command there",
                o.label,
                since(o)
            ),
            None => "this instance is a standby; the active instance has not announced itself yet; run the command there".to_string(),
        }
    }

    /// The router a standby serves: `/health` (200, `role` standby), `/ready`
    /// (503) and 503 with `Retry-After` for everything else, under the base
    /// path like the full router.
    pub(crate) fn standby_router(base: BasePath) -> axum::Router {
        let router = axum::Router::new()
            .route("/health", axum::routing::get(crate::daemon::health))
            .route("/ready", axum::routing::get(crate::daemon::ready))
            .fallback(standby_fallback);
        crate::daemon::under_base_path(router, base)
    }

    async fn standby_fallback() -> axum::response::Response {
        use axum::response::IntoResponse;
        let message = match crate::daemon::current_role() {
            Role::Standby => STANDBY_HTTP,
            Role::Active => STARTING_HTTP,
        };
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            [(
                axum::http::header::RETRY_AFTER,
                RETRY_AFTER_SECS.to_string(),
            )],
            axum::Json(json!({ "error": message })),
        )
            .into_response()
    }

    /// What the early responder knows: the standby's progress and the
    /// database it waits for.
    #[derive(Clone)]
    pub(crate) struct StandbyView {
        pub(crate) status: watch::Receiver<StandbyStatus>,
        /// [`PgTarget::describe`] of the database.
        pub(crate) database: String,
        pub(crate) started: Instant,
    }

    impl StandbyView {
        fn refusal(&self) -> String {
            standby_refusal(self.status.borrow().owner.as_ref())
        }

        /// The `status` answer of a standby: its role, the owner row and how
        /// long it has waited, beside the pid and version a client reads to
        /// recognise a daemon.
        fn status_data(&self) -> Value {
            let status = self.status.borrow().clone();
            let owner = current_owner(status.owner.as_ref()).map(|o| {
                json!({
                    "label": o.label,
                    "instance_id": o.instance_id,
                    "command": o.command,
                    "pid": o.pid,
                    "version": o.version,
                    "since": since(o),
                })
            });
            let waited = (chrono::Utc::now() - status.waiting_since)
                .num_seconds()
                .max(0);
            let intent = crate::instance::serve_intent();
            json!({
                "pid": std::process::id(),
                "version": crystalline_core::VERSION,
                "role": "standby",
                "uptime_secs": self.started.elapsed().as_secs(),
                "database": self.database,
                "owner": owner,
                "waiting_secs": waited,
                "polls": status.polls,
                "last_error": status.last_error,
                "started_by": intent.map(|i| i.started_by.as_str()).unwrap_or("unknown"),
            })
        }
    }

    /// Serve the control socket from the early bind until the daemon takes
    /// it over through `handover`, then hand the listener back.
    ///
    /// A standby answers `sessions` (an empty list, so `diagnose_holder`
    /// sees a responsive holder and never dislodges it), `holder` (its
    /// record, marked with the standby's refusal so a bridge stops there),
    /// `status` (role, owner row, wait) and `shutdown` (which it signals on
    /// `shutdown`). Every other command, and every MCP session, gets the
    /// refusal that names the active instance. Once this instance holds the
    /// lock and opens its store, a connection waits for the daemon and is
    /// then served by it as usual.
    pub(crate) async fn standby_ctl(
        listener: IpcListener,
        view: StandbyView,
        shutdown: watch::Sender<bool>,
        mut handover: watch::Receiver<Option<Arc<Shared>>>,
    ) -> IpcListener {
        use interprocess::local_socket::traits::tokio::Listener as _;
        loop {
            tokio::select! {
                _ = handed_over(&mut handover) => return listener,
                accepted = listener.accept() => match accepted {
                    Ok(stream) => {
                        let view = view.clone();
                        let shutdown = shutdown.clone();
                        let handover = handover.clone();
                        tokio::spawn(async move {
                            serve_early_conn(stream, view, shutdown, handover).await;
                        });
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                },
            }
        }
    }

    /// Resolve once the daemon's shared state arrived, or never when the
    /// sender is gone (the start failed and the process is leaving).
    async fn handed_over(rx: &mut watch::Receiver<Option<Arc<Shared>>>) {
        loop {
            if rx.borrow().is_some() {
                return;
            }
            if rx.changed().await.is_err() {
                return futures::future::pending().await;
            }
        }
    }

    /// What a parked connection found when its wait ended.
    enum Waited {
        /// The daemon serves now.
        Daemon(Arc<Shared>),
        /// This instance is a standby (or turned into one while waiting).
        Standby,
        /// The start failed after the lock was taken; the process is leaving.
        Failed,
    }

    /// Wait for the daemon's shared state while this instance starts.
    async fn wait_for_daemon(rx: &mut watch::Receiver<Option<Arc<Shared>>>) -> Waited {
        let mut role = crate::daemon::role_watch();
        loop {
            if let Some(shared) = rx.borrow().clone() {
                return Waited::Daemon(shared);
            }
            if *role.borrow() == Role::Standby {
                return Waited::Standby;
            }
            tokio::select! {
                changed = rx.changed() => if changed.is_err() { return Waited::Failed },
                _ = role.changed() => {}
            }
        }
    }

    async fn serve_early_conn(
        mut stream: IpcStream,
        view: StandbyView,
        shutdown: watch::Sender<bool>,
        mut handover: watch::Receiver<Option<Arc<Shared>>>,
    ) {
        let Ok(line) = crate::instance::read_mode_line(&mut stream).await else {
            return;
        };
        let (mode, options) = crate::instance::split_mode_line(&line);
        match mode {
            "ctl" => serve_early_ctl(stream, view, shutdown, handover).await,
            "mcp" => match wait_for_daemon(&mut handover).await {
                Waited::Daemon(shared) => {
                    crate::daemon::serve_mcp_conn(stream, &options, shared).await
                }
                Waited::Standby => refuse_mcp(stream, &view.refusal()).await,
                Waited::Failed => refuse_mcp(stream, START_FAILED).await,
            },
            other => tracing::debug!("unknown handshake '{other}'"),
        }
    }

    /// Answer an MCP client's session opener with the refusal as the
    /// initialize error, then close.
    async fn refuse_mcp(stream: IpcStream, refusal: &str) {
        let (read, mut write) = tokio::io::split(stream);
        let mut opener = String::new();
        if BufReader::new(read.take(OPENER_LIMIT))
            .read_line(&mut opener)
            .await
            .is_err()
        {
            return;
        }
        if let Some(reply) = crate::client::initialize_error_reply(opener.trim(), refusal) {
            let _ = write.write_all(reply.as_bytes()).await;
            let _ = write.write_all(b"\n").await;
            let _ = write.flush().await;
        }
    }

    async fn serve_early_ctl(
        stream: IpcStream,
        view: StandbyView,
        shutdown: watch::Sender<bool>,
        mut handover: watch::Receiver<Option<Arc<Shared>>>,
    ) {
        let (read, mut write) = tokio::io::split(stream);
        let mut reader = BufReader::new(read);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let (response, stop) = match serde_json::from_str::<Value>(trimmed) {
                Ok(req) => answer_early(&req, &view, &mut handover).await,
                Err(e) => (envelope_err(format!("invalid json: {e}")), Stop::No),
            };
            let mut out = serde_json::to_string(&response).unwrap_or_default();
            out.push('\n');
            if write.write_all(out.as_bytes()).await.is_err() {
                break;
            }
            let _ = write.flush().await;
            match stop {
                Stop::No => {}
                Stop::Standby => {
                    shutdown.send_replace(true);
                    break;
                }
                Stop::Daemon(shared) => {
                    shared.trigger_shutdown("ctl shutdown");
                    break;
                }
            }
        }
    }

    /// Whether a ctl answer ends this instance, and through whom.
    enum Stop {
        No,
        Standby,
        Daemon(Arc<Shared>),
    }

    async fn answer_early(
        req: &Value,
        view: &StandbyView,
        handover: &mut watch::Receiver<Option<Arc<Shared>>>,
    ) -> (Value, Stop) {
        let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or("");
        let standby = crate::daemon::current_role() == Role::Standby;
        match cmd {
            // Read from memory only, so a probe gets its answer at once in
            // every phase: no session runs before the daemon serves.
            "sessions" => (envelope_ok(json!({ "count": 0, "sessions": [] })), Stop::No),
            "holder" => {
                let mut data = crate::control::holder_data(std::process::id());
                if standby {
                    data["standby"] = json!(view.refusal());
                }
                (envelope_ok(data), Stop::No)
            }
            // Never parked: a stop must reach a standby and a start alike at
            // once (the standby wait and the start guard both hear it).
            "shutdown" => {
                let stop = match handover.borrow().clone() {
                    Some(shared) => Stop::Daemon(shared),
                    None => Stop::Standby,
                };
                (envelope_ok(json!({ "stopping": true })), stop)
            }
            _ => match wait_for_daemon(handover).await {
                Waited::Daemon(shared) => {
                    let (response, stop) = crate::control::handle(req, &shared).await;
                    (response, if stop { Stop::Daemon(shared) } else { Stop::No })
                }
                Waited::Standby => match cmd {
                    "status" => (envelope_ok(view.status_data()), Stop::No),
                    _ => (envelope_err(view.refusal()), Stop::No),
                },
                Waited::Failed => (envelope_err(START_FAILED), Stop::No),
            },
        }
    }

    /// How a standby's wait ended.
    enum WaitEnd {
        /// This instance holds the lock now.
        Held(Option<OwnerRow>),
        /// A signal or a ctl `shutdown`, with its reason.
        Stop(&'static str),
    }

    /// Take the database on the Postgres path, or wait for it as a standby.
    ///
    /// Any other backend gets `(None, Some(stop))`, with `stop` never polled.
    /// On Postgres it binds the control socket and publishes the record,
    /// binds the HTTP endpoint with the standby router in its slot, and tries
    /// the owner lock. When the lock is held elsewhere this instance is a
    /// standby until the lock comes free. Once it holds the lock a start
    /// guard takes `stop` and watches it, and a ctl `shutdown`, until the
    /// handover ([`PgStart::hand_over`] gives `stop` back). A stop at any
    /// point leaves the same way: the lock released when held, the record
    /// removed, `farewell` on the console and exit 0.
    pub(crate) async fn start_on_postgres(
        cfg: &GlobalConfig,
        ownership: &HeldOwnership,
        instance_id: &str,
        http_addr: Option<&str>,
        shutdown: &watch::Sender<bool>,
        mut stop: StopFuture,
        farewell: Option<&'static str>,
    ) -> anyhow::Result<(Option<PgStart>, Option<StopFuture>)> {
        let database = cfg.database();
        if database.backend != DatabaseBackend::Postgres {
            return Ok((None, Some(stop)));
        }
        let target = PgTarget::from_config(&database).map_err(|e| anyhow::anyhow!("{e}"))?;
        let described = target.describe();

        // Bound and published before anything waits or opens: a client that
        // asks the socket is answered from now on.
        let listener = {
            let slot = ownership
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(held) = slot.as_ref() else {
                anyhow::bail!("this instance is stopping");
            };
            let listener = held.bind_listener()?;
            held.publish()?;
            listener
        };
        crate::daemon::start_step("record published");

        let http = match http_addr {
            Some(addr) => bind_early_http(addr, crate::settings::base_path(cfg), shutdown).await,
            None => None,
        };

        let (status_tx, status_rx) = watch::channel(StandbyStatus::new());
        let (handover, handover_rx) = watch::channel(None);
        let view = StandbyView {
            status: status_rx.clone(),
            database: described.clone(),
            started: Instant::now(),
        };
        let ctl = tokio::spawn(standby_ctl(listener, view, shutdown.clone(), handover_rx));

        let me = OwnerIdentity {
            instance_id: instance_id.to_string(),
            label: cfg
                .instance_name()
                .unwrap_or_else(|| instance_id.to_string()),
            command: None,
            version: crystalline_core::VERSION.to_string(),
        };
        let owner = Arc::new(PgOwner::start(target, me).map_err(|e| anyhow::anyhow!("{e}"))?);
        let leaving = Leaving {
            owner: owner.clone(),
            ownership: ownership.clone(),
            farewell,
        };
        let previous = match owner.try_acquire().await {
            Ok(Acquired::Held { previous, .. }) => previous,
            Ok(Acquired::Busy(row)) => {
                crate::daemon::set_role(Role::Standby);
                status_tx.send_modify(|s| s.owner = row);
                let end = wait_as_standby(
                    &owner,
                    &described,
                    status_tx,
                    shutdown.subscribe(),
                    &mut stop,
                )
                .await?;
                match end {
                    WaitEnd::Held(previous) => {
                        crate::daemon::set_role(Role::Active);
                        tracing::info!(
                            "standby: the database {described} came free; this instance takes it over"
                        );
                        previous
                    }
                    WaitEnd::Stop(reason) => leaving.leave("standby", reason).await,
                }
            }
            Err(e) => anyhow::bail!("could not reach the database {described} to take it: {e}"),
        };

        // From the lock to the handover a stop must still end the process at
        // once and give the database back: the store open, the migrations
        // and (from the takeover on) the grace all run in this window.
        let (guard_done, done) = tokio::sync::oneshot::channel();
        let guard = tokio::spawn(guard_start(stop, shutdown.subscribe(), done, leaving));
        // From the lock on, a lost lock ends the process with EXIT_FENCED,
        // during the start as later.
        tokio::spawn(watch_fence(owner.fenced(), described.clone()));
        if let Some(hold) = start_hold() {
            tracing::info!("test hook: holding the start for {}ms", hold.as_millis());
            tokio::time::sleep(hold).await;
        }

        // The takeover: end the predecessor's sessions and wait out the
        // grace unless it released cleanly, then say who owns the database.
        // Both before anything opens a pool, and a failure stops the start.
        let report = owner
            .takeover(previous.as_ref())
            .await
            .map_err(|e| anyhow::anyhow!("{}", plain(e)))?;
        let released = previous.as_ref().is_some_and(|p| p.released_at.is_some());
        tracing::info!("{}", takeover_line(&described, &report, released));
        owner.announce().await.map_err(|e| {
            anyhow::anyhow!(
                "could not record this instance as the owner of the database {described}: {e}"
            )
        })?;
        Ok((
            Some(PgStart {
                ctl,
                handover,
                http,
                owner,
                guard,
                guard_done,
                instance_id: instance_id.to_string(),
                database: described,
            }),
            None,
        ))
    }

    /// An index error as its sentence: an `Invalid` one is already written
    /// for a person (a refused takeover names the grant to give), so it goes
    /// without the "invalid input" prefix.
    fn plain(e: IndexError) -> String {
        match e {
            IndexError::Invalid(sentence) => sentence,
            other => other.to_string(),
        }
    }

    /// The log line of a takeover: how many sessions of the predecessor it
    /// ended, and whether it waited out the grace.
    ///
    /// `released` says the predecessor recorded a clean release, which is
    /// when the takeover skips the grace.
    pub(crate) fn takeover_line(database: &str, report: &TakeoverReport, released: bool) -> String {
        let waited = if released {
            "without a grace (the instance before released it)".to_string()
        } else {
            format!("after a grace of {}ms", report.waited.as_millis())
        };
        let ended = crystalline_core::text::plural(
            usize::try_from(report.terminated).unwrap_or(usize::MAX),
            "session",
            "sessions",
        );
        format!(
            "takeover: this instance owns the database {database}; ended {ended} of the instance before, {waited}"
        )
    }

    /// The line a fenced owner logs before it exits.
    pub(crate) fn fenced_line(database: &str, reason: &str) -> String {
        format!(
            "lost ownership of the database {database}; stopping so another instance can take over ({reason}; a connection pooler in transaction mode between Crystalline and Postgres also causes this)"
        )
    }

    /// Exit with [`crate::daemon::EXIT_FENCED`] once the owner lock is lost:
    /// one error line, and none of the departure's steps, whose writes
    /// (host locks, a checkpoint, a saved sign-in) would race the instance
    /// that owns the database now.
    async fn watch_fence(mut fenced: watch::Receiver<FenceState>, database: String) {
        loop {
            let state = fenced.borrow_and_update().clone();
            if let FenceState::Fenced(reason) = state {
                tracing::error!("{}", fenced_line(&database, &reason));
                crate::daemon::exit_with(crate::daemon::EXIT_FENCED);
            }
            if fenced.changed().await.is_err() {
                return;
            }
        }
    }

    /// How a stop leaves before the daemon serves.
    struct Leaving {
        owner: Arc<PgOwner>,
        ownership: HeldOwnership,
        farewell: Option<&'static str>,
    }

    impl Leaving {
        /// Release the lock when it is held (the release does nothing
        /// otherwise), remove the record, the socket and the lock file, and
        /// exit 0. `phase` names where the stop came, for the log.
        ///
        /// The start may still run beside this (a store open, a migration),
        /// but every connection it opens is fenced on the lock: once the
        /// lock is given back, a new connection is refused at connect, and
        /// the instance that takes the database next ends the ones still
        /// open before it writes anything.
        async fn leave(self, phase: &str, reason: &str) -> ! {
            tracing::info!("{phase}: stopping ({reason})");
            match tokio::time::timeout(RELEASE_WAIT, self.owner.release()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!("could not release the database ({e}); the exit frees it")
                }
                Err(_) => tracing::warn!(
                    "the release did not finish in {}s; the exit frees the database",
                    RELEASE_WAIT.as_secs()
                ),
            }
            drop(
                self.ownership
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take(),
            );
            if let Some(line) = self.farewell {
                crate::logging::console_line(line);
            }
            crate::session_end::notify_stopped();
            crate::daemon::exit_with(0);
        }
    }

    /// Watch `stop` and a ctl `shutdown` from the lock to the handover and
    /// leave on either; give `stop` back once `done` fires (or its sender is
    /// gone: the start failed and the process leaves with the error).
    async fn guard_start(
        mut stop: StopFuture,
        mut shutdown: watch::Receiver<bool>,
        done: tokio::sync::oneshot::Receiver<()>,
        leaving: Leaving,
    ) -> Option<StopFuture> {
        tokio::select! {
            biased;
            _ = done => Some(stop),
            reason = &mut stop => leaving.leave("start", reason).await,
            _ = crate::daemon::wait_true(&mut shutdown) => leaving.leave("start", "ctl shutdown").await,
        }
    }

    /// Bind the HTTP endpoint before the store opens and serve the standby
    /// router from a slot. A failed bind warns, as the normal start does, and
    /// the instance carries on without HTTP.
    async fn bind_early_http(
        addr: &str,
        base: BasePath,
        shutdown: &watch::Sender<bool>,
    ) -> Option<HttpSlot> {
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => listener,
            Err(err) => {
                tracing::warn!(
                    "HTTP endpoint failed on {addr} ({err}); MCP over the socket is unaffected - free the port, bind another address, or turn the endpoint off (serve --http off, or service.http=false when no flag is given)"
                );
                return None;
            }
        };
        let slot = HttpSlot::new(standby_router(base));
        let served = slot.clone();
        let rx = shutdown.subscribe();
        let addr = addr.to_string();
        tokio::spawn(async move {
            if let Err(err) = crate::daemon::serve_http_slot(listener, served, rx).await {
                tracing::warn!(
                    "HTTP endpoint on {addr} stopped ({err}); MCP over the socket is unaffected and a daemon restart brings it back"
                );
            }
        });
        Some(slot)
    }

    /// Poll the owner lock until it comes free, logging who owns the database
    /// at start and every [`STANDBY_LOG_EVERY`], until a stop ends the wait.
    async fn wait_as_standby(
        owner: &PgOwner,
        database: &str,
        status: watch::Sender<StandbyStatus>,
        mut shutdown: watch::Receiver<bool>,
        stop: &mut StopFuture,
    ) -> anyhow::Result<WaitEnd> {
        let seen = status.subscribe();
        let acquire = owner.wait_acquire(status);
        tokio::pin!(acquire);
        let mut log = tokio::time::interval(STANDBY_LOG_EVERY);
        log.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                acquired = &mut acquire => {
                    return match acquired {
                        Ok(Acquired::Held { previous, .. }) => Ok(WaitEnd::Held(previous)),
                        Ok(Acquired::Busy(_)) => {
                            anyhow::bail!("the wait for the database {database} ended without the lock")
                        }
                        Err(e) => anyhow::bail!("the wait for the database {database} failed: {e}"),
                    };
                }
                reason = stop.as_mut() => return Ok(WaitEnd::Stop(reason)),
                _ = crate::daemon::wait_true(&mut shutdown) => return Ok(WaitEnd::Stop("ctl shutdown")),
                _ = log.tick() => {
                    tracing::info!("{}", standby_line(database, seen.borrow().owner.as_ref()));
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use chrono::{TimeZone, Utc};

        fn row(released: bool) -> OwnerRow {
            OwnerRow {
                instance_id: "0123456789abcdef".to_string(),
                label: "prod-a".to_string(),
                command: None,
                pid: 42,
                backend_pid: 4242,
                version: "0.33.0".to_string(),
                since: Utc.with_ymd_and_hms(2026, 11, 16, 9, 30, 0).unwrap(),
                released_at: released
                    .then(|| Utc.with_ymd_and_hms(2026, 11, 16, 10, 0, 0).unwrap()),
            }
        }

        /// The sentences name the active instance when it announced itself,
        /// and say that it has not otherwise; a released row is a
        /// predecessor's and names nobody.
        #[test]
        fn the_standby_sentences_name_the_owner() {
            let owner = row(false);
            assert_eq!(
                standby_line("db:5432/c, schema a", Some(&owner)),
                "standby: the database db:5432/c, schema a is owned by prod-a (pid 42, version 0.33.0, since 2026-11-16T09:30:00Z); waiting"
            );
            assert_eq!(
                standby_refusal(Some(&owner)),
                "this instance is a standby; the active instance is prod-a (since 2026-11-16T09:30:00Z); run the command there"
            );
            for unknown in [None, Some(row(true))] {
                assert!(
                    standby_line("x", unknown.as_ref())
                        .contains("an instance that has not announced itself yet")
                );
                assert!(standby_refusal(unknown.as_ref()).contains("has not announced itself yet"));
            }
        }

        /// The takeover line says whether the grace was waited out, and the
        /// fence line names the database and what a pooler does.
        #[test]
        fn the_takeover_and_fence_lines_say_what_happened() {
            let report = TakeoverReport {
                terminated: 2,
                waited: Duration::from_millis(1500),
            };
            assert_eq!(
                takeover_line("db:5432/c, schema a", &report, false),
                "takeover: this instance owns the database db:5432/c, schema a; ended 2 sessions of the instance before, after a grace of 1500ms"
            );
            assert!(
                takeover_line("x", &report, true)
                    .ends_with("without a grace (the instance before released it)")
            );
            let line = fenced_line("db:5432/c, schema a", "the lock connection was lost");
            assert!(line.starts_with(
                "lost ownership of the database db:5432/c, schema a; stopping so another instance can take over (the lock connection was lost;"
            ));
            assert!(line.contains("pooler"));
        }

        async fn call(router: &axum::Router, path: &str) -> (u16, Option<String>, String) {
            use tower_service::Service;
            let response = router
                .clone()
                .call(
                    axum::http::Request::get(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status().as_u16();
            let retry = response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .map(|v| v.to_str().unwrap().to_string());
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            (status, retry, String::from_utf8(body.to_vec()).unwrap())
        }

        /// The standby router: `/health` 200 with the standby role, `/ready`
        /// 503, everything else 503 with `Retry-After: 5` and a body that
        /// names nobody, under the base path too.
        #[tokio::test]
        async fn the_standby_router_answers_503_but_for_health() {
            let _globals = crate::daemon::PROCESS_STATE_LOCK.lock().await;
            crate::daemon::set_role(Role::Standby);
            crate::daemon::set_ready(false);
            for base in ["/", "/crystalline"] {
                let router = standby_router(BasePath::parse(base).unwrap());
                let prefix = base.trim_end_matches('/');
                let (status, _, body) = call(&router, &format!("{prefix}/health")).await;
                assert_eq!(status, 200);
                let json: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(json["role"], "standby");
                assert_eq!(call(&router, &format!("{prefix}/ready")).await.0, 503);
                let (status, retry, body) =
                    call(&router, &format!("{prefix}/api/v1/domains")).await;
                assert_eq!(status, 503);
                assert_eq!(retry.as_deref(), Some("5"));
                let json: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(json["error"], STANDBY_HTTP);
            }
            crate::daemon::set_role(Role::Active);
        }
    }
}
