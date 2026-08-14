//! Single-instance mechanics: the advisory lock and the local socket.
//!
//! Exactly one process owns the derived index. Ownership is an `fs4` exclusive
//! lock held on `service.lock` for the owner's lifetime; the record describing
//! the owner (pid, socket, version) lives in the separate `service.json`,
//! because Windows region locks are mandatory - reads and writes through any
//! other handle fail - so the locked file itself must never carry data. The
//! socket (a Unix domain socket, or a per-state-directory named pipe on
//! Windows) is how everyone else reaches the owner. See
//! `research/single-instance-ipc.md`.
//!
//! Attaching is version aware: the lock record carries the owner's version,
//! and a client built from a newer version displaces an older daemon with a
//! graceful ctl shutdown before taking over, so a binary upgrade needs no
//! manual daemon restart. The takeover is one-way on purpose - an older
//! client attaches to a newer daemon as-is - which keeps lingering
//! old-binary bridges from flip-flopping an upgraded daemon back.
//!
//! Attaching is also wedge aware. Version takeover travels over the socket,
//! which is exactly what a wedged daemon kills: on 2026-07-28 a daemon stayed
//! alive holding the lock while its socket answered nothing, so no client
//! could attach (nothing answered) and none could spawn (the lock was held),
//! and every session failed for forty minutes until the wedged process died on
//! its own. [`diagnose_holder`] names that state and [`dislodge_unresponsive`]
//! ends it: a bounded socket probe first, then, only for a holder whose
//! recorded pid is alive and identifiably a crystalline binary, a graceful
//! signal followed by a hard one. Every doubt refuses instead of signalling.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// The owner record, written to service.json after the socket is bound.
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

/// The first Crystalline release whose daemon parses an `mcp` handshake line
/// with options after the mode token. An older daemon compares the whole
/// trimmed line against `"mcp"` (see `daemon::handle_conn` before this
/// release) and silently drops anything else, which costs the bridge its
/// socket rather than one option, so the extended line is only ever sent to a
/// daemon at or above this version.
const HARNESS_LINE_MIN_VERSION: &str = "0.13.0";

/// The option token that tells the daemon this stdio session's harness is
/// already onboarded, so the skill surface and the second copy of the routing
/// block are both withheld. Absence means "serve", which is what every older
/// bridge's bare `mcp` line already meant.
pub(crate) const SKILLS_OFF_OPTION: &str = "skills=off";

/// The version of the daemon this process is about to hand its handshake to,
/// read from the lock record. `None` when there is no readable record, which
/// reads as "too old to be sure".
fn daemon_version() -> Option<String> {
    read_lock_info().map(|info| info.version)
}

/// The `mcp` handshake line for a resolved answer against a daemon of
/// `daemon_version`. Bare `mcp` unless there is something to say **and** the
/// daemon is new enough to hear it. Pure so the decision is testable without
/// a daemon.
fn mcp_mode_line(harness_onboarded: bool, daemon_version: Option<&str>) -> String {
    // Positively "at least the minimum", never "not older than": an
    // unparseable or missing version has to read as too old, and a negated
    // `strictly_newer` reads it as new enough because that helper answers
    // false for an unparseable pair on either side.
    let understood = match (
        daemon_version.and_then(version_triple),
        version_triple(HARNESS_LINE_MIN_VERSION),
    ) {
        (Some(daemon), Some(minimum)) => daemon >= minimum,
        _ => false,
    };
    if harness_onboarded && understood {
        format!("mcp {SKILLS_OFF_OPTION}\n")
    } else {
        "mcp\n".to_string()
    }
}

/// A connected client stream, before the handshake line is written.
pub struct Connection {
    stream: IpcStream,
}

impl Connection {
    /// Write the `mcp` handshake and hand back the stream for an rmcp session or
    /// a byte pump.
    ///
    /// `harness_onboarded` is the answer the bridge process resolved at
    /// startup from its `--harness` argument and this machine's install
    /// receipt; the daemon builds its per-socket `McpServer` with it. It rides
    /// the handshake line rather than being re-derived daemon-side on purpose:
    /// the bridge inherits the harness's own environment (and therefore its
    /// state directory), while a long-lived daemon carries whatever
    /// environment spawned it first, and a value resolved once per process
    /// cannot drift across a reconnect.
    ///
    /// **The extended line is only sent to a daemon that can parse it**, see
    /// [`HARNESS_LINE_MIN_VERSION`]. An older daemon compares the whole line
    /// against `"mcp"` and drops anything else, and a failed displacement can
    /// leave one running (`try_attach_reporting` attaches to a daemon that
    /// would not shut down), so the fallback is the bare line, which resolves
    /// to "serve" - the safe direction, and exactly today's behaviour.
    pub async fn into_mcp(self, harness_onboarded: bool) -> io::Result<IpcStream> {
        let line = mcp_mode_line(harness_onboarded, daemon_version().as_deref());
        self.handshake(line.as_bytes()).await
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
    info_path: PathBuf,
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

    /// Publish the owner record now that the socket is bound. Written beside
    /// the lock file, never into it (mandatory locks on Windows), and renamed
    /// into place so a reader never sees a partial record.
    pub fn publish(&self) -> io::Result<()> {
        let info = LockInfo {
            pid: std::process::id(),
            socket_path: self.socket_display(),
            version: crystalline_core::VERSION.to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        let json = serde_json::to_string(&info).unwrap_or_default();
        let tmp = self.info_path.with_extension("json.tmp");
        std::fs::write(&tmp, json.as_bytes())?;
        std::fs::rename(&tmp, &self.info_path)
    }

    /// The socket path (unix) or pipe name (Windows) as a display string.
    pub fn socket_display(&self) -> String {
        #[cfg(windows)]
        {
            format!(r"\\.\pipe\{}", pipe_name(&self.socket_path))
        }
        #[cfg(not(windows))]
        {
            self.socket_path.display().to_string()
        }
    }
}

impl Drop for Ownership {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.info_path);
        let _ = FileExt::unlock(&self.lock_file);
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.socket_path);
        }
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// The Windows pipe name for a given socket path: `crystalline-` plus the
/// FNV-1a hash of the lowercased path. Hashing keeps the name short and free
/// of separator characters; deriving it from the state-directory-scoped
/// socket path isolates users and test homes from each other, where a fixed
/// name would collide machine-wide. FNV-1a is fixed here (not DefaultHasher)
/// so every release derives the same name and can attach across upgrades.
#[cfg(windows)]
fn pipe_name(sock_path: &Path) -> String {
    let lowered = sock_path.to_string_lossy().to_lowercase();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in lowered.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("crystalline-{hash:016x}")
}

/// Build the platform socket name: a filesystem path on unix, a namespaced pipe
/// on Windows.
fn socket_name(sock_path: &Path) -> io::Result<Name<'_>> {
    #[cfg(windows)]
    {
        pipe_name(sock_path).to_ns_name::<GenericNamespaced>()
    }
    #[cfg(not(windows))]
    {
        sock_path.as_os_str().to_fs_name::<GenericFilePath>()
    }
}

/// Read the current owner record, if any is present and parseable. Reads
/// `service.json`; falls back to parsing a legacy record out of `service.lock`
/// itself, which pre-record-split daemons wrote, so an upgraded client can
/// still see (and displace) a daemon from before the split.
pub fn read_lock_info() -> Option<LockInfo> {
    if let Ok(path) = config::service_info_path()
        && let Ok(text) = std::fs::read_to_string(path)
        && let Ok(info) = serde_json::from_str(&text)
    {
        return Some(info);
    }
    let legacy = config::service_lock_path().ok()?;
    let text = std::fs::read_to_string(legacy).ok()?;
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

/// Whether `candidate` is a strictly newer release than `baseline`. Same
/// triple parsing as [`attach_policy`]; an unparseable version on either side
/// is never newer, so an odd record can only ever read as a conflict, never as
/// an upgrade skew.
pub(crate) fn strictly_newer(candidate: &str, baseline: &str) -> bool {
    match (version_triple(candidate), version_triple(baseline)) {
        (Some(candidate), Some(baseline)) => candidate > baseline,
        _ => false,
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

// --- unresponsive holders ---------------------------------------------------

/// How long the holder of the index lock gets to answer a bounded `ctl`
/// handshake before it counts as unresponsive. Generous on purpose: a healthy
/// daemon answers `sessions` straight from in-memory counters, and the `ctl`
/// branch of its accept loop skips the routing-cache refresh the `mcp` branch
/// does, so its reply costs microseconds of work even under load. The wedged
/// daemon in the 2026-07-28 incident answered nothing at all, for forty
/// minutes, so any answer inside this window is proof of life.
const HOLDER_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a dislodged holder gets to exit and release the lock after the
/// graceful signal. On unix that signal is `SIGTERM`, which the daemon handles
/// as a clean shutdown (it drops its ownership, removing the record and the
/// socket), so this window is the clean path's fair chance. A daemon wedged
/// badly enough to ignore its socket may ignore this too, which is what the
/// hard step below is for; three seconds keeps a connecting client moving.
const DISLODGE_TERM_WAIT: Duration = Duration::from_secs(3);

/// How long to wait after the hard signal before giving up. `SIGKILL` (and
/// `TerminateProcess` on Windows) cannot be refused, so this covers only OS
/// teardown of the process and its file locks. A lock still held after it
/// means something other than the recorded process holds it, and the caller
/// refuses rather than hunting for another victim.
const DISLODGE_KILL_WAIT: Duration = Duration::from_secs(3);

/// Poll granularity for both dislodge waits.
const DISLODGE_POLL: Duration = Duration::from_millis(50);

/// What currently owns the index lock, as far as a client can tell from the
/// outside. The three actionable states drive the connect flow, `status` and
/// `doctor`; [`HolderState::Unknown`] is the deliberate catch-all that never
/// leads to a signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HolderState {
    /// Nothing holds the lock; ownership is takeable.
    Free,
    /// A holder answered the bounded `ctl` probe. Attach to it.
    Responsive,
    /// The lock is held, the socket answered nothing within
    /// [`HOLDER_PROBE_TIMEOUT`] and the record names a live process that is
    /// identifiably a crystalline binary. This is the wedge.
    Unresponsive {
        /// The recorded pid of the wedged daemon.
        pid: u32,
        /// How long the socket probe waited before giving up.
        probe: Duration,
    },
    /// The lock is held, nothing answered, and who holds it could not be
    /// established: no record, a record naming a dead pid, a pid whose
    /// executable could not be read, or one that is not a crystalline binary.
    /// Nothing is ever signalled in this state.
    Unknown {
        /// Why the holder could not be identified, for the refusal message.
        detail: String,
    },
}

/// The result of a dislodge attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DislodgeOutcome {
    /// Nothing to do: the lock was free, or its holder answered the probe.
    NotNeeded,
    /// A verified unresponsive holder was signalled away and its stale record
    /// and socket file cleaned up. Ownership is takeable again.
    Dislodged {
        /// The pid that was signalled.
        pid: u32,
    },
}

/// Whether the index lock is free right now, probed by taking it and letting
/// it go again. `fs4` locks are `flock(2)` on unix and `LockFileEx` on
/// Windows, both scoped to the open handle rather than the process, so this
/// probe sees a same-process holder exactly as it sees another process's.
/// A missing lock file means nobody can be holding it.
fn lock_is_free(lock_path: &Path) -> io::Result<bool> {
    let file = match OpenOptions::new().read(true).write(true).open(lock_path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e),
    };
    match FileExt::try_lock(&file) {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            Ok(true)
        }
        Err(fs4::TryLockError::WouldBlock) => Ok(false),
        Err(fs4::TryLockError::Error(e)) => Err(e),
    }
}

/// Ask the daemon socket for a trivial `ctl` answer, bounded by
/// [`HOLDER_PROBE_TIMEOUT`]. `sessions` is the cheapest real command: it reads
/// in-memory counters and touches neither the store nor the routing cache, so
/// a slow index can never make a healthy daemon look wedged. Any well-formed
/// reply counts; the content is irrelevant, only that something served it.
async fn probe_socket_responds() -> bool {
    let Ok(sock) = config::service_sock_path() else {
        return false;
    };
    let exchange = async {
        let name = socket_name(&sock).ok()?;
        let stream = IpcStream::connect(name).await.ok()?;
        let mut stream = Connection { stream }.into_ctl().await.ok()?;
        stream
            .write_all(b"{\"v\":1,\"cmd\":\"sessions\"}\n")
            .await
            .ok()?;
        stream.flush().await.ok()?;
        let mut buf = [0u8; 1];
        match stream.read(&mut buf).await {
            Ok(n) if n > 0 => Some(()),
            _ => None,
        }
    };
    matches!(
        tokio::time::timeout(HOLDER_PROBE_TIMEOUT, exchange).await,
        Ok(Some(()))
    )
}

/// The executable file name of a running process, lowercased and without any
/// `.exe` suffix. `None` whenever the platform cannot answer - an unreadable
/// `/proc` entry, a denied handle, an unsupported target - which the caller
/// must treat as "unidentified", never as "not ours".
fn process_exe_name(pid: u32) -> Option<String> {
    let path = process_exe_path(pid)?;
    let name = Path::new(&path).file_name()?.to_string_lossy().to_string();
    let lower = name.to_ascii_lowercase();
    Some(lower.strip_suffix(".exe").unwrap_or(&lower).to_string())
}

/// The executable path of a running process, per platform.
fn process_exe_path(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        // The `exe` symlink is the real executable path; `comm` is the fallback
        // for a process whose symlink cannot be read (a different user's
        // process denies the readlink but still exposes `comm`). `comm` is
        // truncated to 15 bytes, which "crystalline" fits inside.
        if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
            return Some(exe.to_string_lossy().to_string());
        }
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        let comm = comm.trim();
        if comm.is_empty() {
            return None;
        }
        Some(comm.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        // libproc's `proc_pidpath` is the only portable way to a process path
        // on macOS; there is no /proc. It returns the byte length written, or
        // 0 on failure (a denied or departed pid).
        // PROC_PIDPATHINFO_MAXSIZE, which libproc.h defines as 4 * MAXPATHLEN.
        const PROC_PIDPATHINFO_MAXSIZE: usize = 4 * 1024;
        let mut buf = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
        let written = unsafe {
            libc::proc_pidpath(
                pid as libc::c_int,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
            )
        };
        if written <= 0 {
            return None;
        }
        buf.truncate(written as usize);
        Some(String::from_utf8_lossy(&buf).to_string())
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        };
        if pid == 0 {
            return None;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        // The Win32 path form, not the native \Device\Harddisk one.
        let ok = unsafe {
            QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
        };
        unsafe { CloseHandle(handle) };
        if ok == 0 {
            return None;
        }
        buf.truncate(len as usize);
        Some(String::from_utf16_lossy(&buf))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = pid;
        None
    }
}

/// This binary's own executable file name, lowercased and without any `.exe`
/// suffix.
fn own_exe_name() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let name = exe.file_name()?.to_string_lossy().to_string();
    let lower = name.to_ascii_lowercase();
    Some(lower.strip_suffix(".exe").unwrap_or(&lower).to_string())
}

/// Whether an executable file name identifies a crystalline binary. The
/// canonical name is the gate; a binary installed under some other file name
/// (a versioned release artifact, a renamed copy) spawns its daemon as its own
/// executable, so a holder wearing exactly this client's file name is ours
/// too. Anything else is a stranger and is never signalled.
fn is_crystalline_exe_name(name: &str) -> bool {
    if name == "crystalline" {
        return true;
    }
    own_exe_name().is_some_and(|own| own == name)
}

/// Diagnose what owns the index lock. Read-only and side-effect free: it takes
/// and immediately releases the lock to test it, opens one short-lived socket
/// connection and reads the record. Nothing is signalled here.
pub async fn diagnose_holder() -> HolderState {
    let Ok(lock_path) = config::service_lock_path() else {
        return HolderState::Unknown {
            detail: "the state directory could not be resolved".to_string(),
        };
    };
    match lock_is_free(&lock_path) {
        Ok(true) => return HolderState::Free,
        Ok(false) => {}
        Err(e) => {
            return HolderState::Unknown {
                detail: format!("the lock file could not be tested: {e}"),
            };
        }
    }

    let started = Instant::now();
    if probe_socket_responds().await {
        return HolderState::Responsive;
    }
    let probe = started.elapsed();

    let Some(info) = read_lock_info() else {
        return HolderState::Unknown {
            detail: "no service record names the holder".to_string(),
        };
    };
    if !process_alive(info.pid) {
        return HolderState::Unknown {
            detail: format!(
                "the recorded pid {} is gone but the lock is still held",
                info.pid
            ),
        };
    }
    match process_exe_name(info.pid) {
        Some(name) if is_crystalline_exe_name(&name) => HolderState::Unresponsive {
            pid: info.pid,
            probe,
        },
        Some(name) => HolderState::Unknown {
            detail: format!("pid {} is '{name}', not a Crystalline process", info.pid),
        },
        None => HolderState::Unknown {
            detail: format!("what pid {} is could not be verified", info.pid),
        },
    }
}

/// The refusal a caller reports when the lock is held by something that cannot
/// be identified. It names the lock path and how to look at the holder,
/// because the only safe next step is a person deciding what that process is.
pub fn unknown_holder_error(detail: &str) -> anyhow::Error {
    let lock = config::service_lock_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "the service lock file".to_string());
    let inspect = if cfg!(windows) {
        "Resource Monitor's CPU tab lists which process holds that handle".to_string()
    } else {
        format!("inspect it with `lsof {lock}`")
    };
    anyhow::anyhow!(
        "something holds {lock} but no Crystalline daemon answers there ({detail}); nothing was signalled, {inspect} and stop it, then retry"
    )
}

/// Signal a process: the graceful ask, or the hard kill. Returns whether the
/// signal was delivered. Windows has no graceful equivalent, so both steps are
/// the same `TerminateProcess` call there and the first wait simply succeeds.
fn signal_process(pid: u32, hard: bool) -> bool {
    // A zero pid means "every process in my group" to `kill(2)` and pid 1 is
    // init. Neither is ever a Crystalline daemon, and the callers already
    // refuse both; this is the last line of defense.
    if pid <= 1 {
        return false;
    }
    #[cfg(unix)]
    {
        let sig = if hard { libc::SIGKILL } else { libc::SIGTERM };
        unsafe { libc::kill(pid as libc::pid_t, sig) == 0 }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        let _ = hard;
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if handle.is_null() {
            return false;
        }
        let ok = unsafe { TerminateProcess(handle, 1) } != 0;
        unsafe { CloseHandle(handle) };
        ok
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, hard);
        false
    }
}

/// Wait up to `budget` for the dislodge to take effect. Either half is enough
/// to stop escalating. A free lock is the goal itself: the caller can spawn,
/// and it is what a departing process releases first (the kernel drops its
/// file locks during exit, before the process is reaped, so a not-yet-reaped
/// child of a test harness counts as released here). A pid that is simply
/// gone also ends the escalation, whatever holds the lock now: signalling a
/// departed pid again could only ever hit a recycled one, and the caller's
/// readiness poll handles a successor that legitimately took over.
async fn wait_for_release(pid: u32, lock_path: &Path, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if lock_is_free(lock_path).unwrap_or(false) || !process_alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(DISLODGE_POLL).await;
    }
}

/// Remove the stale record and socket file a dislodged daemon left behind,
/// guarded so a successor that already took over is never disturbed: the
/// record goes only while it still names the dislodged pid, the socket file
/// only while the lock is still free.
fn clean_after_dislodge(pid: u32, _lock_path: &Path) {
    if read_lock_info().is_some_and(|info| info.pid == pid)
        && let Ok(path) = config::service_info_path()
    {
        let _ = std::fs::remove_file(path);
    }
    // Only unix leaves a socket file behind; a Windows named pipe disappears
    // with the process that bound it.
    #[cfg(unix)]
    if lock_is_free(_lock_path).unwrap_or(false)
        && let Ok(sock) = config::service_sock_path()
    {
        let _ = std::fs::remove_file(sock);
    }
}

/// Dislodge an unresponsive lock holder, if that is what owns the index. The
/// one implementation behind both the connect path and `doctor --fix`.
///
/// Diagnoses first and acts only on [`HolderState::Unresponsive`]: a free lock
/// and a holder that answered its socket are both left alone, and an
/// unidentified holder is an error, never a signal. The wedged holder gets the
/// graceful signal, [`DISLODGE_TERM_WAIT`] to leave, then the hard signal and
/// [`DISLODGE_KILL_WAIT`], after which its stale record and socket file go.
pub async fn dislodge_unresponsive() -> anyhow::Result<DislodgeOutcome> {
    let lock_path = config::service_lock_path()
        .map_err(|e| anyhow::anyhow!("could not resolve the service lock path: {e}"))?;
    match diagnose_holder().await {
        HolderState::Free | HolderState::Responsive => Ok(DislodgeOutcome::NotNeeded),
        HolderState::Unknown { detail } => Err(unknown_holder_error(&detail)),
        HolderState::Unresponsive { pid, probe } => {
            // Two last safety gates on the kill path itself, deliberately
            // separate from the identity check above: never signal init and
            // never signal ourselves (the embedded server holds this very lock
            // in-process, and a client must not shoot its own foot).
            if pid <= 1 || pid == std::process::id() {
                return Err(unknown_holder_error(&format!(
                    "pid {pid} is not a process this client may signal"
                )));
            }
            if !signal_process(pid, false) {
                return Err(unknown_holder_error(&format!(
                    "pid {pid} could not be asked to stop"
                )));
            }
            if !wait_for_release(pid, &lock_path, DISLODGE_TERM_WAIT).await {
                signal_process(pid, true);
                if !wait_for_release(pid, &lock_path, DISLODGE_KILL_WAIT).await {
                    return Err(unknown_holder_error(&format!(
                        "pid {pid} was stopped but the lock stayed held"
                    )));
                }
            }
            clean_after_dislodge(pid, &lock_path);
            tracing::warn!(
                "dislodged an unresponsive crystalline daemon (pid {pid}) that held the index lock without answering its socket for {:.1}s; starting a fresh daemon",
                probe.as_secs_f64()
            );
            Ok(DislodgeOutcome::Dislodged { pid })
        }
    }
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

    // Attaching failed. If the lock is already held at this point, a spawn
    // would only lose the race, which is the 2026-07-28 wedge: nothing to
    // attach to and nothing to take. Diagnosing here rather than after the
    // spawn is what makes the lock loss observable at all - the daemon we
    // spawn is detached, so its own lock failure is only ever a line in
    // daemon.log. The lock probe is a single flock on the fast path, so a
    // normal cold start (lock free) pays nothing and behaves exactly as
    // before. Only one dislodge attempt per connect: `unknown_holder` carries
    // the refusal into this call's own error rather than signalling anything.
    let mut unknown_holder = None;
    match diagnose_holder().await {
        // A daemon just won the race and answers: attach, never dislodge.
        HolderState::Responsive => {
            if let Some(conn) = try_attach().await {
                return Ok(conn);
            }
        }
        HolderState::Unresponsive { .. } => match dislodge_unresponsive().await {
            Ok(DislodgeOutcome::Dislodged { .. }) | Ok(DislodgeOutcome::NotNeeded) => {}
            Err(e) => unknown_holder = Some(e),
        },
        HolderState::Unknown { detail } => {
            // Doubt never signals. A daemon that is starting up right now
            // looks exactly like this (lock taken, record not published yet),
            // so fall through to the spawn and the readiness wait, which is
            // what this call did before this branch existed, and keep the
            // refusal for the error this call ends with if the wait runs out.
            unknown_holder = Some(unknown_holder_error(&detail));
        }
        HolderState::Free => {}
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
    // A holder we refused to touch is the better explanation of the timeout
    // than the generic one: it names the lock file and how to look at whoever
    // owns it, instead of pointing at a daemon log the spawn never reached.
    if let Some(e) = unknown_holder {
        return Err(e);
    }
    anyhow::bail!(
        "spawned a daemon but it did not become ready within 15s (see daemon.log in the state directory)"
    )
}

/// Open the daemon stderr log for appending, starting the file over once it
/// outgrows 1 MiB. The cap is checked at spawn time and the reset is
/// best-effort (a live holder can defeat the removal on Windows), so it bounds
/// growth across spawns, not within one daemon's lifetime. `None` (and a null
/// stderr) when the state dir or the file cannot be prepared: logging must
/// never be the reason a daemon fails to spawn.
fn daemon_log_sink() -> Option<std::process::Stdio> {
    let path = config::daemon_log_path().ok()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    if std::fs::metadata(&path)
        .map(|m| m.len() > 1024 * 1024)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(&path);
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    Some(file.into())
}

/// Spawn `current_exe serve --daemon` fully detached, forwarding `--read-only`
/// when this instance was asked to serve read-only.
///
/// No `--http off` is passed and none ever should be: a daemon started this way
/// (an agent's `crystalline mcp` connection, the Desktop extension) serves the
/// HTTP endpoint on 127.0.0.1:7411 by exactly the same default a hand-started
/// `crystalline serve` does. The daemon is a singleton, so an autostarted one
/// that skipped HTTP would leave the web UI dead for the most common population
/// of all, with no way to get it back short of shutting the daemon down. The one
/// coherent opt-out is `service.http=false`, which turns the endpoint off for
/// every daemon however it started; the spawned process inherits this one's
/// environment, so `CRYSTALLINE_SERVICE_HTTP` reaches it too.
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
        .stderr(daemon_log_sink().unwrap_or_else(std::process::Stdio::null));
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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
        };
        // No console window for the detached daemon, its own process group,
        // and a breakaway from the parent's job object so it outlives a
        // harness that kills its job on exit. A job that forbids breakaway
        // fails the spawn outright, so retry inside the job: starting at all
        // beats outliving the parent.
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_err() {
            cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
            cmd.spawn()?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        cmd.spawn()?;
        Ok(())
    }
}

/// Acquire ownership of the index by taking the advisory lock, with stale
/// takeover: a `kill -9`d predecessor's lock is already free, so a short retry
/// loop simply succeeds. Errors with the live owner's pid when a daemon is up.
pub fn acquire_ownership() -> anyhow::Result<Ownership> {
    let dir = config::state_dir()?;
    std::fs::create_dir_all(&dir)?;
    let lock_path = config::service_lock_path()?;
    let info_path = config::service_info_path()?;
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

    // The lock is held. Empty any legacy record bytes (pre-split daemons wrote
    // the record into the lock file itself) through this same handle, the only
    // handle that may touch a mandatorily locked file on Windows.
    let _ = file.set_len(0);

    Ok(Ownership {
        lock_file: file,
        lock_path,
        info_path,
        socket_path,
    })
}

/// How many bytes of handshake line are read before giving up on finding the
/// newline. The line is `mcp`, `ctl` or `mcp` plus options; 64 leaves room for
/// several options without letting a misbehaving client stall the accept loop.
/// It was 16, which fit `mcp claude-code` with one byte to spare, so the cap
/// is raised well clear of the shapes this protocol can grow.
const MODE_LINE_CAP: usize = 64;

/// Read the one-line handshake from an accepted stream without consuming past
/// the newline. Bounded so a misbehaving client cannot stall the accept loop.
///
/// **A line longer than the cap is an error rather than a prefix.** Returning
/// the truncated head would leave the rest of the line in the stream to be
/// read as JSON-RPC, which is a wedged session; the caller drops the
/// connection instead, which the bridge sees as a dead socket and reports.
pub async fn read_mode_line(stream: &mut IpcStream) -> io::Result<String> {
    let mut buf = Vec::with_capacity(8);
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).await?;
        if n == 0 || byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() >= MODE_LINE_CAP {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "handshake line longer than the mode-line cap",
            ));
        }
    }
    Ok(String::from_utf8_lossy(&buf).trim().to_string())
}

/// Split a handshake line into its mode and its options: the first
/// whitespace-delimited token is the mode, the rest are options.
///
/// Deliberately tolerant in both directions of version skew. An older bridge's
/// bare `mcp` line yields no options, and an option this binary does not know
/// is ignored rather than fatal, so a newer bridge talking to this daemon
/// degrades to the default instead of losing its socket.
pub fn split_mode_line(line: &str) -> (&str, Vec<&str>) {
    let mut parts = line.split_whitespace();
    let mode = parts.next().unwrap_or("");
    (mode, parts.collect())
}

/// Best-effort process liveness. On unix a signal-0 probe, on Windows an
/// OpenProcess exit-code query; elsewhere assume alive.
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
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        if pid == 0 {
            return false;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            // Access denied means the process exists but is not ours to query.
            return std::io::Error::last_os_error().raw_os_error()
                == Some(ERROR_ACCESS_DENIED as i32);
        }
        let mut code: u32 = 0;
        let alive =
            unsafe { GetExitCodeProcess(handle, &mut code) } != 0 && code == STILL_ACTIVE as u32;
        unsafe { CloseHandle(handle) };
        alive
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the mcp handshake line ---------------------------------------------

    /// The extended line only goes to a daemon that can parse it. An older one
    /// compares the whole trimmed line against `"mcp"` and drops anything
    /// else, and a displacement that times out leaves exactly such a daemon
    /// running and attached to (`try_attach_reporting` says so in as many
    /// words), so the fallback has to be the bare line rather than a dead
    /// socket.
    #[test]
    fn the_extended_mode_line_is_only_sent_to_a_daemon_that_parses_it() {
        assert_eq!(mcp_mode_line(true, Some("0.13.0")), "mcp skills=off\n");
        assert_eq!(mcp_mode_line(true, Some("0.14.2")), "mcp skills=off\n");
        assert_eq!(
            mcp_mode_line(true, Some("0.12.9")),
            "mcp\n",
            "an older daemon gets the line it understands, and serves the surface"
        );
        assert_eq!(
            mcp_mode_line(true, None),
            "mcp\n",
            "no readable lock record is not a licence to guess"
        );
        assert_eq!(
            mcp_mode_line(true, Some("not-a-version")),
            "mcp\n",
            "an unparseable version reads as too old, never as new enough"
        );
        // Nothing to say, nothing added, whatever the daemon can parse.
        for version in [Some("0.13.0"), Some("0.12.0"), None] {
            assert_eq!(mcp_mode_line(false, version), "mcp\n");
        }
    }

    /// The reader used to stop at 16 bytes and return the truncated head,
    /// leaving the rest of the line in the stream to be read as JSON-RPC.
    /// `mcp claude-code` was 15 bytes, so the old cap had one byte of
    /// headroom. Both halves of the fix are pinned here: a longer line arrives
    /// whole, and a line past the new cap is an error the caller drops the
    /// connection on rather than a prefix that wedges the session.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_long_handshake_line_arrives_whole_and_an_endless_one_errors() {
        async fn round_trip(payload: Vec<u8>) -> io::Result<String> {
            let dir = tempfile::tempdir().unwrap();
            let sock = dir.path().join("crystalline.sock");
            let listener = ListenerOptions::new()
                .name(socket_name(&sock).unwrap())
                .create_tokio()
                .unwrap();
            let server = tokio::spawn(async move {
                let mut stream = listener.accept().await.unwrap();
                read_mode_line(&mut stream).await
            });
            let mut client = IpcStream::connect(socket_name(&sock).unwrap())
                .await
                .unwrap();
            client.write_all(&payload).await.unwrap();
            client.flush().await.unwrap();
            let out = server.await.unwrap();
            drop(client);
            out
        }

        let long = b"mcp skills=off future=1\n".to_vec();
        assert!(long.len() > 16, "longer than the old cap: {}", long.len());
        assert_eq!(round_trip(long).await.unwrap(), "mcp skills=off future=1");

        let endless = vec![b'x'; MODE_LINE_CAP + 8];
        assert!(
            round_trip(endless).await.is_err(),
            "a line past the cap must not come back as a prefix"
        );
    }

    /// The daemon's half: the first token is the mode and the rest are
    /// options, so an old bridge's bare `mcp` and a new bridge's extended line
    /// both serve, and an option this binary does not know is ignored rather
    /// than fatal.
    #[test]
    fn a_mode_line_splits_into_a_mode_and_its_options() {
        assert_eq!(split_mode_line("mcp"), ("mcp", vec![]));
        assert_eq!(
            split_mode_line("mcp skills=off"),
            ("mcp", vec![SKILLS_OFF_OPTION])
        );
        assert_eq!(
            split_mode_line("mcp skills=off future=1"),
            ("mcp", vec![SKILLS_OFF_OPTION, "future=1"])
        );
        assert_eq!(split_mode_line("ctl"), ("ctl", vec![]));
        assert_eq!(split_mode_line(""), ("", vec![]));
    }

    #[cfg(windows)]
    #[test]
    fn pipe_names_are_stable_and_scoped_to_the_socket_path() {
        let a = pipe_name(Path::new(
            r"C:\Users\a\AppData\Roaming\crystalline\service.sock",
        ));
        let b = pipe_name(Path::new(
            r"C:\Users\b\AppData\Roaming\crystalline\service.sock",
        ));
        assert_ne!(a, b, "different homes get different pipes");
        assert_eq!(
            a,
            pipe_name(Path::new(
                r"c:\users\A\appdata\roaming\crystalline\service.sock"
            )),
            "windows paths are case-insensitive, the pipe name must be too"
        );
        assert!(a.starts_with("crystalline-") && a.len() == "crystalline-".len() + 16);
    }

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
    fn strictly_newer_is_true_only_for_a_higher_triple() {
        assert!(strictly_newer("0.9.0", "0.8.2"), "a higher triple is newer");
        assert!(!strictly_newer("0.8.2", "0.8.2"), "equal is not newer");
        assert!(!strictly_newer("0.8.1", "0.8.2"), "older is not newer");
        assert!(
            !strictly_newer("garbage", "0.8.2"),
            "an unparseable candidate is never newer"
        );
        assert!(
            !strictly_newer("0.9.0", "junk"),
            "an unparseable baseline is never newer"
        );
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

    /// Guards `HOME`/`XDG_*_HOME` (and, on Windows, `USERPROFILE`/`APPDATA`/
    /// `LOCALAPPDATA`) for the tests below: each resolves the real
    /// `crystalline_core::config::state_dir()` through these, and cargo runs
    /// test functions from this file on multiple threads, so every test takes
    /// this lock for its duration to avoid observing another's env var state.
    /// The same pattern `crates/core/tests/config.rs` uses for
    /// `CRYSTALLINE_MODELS_DIR`.
    static STATE_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Points `HOME`/`XDG_*_HOME` (and the Windows equivalents) at a fresh
    /// scratch directory for the duration of one test, restoring whatever the
    /// surrounding environment had on drop. A short base path rather than
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
            // `/tmp` keeps the unix socket path short; `temp_dir()` is the
            // Windows equivalent (there is no unix socket path limit to
            // respect there, but `/tmp` does not exist on Windows).
            #[cfg(unix)]
            let base = PathBuf::from("/tmp");
            #[cfg(windows)]
            let base = std::env::temp_dir();
            let dir = base.join(format!("cq-{tag}-{nanos}"));
            std::fs::create_dir_all(dir.join("config")).unwrap();
            std::fs::create_dir_all(dir.join("state")).unwrap();
            std::fs::create_dir_all(dir.join("cache")).unwrap();
            let vars = [
                "HOME",
                "XDG_CONFIG_HOME",
                "XDG_STATE_HOME",
                "XDG_CACHE_HOME",
                "USERPROFILE",
                "APPDATA",
                "LOCALAPPDATA",
            ];
            let previous = vars.iter().map(|v| (*v, std::env::var(v).ok())).collect();
            unsafe {
                std::env::set_var("HOME", &dir);
                std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
                std::env::set_var("XDG_STATE_HOME", dir.join("state"));
                std::env::set_var("XDG_CACHE_HOME", dir.join("cache"));
                std::env::set_var("USERPROFILE", &dir);
                std::env::set_var("APPDATA", dir.join("config"));
                std::env::set_var("LOCALAPPDATA", dir.join("cache"));
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

        let info_path = config::service_info_path().unwrap();
        let info = LockInfo {
            pid,
            socket_path: sock.display().to_string(),
            version: "0.0.1".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        std::fs::write(&info_path, serde_json::to_string(&info).unwrap()).unwrap();

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

        let info_path = config::service_info_path().unwrap();
        let info = LockInfo {
            pid: std::process::id(),
            socket_path: sock.display().to_string(),
            version: crystalline_core::VERSION.to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        std::fs::write(&info_path, serde_json::to_string(&info).unwrap()).unwrap();

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

    /// The record round trip that was impossible while the record lived inside
    /// the locked file: publish writes and read_lock_info reads WHILE the
    /// exclusive lock is held. Deliberately ungated - on Windows CI this is
    /// the regression test for the mandatory-lock bug that broke daemon mode.
    #[tokio::test]
    async fn ownership_record_round_trips_while_the_lock_is_held() {
        let home = ScratchHome::new("record-round-trip");
        let ownership = acquire_ownership().unwrap();
        ownership.publish().unwrap();
        let info = read_lock_info().expect("the record is readable while the lock is held");
        assert_eq!(info.pid, std::process::id());
        assert_eq!(info.version, crystalline_core::VERSION);
        assert_eq!(info.socket_path, ownership.socket_display());
        drop(ownership);
        assert!(read_lock_info().is_none(), "drop removes the record");
        drop(home);
    }

    /// An oversized daemon log starts over rather than growing unbounded.
    /// Deliberately ungated - a detached daemon's stderr sink matters on every
    /// platform, and `ScratchHome` keeps this sync-safe under the same env
    /// lock the other tests here take.
    #[tokio::test]
    async fn daemon_log_sink_caps_the_file_size() {
        let home = ScratchHome::new("daemon-log");
        let path = config::daemon_log_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vec![b'x'; 2 * 1024 * 1024]).unwrap();
        let sink = daemon_log_sink().expect("the sink opens");
        drop(sink);
        assert!(
            std::fs::metadata(&path).unwrap().len() < 1024 * 1024,
            "an oversized log starts over"
        );
        drop(home);
    }

    /// A pre-split daemon wrote its record into service.lock itself; with no
    /// live owner the fallback still surfaces it so displacement works across
    /// the upgrade.
    #[tokio::test]
    async fn read_lock_info_falls_back_to_a_legacy_record() {
        let home = ScratchHome::new("legacy-record");
        let legacy = LockInfo {
            pid: std::process::id(),
            socket_path: "legacy".to_string(),
            version: "0.8.2".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        std::fs::write(
            config::service_lock_path().unwrap(),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();
        let info = read_lock_info().expect("the legacy record is readable");
        assert_eq!(info.version, "0.8.2");
        drop(home);
    }

    /// Acquiring ownership empties legacy bytes out of the lock file, so a
    /// stale pre-split record can never shadow the live service.json. On
    /// Windows the mandatory lock alone already hides the legacy bytes from
    /// any other handle, so this emptying assertion is meaningful on unix.
    #[tokio::test]
    async fn acquire_ownership_empties_a_stale_legacy_record() {
        let home = ScratchHome::new("legacy-emptied");
        let stale = r#"{"pid":1,"socket_path":"gone","version":"0.0.1","started_at":""}"#;
        std::fs::write(config::service_lock_path().unwrap(), stale).unwrap();
        let ownership = acquire_ownership().unwrap();
        assert!(
            read_lock_info().is_none(),
            "no service.json and the legacy bytes are gone"
        );
        drop(ownership);
        drop(home);
    }

    /// The identity gate on the kill path. The canonical name always counts;
    /// this test binary's own file name counts (a differently named binary
    /// spawns its daemon as itself); anything else is a stranger and must
    /// never be signalled.
    #[test]
    fn only_a_crystalline_executable_name_passes_the_identity_gate() {
        assert!(is_crystalline_exe_name("crystalline"));
        assert!(
            is_crystalline_exe_name(&own_exe_name().unwrap()),
            "a holder wearing this binary's own file name is ours"
        );
        assert!(!is_crystalline_exe_name("sleep"));
        assert!(!is_crystalline_exe_name("postgres"));
        assert!(
            !is_crystalline_exe_name("crystalline-backup"),
            "a merely similar name is a stranger"
        );
        assert!(!is_crystalline_exe_name(""));
    }

    /// The exe lookup answers for a live process on the platforms that
    /// support it, and never claims a name for a pid that cannot exist.
    #[test]
    fn process_exe_name_reads_this_process_and_not_a_bogus_pid() {
        let own = process_exe_name(std::process::id());
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        assert!(
            own.as_deref().is_some_and(|n| !n.is_empty()),
            "this process's own executable name must be readable"
        );
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        assert!(own.is_none(), "an unsupported platform never guesses");
        assert!(
            process_exe_name(0).is_none(),
            "pid 0 is never a real process to identify"
        );
    }

    /// The lock probe: free while nobody holds it, held while ownership
    /// lives, free again once it drops. `fs4` locks the open handle, not the
    /// process, so this sees a same-process holder exactly as it sees another
    /// process's - which is what the diagnose tests below rely on.
    #[tokio::test]
    async fn lock_is_free_tracks_the_held_lock() {
        let home = ScratchHome::new("lock-free");
        let lock_path = config::service_lock_path().unwrap();
        assert!(
            lock_is_free(&lock_path).unwrap(),
            "no lock file yet means nothing holds it"
        );
        let ownership = acquire_ownership().unwrap();
        assert!(
            !lock_is_free(&lock_path).unwrap(),
            "a held lock reads as held"
        );
        drop(ownership);
        assert!(
            lock_is_free(&lock_path).unwrap(),
            "the lock frees when ownership drops"
        );
        drop(home);
    }

    /// Nothing holds the lock: there is no holder to diagnose.
    #[tokio::test]
    async fn diagnose_holder_reports_a_free_lock() {
        let home = ScratchHome::new("diag-free");
        assert_eq!(diagnose_holder().await, HolderState::Free);
        drop(home);
    }

    /// A holder whose socket answers is responsive, whatever else is true of
    /// it. This is the scenario that must never lead to a signal: the probe
    /// answers, so the caller attaches.
    #[cfg(unix)]
    #[tokio::test]
    async fn diagnose_holder_reports_a_responsive_holder() {
        let home = ScratchHome::new("diag-live");
        let ownership = acquire_ownership().unwrap();
        let listener = ownership.bind_listener().unwrap();
        ownership.publish().unwrap();
        let server = tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            let _ = read_mode_line(&mut stream).await;
            let mut sink = [0u8; 64];
            let _ = stream.read(&mut sink).await;
            stream.write_all(b"{\"v\":1,\"ok\":true}\n").await.unwrap();
            stream.flush().await.unwrap();
        });

        assert_eq!(diagnose_holder().await, HolderState::Responsive);

        server.await.unwrap();
        drop(ownership);
        drop(home);
    }

    /// The wedge: the lock is held, no socket answers and the record names a
    /// live process whose executable is a crystalline binary (this test
    /// process itself, which the identity gate accepts by its own file name).
    #[cfg(unix)]
    #[tokio::test]
    async fn diagnose_holder_names_a_verified_live_holder_as_unresponsive() {
        let home = ScratchHome::new("diag-wedge");
        let ownership = acquire_ownership().unwrap();
        // Deliberately no listener: this is the wedge, a held lock with
        // nothing serving the socket.
        ownership.publish().unwrap();

        match diagnose_holder().await {
            HolderState::Unresponsive { pid, .. } => assert_eq!(pid, std::process::id()),
            other => panic!("expected an unresponsive holder, got {other:?}"),
        }

        drop(ownership);
        drop(home);
    }

    /// Even a verified unresponsive holder is never signalled when it is this
    /// very process: the kill path's own guard, independent of the identity
    /// check, and the refusal names the lock file and how to look at it.
    #[cfg(unix)]
    #[tokio::test]
    async fn dislodge_refuses_to_signal_this_process() {
        let home = ScratchHome::new("diag-self");
        let ownership = acquire_ownership().unwrap();
        ownership.publish().unwrap();

        let err = dislodge_unresponsive()
            .await
            .expect_err("a client must never signal itself");
        let message = err.to_string();
        assert!(message.contains("service.lock"), "{message}");
        assert!(message.contains("lsof"), "{message}");
        assert!(message.contains("nothing was signalled"), "{message}");
        assert!(
            process_alive(std::process::id()),
            "the refusal left this process alone"
        );

        drop(ownership);
        drop(home);
    }

    /// A held lock whose record names a dead pid is the classic doubt case:
    /// something owns the index and it is not what the record describes.
    /// Refuse and say where to look, never hunt for another victim.
    #[cfg(unix)]
    #[tokio::test]
    async fn diagnose_holder_refuses_a_record_naming_a_dead_pid() {
        let home = ScratchHome::new("diag-dead");
        let ownership = acquire_ownership().unwrap();
        // The lock stays held by this process while the record claims a pid
        // that cannot be alive.
        let info = LockInfo {
            pid: 2_147_483_647,
            socket_path: config::service_sock_path().unwrap().display().to_string(),
            version: crystalline_core::VERSION.to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        std::fs::write(
            config::service_info_path().unwrap(),
            serde_json::to_string(&info).unwrap(),
        )
        .unwrap();

        match diagnose_holder().await {
            HolderState::Unknown { detail } => assert!(detail.contains("2147483647"), "{detail}"),
            other => panic!("expected an unidentified holder, got {other:?}"),
        }
        assert!(
            dislodge_unresponsive().await.is_err(),
            "an unidentified holder is refused, never signalled"
        );

        drop(ownership);
        drop(home);
    }

    /// A held lock with no record at all is equally unidentified. This is
    /// also the shape of a daemon that has taken the lock and not published
    /// yet, which is precisely why it must never be signalled.
    #[cfg(unix)]
    #[tokio::test]
    async fn diagnose_holder_refuses_a_lock_with_no_record() {
        let home = ScratchHome::new("diag-norecord");
        let ownership = acquire_ownership().unwrap();
        match diagnose_holder().await {
            HolderState::Unknown { detail } => {
                assert!(detail.contains("no service record"), "{detail}")
            }
            other => panic!("expected an unidentified holder, got {other:?}"),
        }
        drop(ownership);
        drop(home);
    }

    /// process_alive tracks a real child on every platform.
    #[test]
    fn process_alive_tracks_a_real_child() {
        assert!(process_alive(std::process::id()));
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit 0"])
            .spawn()
            .unwrap();
        #[cfg(unix)]
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!process_alive(pid), "a reaped child is not alive");
    }
}
