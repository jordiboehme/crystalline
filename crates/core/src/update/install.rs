//! How this binary was installed, read from the marker its package shipped
//! beside it, with path heuristics as the fallback.
//!
//! A file marker is always resolved from the canonical exe, so it speaks
//! only for the binary it was shipped with: a copy of the same binary in
//! another folder does not inherit it. An environment marker can only make
//! the updater refuse, never choose an action.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::version::Precedence;

/// The marker file name every package writes.
pub const MARKER_FILE: &str = "install-source";

/// The install-channel variable, read here only to refuse.
pub use crate::desktop_extension::CHANNEL_ENV;

/// How a Crystalline binary was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    Homebrew,
    Deb,
    Msi,
    Zip,
    Tarball,
    Docker,
    Mcpb,
    Unknown,
}

impl InstallMethod {
    /// The name shown to a person.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Homebrew => "homebrew",
            Self::Deb => "deb",
            Self::Msi => "MSI",
            Self::Zip => "zip",
            Self::Tarball => "tar.gz",
            Self::Docker => "docker",
            Self::Mcpb => "Claude Desktop extension",
            Self::Unknown => "unknown",
        }
    }

    /// Whether `update now` has an action for this method.
    pub fn acts(&self) -> bool {
        matches!(
            self,
            Self::Homebrew | Self::Deb | Self::Msi | Self::Zip | Self::Tarball
        )
    }

    /// The method a marker's content names.
    fn from_marker(value: &str) -> Option<Self> {
        match value {
            "homebrew" => Some(Self::Homebrew),
            "deb" => Some(Self::Deb),
            "docker" => Some(Self::Docker),
            "msi" => Some(Self::Msi),
            _ => None,
        }
    }
}

/// Why `update now` will not act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not an official release build (see [`official_build`]).
    NotOfficial,
    /// Running inside an app package.
    AppPackage,
    /// The marker names one method and the path reads as another.
    MarkerPathMismatch {
        marker: InstallMethod,
        path: InstallMethod,
    },
    /// A container: pull the new image.
    Docker,
    /// The Claude Desktop extension: Claude Desktop updates it.
    Mcpb,
    /// No method could be told.
    Unknown,
}

/// The detected method and, when `update now` must not act, why. Ask
/// [`Detection::acts`], never `method.acts()` alone: a refused detection may
/// keep its method for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detection {
    pub method: InstallMethod,
    pub refusal: Option<Refusal>,
}

/// A detection together with the canonical exe it was made for, the one
/// file `update now` may act on and the path it shows. `exe` is `None` when
/// the path did not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedInstall {
    pub exe: Option<PathBuf>,
    pub detection: Detection,
}

impl Detection {
    /// Whether `update now` may act: no refusal and a method it has an
    /// action for.
    pub fn acts(&self) -> bool {
        self.refusal.is_none() && self.method.acts()
    }

    /// Adds the two refusals [`detect`] cannot see, in the spec's order: an
    /// unofficial build (pass [`official_build`]) first, then an app
    /// package. Either wins over what `detect` found and keeps its method
    /// for display. Every caller goes through this before it offers or acts.
    pub fn for_build(self, official: bool, packaged: bool) -> Detection {
        if !official {
            Detection::refused(self.method, Refusal::NotOfficial)
        } else if packaged {
            Detection::refused(self.method, Refusal::AppPackage)
        } else {
            self
        }
    }

    fn acting(method: InstallMethod) -> Self {
        Self {
            method,
            refusal: None,
        }
    }

    fn refused(method: InstallMethod, refusal: Refusal) -> Self {
        Self {
            method,
            refusal: Some(refusal),
        }
    }
}

/// The operating system a detection is made for. Only the compiled one is
/// used outside the tests, which name the others to run every rule on
/// every host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Os {
    Windows,
    Linux,
    /// macOS and any other Unix.
    OtherUnix,
}

impl Os {
    const HERE: Os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else {
        Os::OtherUnix
    };

    /// Whether a package of `method` exists on this system: the MSI on
    /// Windows, Homebrew off Windows, the .deb and the image on Linux.
    fn offers(self, method: InstallMethod) -> bool {
        match method {
            InstallMethod::Msi => self == Os::Windows,
            InstallMethod::Homebrew => self != Os::Windows,
            InstallMethod::Deb | InstallMethod::Docker => self == Os::Linux,
            _ => true,
        }
    }

    /// The method of a binary nothing else speaks for.
    fn default_method(self) -> InstallMethod {
        if self == Os::Windows {
            InstallMethod::Zip
        } else {
            InstallMethod::Tarball
        }
    }
}

/// Detect how the binary at `exe` was installed.
///
/// `exe` is canonicalized here (so Homebrew's `bin/crystalline` symlink is
/// read as its Cellar target) and handed back in the result, so the caller
/// acts on the very file the markers spoke for; a path that does not
/// resolve is refused as unknown. `env` looks up an environment variable; `dpkg_list` is the text
/// of `/var/lib/dpkg/info/crystalline.list`, read by the caller as a file
/// (never through `dpkg`), or `None` when it is missing.
///
/// Markers, relative to the canonical exe's folder:
/// `../libexec/install-source` (`homebrew`),
/// `../lib/crystalline/install-source` (`deb` or `docker`) and
/// `install-source` beside the exe (`msi`). Fallbacks: off Windows, an exe
/// under `Cellar/crystalline/` is Homebrew; on Linux, an exe listed as a
/// whole line of `dpkg_list` is a .deb (and a `deb` marker counts only
/// for an exe the list holds: no list, no .deb); on Windows, an exe in
/// `%ProgramFiles%\Crystalline\bin` is the MSI. Anything else is the zip on
/// Windows and the tarball elsewhere. `ProgramFiles` is the one environment
/// value that selects a method, and only between the MSI and the zip, which
/// both only print steps.
///
/// Fails closed: a marker that does not parse, does not belong where it
/// lies, or two markers that disagree are refused as unknown. A marker that
/// the path contradicts is a `MarkerPathMismatch` naming both: a positive
/// fallback for another method, a package this system does not have (an
/// `msi` marker on Unix, a `deb` marker on Windows), or a `deb` marker for
/// an exe the dpkg list does not hold or with no dpkg list at all. `CRYSTALLINE_CHANNEL=mcpb` (any case)
/// refuses whatever the markers say, and no other value of it changes
/// anything. This function produces only the `MarkerPathMismatch`,
/// `Docker`, `Mcpb` and `Unknown` refusals, each with a method that does
/// not act; [`Detection::for_build`] adds `NotOfficial` and `AppPackage`.
pub fn detect(
    exe: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    dpkg_list: Option<&str>,
) -> DetectedInstall {
    detect_on(Os::HERE, exe, env, dpkg_list)
}

fn detect_on(
    os: Os,
    exe: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    dpkg_list: Option<&str>,
) -> DetectedInstall {
    let exe = std::fs::canonicalize(exe).ok();
    let detection = match &exe {
        Some(exe) => detect_canonical(os, exe, env, dpkg_list),
        None => Detection::refused(InstallMethod::Unknown, Refusal::Unknown),
    };
    // The channel refuses whatever the path and the markers say.
    let detection = if env(CHANNEL_ENV).is_some_and(|v| v.trim().eq_ignore_ascii_case("mcpb")) {
        Detection::refused(InstallMethod::Mcpb, Refusal::Mcpb)
    } else {
        detection
    };
    DetectedInstall { exe, detection }
}

fn detect_canonical(
    os: Os,
    exe: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    dpkg_list: Option<&str>,
) -> Detection {
    let unknown = Detection::refused(InstallMethod::Unknown, Refusal::Unknown);
    let Ok(marker) = marker_method(exe) else {
        return unknown;
    };
    let path = path_method(os, exe, env, dpkg_list);
    let mismatch = |marker, path| {
        Detection::refused(
            InstallMethod::Unknown,
            Refusal::MarkerPathMismatch { marker, path },
        )
    };
    match (marker, path) {
        (Some(marker), Some(path)) if marker != path => mismatch(marker, path),
        (Some(marker), None) if !os.offers(marker) => mismatch(marker, os.default_method()),
        // The list does not hold this exe, or there is none: dpkg does not
        // own it. (When it does, the path reads as Deb too.)
        (Some(InstallMethod::Deb), None) => mismatch(InstallMethod::Deb, os.default_method()),
        (Some(InstallMethod::Docker), _) => {
            Detection::refused(InstallMethod::Docker, Refusal::Docker)
        }
        (Some(method), _) | (None, Some(method)) => Detection::acting(method),
        (None, None) => Detection::acting(os.default_method()),
    }
}

/// The method the markers beside `exe` name, `Ok(None)` when there is none,
/// `Err` when one is unreadable, unknown, in the wrong place, or two
/// disagree.
fn marker_method(exe: &Path) -> Result<Option<InstallMethod>, ()> {
    let Some(dir) = exe.parent() else {
        return Ok(None);
    };
    let up = dir.parent();
    let slots: [(Option<PathBuf>, &[InstallMethod]); 3] = [
        (
            up.map(|up| up.join("libexec").join(MARKER_FILE)),
            &[InstallMethod::Homebrew],
        ),
        (
            up.map(|up| up.join("lib").join("crystalline").join(MARKER_FILE)),
            &[InstallMethod::Deb, InstallMethod::Docker],
        ),
        (Some(dir.join(MARKER_FILE)), &[InstallMethod::Msi]),
    ];
    let mut found = None;
    for (path, allowed) in slots {
        let Some(path) = path else { continue };
        let value = match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() => read_marker(&path)?,
            Ok(_) => return Err(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(()),
        };
        let method = InstallMethod::from_marker(value.trim()).ok_or(())?;
        if !allowed.contains(&method) || found.is_some_and(|f| f != method) {
            return Err(());
        }
        found = Some(method);
    }
    Ok(found)
}

/// The longest marker read; every valid one is a word and a line end.
const MARKER_MAX_BYTES: u64 = 64;

/// A marker's text: at most [`MARKER_MAX_BYTES`] of UTF-8 from a regular
/// file, so a huge file or a device cannot stall the session hook. A longer
/// file is refused, not cut.
fn read_marker(path: &Path) -> Result<String, ()> {
    let file = std::fs::File::open(path).map_err(|_| ())?;
    let mut bytes = Vec::new();
    file.take(MARKER_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > MARKER_MAX_BYTES {
        return Err(());
    }
    String::from_utf8(bytes).map_err(|_| ())
}

/// The method the exe's path alone points to, `None` when no heuristic
/// matches (the zip or tarball default is not a positive reading). Each
/// heuristic is asked only on the system its package exists on.
fn path_method(
    os: Os,
    exe: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    dpkg_list: Option<&str>,
) -> Option<InstallMethod> {
    if os.offers(InstallMethod::Homebrew) && under_cellar(exe) {
        return Some(InstallMethod::Homebrew);
    }
    if os.offers(InstallMethod::Deb) && dpkg_list.is_some_and(|list| lists(list, exe)) {
        return Some(InstallMethod::Deb);
    }
    if os.offers(InstallMethod::Msi) && in_default_msi_folder(exe, env) {
        return Some(InstallMethod::Msi);
    }
    None
}

/// A `Cellar` segment directly followed by `crystalline`.
fn under_cellar(exe: &Path) -> bool {
    let segs: Vec<_> = exe
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();
    segs.windows(2)
        .any(|w| w[0] == "Cellar" && w[1] == "crystalline")
}

/// Whether `exe` is a whole line of the package's file list.
fn lists(list: &str, exe: &Path) -> bool {
    let Some(exe) = exe.to_str() else {
        return false;
    };
    list.lines().any(|line| line.trim_end_matches('\r') == exe)
}

/// Whether the exe's folder is `%ProgramFiles%\Crystalline\bin`. Compared as
/// text, the way Windows compares paths: without the verbatim prefix
/// `canonicalize` adds, with either slash, without a trailing separator and
/// case folded.
fn in_default_msi_folder(exe: &Path, env: &dyn Fn(&str) -> Option<String>) -> bool {
    fn plain(text: &str) -> String {
        display_path(Path::new(text))
            .replace('/', r"\")
            .trim_end_matches('\\')
            .to_lowercase()
    }
    let (Some(dir), Some(program_files)) = (exe.parent(), env("ProgramFiles")) else {
        return false;
    };
    let default = format!(r"{}\Crystalline\bin", plain(&program_files));
    plain(&dir.to_string_lossy()) == plain(&default)
}

/// A path as a person types it: without the verbatim prefix `canonicalize`
/// adds on Windows (`\\?\C:\...` becomes `C:\...`, `\\?\UNC\host\share`
/// becomes `\\host\share`). Any other path is shown as it is.
pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text.into_owned()
    }
}

/// Whether this binary was built by the release workflow, which sets
/// `CRYSTALLINE_OFFICIAL_BUILD=1` at compile time. Any other build refuses
/// `update now`, so a self-built binary is never replaced.
pub fn official_build() -> bool {
    option_env!("CRYSTALLINE_OFFICIAL_BUILD") == Some("1")
}

/// The release platform name of the compiled target, as the release assets
/// spell it; `None` for a target Crystalline does not publish.
pub fn release_platform() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("macos-arm64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("macos-intel")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-amd64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("linux-arm64")
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some("windows-amd64")
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        Some("windows-arm64")
    } else {
        None
    }
}

/// The release asset `update now` downloads for `method` on this target.
/// `None` for a method that downloads nothing (Homebrew, Docker, mcpb,
/// unknown), a target without that asset, or a version that is not stable.
pub fn asset_name(method: InstallMethod, version: &Precedence) -> Option<String> {
    asset_name_for(method, version, release_platform()?)
}

fn asset_name_for(method: InstallMethod, version: &Precedence, platform: &str) -> Option<String> {
    if !version.is_stable() {
        return None;
    }
    let ver = version.render();
    let windows = platform.starts_with("windows-");
    match method {
        InstallMethod::Tarball if !windows => Some(format!("crystalline-v{ver}-{platform}.tar.gz")),
        InstallMethod::Zip if windows => Some(format!("crystalline-v{ver}-{platform}.zip")),
        InstallMethod::Msi if windows => Some(format!("crystalline-v{ver}-{platform}.msi")),
        InstallMethod::Deb => {
            let arch = platform.strip_prefix("linux-")?;
            Some(format!("crystalline_{ver}_{arch}.deb"))
        }
        _ => None,
    }
}

/// The member of the downloaded archive that holds the binary:
/// `crystalline-v<ver>-<platform>/crystalline` in the tar.gz,
/// `crystalline.exe` at the root of the zip. `None` for a target
/// Crystalline does not publish or a version that is not stable.
pub fn archive_member(version: &Precedence) -> Option<String> {
    archive_member_for(version, release_platform()?)
}

fn archive_member_for(version: &Precedence, platform: &str) -> Option<String> {
    if !version.is_stable() {
        return None;
    }
    if platform.starts_with("windows-") {
        Some("crystalline.exe".to_string())
    } else {
        Some(format!(
            "crystalline-v{}-{platform}/crystalline",
            version.render()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const EXE: &str = if cfg!(windows) {
        "crystalline.exe"
    } else {
        "crystalline"
    };

    /// A canonical exe at `rel` under `root`, with its folders.
    fn exe_at(root: &Path, rel: &str) -> PathBuf {
        let path = root.join(rel).join(EXE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
        std::fs::canonicalize(&path).unwrap()
    }

    /// The marker path for `exe` at `rel` from its folder. A leading `../`
    /// steps up by hand, since a verbatim Windows path keeps `..` literally.
    fn marker_path(exe: &Path, rel: &str) -> PathBuf {
        let mut dir = exe.parent().unwrap().to_path_buf();
        let mut rest = rel;
        while let Some(tail) = rest.strip_prefix("../") {
            dir.pop();
            rest = tail;
        }
        if rest != "." {
            dir.push(rest);
        }
        dir.join(MARKER_FILE)
    }

    fn marker(exe: &Path, rel: &str, value: &str) {
        let path = marker_path(exe, rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value).unwrap();
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn with_env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    /// The detection alone; most tests look only at that. Shadows the
    /// glob-imported functions of the same names.
    fn detect(
        exe: &Path,
        env: &dyn Fn(&str) -> Option<String>,
        dpkg_list: Option<&str>,
    ) -> Detection {
        super::detect(exe, env, dpkg_list).detection
    }

    fn detect_on(
        os: Os,
        exe: &Path,
        env: &dyn Fn(&str) -> Option<String>,
        dpkg_list: Option<&str>,
    ) -> Detection {
        super::detect_on(os, exe, env, dpkg_list).detection
    }

    fn acting(method: InstallMethod) -> Detection {
        Detection {
            method,
            refusal: None,
        }
    }

    fn mismatch(marker: InstallMethod, path: InstallMethod) -> Detection {
        Detection {
            method: InstallMethod::Unknown,
            refusal: Some(Refusal::MarkerPathMismatch { marker, path }),
        }
    }

    const UNKNOWN: Detection = Detection {
        method: InstallMethod::Unknown,
        refusal: Some(Refusal::Unknown),
    };

    fn v(text: &str) -> Precedence {
        Precedence::parse(text).unwrap()
    }

    #[test]
    fn labels_and_actions() {
        use InstallMethod::*;
        let all = [Homebrew, Deb, Msi, Zip, Tarball, Docker, Mcpb, Unknown];
        let labels: Vec<_> = all.iter().map(|m| m.label()).collect();
        assert_eq!(
            labels,
            [
                "homebrew",
                "deb",
                "MSI",
                "zip",
                "tar.gz",
                "docker",
                "Claude Desktop extension",
                "unknown"
            ]
        );
        let acts: Vec<_> = all.iter().map(|m| m.acts()).collect();
        assert_eq!(acts, [true, true, true, true, true, false, false, false]);
    }

    #[test]
    fn homebrew_marker_in_libexec() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "prefix/bin");
        marker(&exe, "../libexec", "homebrew\n");
        for os in [Os::OtherUnix, Os::Linux] {
            assert_eq!(
                detect_on(os, &exe, &no_env, None),
                acting(InstallMethod::Homebrew)
            );
        }
    }

    #[test]
    fn cellar_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "Cellar/crystalline/0.33.0/bin");
        assert_eq!(
            detect_on(Os::OtherUnix, &exe, &no_env, None),
            acting(InstallMethod::Homebrew)
        );
        // No Homebrew on Windows: the folder name means nothing there.
        assert_eq!(
            detect_on(Os::Windows, &exe, &no_env, None),
            acting(InstallMethod::Zip)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_read_as_its_target() {
        // Homebrew links `<prefix>/bin/crystalline` to the Cellar copy, and
        // the marker sits beside the target, not beside the link.
        let tmp = tempfile::tempdir().unwrap();
        let target = exe_at(tmp.path(), "prefix/Cellar/crystalline/0.33.0/bin");
        marker(&target, "../libexec", "homebrew\n");
        let link_dir = tmp.path().join("prefix/bin");
        std::fs::create_dir_all(&link_dir).unwrap();
        let link = link_dir.join(EXE);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            detect_on(Os::OtherUnix, &link, &no_env, None),
            acting(InstallMethod::Homebrew)
        );
        assert_eq!(
            detect(&link, &no_env, None),
            acting(InstallMethod::Homebrew)
        );
    }

    #[test]
    fn a_path_that_does_not_resolve_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("bin").join(EXE);
        assert_eq!(detect(&gone, &no_env, None), UNKNOWN);
    }

    #[test]
    fn deb_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "usr/bin");
        marker(&exe, "../lib/crystalline", "deb\n");
        // No dpkg list at all: a real .deb install always has one.
        assert_eq!(
            detect_on(Os::Linux, &exe, &no_env, None),
            mismatch(InstallMethod::Deb, InstallMethod::Tarball)
        );
        let list = format!("{}\n", exe.display());
        assert_eq!(
            detect_on(Os::Linux, &exe, &no_env, Some(&list)),
            acting(InstallMethod::Deb)
        );
    }

    #[test]
    fn a_deb_marker_for_an_exe_dpkg_does_not_own_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "usr/bin");
        marker(&exe, "../lib/crystalline", "deb\n");
        let list = "/.\n/usr\n/usr/bin\n/usr/bin/crystalline\n";
        assert_eq!(
            detect_on(Os::Linux, &exe, &no_env, Some(list)),
            mismatch(InstallMethod::Deb, InstallMethod::Tarball)
        );
    }

    #[test]
    fn dpkg_list_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "usr/bin");
        let list = format!("/.\n/usr\n{}\n/usr/share/doc/crystalline\n", exe.display());
        assert_eq!(
            detect_on(Os::Linux, &exe, &no_env, Some(&list)),
            acting(InstallMethod::Deb)
        );
        // A path that only starts with the exe's is not a listing of it.
        let near = format!("{}-old\n", exe.display());
        assert_eq!(
            detect_on(Os::Linux, &exe, &no_env, Some(&near)),
            acting(InstallMethod::Tarball)
        );
        // No dpkg off Linux.
        assert_eq!(
            detect_on(Os::OtherUnix, &exe, &no_env, Some(&list)),
            acting(InstallMethod::Tarball)
        );
    }

    #[test]
    fn msi_marker_beside_exe() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "Custom/bin");
        marker(&exe, ".", "msi\r\n");
        assert_eq!(
            detect_on(Os::Windows, &exe, &no_env, None),
            acting(InstallMethod::Msi)
        );
    }

    #[cfg(windows)]
    #[test]
    fn msi_marker_beside_a_verbatim_windows_exe() {
        // The real detection on Windows: `canonicalize` adds `\\?\`.
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "Custom Folder/bin");
        assert!(exe.to_string_lossy().starts_with(r"\\?\"));
        marker(&exe, ".", "msi\r\n");
        assert_eq!(detect(&exe, &no_env, None), acting(InstallMethod::Msi));
    }

    #[test]
    fn msi_default_folder_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let root_text = root.to_str().unwrap().to_string();
        let exe = exe_at(&root, "Crystalline/bin");
        for program_files in [
            root_text.clone(),
            root_text.to_uppercase(),
            format!(r"{root_text}\"),
            format!("{root_text}/"),
        ] {
            let env = with_env(&[("ProgramFiles", &program_files)]);
            assert_eq!(
                detect_on(Os::Windows, &exe, &env, None),
                acting(InstallMethod::Msi),
                "ProgramFiles={program_files}"
            );
            // Off Windows there is no MSI, whatever the variable says.
            assert_eq!(
                detect_on(Os::Linux, &exe, &env, None),
                acting(InstallMethod::Tarball)
            );
        }
        // Unset, or another folder under Program Files: the zip.
        assert_eq!(
            detect_on(Os::Windows, &exe, &no_env, None),
            acting(InstallMethod::Zip)
        );
        let other = exe_at(&root, "Other/bin");
        let env = with_env(&[("ProgramFiles", &root_text)]);
        assert_eq!(
            detect_on(Os::Windows, &other, &env, None),
            acting(InstallMethod::Zip)
        );
    }

    #[test]
    fn docker_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "usr/local/bin");
        marker(&exe, "../lib/crystalline", "docker\n");
        assert_eq!(
            detect_on(Os::Linux, &exe, &no_env, None),
            Detection {
                method: InstallMethod::Docker,
                refusal: Some(Refusal::Docker)
            }
        );
    }

    #[test]
    fn a_docker_marker_on_a_cellar_path_is_a_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "Cellar/crystalline/0.33.0/bin");
        marker(&exe, "../lib/crystalline", "docker\n");
        assert_eq!(
            detect_on(Os::Linux, &exe, &no_env, None),
            mismatch(InstallMethod::Docker, InstallMethod::Homebrew)
        );
    }

    #[test]
    fn mcpb_env_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "usr/bin");
        marker(&exe, "../lib/crystalline", "deb\n");
        let mcpb = Detection {
            method: InstallMethod::Mcpb,
            refusal: Some(Refusal::Mcpb),
        };
        for value in ["mcpb", "MCPB ", " Mcpb\n"] {
            let env = with_env(&[(CHANNEL_ENV, value)]);
            assert_eq!(detect_on(Os::Linux, &exe, &env, None), mcpb, "{value:?}");
        }
        // Even for an exe that no longer resolves.
        let env = with_env(&[(CHANNEL_ENV, "mcpb")]);
        assert_eq!(detect(&tmp.path().join("gone"), &env, None), mcpb);
    }

    #[test]
    fn a_deb_marker_does_not_speak_for_a_tarball_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let packaged = exe_at(tmp.path(), "usr/bin");
        marker(&packaged, "../lib/crystalline", "deb\n");
        let copy = exe_at(tmp.path(), "home/me/.local/bin");
        let list = format!("{}\n", packaged.display());
        assert_eq!(
            detect_on(Os::Linux, &packaged, &no_env, Some(&list)),
            acting(InstallMethod::Deb)
        );
        assert_eq!(
            detect_on(Os::Linux, &copy, &no_env, Some(&list)),
            acting(InstallMethod::Tarball)
        );
    }

    #[test]
    fn env_markers_never_choose_an_action() {
        let tmp = tempfile::tempdir().unwrap();
        let plain = exe_at(tmp.path(), "opt/bin");
        let cellar = exe_at(tmp.path(), "Cellar/crystalline/0.33.0/bin");
        for value in ["deb", "homebrew", "msi", "docker", "zip", "mcpb-not"] {
            let env = with_env(&[(CHANNEL_ENV, value)]);
            for os in [Os::Windows, Os::Linux, Os::OtherUnix] {
                for exe in [&plain, &cellar] {
                    assert_eq!(
                        detect_on(os, exe, &env, None),
                        detect_on(os, exe, &no_env, None),
                        "{CHANNEL_ENV}={value} on {os:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn marker_and_path_disagree() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "usr/bin");
        marker(&exe, "../libexec", "homebrew\n");
        let list = format!("{}\n", exe.display());
        assert_eq!(
            detect_on(Os::Linux, &exe, &no_env, Some(&list)),
            mismatch(InstallMethod::Homebrew, InstallMethod::Deb)
        );
    }

    #[test]
    fn a_marker_for_another_platform_is_refused() {
        use InstallMethod::*;
        for (os, rel, value, want) in [
            (Os::Linux, ".", "msi", mismatch(Msi, Tarball)),
            (Os::OtherUnix, ".", "msi", mismatch(Msi, Tarball)),
            (
                Os::OtherUnix,
                "../lib/crystalline",
                "deb",
                mismatch(Deb, Tarball),
            ),
            (
                Os::OtherUnix,
                "../lib/crystalline",
                "docker",
                mismatch(Docker, Tarball),
            ),
            (Os::Windows, "../lib/crystalline", "deb", mismatch(Deb, Zip)),
            (
                Os::Windows,
                "../libexec",
                "homebrew",
                mismatch(Homebrew, Zip),
            ),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let exe = exe_at(tmp.path(), "prefix/bin");
            marker(&exe, rel, value);
            assert_eq!(
                detect_on(os, &exe, &no_env, None),
                want,
                "{value} in {rel} on {os:?}"
            );
        }
    }

    #[test]
    fn a_marker_that_does_not_fit_is_refused() {
        for (rel, value) in [
            ("../libexec", "deb"),
            ("../lib/crystalline", "homebrew"),
            (".", "deb"),
            ("../lib/crystalline", "tarball"),
            ("../libexec", ""),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let exe = exe_at(tmp.path(), "prefix/bin");
            marker(&exe, rel, value);
            for os in [Os::Windows, Os::Linux, Os::OtherUnix] {
                assert_eq!(
                    detect_on(os, &exe, &no_env, None),
                    UNKNOWN,
                    "{rel} = {value:?}"
                );
            }
        }
        // Two markers that name different methods.
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "prefix/bin");
        marker(&exe, "../libexec", "homebrew");
        marker(&exe, "../lib/crystalline", "deb");
        assert_eq!(detect_on(Os::Linux, &exe, &no_env, None), UNKNOWN);
    }

    #[test]
    fn an_unreadable_marker_is_refused() {
        // A folder where the marker file should be: not "not found".
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "prefix/bin");
        std::fs::create_dir_all(marker_path(&exe, "../libexec")).unwrap();
        assert_eq!(detect_on(Os::OtherUnix, &exe, &no_env, None), UNKNOWN);
    }

    #[test]
    fn asset_names_match_the_release_listing() {
        use InstallMethod::*;
        let ver = v("0.33.0");
        let name = |m, p| asset_name_for(m, &ver, p);
        assert_eq!(
            name(Tarball, "linux-amd64").as_deref(),
            Some("crystalline-v0.33.0-linux-amd64.tar.gz")
        );
        assert_eq!(
            name(Tarball, "macos-intel").as_deref(),
            Some("crystalline-v0.33.0-macos-intel.tar.gz")
        );
        assert_eq!(
            name(Msi, "windows-arm64").as_deref(),
            Some("crystalline-v0.33.0-windows-arm64.msi")
        );
        assert_eq!(
            name(Zip, "windows-amd64").as_deref(),
            Some("crystalline-v0.33.0-windows-amd64.zip")
        );
        assert_eq!(
            name(Deb, "linux-arm64").as_deref(),
            Some("crystalline_0.33.0_arm64.deb")
        );
        assert_eq!(
            name(Deb, "linux-amd64").as_deref(),
            Some("crystalline_0.33.0_amd64.deb")
        );
        // Methods that download nothing, and assets that are not published.
        for (m, p) in [
            (Homebrew, "macos-arm64"),
            (Docker, "linux-amd64"),
            (Mcpb, "macos-arm64"),
            (Unknown, "linux-amd64"),
            (Deb, "macos-arm64"),
            (Msi, "linux-amd64"),
            (Zip, "macos-arm64"),
            (Tarball, "windows-amd64"),
        ] {
            assert_eq!(name(m, p), None, "{m:?} on {p}");
        }
        assert_eq!(
            archive_member_for(&ver, "linux-amd64").as_deref(),
            Some("crystalline-v0.33.0-linux-amd64/crystalline")
        );
        assert_eq!(
            archive_member_for(&ver, "windows-arm64").as_deref(),
            Some("crystalline.exe")
        );
    }

    #[test]
    fn a_pre_release_has_no_asset() {
        let pre = v("0.34.0-dev.12");
        assert_eq!(
            asset_name_for(InstallMethod::Tarball, &pre, "linux-amd64"),
            None
        );
        assert_eq!(archive_member_for(&pre, "linux-amd64"), None);
    }

    #[test]
    fn every_published_target_names_its_platform() {
        // CI builds only published targets, so this holds on every runner.
        let published = cfg!(all(
            any(target_os = "macos", target_os = "linux", windows),
            any(target_arch = "x86_64", target_arch = "aarch64")
        ));
        assert_eq!(release_platform().is_some(), published);
        if published {
            assert!(archive_member(&v("0.33.0")).is_some());
        }
    }

    #[test]
    fn the_canonical_exe_comes_back_with_the_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "prefix/bin");
        let found = super::detect(&exe, &no_env, None);
        assert_eq!(found.exe.as_deref(), Some(exe.as_path()));
        let gone = tmp.path().join("gone").join(EXE);
        let found = super::detect(&gone, &no_env, None);
        assert_eq!(found.exe, None);
        assert_eq!(found.detection, UNKNOWN);
        // The channel refuses, and the exe still comes back.
        let found = super::detect(&exe, &with_env(&[(CHANNEL_ENV, "mcpb")]), None);
        assert_eq!(found.exe.as_deref(), Some(exe.as_path()));
        assert_eq!(found.detection.refusal, Some(Refusal::Mcpb));
    }

    #[test]
    fn display_path_drops_the_verbatim_prefix() {
        let d = |p: &str| display_path(Path::new(p));
        assert_eq!(
            d(r"\\?\C:\Program Files\Crystalline\bin"),
            r"C:\Program Files\Crystalline\bin"
        );
        assert_eq!(d(r"\\?\UNC\host\share\bin"), r"\\host\share\bin");
        assert_eq!(
            d("/usr/local/bin/crystalline"),
            "/usr/local/bin/crystalline"
        );
        assert_eq!(d(r"C:\x"), r"C:\x");
    }

    #[test]
    fn a_detect_refusal_never_comes_with_an_acting_method() {
        let tmp = tempfile::tempdir().unwrap();
        let mut seen = Vec::new();
        let cases: [(&str, &str, &str); 7] = [
            ("a", "../libexec", "homebrew\n"),
            ("b", "../lib/crystalline", "deb\n"),
            ("c", "../lib/crystalline", "docker\n"),
            ("d", ".", "msi\n"),
            ("e", ".", "nonsense\n"),
            ("f", "../libexec", "deb\n"),
            ("g", ".", ""),
        ];
        for (dir, rel, value) in cases {
            let exe = exe_at(tmp.path(), &format!("{dir}/Cellar/crystalline/1/bin"));
            if !value.is_empty() {
                marker(&exe, rel, value);
            }
            for os in [Os::Windows, Os::Linux, Os::OtherUnix] {
                for env in [with_env(&[]), with_env(&[(CHANNEL_ENV, "MCPB")])] {
                    seen.push(detect_on(os, &exe, &env, None));
                    seen.push(detect_on(os, &exe, &env, Some("")));
                }
            }
        }
        seen.push(detect(&tmp.path().join("gone"), &no_env, None));
        assert!(seen.iter().any(|d| d.refusal.is_none()));
        assert!(seen.iter().any(|d| d.refusal.is_some()));
        for d in seen {
            if d.refusal.is_some() {
                assert!(!d.method.acts(), "{d:?}");
                assert!(!d.acts(), "{d:?}");
            } else {
                assert_eq!(d.acts(), d.method.acts(), "{d:?}");
            }
        }
    }

    #[test]
    fn the_build_refusals_win_in_order_and_keep_the_method() {
        let outcomes = [
            acting(InstallMethod::Tarball),
            acting(InstallMethod::Homebrew),
            mismatch(InstallMethod::Deb, InstallMethod::Tarball),
            UNKNOWN,
            Detection {
                method: InstallMethod::Mcpb,
                refusal: Some(Refusal::Mcpb),
            },
        ];
        for found in outcomes {
            for packaged in [false, true] {
                let unofficial = found.for_build(false, packaged);
                assert_eq!(unofficial.refusal, Some(Refusal::NotOfficial));
                assert_eq!(unofficial.method, found.method);
                assert!(!unofficial.acts());
            }
            let packaged = found.for_build(true, true);
            assert_eq!(packaged.refusal, Some(Refusal::AppPackage));
            assert_eq!(packaged.method, found.method);
            assert!(!packaged.acts());
            assert_eq!(found.for_build(true, false), found);
        }
        assert!(acting(InstallMethod::Tarball).for_build(true, false).acts());
    }

    #[test]
    fn a_marker_that_is_too_long_or_not_a_file_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = exe_at(tmp.path(), "long/bin");
        marker(&exe, "../libexec", &format!("homebrew{}\n", " ".repeat(64)));
        assert_eq!(detect_on(Os::OtherUnix, &exe, &no_env, None), UNKNOWN);
        let exe = exe_at(tmp.path(), "dir/bin");
        std::fs::create_dir_all(marker_path(&exe, "../libexec")).unwrap();
        assert_eq!(detect_on(Os::OtherUnix, &exe, &no_env, None), UNKNOWN);
        let exe = exe_at(tmp.path(), "bytes/bin");
        let path = marker_path(&exe, "../libexec");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, [0xff, 0xfe]).unwrap();
        assert_eq!(detect_on(Os::OtherUnix, &exe, &no_env, None), UNKNOWN);
        // A marker just under the limit still reads.
        let exe = exe_at(tmp.path(), "padded/bin");
        marker(&exe, "../libexec", &format!("homebrew{}", " ".repeat(56)));
        assert_eq!(
            detect_on(Os::OtherUnix, &exe, &no_env, None),
            acting(InstallMethod::Homebrew)
        );
    }
}
