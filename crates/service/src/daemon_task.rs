//! The Windows Task Scheduler task that starts the daemon outside any app
//! package, `\Crystalline\Daemon`, and how a bridge inside Claude Desktop's
//! package reaches it. The MSI registers the task (`crystalline daemon-task
//! register --all-users`), `crystalline doctor --fix` registers one for the
//! current user when it is missing, and a packaged `crystalline mcp` runs it
//! on demand when no daemon answers.

/// The task the MSI registers for every user.
pub const MACHINE_TASK_NAME: &str = r"\Crystalline\Daemon";

/// The debug-build seam standing in for Task Scheduler: `missing` (no task),
/// `fail` (the run is refused) or `serve` (the run starts this binary's own
/// daemon). A release build never reads it.
pub const TEST_DAEMON_TASK_ENV: &str = "CRYSTALLINE_TEST_DAEMON_TASK";

/// The per-user task `doctor --fix` registers: in the root folder, where a
/// standard user may create a task for themself, and never under the machine
/// name.
pub fn user_task_name(user: &str) -> String {
    format!(r"\Crystalline Daemon for {user}")
}

/// Task Scheduler, as the bridge needs it. A trait so a test stands in for
/// the real `schtasks.exe`.
pub trait DaemonTask: Send + Sync {
    /// The name of a registered task this process may run, if any.
    fn find(&self) -> Option<String>;
    /// Start the task now.
    fn run(&self, name: &str) -> Result<(), String>;
}

/// Why a packaged bridge has no daemon to relay to. Each one is its own
/// sentence in the degraded server's copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeFailure {
    TaskMissing,
    TaskDidNotStart(String),
    NoAnswer,
    /// A daemon answered on the pipe, but the MCP handshake with it failed.
    HandshakeFailed(String),
}

impl BridgeFailure {
    /// The cause of a failed packaged attach: the [`BridgeFailure`] it
    /// carries, or else the failed MCP handshake, which is the only other way
    /// a packaged attach fails. For a handshake only the innermost cause is
    /// kept: the sentence around it already says the handshake failed.
    pub fn of(e: &anyhow::Error) -> BridgeFailure {
        e.downcast_ref::<BridgeFailure>()
            .cloned()
            .unwrap_or_else(|| BridgeFailure::HandshakeFailed(e.root_cause().to_string()))
    }
}

impl std::fmt::Display for BridgeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeFailure::TaskMissing => write!(
                f,
                "the Windows task {MACHINE_TASK_NAME} that starts the Crystalline daemon is missing"
            ),
            BridgeFailure::TaskDidNotStart(detail) => {
                write!(
                    f,
                    "the Windows task {MACHINE_TASK_NAME} did not start ({detail})"
                )
            }
            BridgeFailure::NoAnswer => write!(
                f,
                "the Windows task {MACHINE_TASK_NAME} started, but no Crystalline daemon answered within {} s",
                crate::instance::PACKAGED_TASK_WAIT.as_secs()
            ),
            BridgeFailure::HandshakeFailed(detail) => write!(
                f,
                "the Crystalline daemon answered, but it did not finish the MCP handshake ({detail})"
            ),
        }
    }
}

impl std::error::Error for BridgeFailure {}

/// The task runner this process uses: the debug seam when it is set, else
/// none at all for now (the real `schtasks.exe` runner comes later).
pub fn for_this_process() -> Box<dyn DaemonTask> {
    #[cfg(debug_assertions)]
    if let Ok(seam) = std::env::var(TEST_DAEMON_TASK_ENV)
        && !seam.is_empty()
    {
        return Box::new(Seam(seam));
    }
    Box::new(NoTask)
}

/// No Task Scheduler (any OS but Windows, until the real runner lands).
struct NoTask;

impl DaemonTask for NoTask {
    fn find(&self) -> Option<String> {
        None
    }
    fn run(&self, _name: &str) -> Result<(), String> {
        Err("Task Scheduler exists only on Windows".to_string())
    }
}

#[cfg(debug_assertions)]
struct Seam(String);

#[cfg(debug_assertions)]
impl DaemonTask for Seam {
    fn find(&self) -> Option<String> {
        (self.0 != "missing").then(|| MACHINE_TASK_NAME.to_string())
    }
    fn run(&self, _name: &str) -> Result<(), String> {
        match self.0.as_str() {
            "fail" => Err("the test task was refused".to_string()),
            "serve" => {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let mut cmd = std::process::Command::new(exe);
                cmd.args(["serve", "--daemon", "--autostarted"])
                    .env_remove(crate::runs_in::TEST_PACKAGE_ENV)
                    .env_remove(TEST_DAEMON_TASK_ENV)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
            }
            other => Err(format!("unknown {TEST_DAEMON_TASK_ENV} value '{other}'")),
        }
    }
}
