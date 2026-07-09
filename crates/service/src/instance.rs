//! Single-instance mechanics: the advisory lock and the local socket.
//!
//! Exactly one process owns the derived index. Ownership is an `fs4` advisory
//! exclusive lock held on `service.lock` for the owner's lifetime; the socket
//! (a Unix domain socket, or the named pipe `\\.\pipe\crystalline` on Windows)
//! is how everyone else reaches it. See `research/single-instance-ipc.md`.
//!
//! Attaching is version aware: the lock record carries the owner's version,
//! and a client built from a newer version displaces an older daemon with a
//! graceful ctl shutdown before taking over, so a binary upgrade needs no
//! manual daemon restart. The takeover is one-way on purpose - an older
//! client attaches to a newer daemon as-is - which keeps lingering
//! old-binary bridges from flip-flopping an upgraded daemon back.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs4::FileExt;
#[cfg(not(windows))]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::tokio::{Listener as IpcListener, Stream as IpcStream};
use interprocess::local_socket::{ListenerOptions, Name};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crystalline_core::config;

/// The lock file record, written after the socket is bound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    /// The owning process id.
    pub pid: u32,
    /// The socket path (unix) or pipe name (Windows).
    pub socket_path: String,
    /// The owner's crystalline version.
    pub version: String,
    /// RFC 3339 start time.
    pub started_at: String,
}

/// A connected client stream, before the handshake line is written.
pub struct Connection {
    stream: IpcStream,
}

impl Connection {
    /// Write the `mcp` handshake and hand back the stream for an rmcp session or
    /// a byte pump.
    pub async fn into_mcp(self) -> io::Result<IpcStream> {
        self.handshake(b"mcp\n").await
    }

    /// Write the `ctl` handshake and hand back the stream for the NDJSON control
    /// protocol.
    pub async fn into_ctl(self) -> io::Result<IpcStream> {
        self.handshake(b"ctl\n").await
    }

    async fn handshake(mut self, line: &[u8]) -> io::Result<IpcStream> {
        self.stream.write_all(line).await?;
        self.stream.flush().await?;
        Ok(self.stream)
    }
}

/// Ownership of the index: the held lock plus the paths it governs. Dropping it
/// releases the lock and removes the socket and lock files.
pub struct Ownership {
    lock_file: File,
    lock_path: PathBuf,
    socket_path: PathBuf,
}

impl Ownership {
    /// Bind the local socket, removing any stale socket file first.
    pub fn bind_listener(&self) -> io::Result<IpcListener> {
        // On unix a leftover socket file blocks binding; remove it.
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.socket_path);
        }
        let name = socket_name(&self.socket_path)?;
        ListenerOptions::new().name(name).create_tokio()
    }

    /// Publish the lock record now that the socket is bound.
    pub fn publish(&self) -> io::Result<()> {
        let info = LockInfo {
            pid: std::process::id(),
            socket_path: self.socket_display(),
            version: crystalline_core::VERSION.to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        let json = serde_json::to_string(&info).unwrap_or_default();
        std::fs::write(&self.lock_path, json.as_bytes())
    }

    /// The socket path (unix) or pipe name (Windows) as a display string.
    pub fn socket_display(&self) -> String {
        #[cfg(windows)]
        {
            PIPE_NAME.to_string()
        }
        #[cfg(not(windows))]
        {
            self.socket_path.display().to_string()
        }
    }
}

impl Drop for Ownership {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.lock_file);
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.socket_path);
        }
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// The Windows named pipe name.
#[cfg(windows)]
const PIPE_NAME: &str = r"\\.\pipe\crystalline";

/// Build the platform socket name: a filesystem path on unix, a namespaced pipe
/// on Windows.
fn socket_name(sock_path: &Path) -> io::Result<Name<'_>> {
    #[cfg(windows)]
    {
        let _ = sock_path;
        "crystalline".to_ns_name::<GenericNamespaced>()
    }
    #[cfg(not(windows))]
    {
        sock_path.as_os_str().to_fs_name::<GenericFilePath>()
    }
}

/// Read the current lock record, if any is present and parseable.
pub fn read_lock_info() -> Option<LockInfo> {
    let path = config::service_lock_path().ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Attach to a running daemon if one is reachable. Returns `None` when no live
/// daemon owns the index (no lock record, a dead pid or an unreachable socket),
/// which is the signal that ownership is takeable. A daemon older than this
/// binary is displaced first (graceful shutdown, then `None`), so the caller
/// proceeds exactly as if no daemon ran and the next spawn runs the new
/// version. A thin wrapper over [`try_attach_reporting`] for callers that do
/// not need the displacement flag.
pub async fn try_attach() -> Option<Connection> {
    try_attach_reporting().await.0
}

/// As [`try_attach`], additionally reporting whether this call itself
/// displaced an older daemon (the `Displace` arm ran and `displace` returned
/// true). `ensure_daemon`'s readiness poll needs this to tell "no daemon yet,
/// still starting" apart from "no daemon because this very poll iteration
/// just tore one down", which calls for a re-spawn rather than another wait.
pub async fn try_attach_reporting() -> (Option<Connection>, bool) {
    let Some(info) = read_lock_info() else {
        return (None, false);
    };
    if !process_alive(info.pid) {
        return (None, false);
    }
    if attach_policy(&info.version, crystalline_core::VERSION) == AttachPolicy::Displace {
        let Some(sock) = config::service_sock_path().ok() else {
            return (None, false);
        };
        tracing::info!(
            "displacing crystalline daemon v{} (pid {}) in favor of v{}",
            info.version,
            info.pid,
            crystalline_core::VERSION
        );
        if displace(&sock, info.pid).await {
            return (None, true);
        }
        // The wait ran out. Another client may have finished the takeover
        // in the meantime (its bridge respawns a daemon the moment the old
        // one leaves), so re-read the record: a different pid means the
        // socket already belongs to the successor and attaching is right.
        match read_lock_info() {
            Some(now) if now.pid != info.pid => {}
            _ => {
                tracing::warn!(
                    "daemon v{} (pid {}) did not shut down; attaching to it as-is",
                    info.version,
                    info.pid
                );
            }
        }
    }
    (connect_socket().await, false)
}

/// Connect to the daemon socket at its configured path.
async fn connect_socket() -> Option<Connection> {
    let sock = config::service_sock_path().ok()?;
    let name = socket_name(&sock).ok()?;
    match IpcStream::connect(name).await {
        Ok(stream) => Some(Connection { stream }),
        Err(_) => None,
    }
}

/// What a client should do about a running daemon, given both versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachPolicy {
    /// Attach normally: same version, a newer daemon or an unparseable pair.
    Attach,
    /// The daemon is older than this binary: shut it down and take over.
    Displace,
}

/// Decide between attaching and displacing. Only a strictly newer client
/// displaces; everything else, including versions that fail to parse,
/// attaches, so an odd lock record can never trigger a shutdown.
pub fn attach_policy(daemon_version: &str, own_version: &str) -> AttachPolicy {
    match (version_triple(daemon_version), version_triple(own_version)) {
        (Some(daemon), Some(own)) if daemon < own => AttachPolicy::Displace,
        _ => AttachPolicy::Attach,
    }
}

/// Parse a version string's numeric `major.minor.patch` triple, ignoring any
/// pre-release or build suffix.
fn version_triple(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let mut parts = core.split('.');
    let major = parts.next()?.trim().parse().ok()?;
    let minor = parts.next()?.trim().parse().ok()?;
    let patch = parts.next().unwrap_or("0").trim().parse().ok()?;
    Some((major, minor, patch))
}

/// Ask the daemon behind `sock` to shut down gracefully and wait for `pid` to
/// exit. Returns true once the process is gone, meaning ownership is takeable;
/// false leaves the daemon in place and the caller attaches to it as before,
/// so a failed takeover degrades to the old behavior instead of contending
/// for the index.
async fn displace(sock: &Path, pid: u32) -> bool {
    let Ok(name) = socket_name(sock) else {
        return false;
    };
    let stream = match IpcStream::connect(name).await {
        Ok(stream) => stream,
        // Nothing answers: gone already, or wedged beyond a graceful ask.
        Err(_) => return !process_alive(pid),
    };
    let conn = Connection { stream };
    let Ok(mut stream) = conn.into_ctl().await else {
        return false;
    };
    if stream
        .write_all(b"{\"v\":1,\"cmd\":\"shutdown\"}\n")
        .await
        .is_err()
        || stream.flush().await.is_err()
    {
        return false;
    }
    // Read the ack best-effort, then wait for the process to leave. The
    // daemon exits promptly after the ack - it does not drain active
    // sessions, it cancels them, and bridges resync and answer their
    // orphaned requests with a retry error - so the generous window here
    // tolerates OS process teardown, not a session drain.
    let mut buf = [0u8; 256];
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await;
    for _ in 0..240 {
        if !process_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Attach to a daemon, spawning one detached and polling for readiness (up to
/// ~15s) when none is running and `spawn` is set. The window is generous on
/// purpose: a cold start on modest hardware or a loaded machine can take well
/// over the couple of seconds a warm start needs, and giving up early strands
/// the MCP client with a dead server. `read_only` is passed through only to a
/// daemon this call spawns; attaching to an already-running daemon uses that
/// daemon's own mode, never this flag.
pub async fn ensure_daemon(
    spawn: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
    read_only: bool,
) -> anyhow::Result<Connection> {
    if let Some(conn) = try_attach().await {
        return Ok(conn);
    }
    if !spawn {
        anyhow::bail!("no Crystalline daemon is running; start one with `crystalline serve`");
    }
    spawn_daemon(db, config_path, read_only)?;
    // Poll readiness: lock record present and socket connectable. Another
    // client's lingering old-binary bridge can be reconnecting during this
    // same takeover window: it reads the empty lock this call's displacement
    // (if any) left behind, spawns a daemon from its own old binary and that
    // daemon can win the version-blind `acquire_ownership` race before this
    // call's own spawn lands. `try_attach_reporting` surfaces an in-poll
    // displacement so this loop re-drives `spawn_daemon` instead of waiting
    // out the budget behind a daemon it just tore down again; bounded to 3
    // re-spawns so a pathological interleaving of respawning bridges cannot
    // spawn-storm within the 15s budget.
    let mut respawns = 0u32;
    for _ in 0..300 {
        let (conn, displaced) = try_attach_reporting().await;
        if let Some(conn) = conn {
            return Ok(conn);
        }
        if displaced && respawns < 3 {
            respawns += 1;
            spawn_daemon(db, config_path, read_only)?;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!("spawned a daemon but it did not become ready within 15s")
}

/// Spawn `current_exe serve --daemon` fully detached, forwarding `--read-only`
/// when this instance was asked to serve read-only.
fn spawn_daemon(
    db: Option<&Path>,
    config_path: Option<&Path>,
    read_only: bool,
) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    if let Some(db) = db {
        cmd.arg("--db").arg(db);
    }
    cmd.arg("serve").arg("--daemon");
    if read_only {
        cmd.arg("--read-only");
    }
    if let Some(cfg) = config_path {
        cmd.arg("--config").arg(cfg);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A full new session, not just a process group: the daemon leads its
        // own session with no controlling terminal, so it survives whichever
        // client spawned it and never sees that client's terminal signals.
        // It does not matter who or where starts the daemon; it serves the
        // user's state directory and outlives its clients.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    cmd.spawn()?;
    Ok(())
}

/// Acquire ownership of the index by taking the advisory lock, with stale
/// takeover: a `kill -9`d predecessor's lock is already free, so a short retry
/// loop simply succeeds. Errors with the live owner's pid when a daemon is up.
pub fn acquire_ownership() -> anyhow::Result<Ownership> {
    let dir = config::state_dir()?;
    std::fs::create_dir_all(&dir)?;
    let lock_path = config::service_lock_path()?;
    let socket_path = config::service_sock_path()?;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;

    let mut acquired = false;
    for attempt in 0..20 {
        if FileExt::try_lock(&file).is_ok() {
            acquired = true;
            break;
        }
        if attempt < 19 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    if !acquired {
        let pid = read_lock_info().map(|i| i.pid).unwrap_or(0);
        anyhow::bail!(
            "another Crystalline instance owns the index (pid {pid}); stop it or attach over the socket"
        );
    }

    Ok(Ownership {
        lock_file: file,
        lock_path,
        socket_path,
    })
}

/// Read the one-line handshake from an accepted stream without consuming past
/// the newline. Bounded so a misbehaving client cannot stall the accept loop.
pub async fn read_mode_line(stream: &mut IpcStream) -> io::Result<String> {
    let mut buf = Vec::with_capacity(8);
    let mut byte = [0u8; 1];
    for _ in 0..16 {
        let n = stream.read(&mut byte).await?;
        if n == 0 || byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&buf).trim().to_string())
}

/// Best-effort process liveness. On unix a signal-0 probe; elsewhere the lock
/// and socket reachability govern, so assume alive.
pub fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if pid == 0 {
            return false;
        }
        let res = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if res == 0 {
            return true;
        }
        // EPERM means the process exists but is not ours to signal.
        io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_policy_displaces_only_a_strictly_older_daemon() {
        assert_eq!(attach_policy("0.5.1", "0.5.2"), AttachPolicy::Displace);
        assert_eq!(attach_policy("0.4.9", "0.5.0"), AttachPolicy::Displace);
        assert_eq!(attach_policy("0.5.2", "0.5.2"), AttachPolicy::Attach);
        assert_eq!(
            attach_policy("0.6.0", "0.5.2"),
            AttachPolicy::Attach,
            "an older client never displaces a newer daemon"
        );
    }

    #[test]
    fn attach_policy_never_displaces_on_unparseable_versions() {
        assert_eq!(attach_policy("", "0.5.2"), AttachPolicy::Attach);
        assert_eq!(attach_policy("dev", "0.5.2"), AttachPolicy::Attach);
        assert_eq!(attach_policy("0.5.1", "junk"), AttachPolicy::Attach);
    }

    #[test]
    fn version_triples_ignore_suffixes_and_tolerate_two_parts() {
        assert_eq!(version_triple("1.2.3"), Some((1, 2, 3)));
        assert_eq!(version_triple("1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(version_triple("1.2.3+build7"), Some((1, 2, 3)));
        assert_eq!(version_triple("1.2"), Some((1, 2, 0)));
        assert_eq!(version_triple("nope"), None);
    }

    /// The displacement mechanics against a scripted daemon: a mini ctl
    /// server on a temp socket that records the shutdown request and a real
    /// child process standing in for the daemon pid.
    #[cfg(unix)]
    #[tokio::test]
    async fn displace_sends_shutdown_and_waits_for_the_pid_to_exit() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("crystalline.sock");
        let name = socket_name(&sock).unwrap();
        let listener = ListenerOptions::new().name(name).create_tokio().unwrap();

        // A long-lived child stands in for the daemon process.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();

        let server = tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            let mode = read_mode_line(&mut stream).await.unwrap();
            let mut line = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                let n = stream.read(&mut byte).await.unwrap();
                if n == 0 || byte[0] == b'\n' {
                    break;
                }
                line.push(byte[0]);
            }
            stream.write_all(b"{\"ok\":true}\n").await.unwrap();
            stream.flush().await.unwrap();
            (mode, String::from_utf8(line).unwrap())
        });

        // Kill the stand-in shortly after the ask, like a daemon exiting.
        let killer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let _ = child.kill();
            let _ = child.wait();
        });

        assert!(displace(&sock, pid).await, "the daemon pid went away");
        let (mode, request) = server.await.unwrap();
        assert_eq!(mode, "ctl");
        assert!(request.contains("\"shutdown\""), "{request}");
        killer.await.unwrap();
    }

    /// A daemon that ignores the ask is left in place: displace reports
    /// failure so the caller attaches to it instead of contending.
    #[cfg(unix)]
    #[tokio::test]
    async fn displace_reports_failure_when_the_pid_stays() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("crystalline.sock");
        let name = socket_name(&sock).unwrap();
        let listener = ListenerOptions::new().name(name).create_tokio().unwrap();

        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();

        let server = tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            let _ = read_mode_line(&mut stream).await;
            let mut sink = [0u8; 64];
            let _ = stream.read(&mut sink).await;
            stream.write_all(b"{\"ok\":true}\n").await.unwrap();
            stream.flush().await.unwrap();
            // Keep the stream open; the "daemon" never exits.
            tokio::time::sleep(Duration::from_secs(10)).await;
        });

        assert!(!displace(&sock, pid).await, "the pid never went away");
        server.abort();
        let _ = child.kill();
        let _ = child.wait();
    }

    // `try_attach_reporting` tests below. A true two-version end-to-end is
    // impossible in a single build: `crystalline_core::VERSION` is a
    // compile-time constant, so one test binary can never hold two different
    // versions of itself. These fabricate the lock record's version string
    // directly (older, or the binary's own) against a scripted daemon on a
    // scratch socket instead, the same substitution `displace_*` above makes
    // for the daemon process itself.

    /// Guards `HOME`/`XDG_*_HOME` for the two tests below: `try_attach_reporting`
    /// resolves the real `crystalline_core::config::state_dir()` through
    /// these, and cargo runs test functions from this file on multiple
    /// threads, so both tests take this lock for their duration to avoid
    /// observing each other's env var state. The same pattern
    /// `crates/core/tests/config.rs` uses for `CRYSTALLINE_MODELS_DIR`.
    static STATE_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Points `HOME`/`XDG_*_HOME` at a fresh scratch directory for the
    /// duration of one test, restoring whatever the surrounding environment
    /// had on drop. A short `/tmp/cq-...` path rather than
    /// `tempfile::tempdir()`'s deeper one: the socket bound under it must stay
    /// within the ~104 byte unix socket path limit on macOS, the same reason
    /// the CLI integration tests' `Env` helper uses a short base.
    struct ScratchHome {
        dir: PathBuf,
        previous: Vec<(&'static str, Option<String>)>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl ScratchHome {
        fn new(tag: &str) -> ScratchHome {
            let guard = STATE_DIR_ENV_LOCK.lock().unwrap();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = PathBuf::from("/tmp").join(format!("cq-{tag}-{nanos}"));
            std::fs::create_dir_all(dir.join("config")).unwrap();
            std::fs::create_dir_all(dir.join("state")).unwrap();
            std::fs::create_dir_all(dir.join("cache")).unwrap();
            let vars = [
                "HOME",
                "XDG_CONFIG_HOME",
                "XDG_STATE_HOME",
                "XDG_CACHE_HOME",
            ];
            let previous = vars.iter().map(|v| (*v, std::env::var(v).ok())).collect();
            unsafe {
                std::env::set_var("HOME", &dir);
                std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
                std::env::set_var("XDG_STATE_HOME", dir.join("state"));
                std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));
            }
            // `state_dir()` itself never creates its directory (only
            // `acquire_ownership` does, which these tests bypass), so the
            // lock file's parent must exist before it is written below.
            std::fs::create_dir_all(config::state_dir().unwrap()).unwrap();
            ScratchHome {
                dir,
                previous,
                _guard: guard,
            }
        }
    }

    impl Drop for ScratchHome {
        fn drop(&mut self) {
            for (var, value) in &self.previous {
                unsafe {
                    match value {
                        Some(v) => std::env::set_var(var, v),
                        None => std::env::remove_var(var),
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// An old-version lock record whose pid is a real, killable child: the
    /// Displace arm shuts it down, the pid goes away and the call reports a
    /// completed displacement with no connection, exactly the case
    /// `ensure_daemon`'s readiness poll must react to by re-spawning.
    #[cfg(unix)]
    #[tokio::test]
    async fn try_attach_reporting_reports_a_completed_displacement() {
        let home = ScratchHome::new("try-attach-old");
        let sock = config::service_sock_path().unwrap();
        let name = socket_name(&sock).unwrap();
        let listener = ListenerOptions::new().name(name).create_tokio().unwrap();

        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();

        let lock_path = config::service_lock_path().unwrap();
        let info = LockInfo {
            pid,
            socket_path: sock.display().to_string(),
            version: "0.0.1".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        std::fs::write(&lock_path, serde_json::to_string(&info).unwrap()).unwrap();

        let server = tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            let _ = read_mode_line(&mut stream).await;
            let mut sink = [0u8; 256];
            let _ = stream.read(&mut sink).await;
            stream.write_all(b"{\"ok\":true}\n").await.unwrap();
            stream.flush().await.unwrap();
        });
        let killer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let _ = child.kill();
            let _ = child.wait();
        });

        let (conn, displaced) = try_attach_reporting().await;
        assert!(
            conn.is_none(),
            "the displaced daemon's socket is gone; nothing to attach to yet"
        );
        assert!(displaced, "the Displace arm ran and the pid went away");

        server.await.unwrap();
        killer.await.unwrap();
        drop(home);
    }

    /// A lock record at this binary's own version never reaches the Displace
    /// arm, so a live stub socket just attaches and reports no displacement.
    /// The lock's pid is this test process itself (always alive), which
    /// stands in for a live daemon without spawning a child.
    #[cfg(unix)]
    #[tokio::test]
    async fn try_attach_reporting_does_not_report_when_attaching() {
        let home = ScratchHome::new("try-attach-current");
        let sock = config::service_sock_path().unwrap();
        let name = socket_name(&sock).unwrap();
        let listener = ListenerOptions::new().name(name).create_tokio().unwrap();

        let lock_path = config::service_lock_path().unwrap();
        let info = LockInfo {
            pid: std::process::id(),
            socket_path: sock.display().to_string(),
            version: crystalline_core::VERSION.to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        std::fs::write(&lock_path, serde_json::to_string(&info).unwrap()).unwrap();

        let server = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let (conn, displaced) = try_attach_reporting().await;
        assert!(
            conn.is_some(),
            "a live stub socket at the own version attaches"
        );
        assert!(!displaced, "attaching never runs the Displace arm");

        drop(conn);
        server.await.unwrap();
        drop(home);
    }
}
