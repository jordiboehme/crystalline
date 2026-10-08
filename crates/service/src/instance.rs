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
    /// Whether this daemon parses an `mcp` handshake line carrying options
    /// after the mode token.
    ///
    /// **A declared capability rather than a version comparison, because the
    /// version cannot answer this question.** A daemon predating the extended
    /// line compares the whole trimmed line against `"mcp"` and drops anything
    /// else, so sending it costs the bridge its socket; and the version that
    /// first learned to parse it is a version *this very tree* already
    /// carries, so any threshold spelled here would let a daemon built one
    /// commit earlier through. An older daemon writes no such field and
    /// `serde(default)` reads it as `false`, which is exactly right.
    #[serde(default)]
    pub mcp_line_options: bool,
    /// How the owning daemon was started. `None` on a record written before
    /// 0.18.0, and also on one published by a holder that never served (the
    /// `hold-lock` test command), so a message reading this must say "did not
    /// record it" rather than naming a version. Reported, never used to decide
    /// anything.
    #[serde(default)]
    pub started_by: Option<StartMode>,
    /// What the owning daemon bound its HTTP endpoint to.
    #[serde(default)]
    pub http: HttpBinding,
    /// The `Host` allow-list the owning daemon serves with, on top of
    /// loopback. Empty means loopback only, and also means "not recorded" on a
    /// pre-0.18.0 record; the pair with `started_by` tells those apart.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Set when the holder is not a daemon but a one-shot command that took
    /// the state directory for a moment (a rename, the name adoption after a
    /// sync), naming that command, for example `crystalline domain rename`.
    /// Such a holder serves no socket and leaves on its own: a client and a
    /// starting daemon wait for it, and nothing ever signals it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standalone: Option<String>,
    /// Where the owning daemon runs (working directory, and on Windows its job
    /// and package identity), taken when it published this record: none of it
    /// changes during a daemon's life. `None` on a record from a daemon older
    /// than 0.21.1 and on one from a holder that never served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs_in: Option<crate::runs_in::RunsIn>,
    /// The options the owning daemon was started with, so whoever displaces
    /// it after an upgrade starts the successor the same way. `None` on a
    /// record from a daemon older than 0.22.1 and on one from a holder that
    /// never served; see [`StartOptions`] for what a displacement does then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<StartOptions>,
}

/// The options a daemon was started with that decide what it serves and
/// where: written into its record so a client that displaces it after an
/// upgrade starts the successor with exactly these, not with the defaults
/// (the I2 finding of 0.22.1: a plain CLI command displaced a daemon a
/// read-only or `--db` bridge had started, spawned a writable one on the
/// default index, and the next bridge attached to that).
///
/// What is recorded, and what is not:
///
/// - `db` is the global `--db` as given, made absolute. It is a file path:
///   no flag of this CLI takes a database URL, so the record never holds one.
///   `None` means the state directory's own `index.db`, which is the same
///   file for every reader of this record (the record lives in that
///   directory).
/// - `config` is the config file the daemon resolved (the `--config` flag,
///   else `CRYSTALLINE_CONFIG`, else the default path), absolute and always
///   set by a daemon that served. Recording the default too means the
///   successor never picks up a different file from a `CRYSTALLINE_CONFIG`
///   the displacing shell happens to carry. A `database.url` in that file,
///   password included, stays in that file and is reached through it.
/// - `read_only` is the effective mode (the flag, `service.read_only` in the
///   file or the environment), so a read-only daemon never comes back
///   writable, even when its mode came from a harness's `env` block that a
///   shell never has.
/// - `http` and `allowed_hosts` are the `--http` and `--allowed-host` flags
///   as given, not the bindings they resolved to, so the successor resolves
///   the file and the environment the way its predecessor did.
/// - `exit_when_idle` is the extension's bounded life: a daemon started with
///   it is replaced by one that also ends when idle.
/// - `env` lists the names, never the values, of the `CRYSTALLINE_*`
///   variables the daemon applied over its config file. A database URL or a
///   token read from the environment is a secret, so only the fact that it
///   came from that variable is kept. Whoever displaces the daemon must have
///   every one of them set, or it starts no successor at all (see
///   [`attach_after_displacement`]); a variable it has that is not listed is
///   removed from the successor's environment. Known limit: variables are
///   compared by name only, so a displacing process that sets a listed
///   variable to another value (a `CRYSTALLINE_DATABASE_URL` naming another
///   database, say) hands the successor its own value. Read-only cannot be
///   relaxed that way, since it is replayed as a flag.
/// - `partial` says one of the paths could not be written down (it is not
///   UTF-8); such a record is treated like a missing variable.
///
/// The environment outside the overlay is not recorded either and passes from
/// the displacing process as it is: `CRYSTALLINE_CONFIG` (harmless, the
/// recorded `--config` wins), the cache directories the model is kept in,
/// `RUST_LOG` and proxy variables among it. A successor started with another
/// cache directory downloads the model again; nothing it serves changes.
///
/// Not recorded: `--take-over` (a one-time migration of host locks the
/// predecessor already made; the successor claims the locks its predecessor
/// released under the same instance id), `--autostarted` and `--daemon`
/// (every successor is an autostarted daemon), and `--breakaway-refused`
/// (a fact about the spawner, decided anew at each spawn).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOptions {
    /// The global `--db` as given, absolute. Never a URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db: Option<String>,
    /// The config file the daemon resolved, absolute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// The effective read-only mode.
    #[serde(default)]
    pub read_only: bool,
    /// The `--http` flag as given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<String>,
    /// The `--allowed-host` flags as given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
    /// Started with `--exit-when-idle`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exit_when_idle: bool,
    /// The names of the environment variables the daemon applied over its
    /// config file. Names only, never values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    /// A path could not be recorded.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

impl StartOptions {
    /// What `run_serve` records, from its flags and the config it loaded.
    /// The paths are made absolute against this process's directory, which
    /// for a hand-started `serve` is where the person ran it.
    pub fn capture(
        db: Option<&Path>,
        config: &Path,
        read_only: bool,
        http: Option<&str>,
        allowed_hosts: &[String],
        exit_when_idle: bool,
        overlay: &crate::overlay::EnvOverlay,
    ) -> StartOptions {
        let mut partial = false;
        let mut text = |path: &Path| -> Option<String> {
            let path = absolute_for_daemon(path);
            match path.to_str() {
                Some(text) => Some(text.to_string()),
                None => {
                    partial = true;
                    None
                }
            }
        };
        let db = db.and_then(&mut text);
        let config = text(config);
        StartOptions {
            db,
            config,
            read_only,
            http: http.map(str::to_string),
            allowed_hosts: allowed_hosts.to_vec(),
            exit_when_idle,
            env: overlay_variable_names(overlay),
            partial,
        }
    }
}

/// The names of the variables `overlay` applies, sorted and without
/// repeats. Only the first column of [`EnvOverlay::active_overrides`] is
/// read; its values (masked or not) never leave this function.
///
/// [`EnvOverlay::active_overrides`]: crate::overlay::EnvOverlay::active_overrides
fn overlay_variable_names(overlay: &crate::overlay::EnvOverlay) -> Vec<String> {
    let mut names: Vec<String> = overlay
        .active_overrides()
        .into_iter()
        .map(|(var, _, _)| var)
        .collect();
    names.sort();
    names.dedup();
    names
}

static START_OPTIONS: std::sync::OnceLock<StartOptions> = std::sync::OnceLock::new();

/// Record this daemon's start options for [`Ownership::publish`]. The first
/// call wins, like `record_serve_intent`.
pub fn record_start_options(options: StartOptions) {
    let _ = START_OPTIONS.set(options);
}

pub use crate::serving::{
    HttpBinding, ServeIntent, StartMode, loopback_connect_addr, record_serve_intent, serve_intent,
};

/// The option token that tells the daemon this stdio session's harness is
/// already onboarded, so the skill surface and the second copy of the routing
/// block are both withheld. Absence means "serve", which is what every older
/// bridge's bare `mcp` line already meant.
pub(crate) const SKILLS_OFF_OPTION: &str = "skills=off";

/// The option token that tells the daemon this stdio session's harness has
/// Crystalline's session hook installed but its onboarding is not verified:
/// the routing block is answered with the short conditional pointer and the
/// skill surface stays served. A daemon older than the token ignores it
/// (its check reads only [`SKILLS_OFF_OPTION`]) and serves everything, the
/// safe direction.
pub(crate) const ROUTING_CONDITIONAL_OPTION: &str = "routing=conditional";

/// What the `crystalline mcp` process resolved about the harness that spawned
/// it, from its `--harness` argument and this machine's install receipt,
/// before the session started. Two facts, because they gate two things:
///
/// - `hook_installed`: the receipt lists this harness with Crystalline's
///   session hook wired, so the routing block probably arrives at session
///   start and the instructions can shrink to a pointer.
/// - `onboarding_verified`: the hook part **and** the harness's profile flag,
///   set once a live check confirmed that the harness loads the hook's block
///   and the shipped skills as files. Only this hides the skill surface.
///
/// The default (both false) is "serve everything with the full instructions",
/// which is what every uncertain input resolves to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HarnessGate {
    /// The receipt lists the harness with its session hook installed.
    pub hook_installed: bool,
    /// The hook is installed and the harness's onboarding is verified.
    pub onboarding_verified: bool,
}

impl HarnessGate {
    /// Whether the harness is fully onboarded by its own hook and skills on
    /// disk: the hook is installed and its onboarding is verified.
    pub fn verified(self) -> bool {
        self.hook_installed && self.onboarding_verified
    }

    /// Whether the hook is installed but its onboarding is not verified.
    pub fn conditional(self) -> bool {
        self.hook_installed && !self.onboarding_verified
    }
}

/// The `mcp` handshake line for a resolved gate. Bare `mcp` unless there is
/// something to say **and** the daemon has declared it can hear it. A verified
/// gate sends `skills=off`, byte-identical to what a 0.22.0 bridge sent, and a
/// hook that is installed but unverified sends `routing=conditional`. Pure so
/// the decision is testable without a daemon.
fn mcp_mode_line(gate: HarnessGate, daemon_parses_options: bool) -> String {
    if !daemon_parses_options {
        return "mcp\n".to_string();
    }
    if gate.verified() {
        format!("mcp {SKILLS_OFF_OPTION}\n")
    } else if gate.conditional() {
        format!("mcp {ROUTING_CONDITIONAL_OPTION}\n")
    } else {
        "mcp\n".to_string()
    }
}

/// A connected client stream, before the handshake line is written.
pub struct Connection {
    stream: IpcStream,
    /// Whether the daemon behind this connection declared that it parses
    /// handshake options (`HolderFacts::mcp_line_options`). False for a
    /// connection made without asking, which sends the bare line: the safe
    /// direction.
    mcp_line_options: bool,
}

impl Connection {
    /// Write the `mcp` handshake and hand back the stream for an rmcp session or
    /// a byte pump.
    ///
    /// `gate` is the answer the bridge process resolved at
    /// startup from its `--harness` argument and this machine's install
    /// receipt; the daemon builds its per-socket `McpServer` with it. It rides
    /// the handshake line rather than being re-derived daemon-side on purpose:
    /// the bridge inherits the harness's own environment (and therefore its
    /// state directory), while a long-lived daemon carries whatever
    /// environment spawned it first, and a value resolved once per process
    /// cannot drift across a reconnect.
    ///
    /// **The extended line is only sent to a daemon that declared it parses
    /// one**, declared over the pipe, through `holder`
    /// ([`HolderFacts::mcp_line_options`]). An older daemon compares
    /// the whole line against `"mcp"` and drops anything else, and a failed
    /// displacement can leave one running (`try_attach_reporting` attaches to
    /// a daemon that would not shut down), so the fallback is the bare line,
    /// which resolves to "serve" - the safe direction, and exactly today's
    /// behaviour.
    pub async fn into_mcp(self, gate: HarnessGate) -> io::Result<IpcStream> {
        let line = mcp_mode_line(gate, self.mcp_line_options);
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

/// The usable byte budget of a `sockaddr_un` path: 104 on the BSD family
/// (macOS included), 108 on Linux, minus the trailing NUL.
#[cfg(unix)]
fn socket_path_budget() -> usize {
    if cfg!(any(target_os = "macos", target_os = "ios")) {
        103
    } else {
        107
    }
}

/// Refuse an over-long socket path in words, before the kernel refuses it in
/// an errno.
///
/// A long `HOME` is enough to spend the whole budget, and what a user saw then
/// was two unhelpful things at once: a bare "invalid argument" from `bind` in
/// `daemon.log`, which nobody reads, and a client that waited out its 15s
/// readiness budget before blaming the daemon it spawned. The length, the
/// limit and the way out are all knowable before either happens, so they are
/// said before either happens.
#[cfg(unix)]
fn check_socket_path(path: &Path) -> io::Result<()> {
    let len = path.as_os_str().as_encoded_bytes().len();
    let budget = socket_path_budget();
    if len > budget {
        return Err(io::Error::other(format!(
            "the daemon socket path '{}' is {len} bytes but a unix socket path holds at most {budget}; \
             point XDG_STATE_HOME at a shorter directory and retry",
            path.display()
        )));
    }
    Ok(())
}

impl Ownership {
    /// Bind the local socket, removing any stale socket file first.
    pub fn bind_listener(&self) -> io::Result<IpcListener> {
        // The diagnosis first, so a daemon that cannot bind says why in
        // daemon.log instead of leaving an errno there.
        #[cfg(unix)]
        check_socket_path(&self.socket_path)?;
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
        let intent = serve_intent();
        let info = LockInfo {
            pid: std::process::id(),
            socket_path: self.socket_display(),
            version: crystalline_core::VERSION.to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            // This daemon's `handle_conn` splits the handshake line into a
            // mode and its options, so a bridge may send them.
            mcp_line_options: true,
            // Absent where a process publishes a record without having gone
            // through `run_serve`. The one such publisher in this tree is the
            // `hold-lock` test command, which holds the lock and serves
            // nothing; a real daemon always records its intent first.
            started_by: intent.map(|i| i.started_by),
            http: intent.map(|i| i.http.clone()).unwrap_or_default(),
            allowed_hosts: intent.map(|i| i.allowed_hosts.clone()).unwrap_or_default(),
            standalone: None,
            // Only a process that went through `run_serve` has start facts;
            // `hold-lock` records none, as it records no intent.
            runs_in: intent.map(|_| crate::runs_in::RunsIn::here()),
            // Recorded by `run_serve` only; `hold-lock` records none.
            start: START_OPTIONS.get().cloned(),
        };
        let _ = PUBLISHED.set(info.clone());
        self.write_record(&info)
    }

    /// Publish the record of a one-shot command that holds the state
    /// directory for a moment, naming `command` (for example `crystalline
    /// domain rename`), so a starting daemon and a connecting client can say
    /// what they wait for instead of meeting a holder nobody can name.
    pub fn publish_standalone(&self, command: &str) -> io::Result<()> {
        let info = LockInfo {
            pid: std::process::id(),
            socket_path: String::new(),
            version: crystalline_core::VERSION.to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            mcp_line_options: false,
            started_by: None,
            http: HttpBinding::default(),
            allowed_hosts: Vec::new(),
            standalone: Some(command.to_string()),
            runs_in: None,
            start: None,
        };
        self.write_record(&info)
    }

    /// Write `info` to the record file beside the lock, renamed into place.
    fn write_record(&self, info: &LockInfo) -> io::Result<()> {
        let json = serde_json::to_string(info).unwrap_or_default();
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
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.socket_path);
        }
        // The lock file goes while it is still locked, and only then is the
        // lock let go. A process that opened the file before it went waits
        // on the lock, then finds that the file it locked is no longer the
        // one at the path ([`locked_file_is_current`]) and opens it again,
        // so it can never own a file that is about to disappear while a
        // third process creates a fresh one beside it.
        let _ = std::fs::remove_file(&self.lock_path);
        let _ = FileExt::unlock(&self.lock_file);
    }
}

/// Whether `file`, just locked, is still the file at `path`.
///
/// Every owner removes `service.lock` when it leaves, and `doctor --fix`
/// removes one nobody holds. A process that opened the file just before
/// that and locked it just after holds a lock on a file with no name any
/// more, while the next opener creates a new file at the path and locks
/// that one too: two owners of one state directory. Comparing the locked
/// handle with the path closes that: a lock on a file that is no longer
/// there is let go and the path opened again. Compared by the file's
/// identity (device and inode on unix, volume and file index on Windows),
/// never by its name; a path that is gone reads as not current. A check
/// that cannot be made at all (the identity of either side cannot be read)
/// keeps the lock, which is what every owner did before this check existed:
/// a start must never fail over a question it could not ask.
fn locked_file_is_current(file: &File, path: &Path) -> bool {
    let held = match file.try_clone().and_then(same_file::Handle::from_file) {
        Ok(held) => held,
        Err(e) => {
            tracing::debug!("the locked service.lock could not be identified: {e}");
            return true;
        }
    };
    match same_file::Handle::from_path(path) {
        Ok(named) => held == named,
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        // On Windows a file removed while a handle to it is still open (the
        // one just locked, here) keeps its name until that handle closes,
        // and opening it by that name is refused: the file is on its way
        // out, so the lock on it is not the current one.
        Err(e) if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied => false,
        Err(e) => {
            tracing::debug!("{} could not be identified: {e}", path.display());
            true
        }
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
/// binary is displaced first (a graceful shutdown, then signals when a verified
/// Crystalline process stays, then `None`), so the caller
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
    let (conn, displaced) = try_attach_displacing().await;
    (conn, displaced.is_some())
}

/// As [`try_attach_reporting`], but a displacement hands back what the
/// displaced daemon said about how it was started (the outer `Some`; the
/// inner value is `None` for a daemon older than 0.22.1). The daemon is gone
/// once it has left, so this first ask is the only chance to hear it.
///
/// The pipe is asked first and the record is not read here: a stale record
/// (its daemon was killed before it removed it) or a private one (written
/// inside an app package, which only that package sees) never decides
/// anything, and a one-shot command, which serves no pipe, is no daemon to
/// attach to.
pub async fn try_attach_displacing() -> (Option<Connection>, Option<Option<StartOptions>>) {
    try_attach_displacing_in(crate::runs_in::PackageContext::here()).await
}

/// [`try_attach_displacing`] for a process that runs where `here` says.
///
/// Inside an app package it never displaces, whatever the versions, and it
/// only ever connects. The facts it acts on can come from a record when the
/// daemon gives none itself, and inside a package that record can be the
/// package's private copy, so they decide nothing there but the handshake
/// line and the warning about an older daemon. A listener that gives no
/// facts at all (silent, or a 0.23.1 daemon whose `status` failed, with no
/// usable record beside it) is still a daemon outside the package, so a
/// packaged client connects to it as it is, with the bare `mcp` line, rather
/// than running the task for a daemon that is already there.
/// The cost: a hung daemon that still holds the pipe is attached as it is,
/// so the session hangs instead of getting the stub.
pub(crate) async fn try_attach_displacing_in(
    here: &crate::runs_in::PackageContext,
) -> (Option<Connection>, Option<Option<StartOptions>>) {
    let Ok(sock) = config::service_sock_path() else {
        return (None, None);
    };
    let Some(facts) = ask_holder_at(&sock).await else {
        if here.is_packaged() {
            return (connect_socket_at(&sock, false).await, None);
        }
        return (None, None);
    };
    if attach_policy_for(&facts.version, crystalline_core::VERSION, here) == AttachPolicy::Displace
    {
        tracing::info!(
            "displacing crystalline daemon v{} (pid {}) in favor of v{}",
            facts.version,
            facts.pid,
            crystalline_core::VERSION
        );
        if displace(&sock, facts.pid).await {
            return (None, Some(facts.start));
        }
        // The old daemon is still running: `displace` either could not verify
        // it as a Crystalline process and so never signalled it, or not even
        // the hard signal ended it. Another client may also have finished the
        // takeover in the meantime (its bridge respawns a daemon the moment
        // the old one leaves), so ask the pipe again: a different pid means
        // the socket already belongs to the successor and attaching to it is
        // right. Otherwise the connect below reaches whatever still answers
        // on the socket, if anything, rather than contending for the index.
        match ask_holder_at(&sock).await {
            Some(now) if now.pid != facts.pid => {
                return (connect_socket_at(&sock, now.mcp_line_options).await, None);
            }
            _ => tracing::warn!(
                "daemon v{} (pid {}) is still running after the shutdown ask and could not be stopped; leaving it in place",
                facts.version,
                facts.pid
            ),
        }
    }
    let conn = connect_socket_at(&sock, facts.mcp_line_options).await;
    // Said once the attach worked, so a bridge polling for its daemon does
    // not repeat it on every turn.
    if conn.is_some()
        && here.is_packaged()
        && strictly_newer(crystalline_core::VERSION, &facts.version)
    {
        tracing::warn!(
            "{}",
            older_daemon_warning(&facts.version, crystalline_core::VERSION)
        );
    }
    (conn, None)
}

/// Attach to a running daemon exactly as it is: a bare pipe connect. Unlike
/// [`try_attach`] it reads no record, asks no facts, never displaces an older
/// daemon and never waits on one leaving, so the whole call is one connect -
/// microseconds when no daemon runs, and never the six seconds a graceful
/// takeover can cost.
///
/// That is what a per-prompt hook needs: it runs in front of a person's
/// prompt, it has a one-second budget for the whole exchange, and a takeover
/// is `crystalline mcp`'s to do at the next session start, where seconds are
/// affordable and a respawn follows. A daemon older than this binary answers
/// `tool search_engrams` the same way, so attaching as-is costs nothing but
/// the version's own behaviour. A connection made this way sends the bare
/// `mcp` line, should it ever be turned into a session.
pub async fn try_attach_passive() -> Option<Connection> {
    connect_socket().await
}

/// Connect to the daemon socket at its configured path, without asking who
/// holds it.
async fn connect_socket() -> Option<Connection> {
    let sock = config::service_sock_path().ok()?;
    connect_socket_at(&sock, false).await
}

/// Connect to the daemon socket at `sock`, carrying what its holder declared
/// about the handshake line.
async fn connect_socket_at(sock: &Path, mcp_line_options: bool) -> Option<Connection> {
    let name = socket_name(sock).ok()?;
    IpcStream::connect(name)
        .await
        .ok()
        .map(|stream| Connection {
            stream,
            mcp_line_options,
        })
}

/// The record this process published, kept for `ctl holder`.
static PUBLISHED: std::sync::OnceLock<LockInfo> = std::sync::OnceLock::new();

/// The record this daemon published, `None` before `publish` ran (and in a
/// test that drives the ctl handler without a daemon).
pub(crate) fn published_record() -> Option<LockInfo> {
    PUBLISHED.get().cloned()
}

/// What the daemon on the pipe says about itself: the facts a client used to
/// read from `service.json`. Asked over the pipe, because a record can be
/// stale (its daemon was killed before it removed it) or private (written by
/// a process inside an app package, which only that package sees), and a
/// record like that must never decide an attach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderFacts {
    pub pid: u32,
    pub version: String,
    #[serde(default)]
    pub mcp_line_options: bool,
    #[serde(default)]
    pub runs_in: Option<crate::runs_in::RunsIn>,
    #[serde(default)]
    pub start: Option<StartOptions>,
    #[serde(default)]
    pub state_dir: Option<String>,
}

/// What one ctl request over the pipe got back.
enum Probe {
    /// Nothing listens on the pipe: no daemon.
    Unreached,
    /// A listener took the connection and gave no usable line in time: it
    /// closed, wrote something that is not JSON, or said nothing within
    /// [`HOLDER_PROBE_TIMEOUT`]. Something serves the pipe all the same.
    Silent,
    /// The one-line answer.
    Answered(serde_json::Value),
}

/// One ctl request and its one-line answer over the pipe at `sock`, bounded
/// by [`HOLDER_PROBE_TIMEOUT`].
async fn ctl_once(sock: &Path, request: &str) -> Probe {
    use tokio::io::AsyncBufReadExt;
    let Ok(name) = socket_name(sock) else {
        return Probe::Unreached;
    };
    let deadline = Instant::now() + HOLDER_PROBE_TIMEOUT;
    let stream = match tokio::time::timeout_at(deadline.into(), IpcStream::connect(name)).await {
        Ok(Ok(stream)) => stream,
        _ => return Probe::Unreached,
    };
    let exchange = async {
        let stream = Connection {
            stream,
            mcp_line_options: false,
        }
        .into_ctl()
        .await
        .ok()?;
        let (read, mut write) = tokio::io::split(stream);
        write.write_all(request.as_bytes()).await.ok()?;
        write.flush().await.ok()?;
        let mut line = String::new();
        tokio::io::BufReader::new(read)
            .read_line(&mut line)
            .await
            .ok()?;
        serde_json::from_str::<serde_json::Value>(line.trim()).ok()
    };
    match tokio::time::timeout_at(deadline.into(), exchange).await {
        Ok(Some(answer)) => Probe::Answered(answer),
        _ => Probe::Silent,
    }
}

/// The daemon on this state folder's pipe, asked: `holder` first, and for a
/// daemon older than 0.24.0, which does not know it, `status`. `None` when
/// nothing listens, which is "no daemon" whatever any record says.
///
/// A listener that answers neither with facts is still a daemon (a 0.23.1
/// daemon whose status report failed or is stuck behind a sync, or one
/// dying mid-exchange). Then the record says who it is, as before pipe-first
/// discovery, but only a record that names a live pid and no one-shot
/// command.
pub async fn ask_holder() -> Option<HolderFacts> {
    let sock = config::service_sock_path().ok()?;
    ask_holder_at(&sock).await
}

/// [`ask_holder`] for the pipe at `sock`.
pub(crate) async fn ask_holder_at(sock: &Path) -> Option<HolderFacts> {
    match ctl_once(sock, "{\"v\":1,\"cmd\":\"holder\"}\n").await {
        Probe::Unreached => return None,
        Probe::Answered(answer) if answer["ok"] == true => {
            if let Ok(facts) = serde_json::from_value(answer["data"].clone()) {
                return Some(facts);
            }
        }
        // A daemon that did not answer `holder` in time will not answer
        // `status` either, which takes its store.
        Probe::Silent => return facts_from_record(read_lock_info()),
        Probe::Answered(_) => {}
    }
    let record = read_lock_info();
    match ctl_once(sock, "{\"v\":1,\"cmd\":\"status\"}\n").await {
        // The daemon left between the two asks: nothing listens any more.
        Probe::Unreached => None,
        Probe::Answered(status)
            if status["ok"] == true
                && let Some(facts) = facts_from_status(&status["data"], record.as_ref()) =>
        {
            Some(facts)
        }
        _ => facts_from_record(record),
    }
}

/// The facts a record gives about the daemon that answers on the pipe, when
/// the daemon gave none itself: only from a record that names a live pid and
/// is no one-shot command's.
fn facts_from_record(record: Option<LockInfo>) -> Option<HolderFacts> {
    let record = record.filter(|r| r.standalone.is_none() && process_alive(r.pid))?;
    Some(HolderFacts {
        pid: record.pid,
        version: record.version,
        mcp_line_options: record.mcp_line_options,
        runs_in: record.runs_in,
        start: record.start,
        state_dir: None,
    })
}

/// The facts of a 0.23.1 daemon from its `status` answer. `status` carries
/// no handshake options and no start options, so those come from the record,
/// and only from a record that names the pid that answered: any other
/// record describes some other process.
pub(crate) fn facts_from_status(
    status: &serde_json::Value,
    record: Option<&LockInfo>,
) -> Option<HolderFacts> {
    let pid = u32::try_from(status["pid"].as_u64()?).ok()?;
    let version = status["version"].as_str()?.to_string();
    let same = record.filter(|r| r.pid == pid);
    Some(HolderFacts {
        pid,
        version,
        mcp_line_options: same.is_some_and(|r| r.mcp_line_options),
        runs_in: serde_json::from_value(status["runs_in"].clone()).unwrap_or(None),
        start: same.and_then(|r| r.start.clone()),
        state_dir: None,
    })
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
    match (
        Precedence::parse(daemon_version),
        Precedence::parse(own_version),
    ) {
        (Some(daemon), Some(own)) if daemon < own => AttachPolicy::Displace,
        _ => AttachPolicy::Attach,
    }
}

/// [`attach_policy`] with where this process runs. Inside an app package a
/// client never displaces: the daemon it would start in its place would run
/// from inside the package, with the package's private files, and die with
/// the app. It attaches to whatever runs, older or not.
pub fn attach_policy_for(
    daemon_version: &str,
    own_version: &str,
    here: &crate::runs_in::PackageContext,
) -> AttachPolicy {
    if here.is_packaged() {
        return AttachPolicy::Attach;
    }
    attach_policy(daemon_version, own_version)
}

/// The one line a packaged bridge writes when it attaches to an older
/// daemon. A relayed session has no `status` tool to say it, so it goes to
/// stderr, which Claude Desktop keeps in its MCP log.
pub(crate) fn older_daemon_warning(daemon: &str, own: &str) -> String {
    format!(
        "the Crystalline daemon is v{daemon} and this binary is v{own}: the update is \
         installed, but the old daemon still runs. Run `crystalline status` in a terminal \
         to restart it"
    )
}

/// Whether `candidate` is a strictly newer release than `baseline`. Same
/// precedence as [`attach_policy`]; an unparseable version on either side
/// is never newer, so an odd record can only ever read as a conflict, never as
/// an upgrade skew. The CLI uses it for skill upgrades too: a managed skill
/// whose first release is newer than an install's recorded version is added
/// even though it is missing on disk.
pub fn strictly_newer(candidate: &str, baseline: &str) -> bool {
    match (Precedence::parse(candidate), Precedence::parse(baseline)) {
        (Some(candidate), Some(baseline)) => candidate > baseline,
        _ => false,
    }
}

/// A version's place in semantic-versioning precedence: the numeric
/// `major.minor.patch` triple first, then the pre-release identifiers, with
/// build metadata ignored.
///
/// The pre-release part matters for the dev channel: two dev builds share a
/// triple (`0.21.0-dev.4817` and `0.21.0-dev.4820`), and comparing the triple
/// alone would read them as equal, so the newer one would attach to the older
/// daemon instead of taking over after an upgrade. A release outranks every
/// pre-release of its own triple, as semantic versioning orders them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Precedence {
    triple: (u64, u64, u64),
    /// Empty for a release. Compared identifier by identifier.
    pre: Vec<PreIdent>,
}

/// One dot-separated pre-release identifier. A numeric one sorts below an
/// alphanumeric one, numerics compare as numbers and the rest in ASCII order;
/// the derived order gets all three from the variant order and the payloads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PreIdent {
    Numeric(u64),
    Alpha(String),
}

impl Precedence {
    /// Parse a version string. Tolerates a two-part `major.minor`; `None`
    /// when the triple does not parse or a pre-release identifier is empty.
    fn parse(version: &str) -> Option<Self> {
        let version = version.trim();
        let without_build = version.split('+').next().unwrap_or(version);
        let (core, pre) = match without_build.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (without_build, None),
        };
        let mut parts = core.split('.');
        let major = parts.next()?.trim().parse().ok()?;
        let minor = parts.next()?.trim().parse().ok()?;
        let patch = parts.next().unwrap_or("0").trim().parse().ok()?;
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|ident| {
                    if ident.is_empty() {
                        None
                    } else if ident.bytes().all(|b| b.is_ascii_digit()) {
                        ident.parse().ok().map(PreIdent::Numeric)
                    } else {
                        Some(PreIdent::Alpha(ident.to_string()))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Self {
            triple: (major, minor, patch),
            pre,
        })
    }
}

impl Ord for Precedence {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.triple.cmp(&other.triple).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // Slice order goes identifier by identifier, and a shorter
                // list that is a prefix of the longer one sorts first, which
                // is the semantic-versioning rule for pre-release fields.
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

impl PartialOrd for Precedence {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// How long each stage of a displacement waits for the old daemon to leave.
struct DisplaceWaits {
    /// After the graceful `ctl shutdown` ask.
    ask: Duration,
    /// After the graceful signal (`SIGTERM`).
    term: Duration,
    /// After the hard signal (`SIGKILL`, `TerminateProcess` on Windows).
    kill: Duration,
}

/// The waits a real displacement uses. The ask gets twelve seconds: a daemon
/// of this version is gone inside its own [`crate::daemon::SHUTDOWN_DEADLINE`]
/// of it, and the rest of the window is OS teardown on a loaded machine. The
/// two signal steps take `dislodge_unresponsive`'s waits, since they are the
/// same escalation.
const DISPLACE_WAITS: DisplaceWaits = DisplaceWaits {
    ask: Duration::from_secs(12),
    term: DISLODGE_TERM_WAIT,
    kill: DISLODGE_KILL_WAIT,
};

/// Ask the daemon behind `sock` to shut down gracefully and wait for `pid` to
/// exit, escalating to signals when it does not. Returns true only once the
/// process is gone, meaning ownership is takeable. False means the process is
/// still there: it could not be verified as a Crystalline binary and so was
/// never signalled, or even the hard signal did not end it.
///
/// The escalation is what the 2026-09-23 incident was missing. A 0.18.1
/// daemon answered the ask, removed its record, socket and lock file, and then
/// stayed alive for minutes inside its runtime's drop, waiting for a model
/// download while it held the index. Waiting and then attaching as-is could not
/// end that, and every successor found the index held. This version's own
/// shutdown can no longer linger (see [`crate::daemon::Departure`]), but a
/// daemon being displaced is by definition an older one, and nothing but a
/// signal reaches it once it stops listening.
async fn displace(sock: &Path, pid: u32) -> bool {
    displace_within(sock, pid, &DISPLACE_WAITS).await
}

/// [`displace`] with its waits spelled out, so the tests need not sit through
/// the real ones.
async fn displace_within(sock: &Path, pid: u32, waits: &DisplaceWaits) -> bool {
    // A daemon that took the ask gets its window to leave on its own. One
    // that could not be asked at all - its socket already gone, which is the
    // lingering shape itself, or a ctl exchange that failed - goes straight
    // to the signals, whose first step is just as graceful for a daemon that
    // is still healthy.
    if ask_to_shut_down(sock).await && wait_until_gone(pid, waits.ask).await {
        return true;
    }
    if process_gone(pid) {
        return true;
    }
    tracing::warn!(
        "crystalline daemon pid {pid} is still running after the shutdown ask; stopping it"
    );
    escalate(pid, waits).await
}

/// Send the `ctl shutdown` ask and read the ack best-effort. Returns whether
/// the ask was delivered. The daemon exits promptly after the ack - it does
/// not drain active sessions, it cancels them, and bridges resync and answer
/// their orphaned requests with a retry error - so the caller's window
/// tolerates OS process teardown, not a session drain.
async fn ask_to_shut_down(sock: &Path) -> bool {
    let Ok(name) = socket_name(sock) else {
        return false;
    };
    let Ok(stream) = IpcStream::connect(name).await else {
        return false;
    };
    let Ok(mut stream) = (Connection {
        stream,
        mcp_line_options: false,
    })
    .into_ctl()
    .await
    else {
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
    let mut buf = [0u8; 256];
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await;
    true
}

/// Signal a displaced daemon that did not leave: the graceful signal first,
/// then the hard one, each followed by its wait. The identity gate
/// `dislodge_unresponsive` relies on is checked before each of the two,
/// because a pid is only a number and may have been reused by the time the
/// hard step comes: the process behind it has to be a Crystalline binary, and
/// never init or this process. Returns whether the pid is gone.
async fn escalate(pid: u32, waits: &DisplaceWaits) -> bool {
    for (hard, wait) in [(false, waits.term), (true, waits.kill)] {
        if process_gone(pid) {
            return true;
        }
        if pid <= 1 || pid == std::process::id() {
            tracing::warn!("pid {pid} is not a process this client may signal; leaving it alone");
            return false;
        }
        match process_exe_name(pid) {
            Some(name) if is_crystalline_exe_name(&name) => {}
            Some(name) => {
                tracing::warn!(
                    "pid {pid} is '{name}', not a Crystalline process; nothing was signalled"
                );
                return false;
            }
            None => {
                // A process that left between the two checks is what was
                // wanted; one that is there but unreadable is left alone.
                if process_gone(pid) {
                    return true;
                }
                tracing::warn!("what pid {pid} is could not be verified; nothing was signalled");
                return false;
            }
        }
        signal_process(pid, hard);
        if wait_until_gone(pid, wait).await {
            return true;
        }
    }
    tracing::warn!("crystalline daemon pid {pid} did not stop even after the hard signal");
    false
}

/// Poll until `pid` is gone or `budget` has passed.
async fn wait_until_gone(pid: u32, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if process_gone(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(DISLODGE_POLL).await;
    }
}

/// Whether `pid` no longer runs: it does not exist, or it has exited and only
/// its zombie is left for a parent that has not reaped it yet.
///
/// The zombie matters because of who the parent is. A daemon the Claude
/// Desktop extension started is a plain child of its bridge (it stays inside
/// the bridge's job so it cannot outlive Desktop, see `spawn_daemon`), and
/// that bridge never waits for it. Once such a daemon exits, the signal-0
/// probe [`process_alive`] uses still succeeds on the zombie until the bridge
/// itself goes, so a displacement would wait out every stage and report a
/// daemon that has released everything as still there. A zombie holds no file,
/// no lock and no socket, which is all a successor cares about.
fn process_gone(pid: u32) -> bool {
    !process_alive(pid) || is_zombie(pid)
}

/// Whether `pid` is a zombie: exited, not yet reaped. Only Linux and macOS are
/// asked; a Windows process that exited already reads as not alive, and any
/// other platform answers false.
fn is_zombie(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        // The state is the first field after the command name, which sits in
        // parentheses and may itself contain spaces or parentheses; the last
        // `)` is where it ends.
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state == "Z")
    }
    #[cfg(target_os = "macos")]
    {
        // libproc's `proc_pidinfo` refuses a zombie outright (both BSD info
        // flavours fail on one, which is how this was found), so the answer
        // comes from `sysctl(KERN_PROC_PID)` instead, which still describes a
        // zombie. The `libc` crate does not bind `struct kinfo_proc`, so the
        // one byte needed is read at its offset: `kp_proc.p_stat` sits at
        // byte 36 of the structure on every 64-bit macOS (a 16 byte union, two
        // pointers, an int flag, then the state), unchanged since the
        // structure was introduced.
        const P_STAT_OFFSET: usize = 36;
        let mut mib = [
            libc::CTL_KERN,
            libc::KERN_PROC,
            libc::KERN_PROC_PID,
            pid as libc::c_int,
        ];
        // `struct kinfo_proc` is 648 bytes on 64-bit macOS; the buffer is
        // generous so a larger future layout still fits.
        let mut buf = [0u8; 1024];
        let mut len = buf.len();
        // SAFETY: `mib` names four valid integers, `buf` is `len` writable
        // bytes and the kernel writes at most `len` of them, reporting how
        // many in `len`.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as libc::c_uint,
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        // An unknown pid answers success with nothing written.
        rc == 0 && len > P_STAT_OFFSET && u32::from(buf[P_STAT_OFFSET]) == libc::SZOMB
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        false
    }
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
    /// The lock is held by a one-shot command that published a record
    /// naming itself ([`LockInfo::standalone`]) and is still running. It
    /// serves no socket and leaves when it finishes: wait for it, and never
    /// signal it.
    Standalone {
        /// The command's pid.
        pid: u32,
        /// The command, for example `crystalline domain rename`.
        command: String,
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

/// [`lock_is_free`] for the service lock, over whatever path this process
/// resolves it to. `doctor`'s stale-lock check needs exactly this probe:
/// a lock file with no readable record (`read_lock_info` returns `None`) is
/// not on its own evidence that nobody holds it - the holder may simply have
/// published no record, or published one this reader cannot parse - so a
/// "stale" verdict has to rest on the OS lock itself, not on the record's
/// absence. Conservative on a path that cannot be resolved or a probe that
/// errors: `false`, so a caller never treats an unreadable state directory as
/// license to delete a file that might still be held.
pub fn service_lock_is_free() -> bool {
    let Ok(lock_path) = config::service_lock_path() else {
        return false;
    };
    lock_is_free(&lock_path).unwrap_or(false)
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
        let mut stream = Connection {
            stream,
            mcp_line_options: false,
        }
        .into_ctl()
        .await
        .ok()?;
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
        Some(String::from_utf8_lossy_owned(buf))
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

/// Whether `pid` is alive and its executable is a Crystalline binary. False
/// when either cannot be told.
pub fn process_is_crystalline(pid: u32) -> bool {
    process_alive(pid) && process_exe_name(pid).is_some_and(|name| is_crystalline_exe_name(&name))
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

    // A one-shot command serves no socket, so there is nothing to probe.
    if let Some(holder) = standalone_holder() {
        return holder;
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

/// The one-shot command holding the lock, when the record names one and its
/// pid is alive. Read from the record alone: the caller has already found
/// the lock held.
fn standalone_holder() -> Option<HolderState> {
    let info = read_lock_info()?;
    let command = info.standalone?;
    process_alive(info.pid).then_some(HolderState::Standalone {
        pid: info.pid,
        command,
    })
}

/// How long a starting daemon and a connecting client wait for a one-shot
/// command to let go of the state directory: as long as a rename waits for
/// the writes already running in its domain, its longest step.
const STANDALONE_WAIT: Duration = Duration::from_secs(30);

/// [`STANDALONE_WAIT`], or the test seam's value in milliseconds, so a test
/// of the timeout does not wait out the real one.
fn standalone_wait() -> Duration {
    std::env::var("CRYSTALLINE_TEST_STANDALONE_WAIT_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(STANDALONE_WAIT)
}

/// Why a daemon could not start while a one-shot command held the state
/// directory, naming the command and its pid.
fn standalone_holder_words(pid: u32, command: &str, waited: Duration) -> String {
    format!(
        "the standalone command `{command}` (pid {pid}) holds this machine's state directory, \
         and it did not let go within {} s. A daemon starts once it has finished: wait for it, \
         or stop it, and try again",
        waited.as_secs().max(1)
    )
}

/// Wait up to `budget` for a one-shot command to let go of the lock. `Ok`
/// once the lock is free or held by someone else (a daemon that just won
/// the race is the caller's to handle); the refusal naming the command when
/// it still holds the lock at the end.
async fn wait_for_standalone(pid: u32, command: &str, budget: Duration) -> anyhow::Result<()> {
    let Ok(lock_path) = config::service_lock_path() else {
        return Ok(());
    };
    let deadline = Instant::now() + budget;
    loop {
        let still_held = !lock_is_free(&lock_path).unwrap_or(false)
            && matches!(standalone_holder(), Some(HolderState::Standalone { pid: p, .. }) if p == pid);
        if !still_held {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(standalone_holder_words(pid, command, budget));
        }
        tokio::time::sleep(DISLODGE_POLL).await;
    }
}

/// [`acquire_ownership`] for a one-shot command: takes the lock and
/// publishes a record naming `command` (for example `crystalline domain
/// rename`), which dropping the ownership removes again. A record that
/// cannot be written costs only the name in someone else's message, so it
/// never fails the command.
pub fn acquire_standalone_ownership(command: &str) -> anyhow::Result<Ownership> {
    let ownership = acquire_ownership()?;
    if let Err(e) = ownership.publish_standalone(command) {
        tracing::debug!("the record naming this command could not be written: {e}");
    }
    Ok(ownership)
}

/// [`acquire_ownership`] for a starting daemon: when a one-shot command
/// holds the state directory, wait for it (bounded, and logged, so the wait
/// is visible in the daemon's log) instead of giving up after the usual
/// second. A holder that is not a one-shot command, or one that does not let
/// go in time, is a [`LockHeld`] refusal as before; the latter names the
/// command and its pid.
pub async fn acquire_ownership_after_standalone() -> anyhow::Result<Ownership> {
    let budget = standalone_wait();
    let deadline = Instant::now() + budget;
    let mut told = false;
    loop {
        let err = match acquire_ownership() {
            Ok(ownership) => return Ok(ownership),
            Err(err) => err,
        };
        if err.downcast_ref::<LockHeld>().is_none() {
            return Err(err);
        }
        let Some(HolderState::Standalone { pid, command }) = standalone_holder() else {
            return Err(err);
        };
        if Instant::now() >= deadline {
            return Err(anyhow::Error::new(LockHeld {
                message: standalone_holder_words(pid, &command, budget),
            }));
        }
        if !told {
            tracing::info!(
                "waiting up to {} s for the standalone command `{command}` (pid {pid}) to let go \
                 of this machine's state directory",
                budget.as_secs().max(1)
            );
            told = true;
        }
        tokio::time::sleep(DISLODGE_POLL).await;
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
        // A one-shot command leaves on its own and is never signalled.
        HolderState::Free | HolderState::Responsive | HolderState::Standalone { .. } => {
            Ok(DislodgeOutcome::NotNeeded)
        }
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

/// Attach to whatever daemon follows one this call displaced: another
/// client's respawn when one publishes inside the wait, else one this
/// call spawns. Bounded by the same 15 s readiness budget `ensure_daemon`
/// uses (a shorter one when the record says nothing, see
/// [`successor_plan`]); `None` only when no daemon became ready, and the
/// caller then falls back as before.
///
/// `displaced` is what the displaced daemon's record said about how it was
/// started, and the successor is started with exactly that (see
/// [`successor_plan`]): its `--db`, its config file, its read-only mode, its
/// HTTP flags and its environment variable names. Nothing of the displacing
/// command itself reaches the successor: an explicit `--db` or `--config`
/// never reaches this path (`use_daemon` sends such a command to the index
/// directly), and a `CRYSTALLINE_*` variable the predecessor did not apply is
/// removed from the successor's environment. A daemon whose start cannot be
/// reproduced here, or whose record does not say how it was started, gets no
/// successor from this call; the wait still attaches to one another client
/// starts.
///
/// The spawn-and-poll part is [`ensure_daemon_with`] itself, so a successor
/// another client already spawned wins the ownership race and this call's
/// own spawn exits at once (the 2026-07-28 wedge logic there covers that).
pub async fn attach_after_displacement(displaced: Option<StartOptions>) -> Option<Connection> {
    let current = crate::overlay::EnvOverlay::from_process_env()
        .map(|overlay| overlay_variable_names(&overlay));
    let plan = successor_plan(
        displaced.as_ref(),
        current.as_ref().map(Vec::as_slice),
        |name| std::env::var_os(name).is_some_and(|value| !value.is_empty()),
        std::env::current_exe().is_ok_and(|exe| in_desktop_extension_folder(&exe)),
    );
    let options = match plan {
        SuccessorPlan::Spawn(options) => options,
        SuccessorPlan::Wait { why, budget } => {
            tracing::warn!(
                "not starting a successor for the displaced daemon: {why}; waiting for another client to start one"
            );
            return wait_for_successor(budget).await;
        }
    };
    match ensure_daemon_with(true, &options).await {
        Ok(conn) => Some(conn),
        Err(e) => {
            tracing::warn!("no daemon became ready after displacing an older one: {e:#}");
            None
        }
    }
}

/// What a displacing client does about the successor.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SuccessorPlan {
    /// Start one with these options.
    Spawn(SpawnOptions),
    /// Start none and only wait up to `budget` for another client's, for
    /// the reason `why`.
    Wait { why: String, budget: Duration },
}

/// How long a displacing client waits for another client's successor when it
/// starts none itself because the record could not be reproduced here: the
/// same window [`ensure_daemon_with`] gives its own spawn.
const SUCCESSOR_WAIT: Duration = Duration::from_secs(15);

/// How long it waits when the displaced daemon's record says nothing about
/// how it was started (a daemon older than 0.22.1). Long enough for a
/// connected bridge, which reconnects the moment its socket closes
/// (`pump_stdio` in the client), to start the successor with its own flags;
/// short, because with no bridge connected nobody will, and the command then
/// answers on its own.
const UNRECORDED_SUCCESSOR_WAIT: Duration = Duration::from_secs(3);

/// Decide how to start the successor of a displaced daemon whose record
/// said `displaced`. `current` is the list of overlay variables this process
/// applies itself (an error when its environment does not parse, which the
/// successor would fail on too), and `has_var` asks whether a variable is set
/// here.
///
/// - No recorded options (a record from 0.22.0 or older): no successor, and
///   only a short wait ([`UNRECORDED_SUCCESSOR_WAIT`]). Nobody can tell from
///   such a record whether the daemon ran read-only or on another index, so a
///   successor with the defaults could hand the next bridge a writable daemon
///   on the wrong index (the I2 finding of 0.22.1). A connected bridge starts
///   the successor itself with its own flags inside the wait, the existing
///   "first spawner decides" rule; with none connected the command answers on
///   its own, as before 0.22.1.
/// - A recorded variable this process does not have, a path the record could
///   not hold, or an environment that does not parse: no successor. The
///   database, a domain or the read-only switch may have come from that
///   variable, and a successor without it would serve something else.
/// - Otherwise the recorded options, with every overlay variable of this
///   process that the predecessor did not apply removed. The bounded life
///   (`exit_when_idle`) is replayed only when `in_extension` says this binary
///   itself lies inside a Claude Desktop extension folder: it belongs to where
///   the binary lives, not to the record.
pub(crate) fn successor_plan(
    displaced: Option<&StartOptions>,
    current: Result<&[String], &crate::overlay::OverlayError>,
    has_var: impl Fn(&str) -> bool,
    in_extension: bool,
) -> SuccessorPlan {
    let wait = |why: String| SuccessorPlan::Wait {
        why,
        budget: SUCCESSOR_WAIT,
    };
    let Some(start) = displaced else {
        return SuccessorPlan::Wait {
            why: "its record does not say how it was started".to_string(),
            budget: UNRECORDED_SUCCESSOR_WAIT,
        };
    };
    if start.partial {
        return wait("its record could not hold one of its paths".to_string());
    }
    if let Some(missing) = start.env.iter().find(|name| !has_var(name)) {
        return wait(format!(
            "it read {missing} from its environment and that variable is not set here"
        ));
    }
    let current = match current {
        Ok(current) => current,
        Err(e) => return wait(format!("this environment does not parse ({e})")),
    };
    SuccessorPlan::Spawn(SpawnOptions {
        db: start.db.as_ref().map(PathBuf::from),
        config: start.config.as_ref().map(PathBuf::from),
        read_only: start.read_only,
        http: start.http.clone(),
        allowed_hosts: start.allowed_hosts.clone(),
        exit_when_idle: start.exit_when_idle && in_extension,
        env_remove: current
            .iter()
            .filter(|name| !start.env.contains(name))
            .cloned()
            .collect(),
    })
}

/// Whether `exe` lies inside a Claude Desktop extension folder: some folder
/// on its path is named `Claude Extensions` and its parent `Claude`, which
/// holds on macOS (`~/Library/Application Support/Claude/Claude
/// Extensions/...`) and on Windows (`%APPDATA%\Claude\Claude Extensions\...`,
/// and the package's physical `...\LocalCache\Roaming\Claude\Claude
/// Extensions\...`). A daemon started from such a binary keeps that file in
/// use, so it stays attached and exits when idle (the 0.18.2 mode). Read from
/// the path text, split on both separators, ASCII case ignored.
pub fn in_desktop_extension_folder(exe: &Path) -> bool {
    let text = exe.to_string_lossy();
    let parts: Vec<&str> = text.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    parts.windows(2).any(|w| {
        w[0].eq_ignore_ascii_case("Claude") && w[1].eq_ignore_ascii_case("Claude Extensions")
    })
}

/// Poll for a daemon another client starts, for up to `budget`, without
/// spawning one.
async fn wait_for_successor(budget: Duration) -> Option<Connection> {
    let deadline = Instant::now() + budget;
    loop {
        if let Some(conn) = try_attach().await {
            return Some(conn);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// How [`spawn_daemon`] starts a daemon. `ensure_daemon` fills the first
/// three from a client's own flags; a displacement fills all of them from
/// the displaced daemon's [`StartOptions`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SpawnOptions {
    /// The global `--db`.
    pub db: Option<PathBuf>,
    /// `--config`.
    pub config: Option<PathBuf>,
    /// `--read-only`.
    pub read_only: bool,
    /// `--http`.
    pub http: Option<String>,
    /// `--allowed-host`, once per value.
    pub allowed_hosts: Vec<String>,
    /// `--exit-when-idle`, on top of the extension's own.
    pub exit_when_idle: bool,
    /// Variables removed from the daemon's environment.
    pub env_remove: Vec<String>,
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
    let options = SpawnOptions {
        db: db.map(Path::to_path_buf),
        config: config_path.map(Path::to_path_buf),
        read_only,
        ..SpawnOptions::default()
    };
    if !spawn {
        return ensure_daemon_with(false, &options).await;
    }
    let task = crate::daemon_task::for_this_process();
    ensure_daemon_in(
        crate::runs_in::PackageContext::here(),
        &*task,
        &options,
        PACKAGED_TASK_WAIT,
    )
    .await
}

/// How long a packaged bridge waits for the daemon the task starts: the same
/// window a spawned daemon gets.
pub const PACKAGED_TASK_WAIT: Duration = Duration::from_secs(15);

/// Attach, or start a daemon, as `here` allows. Outside a package this is
/// [`ensure_daemon_with`]. Inside one the process never spawns, never takes
/// the index and writes nothing: it runs `task`, waits up to `wait` for the
/// daemon to answer on the pipe, and otherwise fails with the
/// [`crate::daemon_task::BridgeFailure`] that says why.
pub(crate) async fn ensure_daemon_in(
    here: &crate::runs_in::PackageContext,
    task: &dyn crate::daemon_task::DaemonTask,
    options: &SpawnOptions,
    wait: Duration,
) -> anyhow::Result<Connection> {
    use crate::daemon_task::BridgeFailure;
    if !here.is_packaged() {
        return ensure_daemon_with(true, options).await;
    }
    if let (Some(conn), _) = try_attach_displacing_in(here).await {
        return Ok(conn);
    }
    let Some(name) = task.find() else {
        return Err(BridgeFailure::TaskMissing.into());
    };
    // `schtasks /Run` returns at once; the brief block before any session
    // exists is cheaper than a blocking-thread hop.
    task.run(&name)
        .map_err(|detail| BridgeFailure::TaskDidNotStart {
            task: name.clone(),
            detail,
        })?;
    let deadline = Instant::now() + wait;
    loop {
        if let (Some(conn), _) = try_attach_displacing_in(here).await {
            return Ok(conn);
        }
        if Instant::now() >= deadline {
            return Err(BridgeFailure::NoAnswer { task: name }.into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// [`ensure_daemon`] with every option a spawned daemon can be given.
pub(crate) async fn ensure_daemon_with(
    spawn: bool,
    options: &SpawnOptions,
) -> anyhow::Result<Connection> {
    if let Some(conn) = try_attach().await {
        return Ok(conn);
    }
    if !spawn {
        anyhow::bail!("no Crystalline daemon is running; start one with `crystalline serve`");
    }

    // The daemon we are about to spawn is detached, so its own bind failure
    // only ever reaches `daemon.log`. Asked here as well as there, the person
    // running the command is told what is wrong instead of waiting out the 15s
    // readiness budget for a daemon that could never have bound.
    #[cfg(unix)]
    if let Ok(sock) = config::service_sock_path() {
        check_socket_path(&sock)?;
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
        // A one-shot command leaves on its own and is never signalled. The
        // daemon spawned below would wait for it too, but longer than the
        // readiness poll does, so the wait happens here, before the spawn.
        HolderState::Standalone { pid, command } => {
            wait_for_standalone(pid, &command, standalone_wait()).await?;
        }
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

    spawn_daemon(options)?;
    // Poll readiness: lock record present and socket connectable. Another
    // client's lingering old-binary bridge can be reconnecting during this
    // same takeover window: it reads the empty lock this call's displacement
    // (if any) left behind, spawns a daemon from its own old binary and that
    // daemon can win the version-blind `acquire_ownership` race before this
    // call's own spawn lands. `try_attach_reporting` surfaces an in-poll
    // displacement so this loop re-drives `spawn_daemon` instead of waiting
    // out the budget behind a daemon it just tore down again; bounded to 3
    // re-spawns so a pathological interleaving of respawning bridges cannot
    // spawn-storm within the 15s budget. The budget is a deadline, not a
    // count of turns: one attach asks the pipe first, and against a daemon
    // that answers slowly that ask alone can take seconds.
    let mut respawns = 0u32;
    let deadline = Instant::now() + READINESS_BUDGET;
    while Instant::now() < deadline {
        let (conn, displaced) = try_attach_reporting().await;
        if let Some(conn) = conn {
            return Ok(conn);
        }
        if displaced && respawns < 3 {
            respawns += 1;
            spawn_daemon(options)?;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // A holder we refused to touch is the better explanation of the timeout
    // than the generic one: it names the lock file and how to look at whoever
    // owns it, instead of pointing at a daemon log the spawn never reached.
    if let Some(e) = unknown_holder {
        return Err(e);
    }
    // A one-shot command that took the state directory after the check
    // above: the daemon spawned for this call is still waiting for it, and
    // this is what it waits for.
    if let Some(HolderState::Standalone { pid, command }) = standalone_holder() {
        anyhow::bail!(
            "the standalone command `{command}` (pid {pid}) took this machine's state directory \
             while a daemon was starting, and the daemon was not ready after 15 s of waiting. \
             It starts once the command has finished: wait for it, or stop it, and try again"
        );
    }
    anyhow::bail!(
        "spawned a daemon but it did not become ready within 15s (see daemon.log in the state directory)"
    )
}

/// How long `ensure_daemon` waits for the daemon it spawned to answer.
const READINESS_BUDGET: Duration = Duration::from_secs(15);

/// The daemon log, opened for appending, and started over once it outgrows
/// 1 MiB. The cap is checked when the file is opened (at spawn time, or when
/// a task-started daemon starts) and the reset is best-effort (a live holder
/// can defeat the removal on Windows), so it bounds growth across starts,
/// not within one daemon's lifetime. `None` when the state dir or the file
/// cannot be prepared: logging must never be the reason a daemon fails to
/// start.
pub(crate) fn daemon_log_file() -> Option<File> {
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
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
}

/// The spawned daemon's stderr: [`daemon_log_file`], or a null stderr when
/// it cannot be opened.
fn daemon_log_sink() -> Option<std::process::Stdio> {
    daemon_log_file().map(Into::into)
}

/// Spawn `current_exe serve --daemon`, forwarding `--read-only` when this
/// instance was asked to serve read-only.
///
/// Off the Claude Desktop extension the daemon is fully detached (its own
/// session on unix, a breakaway from the parent's job on Windows) and outlives
/// every client, by design: it serves the user's state directory and the web
/// UI to whoever comes next. When this binary lies inside a Claude Desktop
/// extension folder ([`in_desktop_extension_folder`]) it is neither:
/// `current_exe` is then the in-place binary inside Claude Desktop's
/// own extension folder, and a detached daemon running from it kept that file
/// locked past Desktop's teardown, which on Windows blocked the packaged host
/// from updating or relaunching (2026-09-18). So the extension's daemon stays
/// in the stub's process group and job, where Desktop's own teardown reaches
/// it, and carries `--exit-when-idle`, which ends it on its own once its last
/// client is gone even when that teardown never comes.
///
/// No `--http off` is passed of this call's own accord and none ever should be
/// (the one `--http` it passes is a displaced daemon's own flag, replayed for
/// its successor, see [`attach_after_displacement`]): a daemon started this way
/// (an agent's `crystalline mcp` connection, the Desktop extension) serves the
/// HTTP endpoint on 127.0.0.1:7411 by exactly the same default a hand-started
/// `crystalline serve` does. The daemon is a singleton, so an autostarted one
/// that skipped HTTP would leave the web UI dead for the most common population
/// of all, with no way to get it back short of shutting the daemon down. The one
/// coherent opt-out is `service.http=false`, which turns the endpoint off for
/// every daemon however it started; the spawned process inherits this one's
/// environment, so `CRYSTALLINE_SERVICE_HTTP` reaches it too.
///
/// The daemon works in the state directory on every platform and channel,
/// never in the directory of whoever started it (#115: Windows locks a
/// running process's working directory, and a client started inside another
/// program's folder made the daemon hold that folder). `--db`, `--config` and
/// a relative `CRYSTALLINE_CONFIG` are made absolute first, so they still name
/// the files the client meant.
fn spawn_daemon(options: &SpawnOptions) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let extension = in_desktop_extension_folder(&exe);
    // The daemon works in the state directory (below), so every path it is
    // handed has to name the same file from there as it does here.
    let options = SpawnOptions {
        db: options.db.as_deref().map(absolute_for_daemon),
        config: options.config.as_deref().map(absolute_for_daemon),
        ..options.clone()
    };
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(daemon_args(&options, extension));
    for name in &options.env_remove {
        cmd.env_remove(name);
    }
    if let Some(value) = std::env::var_os(crate::overlay::CONFIG_PATH_ENV)
        && let Some(absolute) = config_env_for_daemon(&value)
    {
        cmd.env(crate::overlay::CONFIG_PATH_ENV, absolute);
    }
    // Never the client's directory: a running process's working directory is
    // locked on Windows, and the client may have been started inside another
    // program's folder (#115). The state directory is the one folder the
    // daemon holds anyway.
    match daemon_working_dir(config::state_dir().map_err(anyhow::Error::from)) {
        Ok(dir) => {
            cmd.current_dir(dir);
        }
        Err(e) => tracing::warn!(
            "the daemon keeps this process's working directory: the state directory could not be prepared ({e})"
        ),
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(daemon_log_sink().unwrap_or_else(std::process::Stdio::null));
    #[cfg(unix)]
    if !extension {
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
        // No console window for the daemon and its own process group. Off the
        // extension, also a breakaway from the parent's job object so it
        // outlives a harness that kills its job on exit; a job that forbids
        // breakaway fails the spawn outright, so retry inside the job:
        // starting at all beats outliving the parent. A refusal is said out
        // loud and handed to the daemon, which reports it. The extension's
        // daemon stays inside the job on purpose (see above).
        if !extension {
            cmd.creation_flags(
                CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
            );
            match cmd.spawn() {
                Ok(_) => return Ok(()),
                Err(e) if breakaway_refusal(&e) => {
                    if let RefusedBreakaway::StartedByTask(name) =
                        after_refused_breakaway(&*crate::daemon_task::for_this_process(), &options)
                    {
                        tracing::info!(
                            "Windows refused the breakaway ({e}); the task {name} started the daemon instead"
                        );
                        return Ok(());
                    }
                    tracing::warn!(
                        "Windows refused the breakaway ({e}); {}",
                        crate::runs_in::BREAKAWAY_REFUSED_WARNING
                    );
                    cmd.arg(crate::runs_in::BREAKAWAY_REFUSED_FLAG);
                }
                Err(e) => {
                    tracing::debug!("the breakaway spawn failed ({e}); retrying inside the job")
                }
            }
        }
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        cmd.spawn()?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        cmd.spawn()?;
        Ok(())
    }
}

/// The spawned daemon's arguments, in the order the CLI parses them: the
/// global `--db` ahead of the subcommand, `--config` after it. `--autostarted`
/// tells the child it is an autostart rather than an invocation somebody made
/// (a hidden flag, not `--daemon`, which an operator passes by hand too, and
/// not an environment variable, which a shell could leave set and mislabel a
/// daemon somebody started deliberately). `--exit-when-idle` is the extension's
/// bounded lifetime, see [`spawn_daemon`], and also the successor's of a
/// displaced daemon that had it. `--http` and `--allowed-host` are passed only
/// when a displaced daemon's record says it was started with them.
fn daemon_args(options: &SpawnOptions, extension: bool) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    if let Some(db) = &options.db {
        args.push("--db".into());
        args.push(db.into());
    }
    args.push("serve".into());
    args.push("--daemon".into());
    args.push("--autostarted".into());
    if extension || options.exit_when_idle {
        args.push("--exit-when-idle".into());
    }
    if options.read_only {
        args.push("--read-only".into());
    }
    if let Some(http) = &options.http {
        args.push("--http".into());
        args.push(http.into());
    }
    for host in &options.allowed_hosts {
        args.push("--allowed-host".into());
        args.push(host.into());
    }
    if let Some(cfg) = &options.config {
        args.push("--config".into());
        args.push(cfg.into());
    }
    args
}

/// A path the spawned daemon will open, made absolute against this process's
/// working directory: the daemon works in the state directory, where a
/// relative path would name another file. Only an empty path makes
/// `std::path::absolute` fail, and that one is passed on as given.
fn absolute_for_daemon(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The value `CRYSTALLINE_CONFIG` has to carry into the daemon: its absolute
/// form when the inherited value is relative once tilde-expanded (the
/// overlay expands it the same way), `None` when it already names the same
/// file from any directory or is empty (which the overlay reads as unset).
fn config_env_for_daemon(value: &std::ffi::OsStr) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    // Tilde expansion needs text; a value that is not UTF-8 is kept as it is,
    // so the daemon never gets a different file name.
    let expanded = match value.to_str() {
        Some(text) => config::expand_tilde(text),
        None => PathBuf::from(value),
    };
    if expanded.is_absolute() {
        return None;
    }
    Some(absolute_for_daemon(&expanded))
}

/// The directory the spawned daemon works in: the state directory, created
/// first. Never the exe's directory as a fallback: on the Claude Desktop
/// extension that is Desktop's own extension folder, the one thing the
/// daemon must not hold.
fn daemon_working_dir(state_dir: anyhow::Result<PathBuf>) -> anyhow::Result<PathBuf> {
    let dir = state_dir?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Whether a failed spawn with `CREATE_BREAKAWAY_FROM_JOB` is Windows
/// refusing the breakaway: the job does not allow it and the call fails with
/// `ERROR_ACCESS_DENIED`.
#[cfg(any(windows, test))]
fn breakaway_refusal(e: &io::Error) -> bool {
    e.raw_os_error() == Some(5)
}

/// What a spawner does after Windows refused the breakaway: a daemon inside
/// the job dies with the program that owns the job, so the task starts it
/// instead whenever one is registered and runs, and `options` ask for
/// nothing the task cannot pass on.
#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RefusedBreakaway {
    StartedByTask(String),
    InsideTheJob,
}

/// Whether the task may start the daemon in place of a spawn with
/// `options` (after their paths are made absolute). The task always runs a
/// plain `serve --daemon --from-task`, so a spawn that asks for a database,
/// a config file, read-only mode, an HTTP address, an allowed host, a
/// bounded life or a cleaned environment keeps its own spawn: otherwise the
/// client would attach to a daemon that serves something else.
#[cfg(any(windows, test))]
fn the_task_can_stand_in(options: &SpawnOptions) -> bool {
    *options == SpawnOptions::default()
}

#[cfg(any(windows, test))]
pub(crate) fn after_refused_breakaway(
    task: &dyn crate::daemon_task::DaemonTask,
    options: &SpawnOptions,
) -> RefusedBreakaway {
    if !the_task_can_stand_in(options) {
        return RefusedBreakaway::InsideTheJob;
    }
    match task.find() {
        Some(name) if task.run(&name).is_ok() => RefusedBreakaway::StartedByTask(name),
        _ => RefusedBreakaway::InsideTheJob,
    }
}

/// The exit code a `crystalline serve` uses when it could not take the index
/// lock, distinct from every other startup failure.
///
/// 3 because 0 and 1 are the ordinary success and failure of every command
/// here, 2 is clap's usage error and `crystalline verify`'s scan failure, and
/// anything from 126 up belongs to the shell. An operator writes it into a
/// unit file as `RestartPreventExitStatus=3`, which is why the code has to
/// exist in Crystalline rather than being left to the deployment: a restart
/// loop on a lock that is not going to free itself produces one identical log
/// line per attempt and no new information.
pub const EXIT_LOCK_HELD: i32 = 3;

/// A `serve` that could not take the index lock. Carried as a typed error so
/// the CLI exits on [`EXIT_LOCK_HELD`] by downcasting rather than by matching
/// message text.
#[derive(Debug)]
pub struct LockHeld {
    pub message: String,
}

impl std::fmt::Display for LockHeld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LockHeld {}

/// What a losing `serve` prints: what this invocation asked to bind, what the
/// holder's record says it bound, and the configuration key that makes the two
/// agree.
///
/// Pure over its two inputs so every shape is testable without a daemon. The
/// old message named only the holder's pid, which is true and useless when the
/// consequence is that the endpoint an operator configured no longer exists.
///
/// Both sides are *recorded intent*, never a probed listener: this process
/// wrote its own before it touched the lock, and the holder wrote its own the
/// same way. So the wording stays with what was asked for and what the record
/// says, and a holder that recorded nothing is reported as having recorded
/// nothing rather than as an older version or as an endpoint that is off.
///
/// The exposure half is earned rather than always printed. A caller that
/// recorded no intent at all asked to bind nothing, and one whose exposure
/// already matches the holder's has nothing to reconcile; both get the shorter
/// message, because advice that does not apply is noise wherever this string
/// travels - and it travels into an agent's `status` payload.
pub fn lock_held_message(intent: Option<&ServeIntent>, holder: Option<&LockInfo>) -> String {
    let asked = match intent.map(|i| &i.http) {
        Some(HttpBinding::Bound(addr)) => format!("this serve asked to bind {addr}"),
        Some(HttpBinding::Off) => {
            "this serve asked for the index with no HTTP endpoint".to_string()
        }
        Some(HttpBinding::Unrecorded) | None => "this process asked for the index".to_string(),
    };
    let hosts_asked = match intent {
        Some(i) if !i.allowed_hosts.is_empty() => {
            format!(", accepting the Host values {}", i.allowed_hosts.join(", "))
        }
        _ => String::new(),
    };
    let held = match holder {
        Some(h) if h.standalone.is_some() => format!(
            "the standalone command `{}` (pid {}) holds it until it finishes",
            h.standalone.as_deref().unwrap_or_default(),
            h.pid
        ),
        Some(h) => {
            let started = match h.started_by {
                Some(mode) => format!(", started by {}", mode.as_str()),
                None => ", which did not record how it started".to_string(),
            };
            let bound = match &h.http {
                HttpBinding::Bound(addr) => format!(", and its record says it bound {addr}"),
                HttpBinding::Off => ", and its record says it bound no HTTP endpoint".to_string(),
                HttpBinding::Unrecorded => ", and it did not record what it bound".to_string(),
            };
            format!(
                "another Crystalline instance already owns it: pid {}, v{}{started}{bound}",
                h.pid, h.version
            )
        }
        None => "another process already owns it and published no service record, so neither its \
                 pid nor its address can be read from here"
            .to_string(),
    };
    // Two callers record no intent: the embedded MCP stack and the `hold-lock`
    // test command. Neither asked to bind anything, so exposure advice would be
    // a non sequitur - and the embedded stack copies this string into the
    // `status` payload and, in the no-record case, the `instructions` an agent
    // reads. What applies to that caller is the holder itself: it is a daemon
    // this process can talk to.
    let Some(intent) = intent else {
        return format!(
            "{asked}{hosts_asked}, but {held}. Only one daemon can own the index. Stop the holder \
             with crystalline ctl shutdown, or attach over the socket."
        );
    };
    // Exposure advice is for two sides that disagree. When this invocation and
    // the holder asked for the same binding and the same allow-list, writing
    // that down changes nothing, so the message says only that the index is
    // owned and by whom. An unrecorded binding on either side is ignorance
    // rather than agreement, and keeps the advice.
    let agreed = holder.is_some_and(|h| {
        intent.http != HttpBinding::Unrecorded
            && h.http != HttpBinding::Unrecorded
            && intent.http == h.http
            && intent.allowed_hosts == h.allowed_hosts
    });
    let mut remedy =
        String::from("Only one daemon can own the index, so this invocation is serving nothing.");
    if agreed {
        remedy.push_str(" Stop the holder with crystalline ctl shutdown and start again.");
        return format!("{asked}{hosts_asked}, but {held}. {remedy}");
    }
    remedy.push_str(
        " Exposure belongs to configuration, where every daemon on this machine reads it however \
         it was started",
    );
    match &intent.http {
        HttpBinding::Bound(addr) => {
            remedy.push_str(&format!(": crystalline config set service.http {addr}"));
        }
        // An invocation that asked for no endpoint is reconciled by writing
        // that down, not by being told to invent a host and port.
        HttpBinding::Off => remedy.push_str(": crystalline config set service.http false"),
        HttpBinding::Unrecorded => {
            remedy.push_str(": crystalline config set service.http <host:port>");
        }
    }
    if !intent.allowed_hosts.is_empty() {
        remedy.push_str(&format!(
            " and crystalline config set service.allowed_hosts {}",
            intent.allowed_hosts.join(",")
        ));
    }
    remedy.push_str(". Then stop the holder with crystalline ctl shutdown and start again.");
    format!("{asked}{hosts_asked}, but {held}. {remedy}")
}

/// The sentence an overridden command owes for taking the direct path:
/// `--db` or `--config` named an exact config and index the daemon may not
/// serve, so it read the file itself rather than asking. `status` prints this
/// on stdout as its report (behind the `Daemon: ` label every one of its
/// notes carries); every standalone fallback in [`crate::client`] prints it
/// on stderr instead, since an empty answer from the wrong index must never
/// read as a genuine miss. One constant so the two crates say it the same
/// way.
pub const BYPASS_NOTE: &str = "bypassed (--db/--config override); reading the index directly";

/// Why the index could not be reached, in words a person can act on, with the
/// holder looked up here.
///
/// A raw lock error names no daemon and no remedy, which is how a colleague's
/// agent spent a session on the wrong diagnosis. One composer for the whole
/// tool: the CLI's own opener reaches it through `cmd::reach_index`, and every
/// standalone fallback in [`crate::client`] reaches it too, so a person meets
/// the same sentence wherever they meet the same state. It lives beside
/// [`lock_held_message`] because both answer "somebody else has the index" and
/// both must keep saying it the same way.
pub fn index_unreachable_words(location: &str, error: &str, bypassed: bool) -> String {
    let info = read_lock_info().filter(|info| process_alive(info.pid));
    // A one-shot command is no daemon to ask or to stop: it lets go when it
    // finishes.
    if let Some(info) = &info
        && let Some(command) = &info.standalone
        && !crystalline_index::is_schema_too_new_text(error)
    {
        return format!(
            "the standalone command `{command}` (pid {}) holds the index at {location} right now; run this again once it has finished. The index reported: {error}",
            info.pid
        );
    }
    words_for_holder(info.map(|info| info.pid), location, error, bypassed)
}

/// [`index_unreachable_words`] with the holder already looked up, so the three
/// sentences can be read back in a test without a daemon on the machine.
///
/// `bypassed` is true when `--db` or `--config` told the command to read a
/// file directly rather than ask the daemon, which changes the remedy: the
/// cheapest fix there is to stop reaching past the holder.
pub fn words_for_holder(
    holder: Option<u32>,
    location: &str,
    error: &str,
    bypassed: bool,
) -> String {
    // An index newer than this binary is not a holder problem, and the lock
    // advice below would send a person looking for a process that is not
    // there. Its own message names the remedy, so it stands alone.
    if crystalline_index::is_schema_too_new_text(error) {
        return format!("the index at {location} cannot be used by this binary: {error}");
    }
    match (holder, bypassed) {
        (Some(pid), true) => format!(
            "the running Crystalline daemon (pid {pid}) owns the index at {location}, and --db or --config told this command to read that file directly instead of asking the daemon. Run it again without --db and --config so the daemon answers, or stop the daemon first with: crystalline ctl shutdown. The index reported: {error}"
        ),
        (Some(pid), false) => format!(
            "the running Crystalline daemon (pid {pid}) owns the index at {location} and did not answer this command. Look at it with: crystalline doctor --fix, or stop it with: crystalline ctl shutdown and run this again. The index reported: {error}"
        ),
        (None, _) => format!(
            "the index at {location} could not be opened, and no Crystalline daemon is running to ask instead. Check that the file is readable and that no other process is holding it; crystalline doctor --fix clears a lock or socket file a killed daemon left behind. The index reported: {error}"
        ),
    }
}

/// Acquire ownership of the index by taking the advisory lock, with stale
/// takeover: a `kill -9`d predecessor's lock is already free, so a short retry
/// loop simply succeeds. When a daemon is up, the error is a [`LockHeld`]
/// carrying [`lock_held_message`]: what this invocation asked to bind, what
/// the holder's record says it bound, and the key that reconciles them.
pub fn acquire_ownership() -> anyhow::Result<Ownership> {
    let dir = config::state_dir()?;
    std::fs::create_dir_all(&dir)?;
    let lock_path = config::service_lock_path()?;
    let info_path = config::service_info_path()?;
    let socket_path = config::service_sock_path()?;

    let mut acquired = None;
    for attempt in 0..20 {
        // Opened afresh on every attempt: a file an owner removed while this
        // process waited is not the one the next owner locks.
        let opened = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path);
        let file = match opened {
            Ok(file) => file,
            // A lock file its owner has just removed stays pending on Windows
            // until every handle to it is closed, and opening or creating it
            // is refused meanwhile: wait and try again, as for a held lock.
            Err(e)
                if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied && attempt < 19 =>
            {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if FileExt::try_lock(&file).is_ok() {
            if locked_file_is_current(&file, &lock_path) {
                acquired = Some(file);
                break;
            }
            let _ = FileExt::unlock(&file);
            tracing::debug!(
                "the service.lock this process locked was removed meanwhile; opening it again"
            );
        }
        if attempt < 19 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let Some(file) = acquired else {
        // Composed here, from the intent this process recorded before it
        // reached the lock, so the two other callers - the embedded MCP stack
        // and the `hold-lock` test command - get the no-intent wording and
        // keep today's exit behaviour. Typed, so the CLI can exit on
        // [`EXIT_LOCK_HELD`] without matching message text.
        return Err(anyhow::Error::new(LockHeld {
            message: lock_held_message(serve_intent(), read_lock_info().as_ref()),
        }));
    };

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

    /// Only the two bind wildcards are rewritten. Every other spelling is an
    /// address something can really dial, and rewriting one would send a
    /// client somewhere it was never pointed at.
    #[test]
    fn loopback_connect_addr_rewrites_only_the_wildcards() {
        assert_eq!(loopback_connect_addr("0.0.0.0:7411"), "127.0.0.1:7411");
        assert_eq!(loopback_connect_addr("[::]:7411"), "127.0.0.1:7411");
        assert_eq!(loopback_connect_addr("127.0.0.1:7411"), "127.0.0.1:7411");
        assert_eq!(loopback_connect_addr("[::1]:7411"), "[::1]:7411");
        assert_eq!(
            loopback_connect_addr("192.168.1.5:7411"),
            "192.168.1.5:7411"
        );
        assert_eq!(
            loopback_connect_addr("fluid.example:7411"),
            "fluid.example:7411"
        );
    }

    #[test]
    fn the_extension_folder_is_found_on_both_platforms() {
        for inside in [
            "/Users/ada/Library/Application Support/Claude/Claude Extensions/local.mcpb.jordi-boehme.crystalline/server/crystalline",
            r"C:\Users\ada\AppData\Roaming\Claude\Claude Extensions\local.mcpb.jordi-boehme.crystalline\server\crystalline.exe",
            r"C:\Users\ada\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude\Claude Extensions\x\server\crystalline.exe",
            r"c:\users\ada\appdata\roaming\claude\claude extensions\x\crystalline.exe",
        ] {
            assert!(in_desktop_extension_folder(Path::new(inside)), "{inside}");
        }
        for outside in [
            "/opt/homebrew/bin/crystalline",
            "/usr/local/bin/crystalline",
            r"C:\Program Files\Crystalline\bin\crystalline.exe",
            "/Users/ada/Claude Extensions/crystalline",
            "/Users/ada/Library/Application Support/Claude/crystalline",
        ] {
            assert!(
                !in_desktop_extension_folder(Path::new(outside)),
                "{outside}"
            );
        }
    }

    fn bounded(start: StartOptions) -> StartOptions {
        StartOptions {
            exit_when_idle: true,
            ..start
        }
    }

    #[test]
    fn a_binary_inside_the_extension_folder_keeps_the_bounded_life() {
        let start = bounded(StartOptions::default());
        let plan = successor_plan(Some(&start), Ok(&[]), |_| false, true);
        assert!(matches!(plan, SuccessorPlan::Spawn(o) if o.exit_when_idle));
        assert!(daemon_args(&SpawnOptions::default(), true).contains(&"--exit-when-idle".into()));
    }

    #[test]
    fn a_successor_from_outside_an_extension_folder_does_not_exit_when_idle() {
        let start = bounded(StartOptions::default());
        let plan = successor_plan(Some(&start), Ok(&[]), |_| false, false);
        assert!(
            matches!(plan, SuccessorPlan::Spawn(o) if !o.exit_when_idle),
            "an MSI or Homebrew binary replacing an old extension's daemon starts a long-lived one"
        );
    }

    /// The spawned daemon's command line: `serve --daemon --autostarted` always,
    /// `--db` ahead of the subcommand and `--config` after it when given, and
    /// `--exit-when-idle` only for the extension's daemon, which must not
    /// outlive Claude Desktop.
    #[test]
    fn daemon_args_add_exit_when_idle_only_for_the_extension() {
        let plain = daemon_args(&SpawnOptions::default(), false);
        assert_eq!(plain, ["serve", "--daemon", "--autostarted"]);

        let idle = daemon_args(&SpawnOptions::default(), true);
        assert_eq!(
            idle,
            ["serve", "--daemon", "--autostarted", "--exit-when-idle"]
        );

        let full = daemon_args(
            &SpawnOptions {
                db: Some(PathBuf::from("/x/index.db")),
                config: Some(PathBuf::from("/x/config.yaml")),
                read_only: true,
                ..SpawnOptions::default()
            },
            true,
        );
        assert_eq!(
            full,
            [
                "--db",
                "/x/index.db",
                "serve",
                "--daemon",
                "--autostarted",
                "--exit-when-idle",
                "--read-only",
                "--config",
                "/x/config.yaml",
            ]
        );
    }

    /// A record a 0.22.0 daemon wrote has no start options. It still parses,
    /// and a displacing command starts no successor for it: it only waits a
    /// moment for a connected bridge to start one with its own flags.
    #[test]
    fn a_record_without_start_options_parses_and_gets_no_successor_from_a_command() {
        let record = r#"{"pid":4242,"socket_path":"/s/service.sock","version":"0.22.0",
            "started_at":"2026-09-30T10:00:00Z","mcp_line_options":true,
            "started_by":"autostart","http":"127.0.0.1:7411","allowed_hosts":[],
            "runs_in":{"working_dir":"/s","breakaway_refused":false,"exits_when_idle":false}}"#;
        let info: LockInfo = serde_json::from_str(record).expect("a 0.22.0 record parses");
        assert_eq!(info.start, None);
        match successor_plan(info.start.as_ref(), Ok(&[]), |_| true, true) {
            SuccessorPlan::Wait { budget, .. } => {
                assert_eq!(budget, UNRECORDED_SUCCESSOR_WAIT);
                assert!(budget <= Duration::from_secs(3), "{budget:?}");
            }
            other => panic!("no successor for a 0.22.0 record: {other:?}"),
        }
    }

    /// What a daemon records about its start carries no secret: a database
    /// URL with a password, a single sign-on client secret and a GitHub
    /// token read from the environment leave only their variable names.
    #[test]
    fn start_options_record_variable_names_and_never_their_values() {
        let overlay = crate::overlay::EnvOverlay::from_vars(
            [
                ("CRYSTALLINE_DATABASE_BACKEND", "postgres"),
                (
                    "CRYSTALLINE_DATABASE_URL",
                    "postgres://team:hunter2@db.internal/crystalline",
                ),
                ("CRYSTALLINE_AUTH_OIDC_CLIENT_ID", "app-1234"),
                ("CRYSTALLINE_AUTH_OIDC_CLIENT_SECRET", "oidc-s3cret"),
                ("CRYSTALLINE_GITHUB_TOKEN", "ghp_tokenvalue"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string())),
        )
        .unwrap();
        let start = StartOptions::capture(
            Some(Path::new("/srv/team.db")),
            Path::new("/srv/config.yaml"),
            true,
            None,
            &[],
            false,
            &overlay,
        );
        let info = LockInfo {
            pid: 1,
            socket_path: "/s".to_string(),
            version: "0.22.1".to_string(),
            started_at: "2026-10-02T00:00:00Z".to_string(),
            mcp_line_options: true,
            started_by: None,
            http: HttpBinding::Unrecorded,
            allowed_hosts: Vec::new(),
            standalone: None,
            runs_in: None,
            start: Some(start),
        };
        let json = serde_json::to_string(&info).unwrap();
        for secret in [
            "hunter2",
            "team:",
            "db.internal",
            "oidc-s3cret",
            "ghp_tokenvalue",
            "app-1234",
            "postgres://",
        ] {
            assert!(!json.contains(secret), "{secret} leaked into {json}");
        }
        for name in [
            "CRYSTALLINE_DATABASE_URL",
            "CRYSTALLINE_DATABASE_BACKEND",
            "CRYSTALLINE_AUTH_OIDC_CLIENT_SECRET",
            "CRYSTALLINE_GITHUB_TOKEN",
        ] {
            assert!(json.contains(name), "{name} is named: {json}");
        }
        // The paths are made absolute, which adds a drive on Windows, so only
        // the file names are checked.
        assert!(
            json.contains("team.db") && json.contains("config.yaml"),
            "{json}"
        );
        let back: LockInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.start, info.start, "the record reads back as written");
    }

    /// The successor runs with the displaced daemon's options; a variable it
    /// read that is missing here means no successor at all, and a variable
    /// set here that it never read is kept out of the successor.
    #[test]
    fn the_successor_plan_follows_the_record_and_its_environment() {
        let start = StartOptions {
            db: Some("/srv/team.db".to_string()),
            config: Some("/srv/config.yaml".to_string()),
            read_only: true,
            http: Some("127.0.0.1:7499".to_string()),
            allowed_hosts: vec!["kb.example".to_string()],
            exit_when_idle: false,
            env: vec!["CRYSTALLINE_DATABASE_URL".to_string()],
            partial: false,
        };
        let here = vec![
            "CRYSTALLINE_DATABASE_URL".to_string(),
            "CRYSTALLINE_SERVICE_HTTP".to_string(),
        ];
        assert_eq!(
            successor_plan(
                Some(&start),
                Ok(&here),
                |name| here.iter().any(|h| h == name),
                true
            ),
            SuccessorPlan::Spawn(SpawnOptions {
                db: Some(PathBuf::from("/srv/team.db")),
                config: Some(PathBuf::from("/srv/config.yaml")),
                read_only: true,
                http: Some("127.0.0.1:7499".to_string()),
                allowed_hosts: vec!["kb.example".to_string()],
                exit_when_idle: false,
                env_remove: vec!["CRYSTALLINE_SERVICE_HTTP".to_string()],
            })
        );
        match successor_plan(Some(&start), Ok(&[]), |_| false, true) {
            SuccessorPlan::Wait { why, budget } => {
                assert!(why.contains("CRYSTALLINE_DATABASE_URL"), "{why}");
                assert_eq!(budget, SUCCESSOR_WAIT);
            }
            other => panic!("a missing variable starts no successor: {other:?}"),
        }
        let partial = StartOptions {
            partial: true,
            env: Vec::new(),
            ..start.clone()
        };
        assert!(matches!(
            successor_plan(Some(&partial), Ok(&[]), |_| true, true),
            SuccessorPlan::Wait { .. }
        ));
        let args = match successor_plan(Some(&start), Ok(&here), |_| true, true) {
            SuccessorPlan::Spawn(options) => daemon_args(&options, false),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            args,
            [
                "--db",
                "/srv/team.db",
                "serve",
                "--daemon",
                "--autostarted",
                "--read-only",
                "--http",
                "127.0.0.1:7499",
                "--allowed-host",
                "kb.example",
                "--config",
                "/srv/config.yaml",
            ]
        );
    }

    /// The daemon works in the state directory, so a path it is handed must
    /// mean the same file from there as it did in the client: relative ones
    /// are resolved against the client's directory before the spawn.
    #[test]
    fn paths_handed_to_the_daemon_are_absolute() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            absolute_for_daemon(Path::new("rel/index.db")),
            cwd.join("rel/index.db")
        );
        let already = cwd.join("config.yaml");
        assert_eq!(absolute_for_daemon(&already), already);
    }

    /// `CRYSTALLINE_CONFIG` picks the config file like `--config` does, so a
    /// relative value is resolved the same way. Empty means unset and a `~`
    /// path is absolute once expanded, so both are left alone.
    #[test]
    fn a_relative_config_variable_is_made_absolute_for_the_daemon() {
        use std::ffi::OsStr;
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            config_env_for_daemon(OsStr::new("conf/config.yaml")),
            Some(cwd.join("conf/config.yaml"))
        );
        assert_eq!(config_env_for_daemon(OsStr::new("")), None);
        assert_eq!(config_env_for_daemon(cwd.join("c.yaml").as_os_str()), None);
        assert_eq!(config_env_for_daemon(OsStr::new("~/c.yaml")), None);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let raw = OsStr::from_bytes(b"conf/\xffconfig.yaml");
            assert_eq!(
                config_env_for_daemon(raw),
                Some(cwd.join(raw)),
                "a value that is not UTF-8 keeps its bytes"
            );
        }
    }

    /// The working directory is created before the spawn: a missing one would
    /// fail both spawns on Windows and read as a refused breakaway. One that
    /// cannot be had is an error, and the spawn then keeps its own directory.
    #[test]
    fn daemon_working_dir_creates_the_state_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state").join("crystalline");
        assert_eq!(daemon_working_dir(Ok(state.clone())).unwrap(), state);
        assert!(state.is_dir(), "created on the way");

        let file = tmp.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        assert!(
            daemon_working_dir(Ok(file.join("below"))).is_err(),
            "a path below a file cannot be a directory"
        );
        assert!(daemon_working_dir(Err(anyhow::anyhow!("no home"))).is_err());
    }

    /// Only an access error is Windows refusing the breakaway (libuv reads it
    /// the same way). Anything else is some other failure and is not reported
    /// as a refusal.
    #[test]
    fn only_an_access_error_is_a_refused_breakaway() {
        assert!(breakaway_refusal(&io::Error::from_raw_os_error(5)));
        assert!(!breakaway_refusal(&io::Error::from_raw_os_error(2)));
        assert!(!breakaway_refusal(&io::Error::other("no")));
    }

    struct Answers {
        find: Option<&'static str>,
        run_ok: bool,
    }

    impl crate::daemon_task::DaemonTask for Answers {
        fn find(&self) -> Option<String> {
            self.find.map(str::to_string)
        }
        fn run(&self, _name: &str) -> Result<(), String> {
            if self.run_ok {
                Ok(())
            } else {
                Err("refused".to_string())
            }
        }
    }

    #[test]
    fn a_refused_breakaway_uses_the_task_when_one_runs() {
        let task = Answers {
            find: Some(crate::daemon_task::MACHINE_TASK_NAME),
            run_ok: true,
        };
        assert_eq!(
            after_refused_breakaway(&task, &SpawnOptions::default()),
            RefusedBreakaway::StartedByTask(crate::daemon_task::MACHINE_TASK_NAME.to_string())
        );
    }

    /// The task starts a plain `serve --daemon --from-task`, so it stands in
    /// only for a spawn that asks for nothing else. Any option the task
    /// cannot pass keeps the spawn inside the job, which serves what the
    /// client asked for.
    #[test]
    fn the_task_stands_in_only_for_a_spawn_with_default_options() {
        assert!(the_task_can_stand_in(&SpawnOptions::default()));
        let asking = [
            SpawnOptions {
                db: Some(PathBuf::from("/abs/index.db")),
                ..SpawnOptions::default()
            },
            SpawnOptions {
                config: Some(PathBuf::from("/abs/config.yaml")),
                ..SpawnOptions::default()
            },
            SpawnOptions {
                read_only: true,
                ..SpawnOptions::default()
            },
            SpawnOptions {
                http: Some("127.0.0.1:7412".to_string()),
                ..SpawnOptions::default()
            },
            SpawnOptions {
                allowed_hosts: vec!["kb.example".to_string()],
                ..SpawnOptions::default()
            },
            SpawnOptions {
                exit_when_idle: true,
                ..SpawnOptions::default()
            },
            SpawnOptions {
                env_remove: vec!["CRYSTALLINE_CONFIG".to_string()],
                ..SpawnOptions::default()
            },
        ];
        let task = Answers {
            find: Some(crate::daemon_task::MACHINE_TASK_NAME),
            run_ok: true,
        };
        for options in asking {
            assert!(!the_task_can_stand_in(&options), "{options:?}");
            assert_eq!(
                after_refused_breakaway(&task, &options),
                RefusedBreakaway::InsideTheJob,
                "{options:?}"
            );
        }
    }

    #[test]
    fn a_refused_breakaway_stays_inside_the_job_without_a_task() {
        assert_eq!(
            after_refused_breakaway(
                &Answers {
                    find: None,
                    run_ok: true
                },
                &SpawnOptions::default()
            ),
            RefusedBreakaway::InsideTheJob
        );
        assert_eq!(
            after_refused_breakaway(
                &Answers {
                    find: Some("x"),
                    run_ok: false
                },
                &SpawnOptions::default()
            ),
            RefusedBreakaway::InsideTheJob
        );
    }

    // --- the words a locked index is refused in -----------------------------

    /// The raw backend error is the tail of the sentence, never its head, and
    /// never the whole of it.
    fn assert_error_is_only_the_tail(words: &str, raw: &str) {
        let marker = "The index reported: ";
        let at = words
            .find(marker)
            .unwrap_or_else(|| panic!("no error marker in: {words}"));
        assert!(
            !words.starts_with(raw),
            "a lock error must not lead the sentence: {words}"
        );
        assert_eq!(
            words.match_indices(raw).map(|(i, _)| i).collect::<Vec<_>>(),
            vec![at + marker.len()],
            "the raw error appears once, after the marker: {words}"
        );
    }

    /// A daemon holds the index and answered nothing: name it by pid, and name
    /// the two commands that do something about it. This is the shape the
    /// service crate's own standalone fallbacks hit, so it is pinned where
    /// both crates read it.
    #[test]
    fn a_silent_holder_is_named_with_its_pid_and_a_remedy() {
        let raw = "Locking error: File is locked by another process";
        let words = words_for_holder(Some(4242), "/tmp/index.db", raw, false);
        assert!(words.contains("(pid 4242)"), "{words}");
        assert!(words.contains("/tmp/index.db"), "{words}");
        assert!(words.contains("crystalline doctor --fix"), "{words}");
        assert!(words.contains("crystalline ctl shutdown"), "{words}");
        assert_error_is_only_the_tail(&words, raw);
    }

    /// The same holder, reached past by `--db`: the remedy is to stop reaching
    /// past it, so that is what the sentence says first.
    #[test]
    fn a_bypassed_holder_is_told_to_drop_the_override() {
        let raw = "Locking error: File is locked by another process";
        let words = words_for_holder(Some(77), "/tmp/index.db", raw, true);
        assert!(words.contains("(pid 77)"), "{words}");
        assert!(words.contains("--db or --config"), "{words}");
        assert!(
            words.contains("without --db and --config"),
            "the first remedy is the one that costs nothing: {words}"
        );
        assert_error_is_only_the_tail(&words, raw);
    }

    /// No holder to name, so the sentence says that rather than implying one.
    /// The unreadable-record shape: no record, an unparseable one and a dead
    /// pid all arrive here as `None`.
    #[test]
    fn with_no_daemon_the_absence_is_stated() {
        let raw = "unable to open database file";
        let words = words_for_holder(None, "/tmp/index.db", raw, false);
        assert!(!words.contains("pid"), "{words}");
        assert!(
            words.contains("no Crystalline daemon is running"),
            "{words}"
        );
        assert!(words.contains("crystalline doctor --fix"), "{words}");
        assert_error_is_only_the_tail(&words, raw);
    }

    /// A newer schema is not a holder problem: whoever holds the lock, the
    /// words carry the refusal's own remedy and none of the lock advice that
    /// would contradict it.
    #[test]
    fn a_schema_newer_than_this_binary_stands_alone() {
        let raw = "this index was upgraded by a newer Crystalline (schema v16, this binary knows v15). \
                   This copy is out of date.";
        for holder in [None, Some(4242)] {
            let words = words_for_holder(holder, "/tmp/index.db", raw, false);
            assert!(words.ends_with(raw), "{words}");
            assert!(!words.contains("doctor --fix"), "{words}");
            assert!(!words.contains("ctl shutdown"), "{words}");
        }
    }

    // --- the mcp handshake line ---------------------------------------------

    /// The extended line only goes to a daemon that declared it parses one. An
    /// older one compares the whole trimmed line against `"mcp"` and drops
    /// anything else, and a displacement that times out leaves exactly such a
    /// daemon running and attached to (`try_attach_reporting` says so in as
    /// many words), so the fallback has to be the bare line rather than a dead
    /// socket.
    #[test]
    fn the_extended_mode_line_is_only_sent_to_a_daemon_that_declared_it() {
        assert_eq!(mcp_mode_line(VERIFIED_GATE, true), "mcp skills=off\n");
        assert_eq!(
            mcp_mode_line(VERIFIED_GATE, false),
            "mcp\n",
            "a daemon that did not declare it gets the line it understands, and \
             serves the surface"
        );
        // Nothing to say, nothing added, whatever the daemon can parse.
        assert_eq!(mcp_mode_line(HarnessGate::default(), true), "mcp\n");
        assert_eq!(mcp_mode_line(HarnessGate::default(), false), "mcp\n");
    }

    /// The gate a verified harness with its hooks wired resolves to.
    const VERIFIED_GATE: HarnessGate = HarnessGate {
        hook_installed: true,
        onboarding_verified: true,
    };

    /// A hook in the receipt without a verified profile asks for the short
    /// conditional instructions and nothing else: it never sends the token
    /// that hides the skills.
    #[test]
    fn an_installed_but_unverified_hook_sends_no_skills_off_token() {
        let unverified = HarnessGate {
            hook_installed: true,
            onboarding_verified: false,
        };
        assert_eq!(mcp_mode_line(unverified, true), "mcp routing=conditional\n");
        assert!(!mcp_mode_line(unverified, true).contains(SKILLS_OFF_OPTION));
        assert_eq!(
            mcp_mode_line(
                HarnessGate {
                    hook_installed: true,
                    onboarding_verified: true
                },
                true
            ),
            "mcp skills=off\n"
        );
        assert_eq!(mcp_mode_line(unverified, false), "mcp\n");
        assert_eq!(mcp_mode_line(HarnessGate::default(), true), "mcp\n");
    }

    /// What a 0.22.0 daemon does with the new token: its check reads only
    /// `skills=off`, so the conditional token falls through to a full serve,
    /// the safe direction.
    #[test]
    fn the_conditional_token_is_ignored_by_a_daemon_that_does_not_know_it() {
        let (mode, options) = split_mode_line("mcp routing=conditional");
        assert_eq!(mode, "mcp");
        assert!(
            !options.contains(&SKILLS_OFF_OPTION),
            "the 0.22.0 check reads no skills=off: full serve"
        );
    }

    /// The capability is read off the record, and a record written before the
    /// field existed reads as "cannot parse options" rather than failing to
    /// deserialize at all. That is the whole reason it is a declared
    /// capability and not a version threshold: the version that first learned
    /// to parse options is one this tree already carries, so no threshold
    /// could tell a daemon built from this commit apart from one built the
    /// commit before.
    #[test]
    fn a_lock_record_without_the_capability_field_reads_as_unable() {
        let legacy =
            r#"{"pid":1,"socket_path":"s","version":"0.13.0","started_at":"2026-08-14T00:00:00Z"}"#;
        let info: LockInfo = serde_json::from_str(legacy).expect("an older record still parses");
        assert!(!info.mcp_line_options);
        assert_eq!(mcp_mode_line(VERIFIED_GATE, info.mcp_line_options), "mcp\n");

        let current = serde_json::to_string(&LockInfo {
            pid: 1,
            socket_path: "s".to_string(),
            version: crystalline_core::VERSION.to_string(),
            started_at: "2026-08-14T00:00:00Z".to_string(),
            mcp_line_options: true,
            started_by: None,
            http: HttpBinding::Unrecorded,
            allowed_hosts: Vec::new(),
            standalone: None,
            runs_in: None,
            start: None,
        })
        .unwrap();
        let info: LockInfo = serde_json::from_str(&current).unwrap();
        assert!(info.mcp_line_options);
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

    fn triple(version: &str) -> Option<(u64, u64, u64)> {
        Precedence::parse(version).map(|p| p.triple)
    }

    #[test]
    fn version_triples_ignore_suffixes_and_tolerate_two_parts() {
        assert_eq!(triple("1.2.3"), Some((1, 2, 3)));
        assert_eq!(triple("1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(triple("1.2.3+build7"), Some((1, 2, 3)));
        assert_eq!(triple("1.2"), Some((1, 2, 0)));
        assert_eq!(triple("nope"), None);
    }

    #[test]
    fn a_newer_dev_build_displaces_an_older_one_of_the_same_triple() {
        assert_eq!(
            attach_policy("0.21.0-dev.4817", "0.21.0-dev.4820"),
            AttachPolicy::Displace,
            "a brew upgrade of the dev channel must take the daemon over"
        );
        assert_eq!(
            attach_policy("0.21.0-dev.9", "0.21.0-dev.10"),
            AttachPolicy::Displace,
            "numeric identifiers compare as numbers, not as text"
        );
        assert_eq!(
            attach_policy("0.21.0-dev.4820", "0.21.0-dev.4817"),
            AttachPolicy::Attach,
            "an older dev client attaches to a newer dev daemon"
        );
        assert_eq!(
            attach_policy("0.21.0-dev.4820", "0.21.0-dev.4820"),
            AttachPolicy::Attach
        );
    }

    #[test]
    fn pre_release_precedence_follows_semantic_versioning() {
        assert_eq!(
            attach_policy("0.20.0", "0.21.0-dev.1"),
            AttachPolicy::Displace,
            "a dev build of the next minor displaces the last release"
        );
        assert_eq!(
            attach_policy("0.21.0-dev.4817", "0.21.0"),
            AttachPolicy::Displace,
            "a release outranks every pre-release of its own triple"
        );
        assert_eq!(
            attach_policy("0.21.0", "0.21.0-dev.4817"),
            AttachPolicy::Attach
        );
        assert!(strictly_newer("1.0.0-alpha.1", "1.0.0-alpha"));
        assert!(strictly_newer("1.0.0-alpha.beta", "1.0.0-alpha.1"));
        assert!(strictly_newer("1.0.0-beta", "1.0.0-alpha.beta"));
        assert!(strictly_newer("1.0.0-beta.11", "1.0.0-beta.2"));
        assert!(strictly_newer("1.0.0-rc.1", "1.0.0-beta.11"));
        assert!(
            !strictly_newer("1.0.0+build.2", "1.0.0+build.1"),
            "build metadata carries no precedence"
        );
        assert!(
            !strictly_newer("0.21.0-dev..1", "0.20.0"),
            "an empty pre-release identifier is unparseable, never newer"
        );
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

    /// Short waits for the displacement tests below: long enough for a
    /// signalled child to be torn down, short enough that a stage which is
    /// meant to run out does not hold the suite up.
    #[cfg(unix)]
    const TEST_WAITS: DisplaceWaits = DisplaceWaits {
        ask: Duration::from_millis(300),
        term: Duration::from_secs(1),
        kill: Duration::from_secs(5),
    };

    /// A scripted daemon socket that acknowledges the shutdown ask and then
    /// does nothing about it, the shape of a daemon that answers and lingers.
    #[cfg(unix)]
    fn acknowledge_and_linger(sock: &Path) -> tokio::task::JoinHandle<()> {
        let name = socket_name(sock).unwrap();
        let listener = ListenerOptions::new().name(name).create_tokio().unwrap();
        tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            let _ = read_mode_line(&mut stream).await;
            let mut sink = [0u8; 64];
            let _ = stream.read(&mut sink).await;
            stream.write_all(b"{\"ok\":true}\n").await.unwrap();
            stream.flush().await.unwrap();
            // Keep the stream open; the "daemon" never exits by itself.
            tokio::time::sleep(Duration::from_secs(30)).await;
        })
    }

    /// A process that ignores the ask and is not a Crystalline binary is
    /// never signalled: displace reports failure and the process is still
    /// running afterwards. `sleep` is the stranger - the identity gate
    /// refuses its name - so this is the gate holding on the displace path,
    /// not a signal that merely failed to land.
    #[cfg(unix)]
    #[tokio::test]
    async fn displace_never_signals_a_stranger() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("crystalline.sock");
        let server = acknowledge_and_linger(&sock);

        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();

        assert!(
            !displace_within(&sock, pid, &TEST_WAITS).await,
            "the pid never went away"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "the stranger is still running: nothing was signalled"
        );
        server.abort();
        let _ = child.kill();
        let _ = child.wait();
    }

    /// The variable that tells [`displace_stand_in`] to act as a daemon.
    #[cfg(unix)]
    const STAND_IN_ENV: &str = "CRYSTALLINE_TEST_DISPLACE_STAND_IN";

    /// Not a test: the process [`displace_stops_a_verified_daemon_that_ignores_both_asks`]
    /// displaces. It runs only when that test starts this very test binary
    /// with [`STAND_IN_ENV`] set, and then just stays alive; run any other
    /// way it returns at once. Being this binary is the point: the identity
    /// gate accepts a holder that wears this client's own executable name,
    /// exactly as it accepts a renamed release binary, so the displacement is
    /// verified honestly rather than with the gate switched off.
    #[cfg(unix)]
    #[test]
    #[ignore = "a stand-in process for a displace test, started by that test"]
    fn displace_stand_in() {
        if std::env::var_os(STAND_IN_ENV).is_some() {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    /// The 2026-09-23 incident's escalation: a verified Crystalline process
    /// that acknowledges the shutdown ask, stays, and ignores `SIGTERM` too
    /// (a lingering tokio daemon swallows it, since its signal handler stays
    /// installed after its signal streams are gone) is ended by the hard
    /// signal, and displace reports it gone.
    #[cfg(unix)]
    #[tokio::test]
    async fn displace_stops_a_verified_daemon_that_ignores_both_asks() {
        use std::os::unix::process::{CommandExt, ExitStatusExt};

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("crystalline.sock");
        let server = acknowledge_and_linger(&sock);

        let mut stand_in = std::process::Command::new(std::env::current_exe().unwrap());
        stand_in
            .args([
                "instance::tests::displace_stand_in",
                "--exact",
                "--ignored",
                "--test-threads=1",
            ])
            .env(STAND_IN_ENV, "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // SAFETY: `signal` is async-signal-safe, which is all `pre_exec`
        // requires. An ignored disposition survives the exec.
        unsafe {
            stand_in.pre_exec(|| {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
                Ok(())
            });
        }
        let mut child = stand_in.spawn().unwrap();
        let pid = child.id();
        // Reaped as soon as it dies, so its end is observed as an exit status
        // and the pid is really gone rather than a zombie of this process.
        let reaper = std::thread::spawn(move || child.wait());

        let gone = displace_within(&sock, pid, &TEST_WAITS).await;
        if !gone {
            signal_process(pid, true);
        }
        let status = reaper.join().unwrap().unwrap();
        server.abort();

        assert!(gone, "displace reports the verified daemon gone");
        assert_eq!(
            status.signal(),
            Some(libc::SIGKILL),
            "it ignored the ask and SIGTERM, so only the hard signal ended it: {status:?}"
        );
    }

    /// An exited process its parent has not reaped yet is gone for a
    /// displacement, although the signal-0 probe still finds it: the case of
    /// an extension daemon whose bridge never waits for it.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn an_unreaped_exited_process_counts_as_gone() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(!process_gone(pid), "a running process is not gone");

        // SIGKILL without reaping: the child is a zombie from here on.
        signal_process(pid, true);
        let start = Instant::now();
        while !is_zombie(pid) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            is_zombie(pid),
            "the killed, unreaped child reads as a zombie"
        );
        assert!(
            process_alive(pid),
            "the signal-0 probe alone still finds the zombie"
        );
        assert!(process_gone(pid), "but for a displacement it is gone");

        child.wait().unwrap();
        assert!(process_gone(pid), "and reaped it is gone either way");
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
    /// The same pattern `crates/core/tests/it/config.rs` uses for
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

    // --- the unix socket path budget ----------------------------------------

    /// A `sockaddr_un` path is ~104 bytes on macOS and ~108 on Linux, and a
    /// deep enough `HOME` spends them on its own. The check has to say all
    /// three things a stuck user needs: how long the path is, how long it may
    /// be and which variable moves it.
    #[cfg(unix)]
    #[test]
    fn an_over_long_socket_path_is_refused_with_both_numbers() {
        let long = PathBuf::from(format!("/tmp/{}/service.sock", "d".repeat(110)));
        let len = long.as_os_str().as_encoded_bytes().len();
        assert!(len > 110, "past every platform's budget: {len}");

        let message = check_socket_path(&long)
            .expect_err("a path this long can never bind")
            .to_string();
        assert!(
            message.contains(&long.display().to_string()),
            "the path itself is named: {message}"
        );
        assert!(
            message.contains(&format!("is {len} bytes")),
            "the measured length is named: {message}"
        );
        assert!(
            message.contains(&format!("at most {}", socket_path_budget())),
            "the budget is named: {message}"
        );
        assert!(
            message.contains("XDG_STATE_HOME"),
            "the way out is named: {message}"
        );

        // A path that fits is not this check's business.
        check_socket_path(Path::new("/tmp/crystalline/service.sock"))
            .expect("a short path passes untouched");
    }

    /// The same diagnosis, on the client's own stderr rather than in a log
    /// nobody opens. The daemon `ensure_daemon` spawns is detached, so without
    /// this the caller spends the whole 15s readiness budget and is then
    /// pointed at `daemon.log` for an errno.
    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_daemon_diagnoses_an_over_long_socket_path_instead_of_waiting() {
        let _home = ScratchHome::new(&"deep".repeat(30));
        let sock = config::service_sock_path().unwrap();
        assert!(
            sock.as_os_str().as_encoded_bytes().len() > socket_path_budget(),
            "the scratch home has to overrun the budget for this to test anything: {}",
            sock.display()
        );

        let started = Instant::now();
        let message = match ensure_daemon(true, None, None, false).await {
            Ok(_) => panic!("no daemon can bind under this state directory"),
            Err(e) => e.to_string(),
        };
        assert!(
            message.contains("unix socket path holds at most"),
            "the bind diagnosis, not the generic timeout: {message}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "refused up front rather than after the 15s readiness budget"
        );
    }

    /// A scripted daemon on `listener` for the pipe-first tests. Every
    /// connection reads its mode line. A `ctl` connection answers `holder`
    /// with `holder` (or, when that is `None`, the refusal a 0.23.1 daemon
    /// gives), `status` with `status`, `shutdown` with an acknowledgement,
    /// and records each request. An `mcp` connection records `mcp` and is
    /// held open for a moment, like a session.
    #[cfg(unix)]
    fn scripted_daemon(
        listener: IpcListener,
        holder: Option<serde_json::Value>,
        status: serde_json::Value,
    ) -> (
        tokio::task::JoinHandle<()>,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        let task = tokio::spawn(async move {
            // One task per connection, as the daemon serves them: a session a
            // client holds open must never keep the next client's probe waiting.
            while let Ok(stream) = listener.accept().await {
                let log = log.clone();
                let holder = holder.clone();
                let status = status.clone();
                tokio::spawn(serve_one(stream, log, holder, status));
            }
        });
        (task, seen)
    }

    /// One connection of [`scripted_daemon`].
    #[cfg(unix)]
    async fn serve_one(
        mut stream: IpcStream,
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        holder: Option<serde_json::Value>,
        status: serde_json::Value,
    ) {
        use tokio::io::AsyncBufReadExt;
        let Ok(mode) = read_mode_line(&mut stream).await else {
            return;
        };
        if mode.starts_with("mcp") {
            log.lock().unwrap().push("mcp".to_string());
            tokio::time::sleep(Duration::from_millis(500)).await;
            return;
        }
        let (read, mut write) = tokio::io::split(stream);
        let mut lines = tokio::io::BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let request: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
            let cmd = request["cmd"].as_str().unwrap_or("").to_string();
            log.lock().unwrap().push(cmd.clone());
            let reply = match (cmd.as_str(), &holder) {
                ("holder", Some(facts)) => {
                    serde_json::json!({ "v": 1, "ok": true, "data": facts })
                }
                ("holder", None) => serde_json::json!({
                    "v": 1, "ok": false,
                    "error": "unknown ctl command 'holder'; expected status, sessions, tool"
                }),
                ("status", _) => serde_json::json!({ "v": 1, "ok": true, "data": status }),
                _ => serde_json::json!({ "v": 1, "ok": true, "data": { "stopping": true } }),
            };
            let mut out = reply.to_string();
            out.push('\n');
            if write.write_all(out.as_bytes()).await.is_err() {
                break;
            }
            let _ = write.flush().await;
        }
    }

    /// A pid that is certainly dead: a child that already exited and was
    /// reaped.
    #[cfg(unix)]
    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pipe_first_attaches_although_the_record_names_a_dead_pid() {
        let home = ScratchHome::new("pipe-dead-record");
        let sock = config::service_sock_path().unwrap();
        let listener = ListenerOptions::new()
            .name(socket_name(&sock).unwrap())
            .create_tokio()
            .unwrap();
        let stale = serde_json::json!({
            "pid": dead_pid(), "socket_path": sock.display().to_string(),
            "version": crystalline_core::VERSION, "started_at": "2026-10-07T00:00:00Z"
        });
        std::fs::write(config::service_info_path().unwrap(), stale.to_string()).unwrap();
        let facts = serde_json::json!({
            "pid": std::process::id(), "version": crystalline_core::VERSION,
            "mcp_line_options": true
        });
        let (server, seen) = scripted_daemon(listener, Some(facts), serde_json::json!({}));

        let (conn, displaced) = try_attach_reporting().await;
        let conn = conn.expect("the pipe answered, so the stale record decides nothing");
        assert!(!displaced);
        assert!(
            conn.mcp_line_options,
            "the holder said it parses handshake options"
        );
        assert_eq!(
            seen.lock().unwrap().first().map(String::as_str),
            Some("holder")
        );
        drop(conn);
        server.abort();
        drop(home);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pipe_first_attaches_with_no_record_at_all() {
        let home = ScratchHome::new("pipe-no-record");
        let sock = config::service_sock_path().unwrap();
        let listener = ListenerOptions::new()
            .name(socket_name(&sock).unwrap())
            .create_tokio()
            .unwrap();
        let facts =
            serde_json::json!({ "pid": std::process::id(), "version": crystalline_core::VERSION });
        let (server, _) = scripted_daemon(listener, Some(facts), serde_json::json!({}));
        assert!(!config::service_info_path().unwrap().exists());
        assert!(try_attach().await.is_some());
        assert!(
            try_attach_passive().await.is_some(),
            "the hook path is a bare pipe connect"
        );
        server.abort();
        drop(home);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_0_23_1_daemon_without_holder_is_attached_through_status() {
        let home = ScratchHome::new("pipe-old-daemon");
        let sock = config::service_sock_path().unwrap();
        let listener = ListenerOptions::new()
            .name(socket_name(&sock).unwrap())
            .create_tokio()
            .unwrap();
        let record = serde_json::json!({
            "pid": std::process::id(), "socket_path": sock.display().to_string(),
            "version": crystalline_core::VERSION, "started_at": "2026-10-07T00:00:00Z",
            "mcp_line_options": true
        });
        std::fs::write(config::service_info_path().unwrap(), record.to_string()).unwrap();
        let status = serde_json::json!({
            "pid": std::process::id(), "version": crystalline_core::VERSION,
            "runs_in": { "working_dir": "/s", "breakaway_refused": false, "exits_when_idle": false }
        });
        let (server, seen) = scripted_daemon(listener, None, status);
        let conn = try_attach()
            .await
            .expect("status answers for an old daemon");
        assert!(
            conn.mcp_line_options,
            "taken from the record, which names the same pid"
        );
        assert_eq!(
            seen.lock().unwrap()[..2],
            ["holder".to_string(), "status".to_string()]
        );
        // Only `status` knows this working directory: the record has no
        // `runs_in`, so this fails if the facts came from the record alone.
        let facts = ask_holder()
            .await
            .expect("status answers for an old daemon");
        assert_eq!(
            facts.runs_in.and_then(|r| r.working_dir).as_deref(),
            Some("/s"),
            "taken from the status answer"
        );
        server.abort();
        drop(home);
    }

    /// A daemon refuses `holder` and is gone before the `status` ask can
    /// reach the pipe. Nothing listens any more, so there is no daemon, and
    /// a record that names a live pid does not bring one back.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_daemon_gone_between_the_two_asks_is_no_daemon() {
        let home = ScratchHome::new("pipe-gone");
        let sock = config::service_sock_path().unwrap();
        let listener = ListenerOptions::new()
            .name(socket_name(&sock).unwrap())
            .create_tokio()
            .unwrap();
        let record = serde_json::json!({
            "pid": std::process::id(), "socket_path": sock.display().to_string(),
            "version": crystalline_core::VERSION, "started_at": "2026-10-07T00:00:00Z"
        });
        std::fs::write(config::service_info_path().unwrap(), record.to_string()).unwrap();
        let socket_file = sock.clone();
        let server = tokio::spawn(async move {
            let Ok(stream) = listener.accept().await else {
                return;
            };
            // Gone before it answers: the listener and its socket file.
            drop(listener);
            let _ = std::fs::remove_file(&socket_file);
            let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            serve_one(stream, log, None, serde_json::json!({})).await;
        });
        assert!(
            ask_holder().await.is_none(),
            "nothing listens when status is asked, so there is no daemon"
        );
        server.abort();
        drop(home);
    }

    /// A daemon answers on the pipe, refuses `holder` and gives a `status`
    /// without its pid (a 0.23.1 daemon whose store failed the status
    /// report). The pipe proves a daemon, so the record, which names a live
    /// pid, says who it is, as it did before pipe-first discovery.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_daemon_that_answers_without_facts_is_taken_from_its_record() {
        let home = ScratchHome::new("pipe-no-facts");
        let sock = config::service_sock_path().unwrap();
        let listener = ListenerOptions::new()
            .name(socket_name(&sock).unwrap())
            .create_tokio()
            .unwrap();
        let record = serde_json::json!({
            "pid": std::process::id(), "socket_path": sock.display().to_string(),
            "version": crystalline_core::VERSION, "started_at": "2026-10-07T00:00:00Z",
            "mcp_line_options": true
        });
        std::fs::write(config::service_info_path().unwrap(), record.to_string()).unwrap();
        let (server, seen) = scripted_daemon(listener, None, serde_json::json!({}));
        let conn = try_attach()
            .await
            .expect("something answers on the pipe, so a daemon is there");
        assert!(conn.mcp_line_options, "taken from the record");
        assert_eq!(
            seen.lock().unwrap()[..2],
            ["holder".to_string(), "status".to_string()]
        );
        server.abort();
        drop(home);
    }

    /// A listener that takes the connection and closes it unanswered (a
    /// daemon dying mid-exchange) is still a daemon on the pipe: the record
    /// that names a live pid says who it is, and the client attaches as it
    /// did before pipe-first discovery. A record naming a dead pid does not.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_listener_that_closes_unanswered_is_taken_from_a_live_record() {
        let home = ScratchHome::new("pipe-closes");
        let sock = config::service_sock_path().unwrap();
        let listener = ListenerOptions::new()
            .name(socket_name(&sock).unwrap())
            .create_tokio()
            .unwrap();
        let server = tokio::spawn(async move {
            while let Ok(stream) = listener.accept().await {
                drop(stream);
            }
        });
        let record = |pid: u32| {
            serde_json::json!({
                "pid": pid, "socket_path": sock.display().to_string(),
                "version": crystalline_core::VERSION, "started_at": "2026-10-07T00:00:00Z"
            })
            .to_string()
        };
        let info = config::service_info_path().unwrap();
        std::fs::write(&info, record(std::process::id())).unwrap();
        let started = Instant::now();
        let facts = ask_holder().await.expect("the live record names it");
        assert_eq!(facts.pid, std::process::id());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a closed connection is no wait: {:?}",
            started.elapsed()
        );
        std::fs::write(&info, record(dead_pid())).unwrap();
        assert!(
            ask_holder().await.is_none(),
            "a record naming a dead pid explains nothing"
        );
        server.abort();
        drop(home);
    }

    #[test]
    fn facts_from_status_take_the_record_only_for_the_same_pid() {
        let status = serde_json::json!({ "pid": 4242, "version": "0.23.1" });
        let mut record: LockInfo = serde_json::from_str(
            r#"{"pid":4242,"socket_path":"s","version":"0.23.1","started_at":"",
                "mcp_line_options":true,"start":{"read_only":true}}"#,
        )
        .unwrap();
        let same = facts_from_status(&status, Some(&record)).unwrap();
        assert!(same.mcp_line_options);
        assert_eq!(same.start.map(|s| s.read_only), Some(true));
        record.pid = 7;
        let other = facts_from_status(&status, Some(&record)).unwrap();
        assert!(
            !other.mcp_line_options,
            "another pid's record says nothing about this daemon"
        );
        assert!(other.start.is_none());
        assert!(facts_from_status(&serde_json::json!({ "version": "x" }), None).is_none());
    }

    /// A holder that answers at an old version, with a real, killable child
    /// as its pid: the Displace arm asks it to shut down, the pid goes away
    /// and the call reports a completed displacement with no connection,
    /// exactly the case `ensure_daemon`'s readiness poll must react to by
    /// re-spawning. The `service.json` written beside it decides nothing.
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
            mcp_line_options: true,
            started_by: None,
            http: HttpBinding::Unrecorded,
            allowed_hosts: Vec::new(),
            standalone: None,
            runs_in: None,
            start: None,
        };
        std::fs::write(&info_path, serde_json::to_string(&info).unwrap()).unwrap();

        let facts = serde_json::json!({ "pid": pid, "version": "0.0.1" });
        let (server, seen) = scripted_daemon(listener, Some(facts), serde_json::json!({}));
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
        assert!(seen.lock().unwrap().contains(&"shutdown".to_string()));

        server.abort();
        killer.await.unwrap();
        drop(home);
    }

    /// Nothing listens on the pipe, and the record names a live, verified
    /// Crystalline process at an older version that ignores `SIGTERM`. A
    /// record-first attach took that record at its word: it entered the
    /// Displace arm, signalled the process, waited out the term step and
    /// reported a displacement. Pipe first, nothing answered, so there is no
    /// daemon: the call returns at once and the process is never touched.
    #[cfg(unix)]
    #[tokio::test]
    async fn nothing_on_the_pipe_is_no_daemon_whatever_the_record_says() {
        use std::os::unix::process::CommandExt;

        let home = ScratchHome::new("pipe-silent");
        let mut stand_in = std::process::Command::new(std::env::current_exe().unwrap());
        stand_in
            .args([
                "instance::tests::displace_stand_in",
                "--exact",
                "--ignored",
                "--test-threads=1",
            ])
            .env(STAND_IN_ENV, "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // SAFETY: `signal` is async-signal-safe, which is all `pre_exec`
        // requires. An ignored disposition survives the exec.
        unsafe {
            stand_in.pre_exec(|| {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
                Ok(())
            });
        }
        let mut child = stand_in.spawn().unwrap();
        let pid = child.id();
        let older = serde_json::json!({
            "pid": pid, "socket_path": config::service_sock_path().unwrap().display().to_string(),
            "version": "0.0.1", "started_at": "2026-10-07T00:00:00Z"
        });
        std::fs::write(config::service_info_path().unwrap(), older.to_string()).unwrap();

        let started = Instant::now();
        let (conn, displaced) = try_attach_displacing().await;
        let elapsed = started.elapsed();
        let untouched = child.try_wait().unwrap().is_none();
        let _ = child.kill();
        let _ = child.wait();
        drop(home);

        assert!(conn.is_none(), "nothing listens, so nothing attaches");
        assert!(
            displaced.is_none(),
            "a record alone is no daemon and nothing is displaced"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "no displace budget is spent: {elapsed:?}"
        );
        assert!(untouched, "the process the record names is never signalled");
    }

    /// A holder at this binary's own version never reaches the Displace arm,
    /// so a live stub socket just attaches and reports no displacement. The
    /// holder's pid is this test process itself (always alive), which stands
    /// in for a live daemon without spawning a child.
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
            mcp_line_options: true,
            started_by: None,
            http: HttpBinding::Unrecorded,
            allowed_hosts: Vec::new(),
            standalone: None,
            runs_in: None,
            start: None,
        };
        std::fs::write(&info_path, serde_json::to_string(&info).unwrap()).unwrap();

        let facts = serde_json::json!({
            "pid": std::process::id(), "version": crystalline_core::VERSION
        });
        let (server, _) = scripted_daemon(listener, Some(facts), serde_json::json!({}));

        let (conn, displaced) = try_attach_reporting().await;
        assert!(
            conn.is_some(),
            "a live stub socket at the own version attaches"
        );
        assert!(!displaced, "attaching never runs the Displace arm");

        drop(conn);
        server.abort();
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
            mcp_line_options: true,
            started_by: None,
            http: HttpBinding::Unrecorded,
            allowed_hosts: Vec::new(),
            standalone: None,
            runs_in: None,
            start: None,
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

    /// A one-shot command that holds the state directory is named as what
    /// it is, never mistaken for a wedged daemon: no socket probe, no
    /// signal, nothing to attach to.
    #[tokio::test]
    async fn a_standalone_holder_is_named_and_never_attached_to() {
        let home = ScratchHome::new("standalone-holder");
        let ownership = acquire_standalone_ownership("crystalline domain rename").unwrap();
        let info = read_lock_info().expect("the command published a record");
        assert_eq!(
            info.standalone.as_deref(),
            Some("crystalline domain rename")
        );
        assert_eq!(
            diagnose_holder().await,
            HolderState::Standalone {
                pid: std::process::id(),
                command: "crystalline domain rename".to_string(),
            }
        );
        assert_eq!(
            dislodge_unresponsive().await.unwrap(),
            DislodgeOutcome::NotNeeded,
            "nothing is signalled"
        );
        assert!(try_attach().await.is_none());
        drop(ownership);
        assert!(read_lock_info().is_none(), "the record goes with the lock");
        drop(home);
    }

    /// A connecting client waits for a one-shot command, bounded, and then
    /// refuses in words naming it, without spawning or signalling anything
    /// (the holder is this very test process, so a signal would end it).
    #[tokio::test]
    async fn ensure_daemon_waits_for_a_standalone_holder_and_names_it() {
        let home = ScratchHome::new("standalone-ensure");
        let ownership = acquire_standalone_ownership("crystalline sync").unwrap();
        unsafe { std::env::set_var("CRYSTALLINE_TEST_STANDALONE_WAIT_MS", "200") };
        let started = Instant::now();
        let refused = ensure_daemon(true, None, None, false).await;
        unsafe { std::env::remove_var("CRYSTALLINE_TEST_STANDALONE_WAIT_MS") };
        let err = refused.err().expect("the holder never let go").to_string();
        assert!(
            err.contains("`crystalline sync`")
                && err.contains(&format!("pid {}", std::process::id())),
            "{err}"
        );
        assert!(started.elapsed() >= Duration::from_millis(200), "it waited");
        assert!(
            read_lock_info().is_some_and(|info| info.standalone.is_some()),
            "the holder's record is untouched"
        );
        drop(ownership);
        drop(home);
    }

    /// A starting daemon that meets a one-shot command which does not let
    /// go in time refuses with [`LockHeld`], naming the command and its pid.
    #[tokio::test]
    async fn a_starting_daemon_names_a_standalone_holder_it_waited_for() {
        let home = ScratchHome::new("standalone-daemon");
        let ownership = acquire_standalone_ownership("crystalline domain rename").unwrap();
        unsafe { std::env::set_var("CRYSTALLINE_TEST_STANDALONE_WAIT_MS", "100") };
        let refused = acquire_ownership_after_standalone().await;
        unsafe { std::env::remove_var("CRYSTALLINE_TEST_STANDALONE_WAIT_MS") };
        let err = refused.err().expect("the holder never let go");
        let held = err
            .downcast_ref::<LockHeld>()
            .expect("typed for exit code 3");
        let text = held.to_string();
        assert!(
            text.contains("`crystalline domain rename`")
                && text.contains(&format!("pid {}", std::process::id())),
            "{text}"
        );
        drop(ownership);
        let owned = acquire_ownership_after_standalone().await;
        assert!(owned.is_ok(), "free once the command let go");
        drop(owned);
        drop(home);
    }

    /// A lock on a file that is no longer the one at the path is told apart
    /// from a lock on the current one, whether the path is gone or names a
    /// new file.
    #[test]
    fn a_lock_on_a_removed_lock_file_is_not_current() {
        let home = ScratchHome::new("lock-current");
        let path = config::service_lock_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let open = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .unwrap()
        };
        let file = open();
        FileExt::try_lock(&file).unwrap();
        assert!(locked_file_is_current(&file, &path));
        let _ = FileExt::unlock(&file);
        std::fs::remove_file(&path).unwrap();
        assert!(!locked_file_is_current(&file, &path), "the path is gone");
        // A new file beside a removed one that is still open: unix allows
        // it, and so does Windows with the delete semantics current NTFS
        // uses, but with the older ones the removed name stays taken until
        // `file` closes, so this half is unix only.
        #[cfg(unix)]
        {
            let fresh = open();
            assert!(!locked_file_is_current(&file, &path), "a new file is there");
            assert!(locked_file_is_current(&fresh, &path));
        }
        drop(home);
    }

    /// The race an owner's departure leaves open, forced: a process opened
    /// `service.lock` while the owner still held it and locks it the moment
    /// the owner lets go. The owner removed the file before letting go, so
    /// what that process locked is not the file at the path, and the next
    /// `acquire_ownership` owns the file that is.
    #[test]
    fn a_file_locked_as_its_owner_leaves_is_seen_as_gone() {
        let home = ScratchHome::new("lock-race");
        let ownership = acquire_ownership().unwrap();
        let path = config::service_lock_path().unwrap();
        let waiting = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(
            FileExt::try_lock(&waiting).is_err(),
            "held while the owner is there"
        );
        drop(ownership);
        FileExt::try_lock(&waiting).expect("the departed owner let the lock go");
        assert!(
            !locked_file_is_current(&waiting, &path),
            "the owner removed the file before letting it go"
        );
        // Closed before the next owner comes: on Windows with the older
        // delete semantics the removed file keeps its name while this handle
        // is open, and the next owner waits for it to go.
        drop(waiting);
        let next = acquire_ownership().expect("the next owner takes the file at the path");
        assert!(locked_file_is_current(&next.lock_file, &path));
        drop(next);
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

    /// A live process counts as Crystalline when it runs this binary's own
    /// file; a pid that cannot be a process never does.
    #[test]
    fn process_is_crystalline_knows_this_process_and_not_a_bogus_pid() {
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        assert!(process_is_crystalline(std::process::id()));
        assert!(!process_is_crystalline(0));
        // Positive as a `pid_t`, so it names one process and never a
        // process group, and above macOS's pid limit; on Linux it could be
        // live, but not a Crystalline process.
        assert!(!process_is_crystalline(4_000_000));
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
            mcp_line_options: true,
            started_by: None,
            http: HttpBinding::Unrecorded,
            allowed_hosts: Vec::new(),
            standalone: None,
            runs_in: None,
            start: None,
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

    // --- the exposure fields of the owner record -----------------------------

    /// A record written before 0.18.0 carries none of the exposure fields, and
    /// `serde(default)` must read that as "unrecorded" rather than failing the
    /// parse: a client that cannot read the record cannot displace or diagnose
    /// the daemon that wrote it.
    #[test]
    fn a_pre_exposure_record_reads_as_unrecorded() {
        let legacy = r#"{"pid":4242,"socket_path":"/tmp/s.sock","version":"0.17.0",
                         "started_at":"2026-09-10T21:33:04Z","mcp_line_options":true}"#;
        let info: LockInfo = serde_json::from_str(legacy).expect("a 0.17.0 record still parses");
        assert_eq!(info.pid, 4242);
        assert_eq!(
            info.started_by, None,
            "an older daemon recorded no start mode"
        );
        assert_eq!(info.http, HttpBinding::Unrecorded);
        assert!(info.allowed_hosts.is_empty());
    }

    /// The three shapes are distinguishable on the wire, and a bound address is a
    /// bare string so a human reading service.json sees the address itself.
    #[test]
    fn http_binding_round_trips_each_shape() {
        for binding in [
            HttpBinding::Unrecorded,
            HttpBinding::Off,
            HttpBinding::Bound("0.0.0.0:7411".to_string()),
        ] {
            let json = serde_json::to_string(&binding).unwrap();
            let back: HttpBinding = serde_json::from_str(&json).unwrap();
            assert_eq!(back, binding, "{json} round trips");
        }
        assert_eq!(
            serde_json::to_string(&HttpBinding::Bound("0.0.0.0:7411".to_string())).unwrap(),
            "\"0.0.0.0:7411\""
        );
        assert_eq!(serde_json::to_string(&HttpBinding::Off).unwrap(), "\"off\"");
    }

    /// The intent is recorded once per process and the first call wins, so a
    /// record can never disagree with the `/health` body of the same daemon.
    ///
    /// This touches a process-global `OnceLock`. Under `cargo nextest` every
    /// test gets its own process, so it is isolated for free; under the
    /// canonical `cargo test --workspace` fallback it shares the process with
    /// every other test in this module. So it must stay the only test in this
    /// file that calls `record_serve_intent`, and no test here may call
    /// `publish()` and then assert on the exposure fields it writes.
    #[test]
    fn the_first_recorded_serve_intent_wins() {
        record_serve_intent(ServeIntent {
            started_by: StartMode::Serve,
            http: HttpBinding::Bound("127.0.0.1:7411".into()),
            allowed_hosts: vec!["muthur.lan".into()],
        });
        record_serve_intent(ServeIntent {
            started_by: StartMode::Autostart,
            http: HttpBinding::Off,
            allowed_hosts: vec![],
        });
        let intent = serve_intent().expect("recorded");
        assert_eq!(intent.started_by, StartMode::Serve);
        assert_eq!(intent.http, HttpBinding::Bound("127.0.0.1:7411".into()));
        assert_eq!(intent.allowed_hosts, vec!["muthur.lan".to_string()]);
    }

    /// A holder record for the refusal tests, with the exposure facts each one
    /// wants and everything else held still.
    fn holder(
        pid: u32,
        version: &str,
        http: HttpBinding,
        started_by: Option<StartMode>,
    ) -> LockInfo {
        LockInfo {
            pid,
            socket_path: "/tmp/s.sock".to_string(),
            version: version.to_string(),
            started_at: "2026-09-10T21:33:04Z".to_string(),
            mcp_line_options: true,
            started_by,
            http,
            allowed_hosts: vec![],
            standalone: None,
            runs_in: None,
            start: None,
        }
    }

    /// The outage shape: a managed serve asked for the LAN endpoint and an
    /// autostarted daemon already holds the index on loopback. The message has
    /// to carry all four facts, because the fifth - that the LAN endpoint is
    /// now gone - is the one an operator is actually losing.
    #[test]
    fn the_refusal_names_both_addresses_and_the_config_key() {
        let intent = ServeIntent {
            started_by: StartMode::Serve,
            http: HttpBinding::Bound("0.0.0.0:7411".into()),
            allowed_hosts: vec!["muthur.lan".into(), "nostromo.lan".into()],
        };
        let held = holder(
            856,
            "0.18.0",
            HttpBinding::Bound("127.0.0.1:7411".into()),
            Some(StartMode::Autostart),
        );
        let msg = lock_held_message(Some(&intent), Some(&held));
        assert!(
            msg.contains("0.0.0.0:7411"),
            "what this invocation asked for: {msg}"
        );
        assert!(
            msg.contains("127.0.0.1:7411"),
            "what the holder bound: {msg}"
        );
        assert!(msg.contains("856"), "the holder's pid: {msg}");
        assert!(msg.contains("autostart"), "how the holder started: {msg}");
        assert!(
            msg.contains("service.http"),
            "the key that reconciles them: {msg}"
        );
        assert!(
            msg.contains("service.allowed_hosts"),
            "and the allow-list key: {msg}"
        );
        assert!(
            msg.contains("muthur.lan"),
            "the allow-list this invocation asked for: {msg}"
        );
        // The prose join and the command join differ on purpose: a space in
        // the command form would split argv, and `parse_allowed_hosts` rejects
        // an entry carrying whitespace, so a pasted command would fail.
        assert!(
            msg.contains("service.allowed_hosts muthur.lan,nostromo.lan"),
            "the command form joins the hosts with no space, the spelling the setting takes: {msg}"
        );
        assert!(
            msg.contains("Host values muthur.lan, nostromo.lan"),
            "while the prose reads as prose: {msg}"
        );
    }

    /// A holder that published no record at all: a process holding the lock
    /// without ever writing service.json. Saying "pid 0" would be a lie, so the
    /// message says the record is missing and still names the key.
    #[test]
    fn the_refusal_is_honest_when_the_holder_published_no_record() {
        let intent = ServeIntent {
            started_by: StartMode::Serve,
            http: HttpBinding::Bound("0.0.0.0:7411".into()),
            allowed_hosts: vec![],
        };
        let msg = lock_held_message(Some(&intent), None);
        assert!(
            msg.contains("published no service record"),
            "the message says the holder is unidentifiable rather than inventing a pid: {msg}"
        );
        assert!(!msg.contains("pid 0"), "never a fabricated pid: {msg}");
        assert!(msg.contains("service.http"), "{msg}");
    }

    /// A holder that recorded no binding: a daemon from before the field, and
    /// equally the `hold-lock` test command, which takes the lock and serves
    /// nothing. The message must not claim its endpoint is off, and must not
    /// blame a version it cannot know.
    #[test]
    fn the_refusal_does_not_claim_an_unrecording_holder_serves_nothing() {
        let intent = ServeIntent {
            started_by: StartMode::Serve,
            http: HttpBinding::Bound("0.0.0.0:7411".into()),
            allowed_hosts: vec![],
        };
        let held = holder(4242, "0.17.0", HttpBinding::Unrecorded, None);
        let msg = lock_held_message(Some(&intent), Some(&held));
        assert!(msg.contains("did not record"), "{msg}");
        assert!(
            !msg.contains("no HTTP endpoint"),
            "an unrecorded binding is not an off one: {msg}"
        );
        assert!(
            !msg.contains("version"),
            "an unrecorded field is not evidence of an older version: {msg}"
        );
    }

    /// Called from a process that recorded no intent (nothing in this tree
    /// does, but the renderer is public and must not panic or lie).
    #[test]
    fn the_refusal_without_a_recorded_intent_still_names_the_holder() {
        let held = holder(
            856,
            "0.18.0",
            HttpBinding::Bound("127.0.0.1:7411".into()),
            Some(StartMode::Serve),
        );
        let msg = lock_held_message(None, Some(&held));
        assert!(msg.contains("856"), "{msg}");
        assert!(msg.contains("127.0.0.1:7411"), "{msg}");
        // A caller with no intent asked to bind nothing, so exposure advice is
        // a non sequitur for it: the embedded MCP stack and `hold-lock` reach
        // this shape, and the embedded stack copies the string into the
        // `status` payload an agent reads. What it gets back is the advice
        // that applies to it.
        assert!(
            !msg.contains("Exposure belongs to configuration"),
            "no exposure lecture for a caller that asked to bind nothing: {msg}"
        );
        assert!(
            !msg.contains("config set service.http"),
            "and no command to reconcile a binding it never asked for: {msg}"
        );
        assert!(
            msg.contains("attach over the socket"),
            "the advice that does apply: the holder is a daemon this caller can use: {msg}"
        );
    }

    /// An invocation whose exposure is exactly the holder's. Telling an
    /// operator to write down what both sides already asked for is advice that
    /// changes nothing, so the message says only that the index is owned and
    /// by whom. The Windows second-serve test is this shape, and so is every
    /// restart of a unit against its own autostarted daemon.
    #[test]
    fn the_refusal_skips_the_exposure_advice_when_both_sides_asked_the_same() {
        let intent = ServeIntent {
            started_by: StartMode::Serve,
            http: HttpBinding::Off,
            allowed_hosts: vec![],
        };
        let held = holder(856, "0.18.0", HttpBinding::Off, Some(StartMode::Autostart));
        let msg = lock_held_message(Some(&intent), Some(&held));
        assert!(
            !msg.contains("Exposure belongs to configuration"),
            "the two sides agree, so there is nothing to reconcile: {msg}"
        );
        assert!(
            !msg.contains("config set service.http"),
            "and no command that would change nothing: {msg}"
        );
        assert!(msg.contains("856"), "it still names the holder: {msg}");
        assert!(
            msg.contains("crystalline ctl shutdown"),
            "and still gives the one thing that helps: {msg}"
        );
    }

    /// Two holders that recorded nothing are not two holders that agree. An
    /// unrecorded binding is ignorance, and suppressing the advice on it would
    /// hide the key from the very operator who cannot read the holder's.
    #[test]
    fn an_unrecorded_binding_on_both_sides_is_not_agreement() {
        let intent = ServeIntent {
            started_by: StartMode::Serve,
            http: HttpBinding::Unrecorded,
            allowed_hosts: vec![],
        };
        let held = holder(4242, "0.17.0", HttpBinding::Unrecorded, None);
        let msg = lock_held_message(Some(&intent), Some(&held));
        assert!(
            msg.contains("Exposure belongs to configuration"),
            "neither side is known, so the key is still worth naming: {msg}"
        );
        assert!(msg.contains("config set service.http <host:port>"), "{msg}");
    }

    /// The addresses agree but the allow-lists do not, which is still a
    /// difference an operator has to write down somewhere.
    #[test]
    fn the_refusal_keeps_the_advice_when_only_the_allow_lists_differ() {
        let intent = ServeIntent {
            started_by: StartMode::Serve,
            http: HttpBinding::Bound("0.0.0.0:7411".into()),
            allowed_hosts: vec!["muthur.lan".into()],
        };
        let mut held = holder(
            856,
            "0.18.0",
            HttpBinding::Bound("0.0.0.0:7411".into()),
            Some(StartMode::Autostart),
        );
        held.allowed_hosts = vec![];
        let msg = lock_held_message(Some(&intent), Some(&held));
        assert!(
            msg.contains("service.allowed_hosts muthur.lan"),
            "the half that differs is still named: {msg}"
        );
    }

    fn packaged() -> crate::runs_in::PackageContext {
        crate::runs_in::PackageContext::Packaged {
            full_name: "Claude_1.0.0.0_x64__pzs8sxrjxfjjc".to_string(),
        }
    }

    #[test]
    fn a_packaged_client_never_displaces_even_when_it_is_newer() {
        assert_eq!(
            attach_policy_for("0.0.1", "0.24.0", &packaged()),
            AttachPolicy::Attach
        );
        assert_eq!(
            attach_policy_for("0.24.0", "0.24.0", &packaged()),
            AttachPolicy::Attach
        );
    }

    #[test]
    fn an_unpackaged_client_still_displaces_an_older_daemon() {
        let here = crate::runs_in::PackageContext::Unpackaged;
        assert_eq!(
            attach_policy_for("0.0.1", "0.24.0", &here),
            AttachPolicy::Displace
        );
        assert_eq!(
            attach_policy_for("0.25.0", "0.24.0", &here),
            AttachPolicy::Attach
        );
    }

    /// A stand-in for Task Scheduler: `run` starts a scripted daemon on this
    /// state folder's pipe the first time it is called and counts every call.
    #[cfg(unix)]
    struct FakeTask {
        name: String,
        registered: bool,
        fails: bool,
        starts: bool,
        runs: std::sync::atomic::AtomicUsize,
        started: std::sync::atomic::AtomicBool,
    }

    #[cfg(unix)]
    impl FakeTask {
        fn new(registered: bool, fails: bool, starts: bool) -> FakeTask {
            FakeTask {
                name: crate::daemon_task::MACHINE_TASK_NAME.to_string(),
                registered,
                fails,
                starts,
                runs: std::sync::atomic::AtomicUsize::new(0),
                started: std::sync::atomic::AtomicBool::new(false),
            }
        }

        /// The same task under the per-user name `doctor --fix` registers.
        fn for_user(mut self, user: &str) -> FakeTask {
            self.name = crate::daemon_task::user_task_name(user);
            self
        }
    }

    #[cfg(unix)]
    impl crate::daemon_task::DaemonTask for FakeTask {
        fn find(&self) -> Option<String> {
            self.registered.then(|| self.name.clone())
        }
        fn run(&self, _name: &str) -> Result<(), String> {
            use std::sync::atomic::Ordering;
            self.runs.fetch_add(1, Ordering::SeqCst);
            if self.fails {
                return Err("the operator or administrator has refused the request".into());
            }
            if self.starts && !self.started.swap(true, Ordering::SeqCst) {
                let sock = config::service_sock_path().unwrap();
                let listener = ListenerOptions::new()
                    .name(socket_name(&sock).unwrap())
                    .create_tokio()
                    .unwrap();
                let facts = serde_json::json!({
                    "pid": std::process::id(), "version": crystalline_core::VERSION,
                    "mcp_line_options": true
                });
                let (task, _) = scripted_daemon(listener, Some(facts), serde_json::json!({}));
                std::mem::forget(task);
            }
            Ok(())
        }
    }

    /// Nothing a packaged client may write exists in the state folder.
    #[cfg(unix)]
    fn assert_wrote_nothing(state: &Path) {
        for name in [
            "service.lock",
            "service.json",
            "daemon.log",
            "index.db",
            "instance-id",
            "tmp",
        ] {
            assert!(!state.join(name).exists(), "{name} was written");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_packaged_client_with_a_live_older_daemon_attaches_and_never_displaces() {
        let home = ScratchHome::new("pkg-attach");
        let sock = config::service_sock_path().unwrap();
        let listener = ListenerOptions::new()
            .name(socket_name(&sock).unwrap())
            .create_tokio()
            .unwrap();
        let facts = serde_json::json!({ "pid": std::process::id(), "version": "0.0.1" });
        let (server, seen) = scripted_daemon(listener, Some(facts), serde_json::json!({}));
        let (conn, displaced) = try_attach_displacing_in(&packaged()).await;
        assert!(conn.is_some(), "the older daemon is attached as it is");
        assert!(displaced.is_none());
        assert!(
            !seen.lock().unwrap().contains(&"shutdown".to_string()),
            "never asked to leave"
        );
        server.abort();
        drop(home);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_packaged_client_with_no_daemon_runs_the_task_then_attaches() {
        let home = ScratchHome::new("pkg-task");
        let task = FakeTask::new(true, false, true);
        let conn = ensure_daemon_in(
            &packaged(),
            &task,
            &SpawnOptions::default(),
            Duration::from_secs(5),
        )
        .await
        .expect("the task started a daemon and the bridge attached");
        drop(conn);
        assert_eq!(task.runs.load(std::sync::atomic::Ordering::SeqCst), 1);
        drop(home);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_packaged_client_whose_task_is_missing_says_so_and_writes_nothing() {
        let home = ScratchHome::new("pkg-missing");
        let state = config::state_dir().unwrap();
        let task = FakeTask::new(false, false, false);
        let err = ensure_daemon_in(
            &packaged(),
            &task,
            &SpawnOptions::default(),
            Duration::from_secs(1),
        )
        .await
        .err()
        .expect("no daemon and no task");
        assert_eq!(
            err.downcast_ref::<crate::daemon_task::BridgeFailure>(),
            Some(&crate::daemon_task::BridgeFailure::TaskMissing)
        );
        assert_wrote_nothing(&state);
        drop(home);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_packaged_client_whose_task_fails_says_it_did_not_start() {
        let home = ScratchHome::new("pkg-fail");
        let task = FakeTask::new(true, true, false).for_user("ada");
        let err = ensure_daemon_in(
            &packaged(),
            &task,
            &SpawnOptions::default(),
            Duration::from_secs(1),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(
            err.downcast_ref::<crate::daemon_task::BridgeFailure>(),
            Some(crate::daemon_task::BridgeFailure::TaskDidNotStart { task, detail })
                if detail.contains("refused") && task == r"\Crystalline Daemon for ada"
        ));
        drop(home);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_packaged_client_gives_up_when_no_daemon_answers_in_time() {
        let home = ScratchHome::new("pkg-silent");
        let state = config::state_dir().unwrap();
        let task = FakeTask::new(true, false, false).for_user("ada");
        let started = Instant::now();
        let err = ensure_daemon_in(
            &packaged(),
            &task,
            &SpawnOptions::default(),
            Duration::from_millis(400),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            err.downcast_ref::<crate::daemon_task::BridgeFailure>(),
            Some(&crate::daemon_task::BridgeFailure::NoAnswer {
                task: r"\Crystalline Daemon for ada".to_string()
            })
        );
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "the wait is bounded"
        );
        assert_wrote_nothing(&state);
        drop(home);
    }

    /// Claude Desktop starts two bridges at once.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_packaged_bridges_with_no_daemon_both_attach_to_one() {
        let home = ScratchHome::new("pkg-two");
        let state = config::state_dir().unwrap();
        let task = std::sync::Arc::new(FakeTask::new(true, false, true));
        let here = packaged();
        let options = SpawnOptions::default();
        let (a, b) = tokio::join!(
            ensure_daemon_in(&here, &*task, &options, Duration::from_secs(5)),
            ensure_daemon_in(&here, &*task, &options, Duration::from_secs(5)),
        );
        assert!(a.is_ok() && b.is_ok(), "both bridges reach the one daemon");
        let runs = task.runs.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            (1..=2).contains(&runs),
            "each bridge runs the task at most once: {runs}"
        );
        assert!(task.started.load(std::sync::atomic::Ordering::SeqCst));
        assert!(
            !state.join("service.lock").exists(),
            "neither bridge took the index"
        );
        drop(home);
    }

    /// A listener that gives no facts (it refuses `holder`, its `status`
    /// names no pid) and no usable record beside it: `ask_holder_at` says
    /// `None`, but a daemon is there. A packaged bridge connects to it as it
    /// is and never runs the task.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_packaged_client_connects_to_a_listener_that_gives_no_facts() {
        let home = ScratchHome::new("pkg-bare");
        let sock = config::service_sock_path().unwrap();
        let listener = ListenerOptions::new()
            .name(socket_name(&sock).unwrap())
            .create_tokio()
            .unwrap();
        let (server, seen) = scripted_daemon(listener, None, serde_json::json!({}));
        assert!(
            ask_holder_at(&sock).await.is_none(),
            "the pipe gives no facts"
        );
        let task = FakeTask::new(true, false, false);
        let conn = ensure_daemon_in(
            &packaged(),
            &task,
            &SpawnOptions::default(),
            Duration::from_secs(1),
        )
        .await
        .expect("the bridge connects to the listener as it is");
        drop(conn);
        assert_eq!(
            task.runs.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no task run"
        );
        assert!(
            !seen.lock().unwrap().contains(&"shutdown".to_string()),
            "never asked to leave"
        );
        server.abort();
        drop(home);
    }

    #[test]
    fn the_older_daemon_warning_names_both_versions_and_the_fix() {
        let text = older_daemon_warning("0.23.1", "0.24.0");
        assert!(
            text.contains("v0.23.1")
                && text.contains("v0.24.0")
                && text.contains("crystalline status"),
            "{text}"
        );
        assert!(
            !text.contains('\u{2014}') && !text.contains('\u{2013}'),
            "{text}"
        );
    }
}
