//! Where the daemon runs: its working directory and, on Windows, the job and
//! the package identity it may have taken over from the program that started
//! it.
//!
//! Issue #115: a daemon started from inside a packaged app kept that app's
//! folder in use, and nobody could tell why. The daemon now works in the state
//! directory (see `instance::spawn_daemon`), and this report is how a person
//! checks that nothing else holds on: `crystalline doctor` shows it in full,
//! `crystalline status` in one line, `ctl status` and `service.json` as
//! `runs_in`.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// The hidden `serve` flag a spawner adds when Windows refused to let the
/// daemon leave the spawner's job. A flag rather than an environment
/// variable, for the reason `--autostarted` is one: a shell can leave a
/// variable set and mislabel a daemon somebody started by hand.
pub const BREAKAWAY_REFUSED_FLAG: &str = "--breakaway-refused";

/// Said by the spawner (with the OS error) and again by the daemon at
/// startup, so it reaches both the client's log and `daemon.log`.
pub const BREAKAWAY_REFUSED_WARNING: &str = "the daemon could not leave the job of the program that started it, so it runs inside that job. It may keep that program's files in use until it stops (`crystalline ctl shutdown` stops it)";

/// What a warning adds: the effect a person saw in #115, and the way out.
const MAY_HOLD_FILES: &str = "It may keep that program's files in use, for example while that program updates itself. `crystalline ctl shutdown` stops it";

/// (breakaway refused, exits when idle), recorded once by `run_serve`.
static START: OnceLock<(bool, bool)> = OnceLock::new();

/// Record the two start facts only the daemon's own command line knows. The
/// first call wins, like `record_serve_intent`.
pub fn record_start(breakaway_refused: bool, exits_when_idle: bool) {
    let _ = START.set((breakaway_refused, exits_when_idle));
}

/// Where a daemon runs. Every key is present on every platform, `null` where
/// a fact does not exist (the job and package keys off Windows), so the JSON
/// has one shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunsIn {
    /// The daemon's working directory; `None` when the OS could not say.
    pub working_dir: Option<String>,
    /// Windows: whether the daemon is in any job. `None` elsewhere.
    pub in_job: Option<bool>,
    /// Windows, in a job: whether its immediate job lets a child break away.
    /// `None` when not in a job, when the limits could not be read and off
    /// Windows.
    pub job_allows_breakaway: Option<bool>,
    /// Windows: the package full name the daemon runs under; `None` when it
    /// has none, and off Windows.
    pub package: Option<String>,
    /// The spawner's breakaway was refused, so it started this daemon inside
    /// its own job.
    pub breakaway_refused: bool,
    /// A bounded life (`--exit-when-idle`): the Claude Desktop extension's
    /// daemon, which stays in Desktop's job on purpose.
    pub exits_when_idle: bool,
}

impl RunsIn {
    /// This process, now. Cheap: a handful of system calls.
    pub fn here() -> RunsIn {
        let (breakaway_refused, exits_when_idle) = START.get().copied().unwrap_or_default();
        let (in_job, job_allows_breakaway, package) = job_and_package();
        RunsIn {
            working_dir: std::env::current_dir()
                .ok()
                .map(|dir| dir.display().to_string()),
            in_job,
            job_allows_breakaway,
            package,
            breakaway_refused,
            exits_when_idle,
        }
    }

    fn dir(&self) -> &str {
        self.working_dir
            .as_deref()
            .unwrap_or("working directory unknown")
    }

    /// The job in words, `None` off Windows (where `in_job` is never set).
    fn job(&self) -> Option<String> {
        let in_job = self.in_job?;
        let mut job = match (in_job, self.job_allows_breakaway) {
            (false, _) => "none".to_string(),
            (true, Some(true)) => "yes, breakaway allowed".to_string(),
            (true, Some(false)) => "yes, breakaway not allowed".to_string(),
            (true, None) => "yes, breakaway unknown".to_string(),
        };
        if self.breakaway_refused {
            job.push_str(", refused at start");
        }
        Some(job)
    }

    fn package_identity(&self) -> &str {
        self.package.as_deref().unwrap_or("none")
    }

    /// The one short line `crystalline status` prints after `Runs in:`.
    pub fn summary(&self) -> String {
        match self.job() {
            Some(job) => format!(
                "{} (job: {job}, package identity: {})",
                self.dir(),
                self.package_identity()
            ),
            None => self.dir().to_string(),
        }
    }

    /// Doctor's lines, one fact each.
    pub fn details(&self) -> Vec<String> {
        let mut lines = vec![format!("daemon working directory: {}", self.dir())];
        if let Some(job) = self.job() {
            lines.push(format!("daemon job: {job}"));
            lines.push(format!(
                "daemon package identity: {}",
                self.package_identity()
            ));
            if self.exits_when_idle && self.in_job == Some(true) {
                lines.push(
                    "the Claude Desktop extension's daemon stays in Claude Desktop's job on purpose and exits when its last client leaves"
                        .to_string(),
                );
            }
        }
        lines
    }

    /// Doctor's warnings. Never a counted problem: a job that forbids
    /// breakaway has no fix on this side. None at all for a daemon with a
    /// bounded life, which is inside on purpose.
    pub fn warnings(&self) -> Vec<String> {
        if self.exits_when_idle {
            return Vec::new();
        }
        let mut warnings = Vec::new();
        let cannot_leave = self.in_job == Some(true)
            && (self.job_allows_breakaway == Some(false) || self.breakaway_refused);
        if cannot_leave {
            warnings.push(format!(
                "the daemon runs inside the job of the program that started it and cannot leave it. {MAY_HOLD_FILES}"
            ));
        }
        if let Some(package) = &self.package {
            warnings.push(format!(
                "the daemon runs with the package identity {package} of the program that started it. {MAY_HOLD_FILES}"
            ));
        }
        warnings
    }
}

/// (in a job, the job allows breakaway, package full name).
#[cfg(not(windows))]
fn job_and_package() -> (Option<bool>, Option<bool>, Option<String>) {
    (None, None, None)
}

/// (in a job, the job allows breakaway, package full name).
#[cfg(windows)]
fn job_and_package() -> (Option<bool>, Option<bool>, Option<String>) {
    use windows_sys::Win32::System::JobObjects::{
        IsProcessInJob, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        QueryInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut member: windows_sys::core::BOOL = 0;
    // This process's pseudo handle, a null job handle (any job at all) and a
    // valid out pointer.
    let asked = unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut member) };
    let in_job = (asked != 0).then_some(member != 0);

    let job_allows_breakaway = if in_job == Some(true) {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // A null job handle reads the immediate job of this process, the one
        // whose limit decides a child's breakaway; the buffer is the struct
        // the class names, with its exact size.
        let read = unsafe {
            QueryInformationJobObject(
                std::ptr::null_mut(),
                JobObjectExtendedLimitInformation,
                (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if read != 0 {
            let flags = info.BasicLimitInformation.LimitFlags;
            Some(
                flags & (JOB_OBJECT_LIMIT_BREAKAWAY_OK | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK) != 0,
            )
        } else {
            None
        }
    } else {
        None
    };

    (in_job, job_allows_breakaway, package_full_name())
}

/// The package full name this process runs under, `None` without one.
#[cfg(windows)]
fn package_full_name() -> Option<String> {
    use windows_sys::Win32::Foundation::{
        APPMODEL_ERROR_NO_PACKAGE, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS,
    };
    use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

    let mut len: u32 = 0;
    // Asked for the length first: a zero length and a null buffer.
    let rc = unsafe { GetCurrentPackageFullName(&mut len, std::ptr::null_mut()) };
    if rc == APPMODEL_ERROR_NO_PACKAGE {
        return None;
    }
    if rc != ERROR_INSUFFICIENT_BUFFER || len == 0 {
        tracing::debug!("could not read this process's package identity (error {rc})");
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    // A buffer of exactly the length the first call asked for.
    let rc = unsafe { GetCurrentPackageFullName(&mut len, buf.as_mut_ptr()) };
    if rc != ERROR_SUCCESS {
        tracing::debug!("could not read this process's package identity (error {rc})");
        return None;
    }
    // `len` counts the terminating NUL.
    buf.truncate((len as usize).saturating_sub(1));
    Some(String::from_utf16_lossy(&buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Windows-shaped report: `in_job` set is what marks one.
    fn windows(in_job: bool, allows: Option<bool>, package: Option<&str>) -> RunsIn {
        RunsIn {
            working_dir: Some(r"C:\s\crystalline".to_string()),
            in_job: Some(in_job),
            job_allows_breakaway: allows,
            package: package.map(str::to_string),
            ..RunsIn::default()
        }
    }

    #[test]
    fn the_summary_is_the_directory_and_on_windows_the_job_and_package() {
        let unix = RunsIn {
            working_dir: Some("/s/crystalline".to_string()),
            ..RunsIn::default()
        };
        assert_eq!(unix.summary(), "/s/crystalline");
        assert_eq!(
            windows(false, None, None).summary(),
            r"C:\s\crystalline (job: none, package identity: none)"
        );
        let mut stuck = windows(true, Some(false), Some("Claude_1.0.0.0_x64__abc"));
        stuck.breakaway_refused = true;
        assert_eq!(
            stuck.summary(),
            r"C:\s\crystalline (job: yes, breakaway not allowed, refused at start, package identity: Claude_1.0.0.0_x64__abc)"
        );
        assert_eq!(
            RunsIn::default().summary(),
            "working directory unknown",
            "a directory the OS could not name is said, not left blank"
        );
    }

    #[test]
    fn details_name_every_fact_and_the_windows_ones_only_on_windows() {
        let unix = RunsIn {
            working_dir: Some("/s/crystalline".to_string()),
            ..RunsIn::default()
        };
        assert_eq!(unix.details(), ["daemon working directory: /s/crystalline"]);
        assert_eq!(
            windows(true, Some(true), None).details(),
            [
                r"daemon working directory: C:\s\crystalline",
                "daemon job: yes, breakaway allowed",
                "daemon package identity: none",
            ]
        );
    }

    #[test]
    fn a_job_it_cannot_leave_or_a_package_warns_unless_it_exits_when_idle() {
        assert!(windows(false, None, None).warnings().is_empty());
        assert!(
            windows(true, Some(true), None).warnings().is_empty(),
            "a job that lets it go is no finding"
        );
        assert!(
            windows(true, None, None).warnings().is_empty(),
            "a job whose limits could not be read is no finding either"
        );

        let stuck = windows(true, Some(false), None).warnings();
        assert_eq!(stuck.len(), 1, "{stuck:?}");
        assert!(
            stuck[0].starts_with("the daemon runs inside the job of the program that started it"),
            "{stuck:?}"
        );

        let mut refused = windows(true, None, None);
        refused.breakaway_refused = true;
        assert_eq!(refused.warnings().len(), 1, "a refusal at start is enough");

        let packaged = windows(false, None, Some("Claude_x")).warnings();
        assert_eq!(packaged.len(), 1, "{packaged:?}");
        assert!(
            packaged[0].contains("package identity Claude_x"),
            "{packaged:?}"
        );

        // The Claude Desktop extension's daemon stays in Desktop's job on
        // purpose and leaves on its own: the facts, but no warning.
        let mut extension = windows(true, Some(false), Some("Claude_x"));
        extension.exits_when_idle = true;
        assert!(
            extension.warnings().is_empty(),
            "{:?}",
            extension.warnings()
        );
        assert!(
            extension.details().iter().any(|l| l.contains("on purpose")),
            "{:?}",
            extension.details()
        );
    }

    #[test]
    fn every_key_is_present_on_every_platform() {
        let value = serde_json::to_value(RunsIn::default()).unwrap();
        for key in [
            "working_dir",
            "in_job",
            "job_allows_breakaway",
            "package",
            "breakaway_refused",
            "exits_when_idle",
        ] {
            assert!(value.get(key).is_some(), "{key} missing: {value}");
        }
        // And an older or partial record still reads.
        let partial: RunsIn = serde_json::from_str(r#"{"working_dir":"/x"}"#).unwrap();
        assert_eq!(partial.working_dir.as_deref(), Some("/x"));
    }

    #[cfg(unix)]
    #[test]
    fn here_reports_this_process_and_nothing_windows_only() {
        let here = RunsIn::here();
        assert_eq!(
            here.working_dir,
            Some(std::env::current_dir().unwrap().display().to_string())
        );
        assert_eq!(
            (
                here.in_job,
                here.job_allows_breakaway,
                here.package.as_deref()
            ),
            (None, None, None)
        );
    }
}
