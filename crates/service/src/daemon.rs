//! The daemon: `crystalline serve`. Owns the lock and socket, runs the file
//! watcher and the background embed queue, listens for `mcp` and `ctl`
//! connections and optionally serves the same tool router over streamable HTTP.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crystalline_core::config::{self, GlobalConfig, HttpSetting};
use crystalline_index::Store;
use interprocess::local_socket::tokio::Stream as IpcStream;
use notify::{RecursiveMode, Watcher};
use serde_json::Value;
use tokio::sync::{Mutex as TokioMutex, watch};

use crate::control::serve_ctl;
use crate::engine::{Engine, WatchEvent};
use crate::instance::{acquire_ownership, read_mode_line};
use crate::mcp::McpServer;

/// The default HTTP bind address when HTTP is enabled without an explicit one.
const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:7411";

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

/// Run the daemon: `crystalline serve [--daemon] [--http <addr>] [--read-only]`.
/// The effective mode is the explicit flag or `service.read_only`.
pub async fn run_serve(
    daemon_flag: bool,
    http_flag: Option<String>,
    db: Option<PathBuf>,
    config_path: Option<PathBuf>,
    read_only: bool,
) -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::INFO)
        .try_init();

    let config = load_config(config_path.as_deref())?;
    let read_only = read_only || config.read_only();
    let db_path = resolve_db(db.as_deref())?;
    let http_addr = resolve_http(http_flag.as_deref(), &config);

    // Take ownership first so a second daemon fails fast with the live pid.
    let ownership = acquire_ownership()?;

    let store = open_store(&config, Some(&db_path)).await?;
    // A channel the engine uses to tell the watcher (spawned below) about a
    // domain registered after this daemon started, so it starts watching that
    // root without a restart. See `Engine::domain_root`'s fresh-config fallback.
    let (watch_tx, watch_rx) = tokio::sync::mpsc::unbounded_channel::<WatchEvent>();
    // The provider is built in the background (see below); text search and the
    // socket never wait on the model download.
    let engine = Arc::new(
        Engine::new(store, config.clone(), None, config_path.clone())
            .with_watch_channel(watch_tx)
            .with_read_only(read_only),
    );

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
        eprintln!(
            "crystalline serving on {} (pid {})",
            ownership.socket_display(),
            shared.pid
        );
        if let Some(addr) = &http_addr {
            eprintln!("crystalline HTTP endpoint on http://{addr}");
        }
        if read_only {
            eprintln!("crystalline serving read-only: content-mutating tools are disabled");
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
        let cfg = config.clone();
        tokio::spawn(async move {
            // Hold the first scan until the watcher has armed the startup
            // watches; a dropped sender (watcher failed to start) lets it run.
            let _ = watches_ready_rx.await;
            if let Err(err) = e.sync(None).await {
                tracing::warn!("initial sync failed: {err}");
            }
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
        let watch_domains = domain_roots(&config);
        let rx = shared.watch();
        tokio::spawn(async move {
            if let Err(err) = run_watcher(e, watch_domains, rx, watch_rx, watches_ready_tx).await {
                tracing::warn!("watcher stopped: {err}");
            }
        });
    }

    // The optional HTTP endpoint.
    if let Some(addr) = http_addr.clone() {
        let e = engine.clone();
        let sessions = http_sessions.clone();
        let rx = shared.watch();
        tokio::spawn(async move {
            if let Err(err) = run_http(addr, e, sessions, rx).await {
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

/// Watch every domain root, debounce bursts by ~300ms and sync the touched
/// domains, then embed. The store's on-disk stamp already matches any file a
/// mutating tool just wrote, so those files are classified unchanged here.
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
    mut shutdown: watch::Receiver<bool>,
    mut new_roots: tokio::sync::mpsc::UnboundedReceiver<WatchEvent>,
    watches_ready: tokio::sync::oneshot::Sender<()>,
) -> anyhow::Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            for path in event.paths {
                let _ = tx.send(path);
            }
        }
    })?;
    let mut domains = domains;
    for (_, root) in &domains {
        let _ = watcher.watch(root, RecursiveMode::Recursive);
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
                            let armed = match watcher.watch(&root, RecursiveMode::Recursive) {
                                Ok(()) => true,
                                Err(err) => {
                                    tracing::warn!("could not watch new domain '{name}' at {}: {err}", root.display());
                                    false
                                }
                            };
                            domains.push((name.clone(), root));
                            // Catch-up sync now that the watch is armed: it
                            // closes the window between the ctl sync that
                            // discovered this domain and the watch going live,
                            // during which an external write would be invisible
                            // to inotify. Reuses the engine sync path the
                            // debounced arm below uses, so lock discipline and
                            // idempotency are unchanged.
                            if armed {
                                if let Err(err) = engine.sync(Some(name.as_str())).await {
                                    tracing::warn!("catch-up sync of new domain '{name}' failed: {err}");
                                }
                                if let Err(err) = engine.embed_pending().await {
                                    tracing::warn!("catch-up embed of new domain '{name}' failed: {err}");
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
                let mut dirty: HashSet<String> =
                    domains_for(&first, &domains).into_iter().collect();
                while let Ok(Some(path)) =
                    tokio::time::timeout(Duration::from_millis(300), rx.recv()).await
                {
                    dirty.extend(domains_for(&path, &domains));
                }
                for name in &dirty {
                    if let Err(err) = engine.sync(Some(name)).await {
                        tracing::warn!("watch sync of '{name}' failed: {err}");
                    }
                }
                if !dirty.is_empty()
                    && let Err(err) = engine.embed_pending().await
                {
                    tracing::warn!("watch embed failed: {err}");
                }
            }
        }
    }
    Ok(())
}

/// Serve the tool router over streamable HTTP until shutdown.
async fn run_http(
    addr: String,
    engine: Arc<Engine>,
    http_sessions: Arc<AtomicUsize>,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::tower::{
        StreamableHttpServerConfig, StreamableHttpService,
    };

    let session_manager = Arc::new(LocalSessionManager::default());
    let factory_engine = engine.clone();
    let service = StreamableHttpService::new(
        move || {
            http_sessions.fetch_add(1, Ordering::Relaxed);
            Ok(McpServer::new(factory_engine.clone()))
        },
        session_manager,
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().fallback_service(service);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(async move { wait_true(&mut shutdown).await })
        .await?;
    Ok(())
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

/// The domain names whose root is a prefix of the event path.
fn domains_for(path: &Path, domains: &[(String, PathBuf)]) -> Vec<String> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    domains
        .iter()
        .filter(|(_, root)| canonical.starts_with(root) || path.starts_with(root))
        .map(|(name, _)| name.clone())
        .collect()
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

pub(crate) fn load_config(path: Option<&Path>) -> anyhow::Result<GlobalConfig> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => config::global_config_path()?,
    };
    if path.is_file() {
        Ok(config::load_yaml(&path)?)
    } else {
        Ok(GlobalConfig::default())
    }
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
}
