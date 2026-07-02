//! Single-instance mechanics: the advisory lock and the local socket.
//!
//! Exactly one process owns the derived index. Ownership is an `fs4` advisory
//! exclusive lock held on `service.lock` for the owner's lifetime; the socket
//! (a Unix domain socket, or the named pipe `\\.\pipe\crystalline` on Windows)
//! is how everyone else reaches it. See `research/single-instance-ipc.md`.

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
/// which is the signal that ownership is takeable.
pub async fn try_attach() -> Option<Connection> {
    let info = read_lock_info()?;
    if !process_alive(info.pid) {
        return None;
    }
    let sock = config::service_sock_path().ok()?;
    let name = socket_name(&sock).ok()?;
    match IpcStream::connect(name).await {
        Ok(stream) => Some(Connection { stream }),
        Err(_) => None,
    }
}

/// Attach to a daemon, spawning one detached and polling for readiness (up to
/// ~2s) when none is running and `spawn` is set.
pub async fn ensure_daemon(
    spawn: bool,
    db: Option<&Path>,
    config_path: Option<&Path>,
) -> anyhow::Result<Connection> {
    if let Some(conn) = try_attach().await {
        return Ok(conn);
    }
    if !spawn {
        anyhow::bail!("no Crystalline daemon is running; start one with `crystalline serve`");
    }
    spawn_daemon(db, config_path)?;
    // Poll readiness: lock record present and socket connectable.
    for _ in 0..40 {
        if let Some(conn) = try_attach().await {
            return Ok(conn);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!("spawned a daemon but it did not become ready within 2s")
}

/// Spawn `current_exe serve --daemon` fully detached.
fn spawn_daemon(db: Option<&Path>, config_path: Option<&Path>) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    if let Some(db) = db {
        cmd.arg("--db").arg(db);
    }
    cmd.arg("serve").arg("--daemon");
    if let Some(cfg) = config_path {
        cmd.arg("--config").arg(cfg);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // New session so the daemon survives the parent and ignores the
        // controlling terminal's signals.
        cmd.process_group(0);
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
