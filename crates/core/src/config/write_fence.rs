//! The tripwire for a process that must not write under AppData.
//!
//! A process with an app package identity on Windows (Claude Desktop's MSIX
//! package) has every new file it creates under `AppData` redirected into
//! the package's private store. Our saves always create a new file (a
//! sibling temp file renamed into place), so a packaged writer would build a
//! second, private state the daemon outside the package never sees (issue
//! 153). Such a process sends its writes to the daemon. This module is the
//! central refusal for a write that was forgotten on the way: the CLI calls
//! [`refuse_appdata_writes`] once at start, and from then on
//! [`super::save_bytes`], the atomic write helpers and the store open fail
//! loudly for a path under the fenced roots, instead of splitting the state.
//!
//! The decision itself is the pure [`WriteFence::refuses`], so it is tested
//! with injected roots and the process-global flag stays a thin wrapper.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The folder under the state directory that stays writable inside a
/// package: per-session hook files, read only by hooks in the same package
/// and cheap to lose.
pub const HOOKS_DIR: &str = "hooks";

/// The one file in [`HOOKS_DIR`] that is no per-session hook file: the
/// daemon's machine-wide maintenance state. A packaged writer would create
/// a private copy that shadows it, so it stays fenced.
pub const MAINTENANCE_FILE: &str = "maintenance.json";

/// Where a process may not write, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteFence {
    reason: String,
    roots: Vec<PathBuf>,
    exempt: Vec<PathBuf>,
    /// Files inside an exempt folder that stay fenced all the same.
    still_fenced: Vec<PathBuf>,
}

impl WriteFence {
    /// A fence over `roots`, with `exempt` folders inside them that stay
    /// writable. `reason` is the sentence a refused write carries.
    pub fn new(reason: impl Into<String>, roots: Vec<PathBuf>, exempt: Vec<PathBuf>) -> Self {
        WriteFence {
            reason: reason.into(),
            roots,
            exempt,
            still_fenced: Vec::new(),
        }
    }

    /// This fence, with `files` inside an exempt folder fenced all the same.
    pub fn still_fencing(mut self, files: Vec<PathBuf>) -> Self {
        self.still_fenced = files;
        self
    }

    /// The sentence a refused write carries.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Whether a write to `path` is refused: it lies under one of the roots
    /// and under none of the exempt folders, or it is one of the files
    /// fenced all the same. A relative path is read against the current
    /// directory first.
    pub fn refuses(&self, path: &Path) -> bool {
        let path = absolute(path);
        let under = |root: &PathBuf| within(&path, &absolute(root));
        self.roots.iter().any(under)
            && (!self.exempt.iter().any(under) || self.still_fenced.iter().any(under))
    }

    /// `Ok` when a write to `path` may go ahead, else the refusal.
    pub fn check(&self, path: &Path) -> Result<(), super::ConfigError> {
        if self.refuses(path) {
            return Err(super::ConfigError::PackagedWrite {
                path: path.display().to_string(),
                reason: self.reason.clone(),
            });
        }
        Ok(())
    }
}

static FENCE: OnceLock<WriteFence> = OnceLock::new();

/// Fence this process off from every write under AppData (see the module
/// doc), with `reason` as the sentence each refusal carries. The roots are
/// read now: the roaming and local AppData folders, and this machine's
/// config and state folders on every OS, so a test that makes a process
/// packaged through the debug seam sees the same refusal on macOS and
/// Linux. The per-session files in `<state_dir>/hooks` stay writable, the
/// machine-wide [`MAINTENANCE_FILE`] there does not. Only the first call
/// counts.
pub fn refuse_appdata_writes(reason: impl Into<String>) {
    let mut roots = appdata_roots();
    roots.extend(super::config_dir().ok());
    roots.extend(super::state_dir().ok());
    let hooks = super::state_dir().ok().map(|dir| dir.join(HOOKS_DIR));
    let exempt: Vec<PathBuf> = hooks.iter().cloned().collect();
    let still: Vec<PathBuf> = hooks.iter().map(|h| h.join(MAINTENANCE_FILE)).collect();
    let _ = FENCE.set(WriteFence::new(reason, roots, exempt).still_fencing(still));
}

/// The fence this process runs under, `None` for an ordinary process.
pub fn fence() -> Option<&'static WriteFence> {
    FENCE.get()
}

/// `Ok` when this process may write `path`, else the refusal.
pub fn check_write(path: &Path) -> Result<(), super::ConfigError> {
    match fence() {
        Some(fence) => fence.check(path),
        None => Ok(()),
    }
}

/// [`WriteFence::check`] under an optional fence, as an IO error, for the
/// helpers that return one.
pub(crate) fn check_io(fence: Option<&WriteFence>, path: &Path) -> std::io::Result<()> {
    match fence {
        Some(fence) => fence
            .check(path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e)),
        None => Ok(()),
    }
}

/// The real roaming and local AppData folders, read the way
/// [`super::config_dir`] reads them: the `APPDATA` value unless it names a
/// package's private store, then the known folder, and the same for
/// `LOCALAPPDATA` ([`super::local_appdata`]). Empty on every other OS.
fn appdata_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut roots = Vec::new();
        roots.extend(super::roaming_root(
            std::env::var_os("APPDATA").as_deref(),
            super::known_roaming_folder,
        ));
        roots.extend(super::local_appdata());
        roots
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// `path` made absolute against the current directory, unless it already
/// is one, Windows-shaped included (`C:\...`, `\\server\...`), so a test
/// of Windows paths reads the same on any OS.
fn absolute(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    let windows_absolute = (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/'))
        || text.starts_with(r"\\");
    if path.is_absolute() || windows_absolute {
        return path.to_path_buf();
    }
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether `path` lies at or under `root`, compared by components: split on
/// both separators, `.` dropped and `..` applied, a Windows verbatim prefix
/// (`\\?\`) removed and, on Windows, without regard to ASCII case. So
/// `AppData-old` never matches `AppData`.
fn within(path: &Path, root: &Path) -> bool {
    let path = parts(path);
    let root = parts(root);
    !root.is_empty() && path.len() >= root.len() && path[..root.len()] == root[..]
}

fn parts(path: &Path) -> Vec<String> {
    let mut text = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = text.strip_prefix("//?/UNC/") {
        text = format!("//{rest}");
    } else if let Some(rest) = text.strip_prefix("//?/") {
        text = rest.to_string();
    }
    if cfg!(windows) {
        text = text.to_ascii_lowercase();
    }
    let mut out: Vec<String> = Vec::new();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other.to_string()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fence_over(roots: &[&str], exempt: &[&str]) -> WriteFence {
        WriteFence::new(
            "inside a package",
            roots.iter().map(PathBuf::from).collect(),
            exempt.iter().map(PathBuf::from).collect(),
        )
    }

    #[test]
    fn a_path_under_a_fenced_root_is_refused() {
        let f = fence_over(&["/home/a/roaming", "/home/a/local"], &[]);
        assert!(f.refuses(Path::new("/home/a/roaming/crystalline/config.yaml")));
        assert!(f.refuses(Path::new("/home/a/local/crystalline/index.db")));
        assert!(f.refuses(Path::new("/home/a/roaming")));
    }

    #[test]
    fn a_path_outside_every_root_is_allowed() {
        let f = fence_over(&["/home/a/roaming"], &[]);
        assert!(!f.refuses(Path::new("/data/x/index.db")));
        assert!(!f.refuses(Path::new("/home/a/roaming-old/config.yaml")));
        assert!(!f.refuses(Path::new("/home/a")));
    }

    #[test]
    fn the_hooks_folder_stays_writable() {
        let f = fence_over(&["/home/a/roaming"], &["/home/a/roaming/crystalline/hooks"]);
        assert!(!f.refuses(Path::new("/home/a/roaming/crystalline/hooks/session.json")));
        assert!(f.refuses(Path::new("/home/a/roaming/crystalline/hooks.json")));
        assert!(f.refuses(Path::new("/home/a/roaming/crystalline/config.yaml")));
    }

    #[test]
    fn the_maintenance_file_in_the_hooks_folder_stays_fenced() {
        let f = fence_over(&["/home/a/roaming"], &["/home/a/roaming/crystalline/hooks"])
            .still_fencing(vec![PathBuf::from(
                "/home/a/roaming/crystalline/hooks/maintenance.json",
            )]);
        assert!(f.refuses(Path::new(
            "/home/a/roaming/crystalline/hooks/maintenance.json"
        )));
        assert!(!f.refuses(Path::new(
            "/home/a/roaming/crystalline/hooks/session-1.json"
        )));
    }

    #[test]
    fn dot_segments_cannot_climb_out_of_the_hooks_folder() {
        let f = fence_over(&["/home/a/roaming"], &["/home/a/roaming/crystalline/hooks"]);
        assert!(f.refuses(Path::new(
            "/home/a/roaming/crystalline/hooks/../config.yaml"
        )));
        assert!(f.refuses(Path::new("/home/a/./roaming/crystalline/config.yaml")));
    }

    #[test]
    fn windows_paths_match_across_separators_and_the_verbatim_prefix() {
        let f = fence_over(&[r"C:\Users\a\AppData\Roaming"], &[]);
        assert!(f.refuses(Path::new(
            r"\\?\C:\Users\a\AppData\Roaming\crystalline\config.yaml"
        )));
        assert!(f.refuses(Path::new("C:/Users/a/AppData/Roaming/crystalline")));
        assert!(!f.refuses(Path::new(r"D:\x\index.db")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_match_without_regard_to_case() {
        let f = fence_over(&[r"C:\Users\a\AppData\Roaming"], &[]);
        assert!(f.refuses(Path::new(r"c:\users\A\appdata\roaming\Crystalline\x")));
    }

    #[test]
    fn a_refusal_names_the_path_and_the_reason() {
        let f = fence_over(&["/home/a/roaming"], &[]);
        let err = f
            .check(Path::new("/home/a/roaming/crystalline/config.yaml"))
            .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("/home/a/roaming/crystalline/config.yaml"),
            "{text}"
        );
        assert!(text.contains("inside a package"), "{text}");
        assert!(f.check(Path::new("/data/x")).is_ok());
    }

    #[test]
    fn an_ordinary_process_has_no_fence() {
        // No test in this crate sets the global: a process that never calls
        // `refuse_appdata_writes` may write anywhere.
        assert!(fence().is_none());
        assert!(check_write(Path::new("/anywhere/config.yaml")).is_ok());
    }
}
