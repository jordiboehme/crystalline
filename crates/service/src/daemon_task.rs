//! The Windows Task Scheduler task that starts the daemon outside any app
//! package, `\Crystalline\Daemon`, and how a bridge inside Claude Desktop's
//! package reaches it. The MSI registers the task (`crystalline daemon-task
//! register --all-users`), `crystalline doctor --fix` registers one for the
//! current user when it is missing, and a packaged `crystalline mcp` runs it
//! on demand when no daemon answers.

use std::path::{Path, PathBuf};

/// The task the MSI registers for every user.
pub const MACHINE_TASK_NAME: &str = r"\Crystalline\Daemon";

/// The debug-build seam standing in for Task Scheduler: `missing` (no task),
/// `fail` (the run is refused), `serve` (the run starts this binary's own
/// daemon), `system` (as `serve`, but the started daemon gets none of the
/// shaping variables set here, as a daemon Task Scheduler starts) or
/// `legacy` (this user's task is there under the 0.24.0 name) or `refused`
/// (Task Scheduler refuses the query). A release build never reads it.
pub const TEST_DAEMON_TASK_ENV: &str = "CRYSTALLINE_TEST_DAEMON_TASK";

/// The per-user task `doctor --fix` and `daemon-task register` register:
/// in the root folder, where a standard user may create a task for
/// themself, never under the machine name, and named for the user and
/// their SID, so two accounts that share a user name (a local `bob` and
/// `CORP\bob`) never share one name.
pub fn user_task_name(user: &str, sid: &str) -> String {
    format!(r"\Crystalline Daemon for {user} ({sid})")
}

/// The name 0.24.0 gave the per-user task, without the SID. Found, run,
/// moved and removed only when its definition runs as this account
/// ([`runs_as`]); another account's task under it is left alone.
pub fn legacy_user_task_name(user: &str) -> String {
    format!(r"\Crystalline Daemon for {user}")
}

/// Task Scheduler, as the bridge needs it. A trait so a test stands in for
/// the real `schtasks.exe`.
pub trait DaemonTask: Send + Sync {
    /// The name of a registered task this process may run: `Ok(None)` when
    /// none is there, `Err` when Task Scheduler refused a query and nothing
    /// else was found.
    fn find(&self) -> Result<Option<String>, Refused>;
    /// Start the task now.
    fn run(&self, name: &str) -> Result<(), String>;
    /// The definition of the task `name`, as Task Scheduler holds it now.
    /// `None` when it cannot be read, and always for a stand-in, so a test
    /// never reads the real Task Scheduler through it.
    fn definition(&self, _name: &str) -> Option<String> {
        None
    }
    /// This user's task under the 0.24.0 name, when it is there and runs
    /// as this account: `doctor` reports it and `--fix` moves it. `None`
    /// for a stand-in that has none.
    fn legacy(&self) -> Option<String> {
        None
    }
}

/// Why a packaged bridge has no daemon to relay to. Each one is its own
/// sentence in the degraded server's copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeFailure {
    TaskMissing,
    /// The task `task` (the one the bridge found and ran) refused to start.
    TaskDidNotStart {
        task: String,
        detail: String,
    },
    /// The task `task` started, but no daemon answered in time.
    NoAnswer {
        task: String,
    },
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
            BridgeFailure::TaskDidNotStart { task, detail } => {
                write!(f, "the Windows task {task} did not start ({detail})")
            }
            BridgeFailure::NoAnswer { task } => write!(
                f,
                "the Windows task {task} started, but no Crystalline daemon answered within {} s",
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
/// the real Task Scheduler through `schtasks.exe` (which answers "only on
/// Windows" on any other OS).
pub fn for_this_process() -> Box<dyn DaemonTask> {
    #[cfg(debug_assertions)]
    if let Ok(seam) = std::env::var(TEST_DAEMON_TASK_ENV)
        && !seam.is_empty()
    {
        return Box::new(Seam(seam));
    }
    Box::new(Schtasks::for_this_user())
}

#[cfg(debug_assertions)]
struct Seam(String);

#[cfg(debug_assertions)]
impl DaemonTask for Seam {
    fn find(&self) -> Result<Option<String>, Refused> {
        match self.0.as_str() {
            "missing" => Ok(None),
            "refused" => Err(Refused {
                task: MACHINE_TASK_NAME.to_string(),
                detail: "the test query was refused".to_string(),
            }),
            "legacy" => Ok(Some(legacy_user_task_name(&ThisUser::here().name))),
            _ => Ok(Some(MACHINE_TASK_NAME.to_string())),
        }
    }
    fn legacy(&self) -> Option<String> {
        (self.0 == "legacy").then(|| legacy_user_task_name(&ThisUser::here().name))
    }
    fn run(&self, _name: &str) -> Result<(), String> {
        match self.0.as_str() {
            "fail" => Err("the test task was refused".to_string()),
            "serve" | "system" => {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                let mut cmd = std::process::Command::new(exe);
                cmd.args(["serve", "--daemon", "--autostarted"])
                    .env_remove(crate::runs_in::TEST_PACKAGE_ENV)
                    .env_remove(TEST_DAEMON_TASK_ENV)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                if self.0 == "system" {
                    for name in crate::shaping::shaping_set_here() {
                        cmd.env_remove(name);
                    }
                }
                cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
            }
            other => Err(format!("unknown {TEST_DAEMON_TASK_ENV} value '{other}'")),
        }
    }
}

/// Who the task runs for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskPrincipal {
    /// Every member of the built-in Users group, each in their own session:
    /// the MSI's task.
    AllUsers,
    /// One account, `DOMAIN\name`: what `doctor --fix` registers.
    User { account: String },
}

/// Read and run for every user, full control for SYSTEM and the
/// administrators: so a standard user may start a task an administrator
/// registered. Only the machine task carries it: a user's own task keeps
/// Task Scheduler's default, which leaves its creator full rights, so that
/// user can replace it (`doctor --fix`) or delete it.
pub const TASK_SDDL: &str = "D:(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGX;;;BU)";

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The task definition, as `schtasks /Create /XML` reads it: the built-in
/// Users group by SID (so a translated Windows works) or one account, a
/// sign-in trigger, no time limit, no battery rule, a start on demand, and
/// `MultipleInstancesPolicy` `Parallel`, because the policy is per task and
/// a second user signing in must get a daemon of their own; the daemon's
/// lock keeps one per user.
pub fn task_xml(exe: &Path, principal: &TaskPrincipal) -> String {
    let (security, description, trigger, who) = match principal {
        TaskPrincipal::AllUsers => (
            format!("    <SecurityDescriptor>{TASK_SDDL}</SecurityDescriptor>\n"),
            "Starts the Crystalline daemon for whoever signs in, outside any app package.",
            "    <LogonTrigger>\n      <Enabled>true</Enabled>\n    </LogonTrigger>\n".to_string(),
            "    <Principal id=\"Author\">\n      <GroupId>S-1-5-32-545</GroupId>\n      \
             <RunLevel>LeastPrivilege</RunLevel>\n    </Principal>\n"
                .to_string(),
        ),
        TaskPrincipal::User { account } => {
            let account = xml_escape(account);
            (
                String::new(),
                "Starts the Crystalline daemon for this user when they sign in, outside any app package.",
                format!(
                    "    <LogonTrigger>\n      <Enabled>true</Enabled>\n      \
                     <UserId>{account}</UserId>\n    </LogonTrigger>\n"
                ),
                format!(
                    "    <Principal id=\"Author\">\n      <UserId>{account}</UserId>\n      \
                     <LogonType>InteractiveToken</LogonType>\n      \
                     <RunLevel>LeastPrivilege</RunLevel>\n    </Principal>\n"
                ),
            )
        }
    };
    let exe = xml_escape(&exe.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.3" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
{security}    <Author>Crystalline</Author>
    <Description>{description}</Description>
  </RegistrationInfo>
  <Triggers>
{trigger}  </Triggers>
  <Principals>
{who}  </Principals>
  <Settings>
    <MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>"{exe}"</Command>
      <Arguments>serve --daemon --from-task</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// UTF-16LE with a byte order mark, the encoding `schtasks /XML` reads
/// without guessing.
pub fn utf16_with_bom(xml: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in xml.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// `DOMAIN\name` of the signed-in user, from the environment Windows sets.
pub fn current_account() -> String {
    match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(name)) if !domain.is_empty() => format!(r"{domain}\{name}"),
        (_, Ok(name)) => name,
        _ => String::new(),
    }
}

/// Who this process runs as, for the per-user task: the user name Windows
/// sets (`USERNAME`), the account (`DOMAIN\name`, [`current_account`]) and
/// its SID ([`current_sid`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThisUser {
    pub name: String,
    pub account: String,
    pub sid: Option<String>,
}

impl ThisUser {
    pub fn here() -> ThisUser {
        ThisUser {
            name: std::env::var("USERNAME").unwrap_or_default(),
            account: current_account(),
            sid: current_sid(),
        }
    }

    fn legacy_name(&self) -> Option<String> {
        (!self.name.is_empty()).then(|| legacy_user_task_name(&self.name))
    }
}

/// This account's SID as text (`S-1-5-21-...`), read from the process
/// token: the account [`current_account`] names. `None` off Windows and
/// when the token cannot be read.
pub fn current_sid() -> Option<String> {
    #[cfg(windows)]
    {
        token_user_sid()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn token_user_sid() -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: this process's pseudo handle and an out pointer for the token.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let mut needed = 0u32;
    // SAFETY: a size query: no buffer, the size is written to `needed`.
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    // Whole u64 words, so the buffer is aligned for TOKEN_USER.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    // SAFETY: a buffer of at least `needed` bytes, aligned for TOKEN_USER.
    let filled = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut needed,
        )
    };
    // SAFETY: the token opened above, closed once.
    unsafe { CloseHandle(token) };
    if filled == 0 {
        return None;
    }
    // SAFETY: the call filled the buffer with a TOKEN_USER, whose SID points
    // into the same buffer, which lives to the end of this function.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut text: windows_sys::core::PWSTR = std::ptr::null_mut();
    // SAFETY: a valid SID and an out pointer the call fills with a string it
    // allocates with LocalAlloc.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 || text.is_null() {
        return None;
    }
    let mut len = 0usize;
    // SAFETY: the call returned a NUL-terminated wide string.
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` wide chars before the NUL, all initialized.
    let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    // SAFETY: frees what ConvertSidToStringSidW allocated.
    unsafe { LocalFree(text.cast()) };
    Some(sid)
}

/// The task names a bridge looks for, in order: the machine task, then this
/// user's own under its name with the SID, then under the 0.24.0 name.
pub fn candidate_names(me: &ThisUser) -> Vec<String> {
    let mut names = vec![MACHINE_TASK_NAME.to_string()];
    if let Some(sid) = me.sid.as_deref().filter(|_| !me.name.is_empty()) {
        names.push(user_task_name(&me.name, sid));
    }
    names.extend(me.legacy_name());
    names
}

/// The names a registration for `principal` may use: the machine name for
/// every user, only the user's own name with their SID for one user. A
/// user's task never takes the machine name, or every other user's bridge
/// would find it first and wait for a daemon that never comes for them.
pub(crate) fn registration_names(principal: &TaskPrincipal, me: &ThisUser) -> Vec<String> {
    match principal {
        TaskPrincipal::AllUsers => vec![MACHINE_TASK_NAME.to_string()],
        TaskPrincipal::User { .. } => me
            .sid
            .as_deref()
            .filter(|_| !me.name.is_empty())
            .map(|sid| vec![user_task_name(&me.name, sid)])
            .unwrap_or_default(),
    }
}

/// The `<UserId>` texts inside the `<Principals>` block of `xml`, trimmed.
/// The logon trigger's `<UserId>` is outside it and never counts. Tags
/// compare without case; ASCII lowercasing keeps every byte offset.
fn principal_user_ids(xml: &str) -> Vec<&str> {
    let lower = xml.to_ascii_lowercase();
    let (Some(start), Some(end)) = (lower.find("<principals>"), lower.find("</principals>")) else {
        return Vec::new();
    };
    if start >= end {
        return Vec::new();
    }
    let (block, lower_block) = (&xml[start..end], &lower[start..end]);
    let mut ids = Vec::new();
    let mut from = 0;
    while let Some(open) = lower_block[from..]
        .find("<userid>")
        .map(|i| from + i + "<userid>".len())
    {
        let Some(close) = lower_block[open..].find("</userid>").map(|i| open + i) else {
            break;
        };
        ids.push(block[open..close].trim());
        from = close + "</userid>".len();
    }
    ids
}

/// Whether `a` and `b` are the same `DOMAIN\name`: both carry a domain and
/// both halves match without case. A bare name proves nothing: a local
/// `bob` and `CORP\bob` share it.
fn same_qualified_account(a: &str, b: &str) -> bool {
    match (a.rsplit_once('\\'), b.rsplit_once('\\')) {
        (Some((a_domain, a_name)), Some((b_domain, b_name))) => {
            !a_domain.is_empty()
                && a_domain.to_lowercase() == b_domain.to_lowercase()
                && a_name.to_lowercase() == b_name.to_lowercase()
        }
        _ => false,
    }
}

/// Whether the task defined by `xml` runs as `me`: its principal names this
/// account's SID or its full `DOMAIN\name`. Anything less, an unreadable
/// definition included, is not ours.
pub(crate) fn runs_as(xml: &str, me: &ThisUser) -> bool {
    principal_user_ids(xml).into_iter().any(|id| {
        me.sid
            .as_deref()
            .is_some_and(|sid| id.eq_ignore_ascii_case(sid))
            || same_qualified_account(id, &me.account)
    })
}

/// This user's 0.24.0-named task, when `exists` finds it and its
/// `definition` runs as `me`.
pub(crate) fn legacy_with(
    me: &ThisUser,
    mut exists: impl FnMut(&str) -> bool,
    mut definition: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    let legacy = me.legacy_name()?;
    (exists(&legacy) && definition(&legacy).is_some_and(|xml| runs_as(&xml, me))).then_some(legacy)
}

/// The first of `me`'s candidate names that is registered and may be run:
/// the 0.24.0 name only when it is this account's own. The first refused
/// query is the error when no name is found.
pub(crate) fn find_with(
    me: &ThisUser,
    mut query: impl FnMut(&str) -> Query,
    mut definition: impl FnMut(&str) -> Option<String>,
) -> Result<Option<String>, Refused> {
    let legacy = me.legacy_name();
    let mut refused = None;
    for name in candidate_names(me) {
        match query(&name) {
            Query::Absent => {}
            Query::Refused(detail) => {
                refused.get_or_insert(Refused { task: name, detail });
            }
            Query::Present => {
                if legacy.as_ref() != Some(&name)
                    || definition(&name).is_some_and(|xml| runs_as(&xml, me))
                {
                    return Ok(Some(name));
                }
            }
        }
    }
    refused.map_or(Ok(None), Err)
}

/// What moving the 0.24.0-named task did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moved {
    /// There was none of this user's.
    Nothing,
    /// Removed, after the new name was registered.
    Removed(String),
    /// Task Scheduler would not remove it; the new task exists all the same.
    Refused { task: String, why: String },
}

/// Remove this user's 0.24.0-named task. Only a caller that has just
/// registered the new name, or that found the machine task ready, calls
/// this.
pub(crate) fn move_legacy_with(
    me: &ThisUser,
    exists: impl FnMut(&str) -> bool,
    definition: impl FnMut(&str) -> Option<String>,
    mut delete: impl FnMut(&str) -> Result<(), String>,
) -> Moved {
    match legacy_with(me, exists, definition) {
        None => Moved::Nothing,
        Some(task) => match delete(&task) {
            Ok(()) => Moved::Removed(task),
            Err(why) => Moved::Refused { task, why },
        },
    }
}

/// Register this user's task under its new name, then remove the old one if
/// it is this user's: in that order, so a refused registration never leaves
/// the user without a task.
pub(crate) fn replace_legacy_with(
    me: &ThisUser,
    register: impl FnOnce() -> Result<String, String>,
    exists: impl FnMut(&str) -> bool,
    definition: impl FnMut(&str) -> Option<String>,
    delete: impl FnMut(&str) -> Result<(), String>,
) -> Result<(String, Moved), String> {
    let name = register()?;
    Ok((name, move_legacy_with(me, exists, definition, delete)))
}

/// `schtasks` output as text: UTF-16LE when it starts with a byte order mark
/// or carries NULs at odd offsets (what `/Query /XML` may write to a pipe),
/// UTF-8 otherwise.
pub(crate) fn decode_schtasks_output(bytes: &[u8]) -> String {
    let bom = bytes.starts_with(&[0xFF, 0xFE]);
    let wide =
        bom || (bytes.len() >= 2 && bytes.iter().skip(1).step_by(2).take(8).all(|b| *b == 0));
    if !wide {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let body = if bom { &bytes[2..] } else { bytes };
    let units: Vec<u16> = body
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16_lossy(&units)
}

/// Create the task under the first name `create` accepts; the last refusal
/// when none does.
pub(crate) fn register_with(
    names: &[String],
    mut create: impl FnMut(&str) -> Result<(), String>,
) -> Result<String, String> {
    let mut last = "no task name to register".to_string();
    for name in names {
        match create(name) {
            Ok(()) => return Ok(name.clone()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// `file` in `System32` under `system_root` (`%SystemRoot%`), or in
/// `C:\Windows\System32` when that is unset.
fn in_system32(system_root: Option<&std::ffi::OsStr>, file: &str) -> PathBuf {
    match system_root.filter(|r| !r.is_empty()) {
        Some(root) => Path::new(root).join("System32").join(file),
        None => PathBuf::from(format!(r"C:\Windows\System32\{file}")),
    }
}

/// `schtasks.exe` from the system folder, never whatever `PATH` finds first.
pub(crate) fn schtasks_exe_in(system_root: Option<&std::ffi::OsStr>) -> PathBuf {
    in_system32(system_root, "schtasks.exe")
}

/// The folder Task Scheduler keeps each task's definition in, one file per
/// task named as the task (`System32\Tasks`).
pub(crate) fn tasks_folder_in(system_root: Option<&std::ffi::OsStr>) -> PathBuf {
    in_system32(system_root, "Tasks")
}

/// Run `schtasks.exe` with `args`: its standard output, or the reason it
/// gave.
fn schtasks(args: &[&std::ffi::OsStr]) -> Result<String, String> {
    if !cfg!(windows) {
        return Err("Task Scheduler exists only on Windows".to_string());
    }
    let mut cmd =
        std::process::Command::new(schtasks_exe_in(std::env::var_os("SystemRoot").as_deref()));
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(decode_schtasks_output(&out.stdout))
    } else {
        let said = decode_schtasks_output(if out.stderr.is_empty() {
            &out.stdout
        } else {
            &out.stderr
        })
        .trim()
        .to_string();
        Err(if said.is_empty() {
            format!("schtasks exited with {}", out.status)
        } else {
            said
        })
    }
}

/// What `schtasks /Query /TN <name>` said about one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Present,
    /// Windows said it cannot find the task or its folder.
    Absent,
    /// Any other failure, in Windows' words: the task may be there.
    Refused(String),
}

/// A task query Task Scheduler refused: the name asked about and Windows'
/// words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub task: String,
    pub detail: String,
}

/// The English words for a missing file and a missing path, the fallback
/// when the system's own words do not match.
const NOT_FOUND_ENGLISH: &[&str] = &[
    "cannot find the file specified",
    "cannot find the path specified",
];

/// Windows' words, in this system's language, for error 2 (a missing task)
/// and error 3 (a missing task folder): `io::Error` formats them through
/// `FormatMessageW`, the text `schtasks` prints.
fn not_found_texts() -> Vec<String> {
    [2, 3]
        .into_iter()
        .map(|code| {
            let text = std::io::Error::from_raw_os_error(code).to_string();
            text.split(" (os error")
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .filter(|text| !text.is_empty())
        .collect()
}

/// `text` reduced to its ASCII letters and digits, lowercased. `schtasks`
/// writes in the console's code page, which this binary decodes as UTF-8,
/// so a letter outside ASCII arrives as U+FFFD: compared this way, the
/// system's own words still match what `schtasks` printed.
fn ascii_letters(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The outcome of a query from what `schtasks` returned and the words that
/// mean "not there". A needle with fewer than 8 ASCII letters left (a
/// language written in another script) is skipped, so it can never match
/// everything; the English fallback still applies.
pub(crate) fn query_outcome(result: Result<String, String>, not_found: &[String]) -> Query {
    match result {
        Ok(_) => Query::Present,
        Err(said) => {
            let reduced = ascii_letters(&said);
            let absent = not_found
                .iter()
                .map(|text| ascii_letters(text))
                .filter(|needle| needle.len() >= 8)
                .chain(NOT_FOUND_ENGLISH.iter().map(|text| ascii_letters(text)))
                .any(|needle| reduced.contains(&needle));
            if absent {
                Query::Absent
            } else {
                Query::Refused(said)
            }
        }
    }
}

/// Ask Task Scheduler about the task `name`. Off Windows there is none.
pub(crate) fn query_task(name: &str) -> Query {
    if !cfg!(windows) {
        return Query::Absent;
    }
    query_outcome(
        schtasks(&["/Query".as_ref(), "/TN".as_ref(), name.as_ref()]),
        &not_found_texts(),
    )
}

/// Whether Task Scheduler has a task named `name`. A refused query is not a
/// yes.
fn task_exists(name: &str) -> bool {
    query_task(name) == Query::Present
}

/// Delete the task `name`.
fn delete_task(name: &str) -> Result<(), String> {
    schtasks(&[
        "/Delete".as_ref(),
        "/TN".as_ref(),
        name.as_ref(),
        "/F".as_ref(),
    ])
    .map(|_| ())
}

/// The real Task Scheduler, through `schtasks.exe`.
pub struct Schtasks {
    pub me: ThisUser,
}

impl Schtasks {
    pub fn for_this_user() -> Schtasks {
        Schtasks {
            me: ThisUser::here(),
        }
    }
}

impl DaemonTask for Schtasks {
    fn find(&self) -> Result<Option<String>, Refused> {
        find_with(&self.me, query_task, registered_xml)
    }

    fn run(&self, name: &str) -> Result<(), String> {
        schtasks(&["/Run".as_ref(), "/TN".as_ref(), name.as_ref()]).map(|_| ())
    }

    fn definition(&self, name: &str) -> Option<String> {
        registered_xml(name)
    }

    fn legacy(&self) -> Option<String> {
        legacy_with(&self.me, task_exists, registered_xml)
    }
}

/// Write `bytes` to a file at `path` that this call creates: never through a
/// file or a link that is already there.
pub(crate) fn write_new_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)
}

/// Write the task definition to a new file in `dir` under a random name and
/// answer its path. The MSI runs `register` as SYSTEM, so the name must not
/// be guessable and the file must be new: otherwise another user could put
/// their own definition where `schtasks /Create` reads it.
pub(crate) fn write_task_file(dir: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| e.to_string())?;
    let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let file = dir.join(format!("crystalline-daemon-task-{suffix}.xml"));
    if let Err(e) = write_new_file(&file, bytes) {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = std::fs::remove_file(&file);
        }
        return Err(e.to_string());
    }
    Ok(file)
}

/// Register the task for `principal`, starting `exe`. Answers the name it
/// was registered under.
pub fn register(principal: &TaskPrincipal, exe: &Path) -> Result<String, String> {
    let names = registration_names(principal, &ThisUser::here());
    if names.is_empty() {
        return Err(
            "this account's user name or SID could not be read, so its task has no name"
                .to_string(),
        );
    }
    let file = write_task_file(
        &std::env::temp_dir(),
        &utf16_with_bom(&task_xml(exe, principal)),
    )?;
    let result = register_with(&names, |name| {
        schtasks(&[
            "/Create".as_ref(),
            "/TN".as_ref(),
            name.as_ref(),
            "/XML".as_ref(),
            file.as_os_str(),
            "/F".as_ref(),
        ])
        .map(|_| ())
    });
    let _ = std::fs::remove_file(&file);
    result
}

/// Remove this user's 0.24.0-named task and register nothing: for when the
/// machine task is ready, which every bridge finds first, so a per-user
/// task beside it would never run.
pub fn remove_legacy_for_this_user() -> Moved {
    move_legacy_with(&ThisUser::here(), task_exists, registered_xml, delete_task)
}

/// Register the current user's own task, then move away the one 0.24.0
/// registered under the name without the SID, when it is this account's.
pub fn register_for_this_user(exe: &Path) -> Result<(String, Moved), String> {
    let me = ThisUser::here();
    let principal = TaskPrincipal::User {
        account: me.account.clone(),
    };
    replace_legacy_with(
        &me,
        || register(&principal, exe),
        task_exists,
        registered_xml,
        delete_task,
    )
}

/// The per-user tasks in a `schtasks /Query /FO CSV /NH` listing: every
/// `\Crystalline Daemon for <user>` and `\Crystalline Daemon for <user>
/// (<SID>)` in the root folder, each once. The task
/// name is the first quoted field of a row; any other line is skipped.
pub(crate) fn per_user_task_names(listing: &str) -> Vec<String> {
    let prefix = legacy_user_task_name("");
    let mut names: Vec<String> = Vec::new();
    for line in listing.lines() {
        let Some(name) = line
            .trim()
            .strip_prefix('"')
            .and_then(|rest| rest.split('"').next())
        else {
            continue;
        };
        let is_user_task = name
            .strip_prefix(prefix.as_str())
            .is_some_and(|user| !user.trim().is_empty() && !user.contains('\\'));
        if is_user_task && !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// The per-user tasks among the file names in Task Scheduler's tasks
/// folder (see [`tasks_folder_in`]), every `\Crystalline Daemon for <user>`
/// and `\Crystalline Daemon for <user> (<SID>)`: each root-level file is one
/// task, named exactly as the task, so a user name with any letters reads
/// back as it is.
pub(crate) fn per_user_task_names_in_folder(
    files: impl IntoIterator<Item = std::ffi::OsString>,
) -> Vec<String> {
    let prefix = legacy_user_task_name("");
    let prefix = prefix.trim_start_matches('\\');
    let mut names: Vec<String> = Vec::new();
    for file in files {
        let Some(file) = file.to_str() else {
            continue;
        };
        let is_user_task = file
            .strip_prefix(prefix)
            .is_some_and(|user| !user.trim().is_empty());
        let name = format!("\\{file}");
        if is_user_task && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The names an unregistration for `principal` removes. For one user, their
/// own task, and the 0.24.0-named one only when `legacy_is_mine` says it
/// runs as them. For every user (the MSI's uninstall), the machine task and
/// every per-user task in `per_user`, both name forms: a task `doctor
/// --fix` registered must not outlive the binary it starts.
pub(crate) fn unregistration_names(
    principal: &TaskPrincipal,
    me: &ThisUser,
    per_user: &[String],
    mut legacy_is_mine: impl FnMut(&str) -> bool,
) -> Vec<String> {
    let mut names = registration_names(principal, me);
    match principal {
        TaskPrincipal::AllUsers => {
            for name in per_user {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        TaskPrincipal::User { .. } => {
            if let Some(legacy) = me.legacy_name().filter(|name| legacy_is_mine(name)) {
                names.push(legacy);
            }
        }
    }
    names
}

/// What an unregistration did: the tasks it removed and, for each one Task
/// Scheduler would not remove, the reason it gave.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Removal {
    pub removed: Vec<String>,
    pub refused: Vec<(String, String)>,
}

impl Removal {
    /// The removed tasks, or an error that names every refusal.
    pub fn into_result(self) -> Result<Vec<String>, String> {
        if self.refused.is_empty() {
            return Ok(self.removed);
        }
        let refusals: Vec<String> = self
            .refused
            .iter()
            .map(|(name, why)| format!("could not remove the task {name} ({why})"))
            .collect();
        Err(refusals.join("; "))
    }
}

/// Delete each of `names` that `query` finds, with `delete`. A task that is
/// not there is skipped, not an error: an install older than 0.24.0
/// registered none. A query Task Scheduler refused is reported with its
/// words, because the task may well be there.
pub(crate) fn unregister_with(
    names: &[String],
    mut query: impl FnMut(&str) -> Query,
    mut delete: impl FnMut(&str) -> Result<(), String>,
) -> Removal {
    let mut removal = Removal::default();
    for name in names {
        match query(name) {
            Query::Absent => {}
            Query::Refused(why) => removal.refused.push((name.clone(), why)),
            Query::Present => match delete(name) {
                Ok(()) => removal.removed.push(name.clone()),
                Err(why) => removal.refused.push((name.clone(), why)),
            },
        }
    }
    removal
}

/// The per-user tasks on this machine, for the uninstall: from the file names
/// in the tasks folder, which keep every letter of a user name, or from
/// `schtasks /Query` when that folder cannot be read.
fn listed_per_user_tasks() -> Vec<String> {
    let folder = tasks_folder_in(std::env::var_os("SystemRoot").as_deref());
    if let Ok(entries) = std::fs::read_dir(&folder) {
        return per_user_task_names_in_folder(
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
                .map(|entry| entry.file_name()),
        );
    }
    schtasks(&[
        "/Query".as_ref(),
        "/FO".as_ref(),
        "CSV".as_ref(),
        "/NH".as_ref(),
    ])
    .map(|listing| per_user_task_names(&listing))
    .unwrap_or_default()
}

/// Remove the task(s) for `principal`: for every user, the machine task and
/// each user's own; for one user, only theirs. Says what it removed and
/// every removal Task Scheduler refused.
pub fn unregister(principal: &TaskPrincipal) -> Removal {
    let per_user = match principal {
        TaskPrincipal::AllUsers if cfg!(windows) => listed_per_user_tasks(),
        _ => Vec::new(),
    };
    let me = ThisUser::here();
    let names = unregistration_names(principal, &me, &per_user, |name| {
        registered_xml(name).is_some_and(|xml| runs_as(&xml, &me))
    });
    unregister_with(&names, query_task, delete_task)
}

/// The definition of a registered task, as Task Scheduler holds it now.
pub fn registered_xml(name: &str) -> Option<String> {
    schtasks(&[
        "/Query".as_ref(),
        "/TN".as_ref(),
        name.as_ref(),
        "/XML".as_ref(),
    ])
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Program Files\Crystalline\bin\crystalline.exe";

    const ADA_SID: &str = "S-1-5-21-1004336348-1177238915-682003330-1001";

    fn ada() -> ThisUser {
        ThisUser {
            name: "ada".to_string(),
            account: r"WORK\ada".to_string(),
            sid: Some(ADA_SID.to_string()),
        }
    }

    /// A definition as `schtasks /Query /XML` prints a per-user task, with
    /// `user_id` as principal and in the trigger.
    fn principal_xml(user_id: &str) -> String {
        format!(
            "<Task><Triggers><LogonTrigger><UserId>{user_id}</UserId></LogonTrigger></Triggers>\
             <Principals><Principal id=\"Author\"><UserId>{user_id}</UserId>\
             <LogonType>InteractiveToken</LogonType></Principal></Principals></Task>"
        )
    }

    #[test]
    fn a_user_task_is_named_for_the_user_and_their_sid() {
        assert_eq!(
            user_task_name("ada", ADA_SID),
            format!(r"\Crystalline Daemon for ada ({ADA_SID})")
        );
        assert_eq!(legacy_user_task_name("ada"), r"\Crystalline Daemon for ada");
        assert!(
            user_task_name("ada", ADA_SID).starts_with(&legacy_user_task_name("")),
            "one prefix finds both forms for the uninstall"
        );
    }

    #[test]
    fn only_a_sid_or_the_whole_account_proves_a_task_runs_as_this_user() {
        let me = ada();
        assert!(runs_as(&principal_xml(ADA_SID), &me));
        assert!(
            runs_as(&principal_xml(r"work\ADA"), &me),
            "case is no difference"
        );
        assert!(
            !runs_as(&principal_xml("ada"), &me),
            "a bare name may be another account's"
        );
        assert!(
            !runs_as(&principal_xml(r"PC1\ada"), &me),
            "another domain is another account"
        );
        assert!(!runs_as(&principal_xml("S-1-5-21-9-9-9-1001"), &me));
        assert!(
            !runs_as(
                "<Task><Triggers><LogonTrigger><UserId>WORK\\ada</UserId></LogonTrigger></Triggers>\
                 <Principals><Principal><GroupId>S-1-5-32-545</GroupId></Principal></Principals></Task>",
                &me
            ),
            "the trigger says when it starts, not who it runs as"
        );
        assert!(!runs_as("\u{fffd}\u{fffd}", &me), "unreadable is not ours");
        let no_sid = ThisUser { sid: None, ..ada() };
        assert!(!runs_as(&principal_xml(ADA_SID), &no_sid));
    }

    /// Two accounts share the user name bob: the local bob and CORP\bob.
    /// Only one `\Crystalline Daemon for bob` can exist. When it is the
    /// local bob's, CORP\bob never finds, runs, moves, rewrites or deletes it.
    #[test]
    fn another_account_with_the_same_user_name_keeps_its_old_task() {
        let corp_bob = ThisUser {
            name: "bob".to_string(),
            account: r"CORP\bob".to_string(),
            sid: Some("S-1-5-21-1-2-3-1104".to_string()),
        };
        let old = legacy_user_task_name("bob");
        let principal = TaskPrincipal::User {
            account: corp_bob.account.clone(),
        };
        for local_bob in ["S-1-5-21-7-8-9-1001", r"PC1\bob", "bob"] {
            let xml = principal_xml(local_bob);
            let exists = |name: &str| name == old;
            let query = |name: &str| {
                if name == old {
                    Query::Present
                } else {
                    Query::Absent
                }
            };
            let definition = |name: &str| (name == old).then(|| xml.clone());
            assert_eq!(
                find_with(&corp_bob, query, definition),
                Ok(None),
                "{local_bob}"
            );
            assert_eq!(
                legacy_with(&corp_bob, exists, definition),
                None,
                "{local_bob}"
            );
            assert_eq!(
                move_legacy_with(&corp_bob, exists, definition, |name| panic!(
                    "deleted {name}"
                )),
                Moved::Nothing,
                "{local_bob}"
            );
            assert_eq!(
                replace_legacy_with(
                    &corp_bob,
                    || Ok(user_task_name("bob", "S-1-5-21-1-2-3-1104")),
                    exists,
                    definition,
                    |name| panic!("deleted {name}"),
                ),
                Ok((user_task_name("bob", "S-1-5-21-1-2-3-1104"), Moved::Nothing)),
                "the new name carries CORP\\bob's SID, so the local bob's task is never rewritten"
            );
            assert_eq!(
                unregistration_names(&principal, &corp_bob, &[], |name| {
                    legacy_with(&corp_bob, exists, definition).as_deref() == Some(name)
                }),
                [user_task_name("bob", "S-1-5-21-1-2-3-1104")],
                "{local_bob}"
            );
        }
        for own in ["S-1-5-21-1-2-3-1104", r"corp\BOB"] {
            let xml = principal_xml(own);
            let exists = |name: &str| name == old;
            let query = |name: &str| {
                if name == old {
                    Query::Present
                } else {
                    Query::Absent
                }
            };
            let definition = |name: &str| (name == old).then(|| xml.clone());
            assert_eq!(
                find_with(&corp_bob, query, definition),
                Ok(Some(old.clone())),
                "{own}"
            );
            assert_eq!(
                unregistration_names(&principal, &corp_bob, &[], |name| {
                    legacy_with(&corp_bob, exists, definition).as_deref() == Some(name)
                }),
                [user_task_name("bob", "S-1-5-21-1-2-3-1104"), old.clone()],
                "{own}"
            );
        }
    }

    /// The new name first, the old one after: a refused registration never
    /// leaves the user without a task, and a refused delete is said.
    #[test]
    fn the_old_task_goes_only_after_the_new_one_is_registered() {
        let me = ada();
        let old = legacy_user_task_name("ada");
        let new = user_task_name("ada", ADA_SID);
        let xml = principal_xml(r"WORK\ada");
        let exists = |name: &str| name == old;
        let definition = |name: &str| (name == old).then(|| xml.clone());
        assert_eq!(
            replace_legacy_with(
                &me,
                || Err("ERROR: Access is denied.".to_string()),
                exists,
                definition,
                |name| panic!("deleted {name} before a new task existed"),
            ),
            Err("ERROR: Access is denied.".to_string())
        );
        let order = std::cell::RefCell::new(Vec::new());
        let done = replace_legacy_with(
            &me,
            || {
                order.borrow_mut().push("register".to_string());
                Ok(new.clone())
            },
            exists,
            definition,
            |name| {
                order.borrow_mut().push(format!("delete {name}"));
                Ok(())
            },
        );
        assert_eq!(done, Ok((new.clone(), Moved::Removed(old.clone()))));
        assert_eq!(
            *order.borrow(),
            ["register".to_string(), format!("delete {old}")]
        );
        assert_eq!(
            replace_legacy_with(
                &me,
                || Ok(new.clone()),
                exists,
                definition,
                |_| { Err("ERROR: Access is denied.".to_string()) }
            ),
            Ok((
                new.clone(),
                Moved::Refused {
                    task: old.clone(),
                    why: "ERROR: Access is denied.".to_string()
                }
            ))
        );
        assert_eq!(
            replace_legacy_with(
                &me,
                || Ok(new.clone()),
                |_| false,
                definition,
                |name| { panic!("deleted {name}") }
            ),
            Ok((new, Moved::Nothing)),
            "no old task, nothing to move"
        );
    }

    #[test]
    fn the_machine_task_matches_the_checked_in_definition() {
        assert_eq!(
            task_xml(std::path::Path::new(EXE), &TaskPrincipal::AllUsers),
            include_str!("../tests/fixtures/daemon-task-all-users.xml")
        );
    }

    #[test]
    fn the_machine_task_runs_as_whoever_signs_in() {
        let xml = task_xml(std::path::Path::new(EXE), &TaskPrincipal::AllUsers);
        assert!(
            xml.contains("<GroupId>S-1-5-32-545</GroupId>"),
            "the Users group, by SID"
        );
        assert!(
            !xml.contains("<UserId>"),
            "no user, neither as principal nor on the trigger"
        );
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<Arguments>serve --daemon --from-task</Arguments>"));
    }

    #[test]
    fn a_second_user_signing_in_gets_a_daemon_too() {
        let xml = task_xml(std::path::Path::new(EXE), &TaskPrincipal::AllUsers);
        assert!(
            xml.contains("<MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>"),
            "IgnoreNew is per task, so a second user would get none"
        );
    }

    #[test]
    fn the_task_lets_every_user_read_and_run_it_without_limits() {
        let xml = task_xml(std::path::Path::new(EXE), &TaskPrincipal::AllUsers);
        assert!(xml.contains(&format!(
            "<SecurityDescriptor>{TASK_SDDL}</SecurityDescriptor>"
        )));
        assert!(TASK_SDDL.contains("(A;;GRGX;;;BU)"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
    }

    #[test]
    fn a_user_task_names_the_user_with_an_interactive_token() {
        let xml = task_xml(
            std::path::Path::new(EXE),
            &TaskPrincipal::User {
                account: r"WORK\ada".to_string(),
            },
        );
        assert!(xml.contains(r"<UserId>WORK\ada</UserId>"));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(!xml.contains("<GroupId>"));
        assert_eq!(
            xml.matches(r"<UserId>WORK\ada</UserId>").count(),
            2,
            "principal and trigger"
        );
    }

    #[test]
    fn the_install_path_is_escaped_and_quoted() {
        let xml = task_xml(
            std::path::Path::new(r"D:\Tools & More\<x>\crystalline.exe"),
            &TaskPrincipal::AllUsers,
        );
        assert!(
            xml.contains(r#"<Command>"D:\Tools &amp; More\&lt;x&gt;\crystalline.exe"</Command>"#),
            "{xml}"
        );
    }

    #[test]
    fn the_definition_is_written_as_utf16_with_a_bom() {
        let bytes = utf16_with_bom("<a/>");
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        assert_eq!(&bytes[2..], &[b'<', 0, b'a', 0, b'/', 0, b'>', 0]);
    }

    #[test]
    fn the_bridge_looks_for_the_machine_task_then_the_user_task() {
        assert_eq!(
            candidate_names(&ThisUser::default()),
            [MACHINE_TASK_NAME.to_string()]
        );
        assert_eq!(
            candidate_names(&ada()),
            [
                MACHINE_TASK_NAME.to_string(),
                user_task_name("ada", ADA_SID),
                legacy_user_task_name("ada"),
            ]
        );
        let no_sid = ThisUser { sid: None, ..ada() };
        assert_eq!(
            candidate_names(&no_sid),
            [MACHINE_TASK_NAME.to_string(), legacy_user_task_name("ada")]
        );
    }

    #[test]
    fn a_user_task_never_takes_the_machine_name() {
        assert_eq!(
            registration_names(&TaskPrincipal::AllUsers, &ada()),
            [MACHINE_TASK_NAME.to_string()]
        );
        let user = TaskPrincipal::User {
            account: r"WORK\ada".to_string(),
        };
        assert_eq!(
            registration_names(&user, &ada()),
            [user_task_name("ada", ADA_SID)]
        );
        assert!(
            registration_names(&user, &ThisUser::default()).is_empty(),
            "no user name, no user task"
        );
        assert!(
            registration_names(&user, &ThisUser { sid: None, ..ada() }).is_empty(),
            "no SID, no user task name"
        );
        let mut tried = Vec::new();
        let got = register_with(&registration_names(&user, &ada()), |name| {
            tried.push(name.to_string());
            Ok(())
        });
        assert_eq!(got.as_deref(), Ok(user_task_name("ada", ADA_SID).as_str()));
        assert_eq!(tried, [user_task_name("ada", ADA_SID)]);
        let refused = register_with(&registration_names(&user, &ada()), |_| {
            Err("ERROR: Access is denied.".to_string())
        });
        assert_eq!(
            refused,
            Err("ERROR: Access is denied.".to_string()),
            "the refusal is said"
        );
    }

    #[test]
    fn schtasks_output_in_utf16_is_read() {
        let wide: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(
                "<GroupId>S-1-5-32-545</GroupId>"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes),
            )
            .collect();
        assert_eq!(
            decode_schtasks_output(&wide),
            "<GroupId>S-1-5-32-545</GroupId>"
        );
        let bare: Vec<u8> = "<a/>".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(
            decode_schtasks_output(&bare),
            "<a/>",
            "no BOM, NULs at odd offsets"
        );
        assert_eq!(decode_schtasks_output(b"SUCCESS: done"), "SUCCESS: done");
    }

    #[test]
    fn schtasks_is_taken_from_system32_and_never_from_path() {
        assert_eq!(
            schtasks_exe_in(Some(std::ffi::OsStr::new(r"C:\Windows"))),
            std::path::Path::new(r"C:\Windows")
                .join("System32")
                .join("schtasks.exe")
        );
        assert_eq!(
            schtasks_exe_in(None),
            std::path::PathBuf::from(r"C:\Windows\System32\schtasks.exe")
        );
    }

    #[test]
    fn an_uninstall_finds_every_per_user_task_in_the_root_folder() {
        let listing = "\r\n\
            \"\\Crystalline\\Daemon\",\"N/A\",\"Ready\"\r\n\
            \"\\Crystalline Daemon for ada\",\"N/A\",\"Ready\"\r\n\
            \"\\Crystalline Daemon for ada (S-1-5-21-1-2-3-1001)\",\"N/A\",\"Ready\"\r\n\
            \"\\Crystalline Daemon for Bob Smith\",\"N/A\",\"Running\"\r\n\
            \"\\Crystalline Daemon for ada\",\"N/A\",\"Ready\"\r\n\
            \"\\Other\\Crystalline Daemon for eve\",\"N/A\",\"Ready\"\r\n\
            \"\\Crystalline Daemon for \",\"N/A\",\"Ready\"\r\n\
            \"\\Microsoft\\Windows\\Defrag\\ScheduledDefrag\",\"N/A\",\"Ready\"\r\n\
            INFO: There are no scheduled tasks presently available at your access level.\r\n";
        assert_eq!(
            per_user_task_names(listing),
            [
                r"\Crystalline Daemon for ada".to_string(),
                r"\Crystalline Daemon for ada (S-1-5-21-1-2-3-1001)".to_string(),
                r"\Crystalline Daemon for Bob Smith".to_string(),
            ],
            "root folder only, each name once, never the machine task"
        );
        assert!(per_user_task_names("").is_empty());
    }

    #[test]
    fn an_uninstall_removes_the_machine_task_and_every_per_user_task() {
        let listing = "\"\\Crystalline Daemon for ada\",\"N/A\",\"Ready\"\r\n\
            \"\\Crystalline Daemon for bob\",\"N/A\",\"Ready\"\r\n";
        let listed = per_user_task_names(listing);
        assert_eq!(
            unregistration_names(&TaskPrincipal::AllUsers, &ada(), &listed, |_| false),
            [
                MACHINE_TASK_NAME.to_string(),
                r"\Crystalline Daemon for ada".to_string(),
                r"\Crystalline Daemon for bob".to_string(),
            ]
        );
        assert_eq!(
            unregistration_names(&TaskPrincipal::AllUsers, &ThisUser::default(), &[], |_| {
                false
            }),
            [MACHINE_TASK_NAME.to_string()],
            "a listing that failed still removes the machine task"
        );
        let user = TaskPrincipal::User {
            account: r"WORK\ada".to_string(),
        };
        assert_eq!(
            unregistration_names(&user, &ada(), &listed, |_| false),
            [user_task_name("ada", ADA_SID)],
            "one user removes only their own task"
        );
    }

    #[test]
    fn a_user_task_carries_no_security_descriptor() {
        let xml = task_xml(
            std::path::Path::new(EXE),
            &TaskPrincipal::User {
                account: r"WORK\ada".to_string(),
            },
        );
        assert!(
            !xml.contains("<SecurityDescriptor>"),
            "the default keeps the creator's full rights, so the user can replace or delete it: {xml}"
        );
        assert!(xml.contains("  <RegistrationInfo>\n    <Author>Crystalline</Author>\n"));
    }

    #[test]
    fn an_uninstall_reads_per_user_names_from_the_tasks_folder() {
        let files = [
            "Crystalline Daemon for ada",
            "Crystalline Daemon for ada (S-1-5-21-1-2-3-1001)",
            "Crystalline Daemon for J\u{f6}rg",
            "Crystalline Daemon for ",
            "Crystalline",
            "Adobe Acrobat Update Task",
            "Crystalline Daemon for ada",
        ]
        .map(std::ffi::OsString::from);
        assert_eq!(
            per_user_task_names_in_folder(files),
            [
                r"\Crystalline Daemon for ada".to_string(),
                r"\Crystalline Daemon for ada (S-1-5-21-1-2-3-1001)".to_string(),
                "\\Crystalline Daemon for J\u{f6}rg".to_string(),
            ],
            "each per-user task once, a non-ASCII name exactly as it is"
        );
        assert_eq!(
            tasks_folder_in(Some(std::ffi::OsStr::new(r"C:\Windows"))),
            std::path::Path::new(r"C:\Windows")
                .join("System32")
                .join("Tasks")
        );
    }

    #[test]
    fn an_uninstall_removes_only_what_is_there_and_names_each_refusal() {
        let names = [
            MACHINE_TASK_NAME.to_string(),
            r"\Crystalline Daemon for ada".to_string(),
            r"\Crystalline Daemon for bob".to_string(),
            r"\Crystalline Daemon for eve".to_string(),
        ];
        let mut deleted = Vec::new();
        let removal = unregister_with(
            &names,
            |name| match name {
                r"\Crystalline Daemon for ada" => Query::Absent,
                r"\Crystalline Daemon for eve" => {
                    Query::Refused("ERROR: Access is denied.".to_string())
                }
                _ => Query::Present,
            },
            |name| {
                deleted.push(name.to_string());
                if name.ends_with("bob") {
                    Err("ERROR: Access is denied.".to_string())
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(
            deleted,
            [
                MACHINE_TASK_NAME.to_string(),
                r"\Crystalline Daemon for bob".to_string()
            ],
            "a task that is not there, or that could not be asked about, is not deleted"
        );
        assert_eq!(removal.removed, [MACHINE_TASK_NAME.to_string()]);
        assert_eq!(
            removal.refused,
            [
                (
                    r"\Crystalline Daemon for bob".to_string(),
                    "ERROR: Access is denied.".to_string()
                ),
                (
                    r"\Crystalline Daemon for eve".to_string(),
                    "ERROR: Access is denied.".to_string()
                ),
            ],
            "a refused query is reported, not skipped"
        );
        let said = removal.into_result().unwrap_err();
        assert!(
            said.contains(r"\Crystalline Daemon for eve") && said.contains("Access is denied"),
            "{said}"
        );
        let nothing = unregister_with(&names, |_| Query::Absent, |_| panic!("nothing to delete"));
        assert_eq!(
            nothing.into_result(),
            Ok(Vec::new()),
            "nothing there is no error"
        );
    }

    #[test]
    fn a_task_query_is_present_absent_or_refused_in_any_language() {
        let english = [
            "The system cannot find the file specified.".to_string(),
            "The system cannot find the path specified.".to_string(),
        ];
        assert_eq!(
            query_outcome(Ok("Folder: \\".to_string()), &english),
            Query::Present
        );
        assert_eq!(
            query_outcome(
                Err("ERROR: The system cannot find the file specified.".to_string()),
                &english
            ),
            Query::Absent
        );
        assert_eq!(
            query_outcome(
                Err("ERROR: The system cannot find the path specified.".to_string()),
                &english
            ),
            Query::Absent,
            "no \\Crystalline folder on a machine without the MSI"
        );
        let german = [
            "Das System kann die angegebene Datei nicht finden.".to_string(),
            "Das System kann den angegebenen Pfad nicht finden.".to_string(),
        ];
        assert_eq!(
            query_outcome(
                Err("FEHLER: Das System kann die angegebene Datei nicht finden.".to_string()),
                &german
            ),
            Query::Absent
        );
        assert_eq!(
            query_outcome(Err("FEHLER: Zugriff verweigert".to_string()), &german),
            Query::Refused("FEHLER: Zugriff verweigert".to_string())
        );
        assert_eq!(
            query_outcome(Err("ERROR: Access is denied.".to_string()), &[]),
            Query::Refused("ERROR: Access is denied.".to_string()),
            "the English fallback knows only not-found"
        );
        assert_eq!(
            query_outcome(
                Err("ERROR: The system cannot find the file specified.".to_string()),
                &[]
            ),
            Query::Absent,
            "the English fallback works without the system's own words"
        );
        // French as `schtasks` prints it in an OEM code page, decoded as
        // lossy UTF-8: the accented letters arrive as U+FFFD.
        let french = ["Le fichier sp\u{e9}cifi\u{e9} est introuvable.".to_string()];
        assert_eq!(
            query_outcome(
                Err("ERREUR\u{a0}: Le fichier sp\u{fffd}cifi\u{fffd} est introuvable.".to_string()),
                &french
            ),
            Query::Absent
        );
        // A needle in another script reduces to nothing and must not match
        // every refusal.
        let russian =
            ["\u{41d}\u{435} \u{443}\u{434}\u{430}\u{435}\u{442}\u{441}\u{44f}".to_string()];
        assert_eq!(
            query_outcome(
                Err("\u{41e}\u{428}\u{418}\u{411}\u{41a}\u{410}: access".to_string()),
                &russian
            ),
            Query::Refused("\u{41e}\u{428}\u{418}\u{411}\u{41a}\u{410}: access".to_string())
        );
    }

    /// A refusal is said only when nothing usable was found: a later name
    /// that is there still wins.
    #[test]
    fn find_reports_a_refused_query_only_when_nothing_else_is_there() {
        let me = ada();
        let refused_machine = |name: &str| {
            if name == MACHINE_TASK_NAME {
                Query::Refused("ERROR: Access is denied.".to_string())
            } else {
                Query::Absent
            }
        };
        assert_eq!(
            find_with(&me, refused_machine, |_| None),
            Err(Refused {
                task: MACHINE_TASK_NAME.to_string(),
                detail: "ERROR: Access is denied.".to_string()
            })
        );
        let own = user_task_name("ada", ADA_SID);
        assert_eq!(
            find_with(
                &me,
                |name: &str| if name == own {
                    Query::Present
                } else {
                    refused_machine(name)
                },
                |_| None
            ),
            Ok(Some(own.clone()))
        );
        assert_eq!(find_with(&me, |_| Query::Absent, |_| None), Ok(None));
    }

    #[test]
    fn the_task_file_is_always_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_task_file(dir.path(), b"one").unwrap();
        let second = write_task_file(dir.path(), b"two").unwrap();
        assert_ne!(first, second, "a fresh random name each time");
        assert_eq!(first.parent(), Some(dir.path()));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert!(
            write_new_file(&first, b"swap").is_err(),
            "a file already at the path is never written through"
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
    }
}
