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

/// The task definition, as `schtasks /Create /XML` reads it. See D2 of the
/// 0.24.0 plan for each setting, and `MultipleInstancesPolicy` in particular:
/// `Parallel`, because the policy is per task and a second user signing in
/// must get a daemon of their own; the daemon's lock keeps one per user.
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

/// The task names a bridge looks for, in order: the machine task, then this
/// user's own (D4).
pub fn candidate_names(user: Option<&str>) -> Vec<String> {
    let mut names = vec![MACHINE_TASK_NAME.to_string()];
    if let Some(user) = user.filter(|u| !u.is_empty()) {
        names.push(user_task_name(user));
    }
    names
}

/// The names a registration for `principal` may use (D4): the machine name
/// for every user, only the user's own name for one user.
pub(crate) fn registration_names(principal: &TaskPrincipal, user: Option<&str>) -> Vec<String> {
    match principal {
        TaskPrincipal::AllUsers => vec![MACHINE_TASK_NAME.to_string()],
        TaskPrincipal::User { .. } => user
            .filter(|u| !u.is_empty())
            .map(|u| vec![user_task_name(u)])
            .unwrap_or_default(),
    }
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

/// The real Task Scheduler, through `schtasks.exe`.
pub struct Schtasks {
    pub user: Option<String>,
}

impl Schtasks {
    pub fn for_this_user() -> Schtasks {
        Schtasks {
            user: std::env::var("USERNAME").ok(),
        }
    }
}

impl DaemonTask for Schtasks {
    fn find(&self) -> Option<String> {
        candidate_names(self.user.as_deref())
            .into_iter()
            .find(|name| schtasks(&["/Query".as_ref(), "/TN".as_ref(), name.as_ref()]).is_ok())
    }

    fn run(&self, name: &str) -> Result<(), String> {
        schtasks(&["/Run".as_ref(), "/TN".as_ref(), name.as_ref()]).map(|_| ())
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
    let file = write_task_file(
        &std::env::temp_dir(),
        &utf16_with_bom(&task_xml(exe, principal)),
    )?;
    let names = registration_names(principal, std::env::var("USERNAME").ok().as_deref());
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

/// The per-user tasks in a `schtasks /Query /FO CSV /NH` listing: every
/// `\Crystalline Daemon for <user>` in the root folder, each once. The task
/// name is the first quoted field of a row; any other line is skipped.
pub(crate) fn per_user_task_names(listing: &str) -> Vec<String> {
    let prefix = user_task_name("");
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
/// folder (see [`tasks_folder_in`]): each root-level file is one task, named
/// exactly as the task, so a user name with any letters reads back as it is.
pub(crate) fn per_user_task_names_in_folder(
    files: impl IntoIterator<Item = std::ffi::OsString>,
) -> Vec<String> {
    let prefix = user_task_name("");
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

/// The names an unregistration for `principal` removes. For one user, only
/// their own task. For every user (the MSI's uninstall), the machine task
/// and every per-user task in `per_user`: a task `doctor --fix` registered
/// must not outlive the binary it starts, or a bridge would still find it.
pub(crate) fn unregistration_names(
    principal: &TaskPrincipal,
    user: Option<&str>,
    per_user: &[String],
) -> Vec<String> {
    let mut names = registration_names(principal, user);
    if *principal == TaskPrincipal::AllUsers {
        for name in per_user {
            if !names.contains(name) {
                names.push(name.clone());
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

/// Delete each of `names` that `exists` finds, with `delete`. A task that is
/// not there is skipped, not an error: an install older than 0.24.0
/// registered none.
pub(crate) fn unregister_with(
    names: &[String],
    mut exists: impl FnMut(&str) -> bool,
    mut delete: impl FnMut(&str) -> Result<(), String>,
) -> Removal {
    let mut removal = Removal::default();
    for name in names {
        if !exists(name) {
            continue;
        }
        match delete(name) {
            Ok(()) => removal.removed.push(name.clone()),
            Err(why) => removal.refused.push((name.clone(), why)),
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
    let user = std::env::var("USERNAME").ok();
    unregister_with(
        &unregistration_names(principal, user.as_deref(), &per_user),
        |name| schtasks(&["/Query".as_ref(), "/TN".as_ref(), name.as_ref()]).is_ok(),
        |name| {
            schtasks(&[
                "/Delete".as_ref(),
                "/TN".as_ref(),
                name.as_ref(),
                "/F".as_ref(),
            ])
            .map(|_| ())
        },
    )
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
        assert_eq!(candidate_names(None), [MACHINE_TASK_NAME.to_string()]);
        assert_eq!(
            candidate_names(Some("ada")),
            [
                MACHINE_TASK_NAME.to_string(),
                r"\Crystalline Daemon for ada".to_string()
            ]
        );
    }

    #[test]
    fn a_user_task_never_takes_the_machine_name() {
        assert_eq!(
            registration_names(&TaskPrincipal::AllUsers, Some("ada")),
            [MACHINE_TASK_NAME.to_string()]
        );
        let user = TaskPrincipal::User {
            account: r"WORK\ada".to_string(),
        };
        assert_eq!(
            registration_names(&user, Some("ada")),
            [r"\Crystalline Daemon for ada".to_string()]
        );
        assert!(
            registration_names(&user, None).is_empty(),
            "no user name, no user task"
        );
        let mut tried = Vec::new();
        let got = register_with(&registration_names(&user, Some("ada")), |name| {
            tried.push(name.to_string());
            Ok(())
        });
        assert_eq!(got.as_deref(), Ok(r"\Crystalline Daemon for ada"));
        assert_eq!(tried, [r"\Crystalline Daemon for ada".to_string()]);
        let refused = register_with(&registration_names(&user, Some("ada")), |_| {
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
            unregistration_names(&TaskPrincipal::AllUsers, Some("ada"), &listed),
            [
                MACHINE_TASK_NAME.to_string(),
                r"\Crystalline Daemon for ada".to_string(),
                r"\Crystalline Daemon for bob".to_string(),
            ]
        );
        assert_eq!(
            unregistration_names(&TaskPrincipal::AllUsers, None, &[]),
            [MACHINE_TASK_NAME.to_string()],
            "a listing that failed still removes the machine task"
        );
        let user = TaskPrincipal::User {
            account: r"WORK\ada".to_string(),
        };
        assert_eq!(
            unregistration_names(&user, Some("ada"), &listed),
            [r"\Crystalline Daemon for ada".to_string()],
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
        ];
        let mut deleted = Vec::new();
        let removal = unregister_with(
            &names,
            |name| name != r"\Crystalline Daemon for ada",
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
            "a task that is not there is not deleted"
        );
        assert_eq!(removal.removed, [MACHINE_TASK_NAME.to_string()]);
        assert_eq!(
            removal.refused,
            [(
                r"\Crystalline Daemon for bob".to_string(),
                "ERROR: Access is denied.".to_string()
            )]
        );
        let said = removal.into_result().unwrap_err();
        assert!(
            said.contains(r"\Crystalline Daemon for bob") && said.contains("Access is denied"),
            "{said}"
        );
        let nothing = unregister_with(&names, |_| false, |_| panic!("nothing to delete"));
        assert_eq!(
            nothing.into_result(),
            Ok(Vec::new()),
            "nothing there is no error"
        );
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
