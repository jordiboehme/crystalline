//! The daemon: `crystalline serve`. Owns the lock and socket, runs the file
//! watcher and the background embed queue, listens for `mcp` and `ctl`
//! connections and optionally serves the same tool router over streamable HTTP.

use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crystalline_core::config::{self, GlobalConfig, HttpSetting};
use crystalline_index::{HostClaim, Store};
use interprocess::local_socket::tokio::Stream as IpcStream;
use notify::{RecursiveMode, Watcher};
use serde_json::Value;
use tokio::sync::{Mutex as TokioMutex, watch};

use crate::control::serve_ctl;
use crate::engine::{Engine, WatchEvent};
use crate::instance::{acquire_ownership, read_mode_line};
use crate::mcp::McpServer;
use crate::overlay;

/// The default HTTP bind address when HTTP is enabled without an explicit one.
const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:7411";

/// Startup banner, shown on a foreground start when stderr is a terminal.
const BANNER: &str = r"
                                   ·              *
                                 ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄
                                ▐░░░▒▒▒▒▓▓▓█▓▓▓▒▒▒▒░░░▌
                                 ▀█░░░▒▒▒▓▓█▓▓▒▒▒░░░█▀   ·
                                   ▀█░░▒▒▒▓█▓▒▒▒░░█▀
                            *        ▀█░▒▒▓█▓▒▒░█▀
                                       ▀█▒▒█▒▒█▀
                                         ▀███▀     ·
                                           ▀

 ██████╗██████╗ ██╗   ██╗███████╗████████╗ █████╗ ██╗     ██╗     ██╗███╗   ██╗███████╗
██╔════╝██╔══██╗╚██╗ ██╔╝██╔════╝╚══██╔══╝██╔══██╗██║     ██║     ██║████╗  ██║██╔════╝
██║     ██████╔╝ ╚████╔╝ ███████╗   ██║   ███████║██║     ██║     ██║██╔██╗ ██║█████╗
██║     ██╔══██╗  ╚██╔╝  ╚════██║   ██║   ██╔══██║██║     ██║     ██║██║╚██╗██║██╔══╝
╚██████╗██║  ██║   ██║   ███████║   ██║   ██║  ██║███████╗███████╗██║██║ ╚████║███████╗
 ╚═════╝╚═╝  ╚═╝   ╚═╝   ╚══════╝   ╚═╝   ╚═╝  ╚═╝╚══════╝╚══════╝╚═╝╚═╝  ╚═══╝╚══════╝
";

/// A tracked live session.
#[derive(Clone, serde::Serialize)]
struct SessionInfo {
    id: u64,
    kind: String,
    since: String,
}

/// Daemon-wide shared state, reachable from every session and the ctl handler.
pub struct Shared {
    /// The shared engine.
    pub engine: Arc<Engine>,
    /// The owning pid.
    pub pid: u32,
    /// The resolved HTTP address, if any.
    pub http_addr: Option<String>,
    started: Instant,
    sessions: std::sync::Mutex<HashMap<u64, SessionInfo>>,
    next_session: AtomicU64,
    http_sessions: Arc<AtomicUsize>,
    shutdown_tx: watch::Sender<bool>,
}

impl Shared {
    /// Seconds since the daemon started.
    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// The number of live socket sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    /// The cumulative number of HTTP sessions the daemon has served.
    pub fn http_session_count(&self) -> usize {
        self.http_sessions.load(Ordering::Relaxed)
    }

    /// The live sessions as JSON, newest ids first.
    pub fn sessions_json(&self) -> Value {
        let mut list: Vec<SessionInfo> = self.sessions.lock().unwrap().values().cloned().collect();
        list.sort_by_key(|s| std::cmp::Reverse(s.id));
        serde_json::to_value(list).unwrap_or(Value::Null)
    }

    fn begin_session(&self, kind: &str) -> u64 {
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        self.sessions.lock().unwrap().insert(
            id,
            SessionInfo {
                id,
                kind: kind.to_string(),
                since: chrono::Utc::now().to_rfc3339(),
            },
        );
        id
    }

    fn end_session(&self, id: u64) {
        self.sessions.lock().unwrap().remove(&id);
    }

    /// Signal shutdown to every watcher.
    pub fn trigger_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    fn watch(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    /// Resolve once shutdown has been signalled.
    pub async fn wait_shutdown(&self) {
        let mut rx = self.shutdown_tx.subscribe();
        loop {
            if *rx.borrow() {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

/// Run the daemon: `crystalline serve [--daemon] [--http <addr>] [--read-only]
/// [--take-over]`. The effective read-only mode is the explicit flag or
/// `service.read_only`; `take_over` forces host-lock claims for a deliberate host
/// migration in a shared database.
pub async fn run_serve(
    daemon_flag: bool,
    http_flag: Option<String>,
    allowed_host_flag: Vec<String>,
    db: Option<PathBuf>,
    config_path: Option<PathBuf>,
    read_only: bool,
    take_over: bool,
) -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::INFO)
        .try_init();

    // The single load chokepoint: parse the environment overlay, resolve the
    // config path (flag, then CRYSTALLINE_CONFIG, then the default) and layer
    // the overlay over the file. A bad known variable aborts startup here with
    // a message naming it.
    let loaded = overlay::load(config_path.as_deref())?;
    let read_only = read_only || loaded.effective.read_only();
    let db_path = resolve_db(db.as_deref())?;
    let http_addr = resolve_http(http_flag.as_deref(), &loaded.effective);
    let allowed_hosts = resolve_allowed_hosts(&allowed_host_flag, &loaded.effective);

    // An env-defined domain that shadows a config file entry is worth one
    // startup warning (not one per `apply`, which runs constantly): the file
    // entry is silently overridden while the variable is set.
    for name in loaded.overlay.shadowed_domains(&loaded.file) {
        if let Some(env) = loaded.overlay.env_domain(name) {
            tracing::warn!(
                "domain '{name}' from {} shadows the config file entry of the same name",
                env.var
            );
        }
    }
    // Create any env-defined domain root that does not exist yet, before the
    // watcher computes its roots below: notify refuses to watch a missing
    // directory, so pre-creating the (possibly empty) root lets the watch arm
    // and preserves the watch-before-scan invariant with no watcher changes.
    // An env-origin domain is then bootstrapped into this root by the startup
    // task, its writes caught by the already-armed watch.
    for (name, env_domain) in loaded.overlay.env_domains() {
        if let Some(root) = env_domain.entry.file_path()
            && !root.exists()
            && let Err(e) = std::fs::create_dir_all(&root)
        {
            tracing::warn!(
                "could not create env-defined domain '{name}' root {}: {e}",
                root.display()
            );
        }
    }
    // This machine and state-directory's stable identity, generated on first use.
    // It turns on shared-database collaboration: the daemon claims a host lock per
    // file domain, renews it on a timer and releases it on shutdown.
    let instance_id = config::read_or_create_instance_id()?;

    // Take ownership first so a second daemon fails fast with the live pid.
    let ownership = acquire_ownership()?;

    let store = open_store(&loaded.effective, Some(&db_path)).await?;
    // A channel the engine uses to tell the watcher (spawned below) about a
    // domain registered after this daemon started, so it starts watching that
    // root without a restart. See `Engine::domain_root`'s fresh-config fallback.
    let (watch_tx, watch_rx) = tokio::sync::mpsc::unbounded_channel::<WatchEvent>();
    // A channel the embed worker (spawned below) listens on: every engine
    // operation that touches embeddings schedules its pass here instead of
    // running it inline, so the triggering request returns without waiting
    // on the model.
    let (embed_tx, embed_rx) = tokio::sync::mpsc::unbounded_channel();
    // The provider is built in the background (see below); text search and the
    // socket never wait on the model download. The engine holds the file config
    // and the overlay separately (persist and refresh hit the resolved file even
    // when it came from CRYSTALLINE_CONFIG); its effective config drives reads.
    let engine = Arc::new(
        Engine::new(store, loaded.file.clone(), None, Some(loaded.path.clone()))
            .with_watch_channel(watch_tx)
            .with_embed_channel(embed_tx)
            .with_read_only(read_only)
            .with_instance_id(instance_id)
            .with_env_overlay(loaded.overlay.clone()),
    );
    tokio::spawn(crate::engine::run_embed_worker(engine.clone(), embed_rx));

    // Prime the routing cache once as the HTTP baseline: every HTTP session
    // shares this engine and reads its cache at initialize, and each socket
    // connection refreshes it again in `handle_conn` before serving.
    engine.refresh_routing_cache().await;

    // Bind the socket and publish the lock record: this is the readiness point,
    // reached before the provider build and the initial sync so clients attach
    // fast.
    let listener = ownership.bind_listener()?;
    ownership.publish()?;

    let (shutdown_tx, _shutdown_rx) = watch::channel(false);
    let http_sessions = Arc::new(AtomicUsize::new(0));
    let shared = Arc::new(Shared {
        engine: engine.clone(),
        pid: std::process::id(),
        http_addr: http_addr.clone(),
        started: Instant::now(),
        sessions: std::sync::Mutex::new(HashMap::new()),
        next_session: AtomicU64::new(1),
        http_sessions: http_sessions.clone(),
        shutdown_tx,
    });

    if !daemon_flag {
        if std::io::stderr().is_terminal() {
            eprintln!("{BANNER}");
        }
        eprintln!(
            "crystalline {} serving on {} (pid {})",
            env!("CARGO_PKG_VERSION"),
            ownership.socket_display(),
            shared.pid
        );
        if let Some(addr) = &http_addr {
            eprintln!("crystalline HTTP endpoint on http://{addr}");
        }
        if read_only {
            eprintln!("crystalline serving read-only: content-mutating tools are disabled");
        }
        if loaded.effective.domains.is_empty() {
            eprintln!(
                "no domains registered yet - agents can create one with add_domain, or run: crystalline domain add <name> <path>"
            );
        }
    }

    // The watcher arms every startup domain's watch and only then fires this
    // one-shot; the initial sync below waits on it so its first scan never
    // begins before the watches are live. That watch-first-then-scan order
    // closes the startup twin of the dynamic-watch race: an external write into
    // a startup domain landing between the scan and the watch would otherwise be
    // lost, because inotify reports only events registered after `watch()`. See
    // the ordering invariant on `run_watcher`.
    let (watches_ready_tx, watches_ready_rx) = tokio::sync::oneshot::channel::<()>();

    // Initial sync, provider build and embed run in the background so readiness
    // is immediate. Sync needs no provider; the embed pass waits for it.
    {
        let e = engine.clone();
        let cfg = loaded.effective.clone();
        tokio::spawn(async move {
            // Hold the first scan until the watcher has armed the startup
            // watches; a dropped sender (watcher failed to start) lets it run.
            let _ = watches_ready_rx.await;
            // The startup sync claims each file domain's host lock (take_over
            // forces a migration); a domain held by another live instance is
            // skipped and served read-from-database.
            if let Err(err) = e.sync_take_over(None, take_over).await {
                tracing::warn!("initial sync failed: {err}");
            }
            // Bootstrap env-defined team domains that have no local state
            // yet: the zero-config read-only node's first contact with GitHub.
            // Runs before the embedding provider is built so it is not gated on
            // a model download; the embed pass just below covers the engrams it
            // writes. A missing connection is not fatal (the poller retries).
            e.bootstrap_env_origins().await;
            if let Some(provider) = crate::engine::build_provider(&cfg).await {
                e.set_provider(provider);
                match e.embed_pending().await {
                    Ok(n) if n > 0 => tracing::info!("embedded {n} chunks on startup"),
                    Ok(_) => {}
                    Err(err) => tracing::warn!("initial embed failed: {err}"),
                }
            }
        });
    }

    // The file watcher.
    {
        let e = engine.clone();
        let watch_domains = domain_roots(&loaded.effective);
        let rx = shared.watch();
        tokio::spawn(async move {
            if let Err(err) =
                run_watcher(e, watch_domains, take_over, rx, watch_rx, watches_ready_tx).await
            {
                tracing::warn!("watcher stopped: {err}");
            }
        });
    }

    // The host-lock heartbeat timer: renews every lock this instance holds so
    // another instance does not take over a live host. A no-op when this instance
    // hosts nothing (uncontended single-instance deployments).
    {
        let e = engine.clone();
        let rx = shared.watch();
        let secs = engine.heartbeat_secs();
        tokio::spawn(async move {
            run_heartbeat(e, secs, rx).await;
        });
    }

    // The background origin poller: brings origin-connected domains up to
    // date on its own schedule, with no user call, by running one scheduling
    // pass (`Engine::origin_poll_tick`) on a short heartbeat. A no-op tick
    // when collaboration is off, unconnected or paused for a rate limit; see
    // that method for the full gating and backoff rules. Runs even on a
    // read-only instance: a pull is a derived-truth update, not a user
    // content write.
    {
        let e = engine.clone();
        let rx = shared.watch();
        tokio::spawn(async move {
            run_origin_poller(e, rx).await;
        });
    }

    // The embed self-heal tick: re-fires the event-driven embed worker on a low
    // cadence whenever a backlog is left outstanding, so a transient provider
    // failure that consumed the worker's signal does not strand the backlog
    // until the next write. Never embeds inline (see run_embed_tick).
    {
        let e = engine.clone();
        let rx = shared.watch();
        tokio::spawn(async move {
            run_embed_tick(e, EMBED_TICK, rx).await;
        });
    }

    // The optional HTTP endpoint.
    if let Some(addr) = http_addr.clone() {
        let e = engine.clone();
        let sessions = http_sessions.clone();
        let rx = shared.watch();
        tokio::spawn(async move {
            if let Err(err) = run_http(addr, allowed_hosts, e, sessions, rx).await {
                tracing::warn!("HTTP endpoint stopped: {err}");
            }
        });
    }

    let accept = tokio::spawn(accept_loop(listener, shared.clone()));

    tokio::select! {
        _ = wait_signal() => {}
        _ = shared.wait_shutdown() => {}
    }
    shared.trigger_shutdown();
    let _ = accept.await;

    // Release every host lock this instance holds so a successor daemon acquires
    // immediately instead of waiting out the stale threshold. A no-op when this
    // instance hosts nothing.
    engine.release_hosts().await;

    // Best-effort WAL checkpoint so a stopped daemon's state dir holds a clean
    // single-file db, backup and copy friendly. Never blocks or fails
    // shutdown: checkpoint_wal logs and swallows any error itself.
    engine.checkpoint_wal().await;

    // Dropping ownership releases the lock and removes the socket and lock files.
    drop(ownership);
    if !daemon_flag {
        eprintln!("crystalline stopped");
    }
    Ok(())
}

/// Accept connections until shutdown, dispatching each by its handshake line.
async fn accept_loop(listener: interprocess::local_socket::tokio::Listener, shared: Arc<Shared>) {
    use interprocess::local_socket::traits::tokio::Listener as _;
    let mut shutdown = shared.watch();
    loop {
        tokio::select! {
            _ = wait_true(&mut shutdown) => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok(stream) => {
                        let shared = shared.clone();
                        tokio::spawn(async move { handle_conn(stream, shared).await; });
                    }
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                }
            }
        }
    }
}

/// Dispatch one accepted connection by its `mcp` or `ctl` handshake.
async fn handle_conn(mut stream: IpcStream, shared: Arc<Shared>) {
    let mode = match read_mode_line(&mut stream).await {
        Ok(m) => m,
        Err(_) => return,
    };
    match mode.as_str() {
        "mcp" => {
            let id = shared.begin_session("mcp");
            // Refresh the routing cache before this connection initializes so its
            // instructions reflect the latest virtual MANIFESTs, including edits
            // made by other instances sharing the database.
            shared.engine.refresh_routing_cache().await;
            let server = McpServer::new(shared.engine.clone());
            match rmcp::serve_server(server, stream).await {
                Ok(running) => {
                    let _ = running.waiting().await;
                }
                Err(err) => tracing::debug!("mcp session ended during init: {err}"),
            }
            shared.end_session(id);
        }
        "ctl" => {
            // ctl connections are transient operator requests, not agent
            // sessions, so they are deliberately not counted in `sessions`.
            serve_ctl(stream, shared.clone()).await;
        }
        other => {
            tracing::debug!("unknown handshake '{other}'");
        }
    }
}

/// One item on the watcher's debounce channel: a concrete filesystem path from
/// a notify event, or a rescan/overflow signal meaning events were dropped and
/// every watched domain must fall back to a full rescan this flush.
enum WatchTick {
    Path(PathBuf),
    Rescan,
}

/// Watch every domain root, debounce bursts by ~300ms and sync the touched
/// domains, then schedule an embedding pass on the worker (falling back to an
/// inline pass only if the worker channel is unwired or its receiver already
/// dropped, which in practice only a shutdown race produces). The store's
/// on-disk stamp already matches
/// any file a mutating tool just wrote, so those files are classified
/// unchanged here.
///
/// A debounce flush accumulates dirty relative PATHS per domain (see
/// [`DirtyPaths`]), not just dirty domain names: a small batch runs a
/// path-targeted [`Engine::sync_paths`] over exactly those paths, so a one-file
/// edit in a large domain no longer walks every entry. A batch that cannot be
/// reduced to clean markdown paths - a rescan/overflow notice, a directory
/// event, an ambiguous deletion or more than [`MAX_DIRTY_PATHS`] paths -
/// escalates that domain to today's full [`Engine::sync`] rescan. That full
/// fallback, plus the startup sync and manual `crystalline sync`, cover every
/// watcher gap, so the targeted path only has to be convergent, never perfect.
///
/// `new_roots` carries domains discovered after startup (see
/// `Engine::domain_root`'s fresh-config fallback): a `domain add` while this
/// daemon is running adds a watch here without a restart, and a `domain
/// remove` drops one the same way.
///
/// Ordering invariant: a watch is always armed before the matching sync scans
/// the same root, never after. On Linux inotify reports only events registered
/// after `watch()`, so a file written into a root between its scan and its
/// watch would be lost forever. The startup roots satisfy this by arming every
/// watch below and only then firing `watches_ready` to release the initial sync
/// in `run_serve`; a root added later satisfies it by arming its watch and then
/// running one catch-up sync in the `WatchEvent::Add` arm. Sync is checksum
/// idempotent so a file caught by both the catch-up and an inotify event is
/// never processed twice.
async fn run_watcher(
    engine: Arc<Engine>,
    domains: Vec<(String, PathBuf)>,
    take_over: bool,
    mut shutdown: watch::Receiver<bool>,
    mut new_roots: tokio::sync::mpsc::UnboundedReceiver<WatchEvent>,
    watches_ready: tokio::sync::oneshot::Sender<()>,
) -> anyhow::Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WatchTick>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            // A rescan/overflow notice (an inotify IN_Q_OVERFLOW, an fsevents
            // rescan) means events were dropped, so which paths changed is
            // unknown: signal a full rescan rather than trust a partial stream.
            if event.need_rescan() {
                let _ = tx.send(WatchTick::Rescan);
            }
            for path in event.paths {
                let _ = tx.send(WatchTick::Path(path));
            }
        }
    })?;
    // Claim the host lock for each startup file domain before arming its watch:
    // an acquired domain is watched (and synced) here, a domain held by another
    // live instance is skipped and served read-from-database only. Claiming
    // before arming keeps the watch-before-scan ordering the invariant needs.
    let startup = domains;
    let mut domains: Vec<(String, PathBuf)> = Vec::new();
    for (name, root) in startup {
        match engine.claim_host(&name, take_over).await {
            Ok(HostClaim::Acquired) => {
                if let Err(err) = watcher.watch(&root, RecursiveMode::Recursive) {
                    tracing::warn!(
                        "could not watch domain '{name}' at {}: {err}",
                        root.display()
                    );
                }
                domains.push((name, root));
            }
            Ok(HostClaim::HeldByOther(host)) => {
                tracing::info!(
                    "domain '{name}' is hosted by instance {} (last heartbeat {}); not watching, serving read-from-database only",
                    host.instance_id,
                    host.heartbeat_at
                );
            }
            Err(err) => {
                tracing::warn!("could not claim host for domain '{name}': {err}; not watching");
            }
        }
    }
    // Every startup watch is now armed, so the initial sync in `run_serve` may
    // safely scan (see the ordering invariant above). A dropped receiver just
    // means the sync runs unblocked.
    let _ = watches_ready.send(());

    loop {
        tokio::select! {
            _ = wait_true(&mut shutdown) => break,
            event = new_roots.recv() => {
                match event {
                    Some(WatchEvent::Add(name, root)) => {
                        if !domains.iter().any(|(n, _)| *n == name) {
                            // Claim the host lock before arming, as at startup: a
                            // domain another live instance hosts is not watched
                            // here and is served read-from-database only.
                            match engine.claim_host(&name, false).await {
                                Ok(HostClaim::Acquired) => {
                                    let armed = match watcher.watch(&root, RecursiveMode::Recursive) {
                                        Ok(()) => true,
                                        Err(err) => {
                                            tracing::warn!("could not watch new domain '{name}' at {}: {err}", root.display());
                                            false
                                        }
                                    };
                                    domains.push((name.clone(), root));
                                    // Catch-up sync now that the watch is armed:
                                    // it closes the window between the ctl sync
                                    // that discovered this domain and the watch
                                    // going live, during which an external write
                                    // would be invisible to inotify. Reuses the
                                    // engine sync path the debounced arm below
                                    // uses, so lock discipline and idempotency are
                                    // unchanged.
                                    if armed {
                                        if let Err(err) = engine.sync(Some(name.as_str())).await {
                                            tracing::warn!("catch-up sync of new domain '{name}' failed: {err}");
                                        }
                                        if !engine.request_embed()
                                            && let Err(err) = engine.embed_pending().await
                                        {
                                            tracing::warn!("catch-up embed of new domain '{name}' failed: {err}");
                                        }
                                    }
                                }
                                Ok(HostClaim::HeldByOther(host)) => {
                                    tracing::info!(
                                        "new domain '{name}' is hosted by instance {}; not watching, serving read-from-database only",
                                        host.instance_id
                                    );
                                }
                                Err(err) => {
                                    tracing::warn!("could not claim host for new domain '{name}': {err}; not watching");
                                }
                            }
                        }
                    }
                    Some(WatchEvent::Remove(name)) => {
                        if let Some(pos) = domains.iter().position(|(n, _)| *n == name) {
                            let (_, root) = domains.remove(pos);
                            let _ = watcher.unwatch(&root);
                        }
                    }
                    // The engine that owns the sending half is gone, which only
                    // happens alongside the daemon itself going away.
                    None => break,
                }
            }
            first = rx.recv() => {
                let Some(first) = first else { break };
                // Accumulate dirty relative paths per domain over the debounce
                // window, escalating a domain to a full rescan whenever a batch
                // cannot be reduced to clean markdown paths (see DirtyPaths).
                let mut dirty: HashMap<String, DirtyPaths> = HashMap::new();
                accumulate_tick(&mut dirty, first, &domains);
                while let Ok(Some(tick)) =
                    tokio::time::timeout(Duration::from_millis(300), rx.recv()).await
                {
                    accumulate_tick(&mut dirty, tick, &domains);
                }
                let touched = !dirty.is_empty();
                for (name, work) in dirty {
                    // full: today's walk-based rescan; otherwise a targeted pass
                    // over just the dirty paths. The full fallback plus the
                    // startup and manual syncs cover any gap, so the targeted
                    // pass only has to be convergent, never perfect.
                    if work.full {
                        if let Err(err) = engine.sync(Some(&name)).await {
                            tracing::warn!("watch sync of '{name}' failed: {err}");
                        }
                    } else if !work.paths.is_empty() {
                        let paths: Vec<String> = work.paths.into_iter().collect();
                        if let Err(err) = engine.sync_paths(&name, paths).await {
                            tracing::warn!("targeted watch sync of '{name}' failed: {err}");
                        }
                    }
                }
                if touched
                    && !engine.request_embed()
                    && let Err(err) = engine.embed_pending().await
                {
                    tracing::warn!("watch embed failed: {err}");
                }
            }
        }
    }
    Ok(())
}

/// Serve the tool router over streamable HTTP until shutdown. `allowed_hosts`
/// carries the resolved `Host` header allow-list on top of loopback (a single
/// `*` disables the guard); see [`http_config`].
async fn run_http(
    addr: String,
    allowed_hosts: Vec<String>,
    engine: Arc<Engine>,
    http_sessions: Arc<AtomicUsize>,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::tower::StreamableHttpService;

    let session_manager = Arc::new(LocalSessionManager::default());
    let factory_engine = engine.clone();
    let service = StreamableHttpService::new(
        move || {
            http_sessions.fetch_add(1, Ordering::Relaxed);
            Ok(McpServer::new(factory_engine.clone()))
        },
        session_manager,
        http_config(&allowed_hosts),
    );
    let router = axum::Router::new()
        .route("/health", axum::routing::get(health))
        .fallback_service(service);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(async move { wait_true(&mut shutdown).await })
        .await?;
    Ok(())
}

/// Liveness probe for load balancers and uptime monitors: a static payload
/// with no engine or database work, so a probe can never queue behind
/// indexing and never needs an MCP handshake.
async fn health() -> axum::Json<Value> {
    axum::Json(serde_json::json!({
        "status": "ok",
        "version": crystalline_core::VERSION,
    }))
}

/// Renew this instance's host locks every `secs` seconds until shutdown, so a
/// live host is never mistaken for stale. The first tick is consumed so renewal
/// starts one interval in, after the startup claims have settled.
async fn run_heartbeat(engine: Arc<Engine>, secs: u64, mut shutdown: watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(secs.max(1)));
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = wait_true(&mut shutdown) => break,
            _ = ticker.tick() => engine.renew_hosts().await,
        }
    }
}

/// How often the origin poller wakes up to ask the engine which domains are
/// due. This is only the scheduler's own heartbeat, not the poll interval
/// any domain actually keeps: `Engine::origin_poll_tick` tracks each
/// domain's own due instant (its `poll_secs`, else `github.poll_secs`, else
/// 300 seconds, floored at 60 with jitter) and this loop just checks on it
/// often enough that a newly due domain is never kept waiting long.
const POLLER_HEARTBEAT: Duration = Duration::from_secs(5);

/// Run the background origin poller until shutdown: every
/// [`POLLER_HEARTBEAT`], ask the engine to run one scheduling pass. All the
/// actual gating (collaboration on, connected, not rate-limited), due/not-due
/// bookkeeping, jitter and per-domain pulling live in
/// [`Engine::origin_poll_tick`], which this loop never reimplements; it only
/// wakes it on a modest cadence and exits promptly on shutdown.
async fn run_origin_poller(engine: Arc<Engine>, mut shutdown: watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(POLLER_HEARTBEAT);
    loop {
        tokio::select! {
            _ = wait_true(&mut shutdown) => break,
            _ = ticker.tick() => {
                engine.origin_poll_tick(Instant::now(), chrono::Utc::now()).await;
            }
        }
    }
}

/// How often the embed self-heal tick wakes to re-fire the worker when a
/// backlog remains. Low by design: the worker is event-driven and this only
/// closes the gap a transient provider failure opens, so it never needs to run
/// often.
const EMBED_TICK: Duration = Duration::from_secs(300);

/// Re-fire the event-driven embed worker whenever a backlog is left outstanding,
/// until shutdown. The worker only runs when a write signals it; a transient
/// provider failure consumes that signal and would strand the backlog until the
/// next write. Every `cadence` this checks the engine's backlog probe and, only
/// when it is non-empty, sends one more signal on the worker channel. It never
/// falls back to an inline embed when no worker is wired (an inline pass on this
/// timer would reintroduce the request-path stall the worker exists to
/// prevent), so an unwired tick is a silent no-op. The cadence is a parameter so
/// a test can drive it fast; production passes [`EMBED_TICK`]. The first tick is
/// consumed so a self-heal never races the startup embed.
pub async fn run_embed_tick(
    engine: Arc<Engine>,
    cadence: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(cadence);
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = wait_true(&mut shutdown) => break,
            _ = ticker.tick() => match engine.embedding_backlog().await {
                Ok(0) => {}
                Ok(_) => {
                    engine.request_embed();
                }
                Err(err) => tracing::warn!("embed self-heal backlog probe failed: {err}"),
            },
        }
    }
}

// --- shutdown + watcher helpers ---------------------------------------------

async fn wait_true(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

async fn wait_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => return futures::future::pending().await,
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(_) => return futures::future::pending().await,
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The registered file domains as `(name, canonical root)` pairs for the
/// watcher. Virtual domains have no filesystem root, so they are never watched.
fn domain_roots(config: &GlobalConfig) -> Vec<(String, PathBuf)> {
    config
        .domains
        .iter()
        .filter_map(|(name, entry)| {
            let root = entry.file_path().filter(|_| !entry.is_virtual())?;
            let canonical = std::fs::canonicalize(&root).unwrap_or(root);
            Some((name.clone(), canonical))
        })
        .collect()
}

/// Cap on targeted dirty paths per domain per debounce flush before the batch
/// escalates to a full rescan: past this many distinct paths a full walk is the
/// cheaper way to reconcile, and the cap also bounds the memory one burst holds.
const MAX_DIRTY_PATHS: usize = 256;

/// One domain's pending watcher work for a single debounce flush: a set of dirty
/// relative markdown paths, or `full` when the batch must fall back to a full
/// rescan.
///
/// `full` is the safety valve. The startup sync, a manual `crystalline sync` and
/// this full fallback all cover any watcher gap, so targeted mode only has to be
/// convergent, never perfect: anything the watcher cannot reduce to a clean
/// markdown path escalates here rather than risk a missed or wrong targeted
/// change. Once `full` is set the individual paths no longer matter, so they are
/// dropped to keep the set bounded.
#[derive(Debug, Default)]
struct DirtyPaths {
    paths: HashSet<String>,
    full: bool,
}

impl DirtyPaths {
    /// Escalate this domain to a full rescan for this flush.
    fn mark_full(&mut self) {
        self.full = true;
        self.paths.clear();
    }

    /// Add one dirty relative path, escalating to a full rescan once the batch
    /// grows past the cap.
    fn add(&mut self, rel: String) {
        if self.full {
            return;
        }
        self.paths.insert(rel);
        if self.paths.len() > MAX_DIRTY_PATHS {
            self.mark_full();
        }
    }
}

/// How one watcher event maps onto a domain: a clean relative markdown path to
/// target, a signal to escalate the whole domain to a full rescan, or nothing.
enum DirtyKind {
    /// A clean domain-relative markdown path to sync in the targeted pass.
    Path(String),
    /// Fall back to a full rescan of the domain (a directory event, an
    /// unresolvable relative path or an ambiguous deletion).
    Escalate,
    /// Not index-relevant (a hidden path or a live non-markdown file); ignore.
    Ignore,
}

/// Fold one debounce tick into the per-domain dirty map.
fn accumulate_tick(
    dirty: &mut HashMap<String, DirtyPaths>,
    tick: WatchTick,
    domains: &[(String, PathBuf)],
) {
    match tick {
        // A dropped-event / overflow signal: which paths were lost is unknown,
        // so reconcile every watched domain with a full rescan.
        WatchTick::Rescan => {
            for (name, _) in domains {
                dirty.entry(name.clone()).or_default().mark_full();
            }
        }
        WatchTick::Path(path) => {
            for (name, kind) in classify_event(&path, domains) {
                let entry = dirty.entry(name).or_default();
                match kind {
                    DirtyKind::Path(rel) => entry.add(rel),
                    DirtyKind::Escalate => entry.mark_full(),
                    DirtyKind::Ignore => {}
                }
            }
        }
    }
}

/// Classify one event path against the watched domains, then reduce the event to
/// a [`DirtyKind`] per matched domain. A path under no root yields nothing.
///
/// `domains` roots are already canonical (see [`domain_roots`]), so a raw prefix
/// match against the event path is tried first and needs no syscall. Only when
/// that finds nothing is the event path itself canonicalized and retried, which
/// still matches a root reached through a symlink.
fn classify_event(path: &Path, domains: &[(String, PathBuf)]) -> Vec<(String, DirtyKind)> {
    // Raw prefix match first (no syscall).
    let raw: Vec<(String, PathBuf)> = domains
        .iter()
        .filter(|(_, root)| path.starts_with(root))
        .map(|(name, root)| (name.clone(), root.clone()))
        .collect();
    let canonical;
    let (match_path, hits): (&Path, Vec<(String, PathBuf)>) = if !raw.is_empty() {
        (path, raw)
    } else {
        // Only when the raw check finds nothing is the event path canonicalized
        // and retried, which still matches a root reached through a symlink.
        canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let hits = domains
            .iter()
            .filter(|(_, root)| canonical.starts_with(root))
            .map(|(name, root)| (name.clone(), root.clone()))
            .collect();
        (canonical.as_path(), hits)
    };
    hits.into_iter()
        .map(|(name, root)| (name, classify_in_root(match_path, &root)))
        .collect()
}

/// Reduce one event path, already known to sit under `root`, to a [`DirtyKind`].
fn classify_in_root(path: &Path, root: &Path) -> DirtyKind {
    // A directory event (a create, a rename, a bulk delete) can hide markdown
    // children notify never reports individually, so fall back to a full rescan
    // rather than guess at the children.
    if path.is_dir() {
        return DirtyKind::Escalate;
    }
    let Some(rel) = clean_rel(root, path) else {
        // Not cleanly under this root (a `..` component or a non-UTF8 name once
        // the prefix matched): the safe side is a full rescan.
        return DirtyKind::Escalate;
    };
    // Hidden paths (dotfiles, anything under a dot-directory) are pruned by the
    // scan walk, so they never map to an engram: ignore them, matching the walk.
    if rel.split('/').any(is_hidden) {
        return DirtyKind::Ignore;
    }
    let is_md = rel.to_lowercase().ends_with(".md");
    if path.exists() {
        // An existing non-markdown regular file is never indexed (directories
        // were escalated above), so ignore it; a markdown file is a targeted
        // candidate, resolved against the stamps by scan_paths.
        if is_md {
            DirtyKind::Path(rel)
        } else {
            DirtyKind::Ignore
        }
    } else if is_md {
        // A vanished markdown file is a targeted delete candidate.
        DirtyKind::Path(rel)
    } else {
        // A vanished non-markdown path is ambiguous - it may have been a
        // directory whose markdown children notify never reported - so escalate
        // to a full rescan, the convergent safe side.
        DirtyKind::Escalate
    }
}

/// The domain-relative path of `path` under `root`, joined with `/` to match the
/// stamp keys, or `None` when the path is the root itself or holds any non-normal
/// component (`..`, a prefix), which must escalate instead.
fn clean_rel(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for comp in rel.components() {
        match comp {
            std::path::Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            _ => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Whether a single path component is hidden (a dotfile or dot-directory),
/// mirroring the scan walk's own pruning.
fn is_hidden(name: &str) -> bool {
    name.starts_with('.') && name != "." && name != ".."
}

// --- config + path resolution -----------------------------------------------

/// Resolve the HTTP bind address from the flag then the config.
fn resolve_http(flag: Option<&str>, config: &GlobalConfig) -> Option<String> {
    if let Some(f) = flag {
        let f = f.trim();
        return match f {
            "" | "true" | "on" => Some(DEFAULT_HTTP_ADDR.to_string()),
            "false" | "off" => None,
            addr => Some(addr.to_string()),
        };
    }
    match config.service.as_ref().and_then(|s| s.http.as_ref()) {
        Some(HttpSetting::Enabled(true)) => Some(DEFAULT_HTTP_ADDR.to_string()),
        Some(HttpSetting::Address(a)) => Some(a.clone()),
        _ => None,
    }
}

/// Resolve the HTTP `Host` allow-list from the flag then the config. The
/// repeatable `--allowed-host` flag wins over the comma-separated
/// `service.allowed_hosts` setting; both normalize the same way. An empty
/// result means loopback-only (the secure default).
fn resolve_allowed_hosts(flag: &[String], config: &GlobalConfig) -> Vec<String> {
    let split = |entries: &[String]| -> Vec<String> {
        entries
            .iter()
            .flat_map(|s| crate::settings::parse_allowed_hosts(s))
            .collect()
    };
    if !flag.is_empty() {
        return split(flag);
    }
    config
        .service
        .as_ref()
        .and_then(|s| s.allowed_hosts.as_deref())
        .map(split)
        .unwrap_or_default()
}

/// Build the streamable-HTTP config, applying the DNS-rebinding `Host` guard.
/// An empty list keeps rmcp's loopback-only default; a single `*` disables the
/// guard (any Host allowed); otherwise loopback is merged with the configured
/// hosts so local access never breaks.
fn http_config(
    allowed_hosts: &[String],
) -> rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig {
    use rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig;
    if allowed_hosts.iter().any(|h| h == "*") {
        return StreamableHttpServerConfig::default().disable_allowed_hosts();
    }
    if allowed_hosts.is_empty() {
        return StreamableHttpServerConfig::default();
    }
    let mut hosts = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    hosts.extend(allowed_hosts.iter().cloned());
    StreamableHttpServerConfig::default().with_allowed_hosts(hosts)
}

pub(crate) fn resolve_db(db: Option<&Path>) -> anyhow::Result<PathBuf> {
    match db {
        Some(p) => Ok(p.to_path_buf()),
        None => Ok(config::index_db_path()?),
    }
}

pub(crate) async fn open_store(
    cfg: &GlobalConfig,
    db: Option<&Path>,
) -> anyhow::Result<Arc<TokioMutex<dyn Store>>> {
    Ok(crystalline_index::open_store(&cfg.database(), db, false).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These pin down the exact `--http` semantics containers rely on: a
    // container must bind 0.0.0.0 (not the 127.0.0.1 default) to be reachable
    // from outside its network namespace, so `serve --http 0.0.0.0:7411` has
    // to pass the address through unchanged rather than only accepting the
    // bare toggle spellings.

    #[test]
    fn resolve_http_passes_through_an_explicit_non_loopback_address() {
        let config = GlobalConfig::default();
        assert_eq!(
            resolve_http(Some("0.0.0.0:7411"), &config),
            Some("0.0.0.0:7411".to_string())
        );
    }

    #[test]
    fn resolve_http_bare_toggle_still_defaults_to_loopback() {
        let config = GlobalConfig::default();
        assert_eq!(
            resolve_http(Some(""), &config),
            Some(DEFAULT_HTTP_ADDR.to_string())
        );
        assert_eq!(
            resolve_http(Some("true"), &config),
            Some(DEFAULT_HTTP_ADDR.to_string())
        );
    }

    #[test]
    fn resolve_http_off_disables_regardless_of_config() {
        let mut config = GlobalConfig::default();
        config.service = Some(crystalline_core::config::ServiceConfig {
            http: Some(HttpSetting::Address("0.0.0.0:7411".to_string())),
            ..Default::default()
        });
        assert_eq!(resolve_http(Some("off"), &config), None);
    }

    #[test]
    fn resolve_http_falls_back_to_config_address_without_a_flag() {
        let mut config = GlobalConfig::default();
        config.service = Some(crystalline_core::config::ServiceConfig {
            http: Some(HttpSetting::Address("0.0.0.0:7411".to_string())),
            ..Default::default()
        });
        assert_eq!(
            resolve_http(None, &config),
            Some("0.0.0.0:7411".to_string())
        );
    }

    #[test]
    fn resolve_http_none_without_flag_or_config() {
        let config = GlobalConfig::default();
        assert_eq!(resolve_http(None, &config), None);
    }

    fn config_with_allowed_hosts(hosts: Vec<String>) -> GlobalConfig {
        let mut config = GlobalConfig::default();
        config.service = Some(crystalline_core::config::ServiceConfig {
            allowed_hosts: Some(hosts),
            ..Default::default()
        });
        config
    }

    #[test]
    fn resolve_allowed_hosts_flag_wins_and_splits_commas() {
        let config = config_with_allowed_hosts(vec!["ignored.example".to_string()]);
        // A single flag value may itself carry a comma-separated list.
        let flag = vec!["muthur.lan, mcp.example.com".to_string()];
        assert_eq!(
            resolve_allowed_hosts(&flag, &config),
            vec!["muthur.lan".to_string(), "mcp.example.com".to_string()]
        );
    }

    #[test]
    fn resolve_allowed_hosts_falls_back_to_config() {
        let config = config_with_allowed_hosts(vec!["muthur.lan".to_string()]);
        assert_eq!(
            resolve_allowed_hosts(&[], &config),
            vec!["muthur.lan".to_string()]
        );
    }

    #[test]
    fn resolve_allowed_hosts_empty_without_flag_or_config() {
        let config = GlobalConfig::default();
        assert!(resolve_allowed_hosts(&[], &config).is_empty());
    }

    #[test]
    fn http_config_empty_keeps_loopback_only_default() {
        let cfg = http_config(&[]);
        assert_eq!(cfg.allowed_hosts, vec!["localhost", "127.0.0.1", "::1"]);
    }

    #[test]
    fn http_config_star_disables_the_guard() {
        let cfg = http_config(&["*".to_string()]);
        assert!(
            cfg.allowed_hosts.is_empty(),
            "an empty allow-list makes rmcp accept any Host"
        );
    }

    #[test]
    fn http_config_merges_loopback_with_configured_hosts() {
        let cfg = http_config(&["muthur.lan".to_string()]);
        assert_eq!(
            cfg.allowed_hosts,
            vec!["localhost", "127.0.0.1", "::1", "muthur.lan"]
        );
    }

    // classify_event: the raw prefix match must short-circuit before any
    // canonicalize syscall and the canonicalize fallback must still resolve an
    // event path reached through a symlinked root - the same matching the old
    // domains_for had - and each matched event reduces to the right DirtyKind.

    fn as_path(kind: &DirtyKind) -> Option<&str> {
        match kind {
            DirtyKind::Path(p) => Some(p.as_str()),
            _ => None,
        }
    }

    #[test]
    fn classify_event_matches_via_raw_prefix_without_canonicalizing() {
        // A path that does not exist on disk still matches through the raw
        // check, proving the fallback canonicalize call is never reached; a
        // vanished markdown path is a targeted delete candidate.
        let root = PathBuf::from("/some/canonical/root");
        let domains = vec![("domain".to_string(), root.clone())];
        let event_path = root.join("sub/does-not-exist.md");
        let hits = classify_event(&event_path, &domains);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "domain");
        assert_eq!(as_path(&hits[0].1), Some("sub/does-not-exist.md"));
    }

    #[test]
    fn classify_event_returns_empty_when_no_root_matches() {
        let root = PathBuf::from("/some/canonical/root");
        let domains = vec![("domain".to_string(), root)];
        let event_path = PathBuf::from("/completely/unrelated/path/file.md");
        assert!(classify_event(&event_path, &domains).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn classify_event_matches_a_symlinked_event_path_via_canonicalize_fallback() {
        let base = tempfile::tempdir().unwrap();
        let real_root = base.path().join("real");
        std::fs::create_dir(&real_root).unwrap();
        std::fs::write(real_root.join("note.md"), "body").unwrap();
        // domain_roots stores the already-canonicalized root, so mirror that
        // here rather than the raw pre-canonicalize path.
        let canonical_root = std::fs::canonicalize(&real_root).unwrap();

        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real_root, &link).unwrap();

        let domains = vec![("domain".to_string(), canonical_root)];
        // The event path travels through the symlink, so it does not textually
        // start with the canonical root: only the canonicalize fallback resolves
        // it, and the resolved markdown file is a targeted path.
        let event_path = link.join("note.md");

        let hits = classify_event(&event_path, &domains);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "domain");
        assert_eq!(as_path(&hits[0].1), Some("note.md"));
    }

    #[test]
    fn classify_event_escalates_on_a_directory() {
        // A directory event can hide markdown children notify never reports
        // individually, so it must fall back to a full rescan.
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let subdir = root.join("sub");
        std::fs::create_dir(&subdir).unwrap();
        let domains = vec![("domain".to_string(), root)];
        let hits = classify_event(&subdir, &domains);
        assert_eq!(hits.len(), 1);
        assert!(matches!(hits[0].1, DirtyKind::Escalate));
    }

    #[test]
    fn classify_event_ignores_a_live_non_markdown_file_and_hidden_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("scratch.txt"), "x").unwrap();
        std::fs::write(root.join(".hidden.md"), "x").unwrap();
        let domains = vec![("domain".to_string(), root.clone())];

        // A live non-markdown file is never indexed, so it is ignored.
        let txt = classify_event(&root.join("scratch.txt"), &domains);
        assert_eq!(txt.len(), 1);
        assert!(matches!(txt[0].1, DirtyKind::Ignore));

        // A hidden markdown file is pruned by the scan walk, so it is ignored.
        let hidden = classify_event(&root.join(".hidden.md"), &domains);
        assert_eq!(hidden.len(), 1);
        assert!(matches!(hidden[0].1, DirtyKind::Ignore));
    }

    #[test]
    fn classify_event_escalates_on_a_vanished_non_markdown_path() {
        // A vanished non-markdown path is ambiguous - it may have been a
        // directory whose markdown children notify never reported - so it
        // escalates to a full rescan, the convergent safe side.
        let root = PathBuf::from("/some/canonical/root");
        let domains = vec![("domain".to_string(), root.clone())];
        let hits = classify_event(&root.join("was-a-dir"), &domains);
        assert_eq!(hits.len(), 1);
        assert!(matches!(hits[0].1, DirtyKind::Escalate));
    }

    // DirtyPaths: distinct paths accumulate until the cap, an overflow escalates
    // to a full rescan and full is sticky (a later path never un-escalates it).

    #[test]
    fn dirty_paths_accumulates_distinct_paths_under_the_cap() {
        let mut d = DirtyPaths::default();
        d.add("a.md".to_string());
        d.add("b.md".to_string());
        d.add("a.md".to_string()); // a duplicate does not double-count
        assert!(!d.full);
        assert_eq!(d.paths.len(), 2);
    }

    #[test]
    fn dirty_paths_overflow_past_the_cap_escalates_to_full() {
        let mut d = DirtyPaths::default();
        for i in 0..=MAX_DIRTY_PATHS {
            d.add(format!("f{i}.md"));
        }
        assert!(
            d.full,
            "past {MAX_DIRTY_PATHS} distinct paths the batch is full"
        );
        assert!(d.paths.is_empty(), "the paths are dropped once full");
    }

    #[test]
    fn dirty_paths_full_is_sticky() {
        let mut d = DirtyPaths::default();
        d.mark_full();
        d.add("a.md".to_string());
        assert!(d.full, "a later path never un-escalates a full batch");
        assert!(d.paths.is_empty());
    }

    // accumulate_tick: a rescan/overflow signal marks every watched domain full;
    // a single markdown event targets exactly that path.

    #[test]
    fn accumulate_tick_rescan_marks_every_watched_domain_full() {
        let domains = vec![
            ("a".to_string(), PathBuf::from("/roots/a")),
            ("b".to_string(), PathBuf::from("/roots/b")),
        ];
        let mut dirty: HashMap<String, DirtyPaths> = HashMap::new();
        accumulate_tick(&mut dirty, WatchTick::Rescan, &domains);
        assert!(dirty["a"].full);
        assert!(dirty["b"].full);
    }

    #[test]
    fn accumulate_tick_a_single_markdown_event_targets_that_path() {
        let root = PathBuf::from("/roots/a");
        let domains = vec![("a".to_string(), root.clone())];
        let mut dirty: HashMap<String, DirtyPaths> = HashMap::new();
        // A vanished markdown path (delete candidate) is targeted, not full.
        accumulate_tick(&mut dirty, WatchTick::Path(root.join("note.md")), &domains);
        assert!(!dirty["a"].full);
        assert!(dirty["a"].paths.contains("note.md"));
    }
}
