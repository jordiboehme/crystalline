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

use crate::daemon::{HttpSlot, Shared};

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
    /// The owner lock, held from here to the exit.
    #[cfg(feature = "postgres")]
    owner: crystalline_index::postgres::PgOwner,
    /// The owner row as the predecessor left it, for the takeover.
    #[cfg(feature = "postgres")]
    #[allow(dead_code, reason = "read by the takeover (Task 1.11)")]
    previous: Option<crystalline_index::postgres::OwnerRow>,
}

/// What the daemon keeps after the handover: the owner lock, which goes with
/// the process.
pub(crate) struct PgHeld {
    #[cfg(feature = "postgres")]
    _owner: crystalline_index::postgres::PgOwner,
}

impl PgStart {
    /// The early HTTP slot: the full router goes into it once built.
    pub(crate) fn http_slot(&self) -> Option<HttpSlot> {
        self.http.clone()
    }

    /// Hand the control socket to the daemon: every connection the early
    /// responder holds for the start is served on `shared` from now on, and
    /// the listener comes back for the accept loop.
    pub(crate) async fn hand_over(
        self,
        shared: &Arc<Shared>,
    ) -> anyhow::Result<(IpcListener, PgHeld)> {
        self.handover.send_replace(Some(shared.clone()));
        let listener = self
            .ctl
            .await
            .map_err(|e| anyhow::anyhow!("the early control socket responder failed: {e}"))?;
        Ok((
            listener,
            PgHeld {
                #[cfg(feature = "postgres")]
                _owner: self.owner,
            },
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
    ownership: crate::instance::Ownership,
    _instance_id: &str,
    _http_addr: Option<&str>,
    _shutdown: &watch::Sender<bool>,
    _stop_signals: impl std::future::Future<Output = &'static str>,
    _farewell: Option<&'static str>,
) -> anyhow::Result<(crate::instance::Ownership, Option<PgStart>)> {
    Ok((ownership, None))
}

#[cfg(feature = "postgres")]
pub(crate) use pg::*;

#[cfg(feature = "postgres")]
mod pg {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crystalline_core::base::BasePath;
    use crystalline_core::config::{DatabaseBackend, GlobalConfig};
    use crystalline_index::postgres::{
        Acquired, OwnerIdentity, OwnerRow, PgOwner, PgTarget, StandbyStatus,
    };
    use interprocess::local_socket::tokio::{Listener as IpcListener, Stream as IpcStream};
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::watch;

    use super::PgStart;
    use crate::control::{envelope_err, envelope_ok};
    use crate::daemon::{HttpSlot, Role, Shared};

    /// The `Retry-After` the standby's HTTP answers carry, in seconds.
    pub(crate) const RETRY_AFTER_SECS: u64 = 5;

    /// The body of every HTTP answer of a standby but `/health` and `/ready`.
    /// It never names the active instance: this surface is unauthenticated.
    pub(crate) const STANDBY_HTTP: &str =
        "this instance is a standby; another instance is active on this database";

    /// The same answer while the instance that just took the database still
    /// opens its store.
    pub(crate) const STARTING_HTTP: &str = "this instance is starting; try again in a moment";

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

    /// The daemon's shared state once it is there, or `None` while this
    /// instance is a standby (or turns into one while waiting) and when the
    /// start failed.
    async fn wait_for_daemon(rx: &mut watch::Receiver<Option<Arc<Shared>>>) -> Option<Arc<Shared>> {
        let mut role = crate::daemon::role_watch();
        loop {
            if let Some(shared) = rx.borrow().clone() {
                return Some(shared);
            }
            if *role.borrow() == Role::Standby {
                return None;
            }
            tokio::select! {
                changed = rx.changed() => if changed.is_err() { return None },
                changed = role.changed() => if changed.is_err() { return None },
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
                Some(shared) => crate::daemon::serve_mcp_conn(stream, &options, shared).await,
                None => refuse_mcp(stream, &view.refusal()).await,
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
            _ => match wait_for_daemon(handover).await {
                Some(shared) => {
                    let (response, stop) = crate::control::handle(req, &shared).await;
                    (response, if stop { Stop::Daemon(shared) } else { Stop::No })
                }
                None => match cmd {
                    "status" => (envelope_ok(view.status_data()), Stop::No),
                    "shutdown" => (envelope_ok(json!({ "stopping": true })), Stop::Standby),
                    _ => (envelope_err(view.refusal()), Stop::No),
                },
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
    /// Answers `None` for any other backend. On Postgres it binds the
    /// control socket and publishes the record, binds the HTTP endpoint with
    /// the standby router in its slot, and tries the owner lock. When the lock
    /// is held elsewhere this instance is a standby until the lock comes free;
    /// a stop while it waits removes the record and exits 0, since there is
    /// nothing to release (`farewell` goes to the console then, as a stopping
    /// daemon's does). It returns once this instance holds the lock, for the
    /// normal start, and hands `ownership` back.
    pub(crate) async fn start_on_postgres(
        cfg: &GlobalConfig,
        ownership: crate::instance::Ownership,
        instance_id: &str,
        http_addr: Option<&str>,
        shutdown: &watch::Sender<bool>,
        stop_signals: impl std::future::Future<Output = &'static str>,
        farewell: Option<&'static str>,
    ) -> anyhow::Result<(crate::instance::Ownership, Option<PgStart>)> {
        let database = cfg.database();
        if database.backend != DatabaseBackend::Postgres {
            return Ok((ownership, None));
        }
        let target = PgTarget::from_config(&database).map_err(|e| anyhow::anyhow!("{e}"))?;
        let described = target.describe();

        // Bound and published before anything waits or opens: a client that
        // asks the socket is answered from now on.
        let listener = ownership.bind_listener()?;
        ownership.publish()?;
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
        let owner = PgOwner::start(target, me).map_err(|e| anyhow::anyhow!("{e}"))?;
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
                    stop_signals,
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
                    WaitEnd::Stop(reason) => {
                        tracing::info!("standby: stopping ({reason})");
                        // The record, the socket and the lock file go; the
                        // database was never this instance's.
                        drop(ownership);
                        if let Some(line) = farewell {
                            crate::logging::console_line(line);
                        }
                        crate::session_end::notify_stopped();
                        crate::daemon::exit_with(0);
                    }
                }
            }
            Err(e) => anyhow::bail!("could not reach the database {described} to take it: {e}"),
        };
        Ok((
            ownership,
            Some(PgStart {
                ctl,
                handover,
                http,
                owner,
                previous,
            }),
        ))
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
        stop_signals: impl std::future::Future<Output = &'static str>,
    ) -> anyhow::Result<WaitEnd> {
        let seen = status.subscribe();
        let acquire = owner.wait_acquire(status);
        tokio::pin!(acquire);
        tokio::pin!(stop_signals);
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
                reason = &mut stop_signals => return Ok(WaitEnd::Stop(reason)),
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
