//! The daemon: `crystalline serve`. Owns the lock and socket, runs the file
//! watcher and the background embed queue, listens for `mcp` and `ctl`
//! connections and optionally serves the same tool router over streamable HTTP.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crystalline_core::config::{self, GlobalConfig, HttpSetting};
use interprocess::local_socket::tokio::Stream as IpcStream;
use notify::{RecursiveMode, Watcher};
use serde_json::Value;
use tokio::sync::{Mutex as TokioMutex, watch};

use crate::control::serve_ctl;
use crate::engine::Engine;
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

/// Run the daemon: `crystalline serve [--daemon] [--http <addr>]`.
pub async fn run_serve(
    daemon_flag: bool,
    http_flag: Option<String>,
    db: Option<PathBuf>,
    config_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::INFO)
        .try_init();

    let config = load_config(config_path.as_deref())?;
    let db_path = resolve_db(db.as_deref())?;
    let http_addr = resolve_http(http_flag.as_deref(), &config);

    // Take ownership first so a second daemon fails fast with the live pid.
    let ownership = acquire_ownership()?;

    let store = open_store(&db_path).await?;
    // The provider is built in the background (see below); text search and the
    // socket never wait on the model download.
    let engine = Arc::new(Engine::new(
        Arc::new(TokioMutex::new(store)),
        config.clone(),
        None,
    ));

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
    }

    // Initial sync, provider build and embed run in the background so readiness
    // is immediate. Sync needs no provider; the embed pass waits for it.
    {
        let e = engine.clone();
        let cfg = config.clone();
        tokio::spawn(async move {
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
            if let Err(err) = run_watcher(e, watch_domains, rx).await {
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
async fn run_watcher(
    engine: Arc<Engine>,
    domains: Vec<(String, PathBuf)>,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            for path in event.paths {
                let _ = tx.send(path);
            }
        }
    })?;
    for (_, root) in &domains {
        let _ = watcher.watch(root, RecursiveMode::Recursive);
    }

    loop {
        tokio::select! {
            _ = wait_true(&mut shutdown) => break,
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

/// The registered domains as `(name, canonical root)` pairs for the watcher.
fn domain_roots(config: &GlobalConfig) -> Vec<(String, PathBuf)> {
    config
        .domains
        .iter()
        .map(|(name, entry)| {
            let root = config::expand_tilde(&entry.path.to_string_lossy());
            let canonical = std::fs::canonicalize(&root).unwrap_or(root);
            (name.clone(), canonical)
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

pub(crate) async fn open_store(db: &Path) -> anyhow::Result<crystalline_index::TursoStore> {
    if let Some(parent) = db.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(crystalline_index::TursoStore::open(db).await?)
}
