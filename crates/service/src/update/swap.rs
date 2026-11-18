//! The tar.gz update on Unix (S12): the one member that holds the binary is
//! extracted into a file the updater creates in the exe's own folder, the
//! new binary is checked (its macOS code signature, then its `--version`),
//! and a plain `rename` puts it over the canonical exe. The old binary is
//! never written in place, which on macOS would break its code signature
//! for every process that still runs it; a failed rename leaves it as it
//! was, byte for byte.
//!
//! Under `sudo` (S5) only the download, the checks and the swap run; the
//! caller skips `update finish`, so root never touches the person's daemon.
//!
//! Every program the update runs has a time limit: the new binary's
//! `--version` ([`VERSION_TIME_LIMIT`]) and `update finish`
//! ([`FINISH_TIME_LIMIT`]), which runs after the caller let go of the
//! update lock. The extracted file carries the run's process id, and a run
//! removes only its own leftovers and those of a process that is gone, so
//! two runs with different locks (a `sudo` run and a normal one) never
//! remove each other's file.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crystalline_core::update::{Checks, LatestRelease};
use crystalline_core::version::Precedence;

use super::client::UpdateError;
use super::codesign::codesign_ok;
use super::now::fetch_verified;
use super::spawn::Spawner;

/// The name prefix of the file a run extracts the new binary into, followed
/// by the run's process id. Hidden, and never a name anything runs.
pub const NEW_PREFIX: &str = ".crystalline-new-";

/// The largest binary an archive member may unpack to.
pub(super) const MAX_BINARY_BYTES: u64 = 1024 * 1024 * 1024;

/// Whether this process runs under `sudo`: effective user id 0 with
/// `SUDO_UID` naming the person who ran it (S5). Root itself running `sudo`
/// (`SUDO_UID` 0) is no person to hand anything back to.
pub fn under_sudo(euid: u32, sudo_uid: Option<&str>) -> bool {
    euid == 0
        && sudo_uid
            .and_then(|uid| uid.trim().parse::<u32>().ok())
            .is_some_and(|uid| uid != 0)
}

/// Whether this process may create and rename files in the folder of
/// `canonical_exe`, checked with the effective user id. The swap needs the
/// folder, not the exe, to be writable.
pub fn writable_install_dir(canonical_exe: &Path) -> bool {
    let Some(dir) = canonical_exe.parent() else {
        return false;
    };
    writable(dir)
}

#[cfg(unix)]
fn writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a valid NUL-terminated string that outlives the
    // call; faccessat only reads it.
    unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::W_OK, libc::AT_EACCESS) == 0 }
}

#[cfg(not(unix))]
fn writable(_dir: &Path) -> bool {
    false
}

/// Whether the folder of `canonical_exe` belongs to root, so `sudo
/// crystalline update now` is the way to update it.
pub fn root_owned_install_dir(canonical_exe: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        canonical_exe
            .parent()
            .and_then(|dir| std::fs::metadata(dir).ok())
            .is_some_and(|meta| meta.uid() == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = canonical_exe;
        false
    }
}

/// Extract exactly `member` of the gzip-compressed tar `archive` into a new
/// file `into/.crystalline-new-<pid>`, mode 0755, and hand back its path.
/// Nothing else of the archive is ever written: every other entry, a
/// `../` path among them, is read past. The member must be one plain file
/// and appear once; a link, a folder, a second copy or no copy at all is
/// [`UpdateError::NoMember`]. The file is created new (never an existing
/// one opened) and removed again on any failure.
pub fn extract_member(archive: &Path, member: &str, into: &Path) -> Result<PathBuf, UpdateError> {
    let target = into.join(format!("{NEW_PREFIX}{}", std::process::id()));
    let mut created = false;
    let result = extract_into(archive, member, &target, &mut created);
    // Only a file this run created is removed: a name that was taken
    // already is someone else's.
    if result.is_err() && created {
        let _ = std::fs::remove_file(&target);
    }
    result.map(|()| target)
}

fn extract_into(
    archive: &Path,
    member: &str,
    target: &Path,
    created: &mut bool,
) -> Result<(), UpdateError> {
    let file = File::open(archive).map_err(UpdateError::Extract)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut found = false;
    for entry in tar.entries().map_err(UpdateError::Extract)? {
        let entry = entry.map_err(UpdateError::Extract)?;
        if entry.path_bytes().as_ref() != member.as_bytes() {
            continue;
        }
        if found || !entry.header().entry_type().is_file() {
            return Err(UpdateError::NoMember);
        }
        found = true;
        let mut out = create_new_executable(target).map_err(UpdateError::Extract)?;
        *created = true;
        set_mode(&out, 0o755).map_err(UpdateError::Extract)?;
        let copied = std::io::copy(&mut entry.take(MAX_BINARY_BYTES + 1), &mut out)
            .map_err(UpdateError::Extract)?;
        if copied > MAX_BINARY_BYTES {
            return Err(UpdateError::Extract(std::io::Error::other(
                "the binary is larger than 1 GiB",
            )));
        }
        out.sync_all().map_err(UpdateError::Extract)?;
    }
    if found {
        Ok(())
    } else {
        Err(UpdateError::NoMember)
    }
}

/// Set the mode of the open `file` (`fchmod`), which no umask reduces.
fn set_mode(file: &File, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (file, mode);
        Ok(())
    }
}

/// Give `new` the permission bits of `canonical_exe` (masked to `0o777`),
/// set on its open handle, so neither the umask nor the extract decides who
/// may run the installed binary. An old exe that cannot be read leaves
/// `new` at 0755.
fn keep_mode(new: &Path, canonical_exe: &Path) -> Result<(), UpdateError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(canonical_exe)
            .map(|old| old.permissions().mode() & 0o777)
            .unwrap_or(0o755);
        let file = File::open(new).map_err(UpdateError::Swap)?;
        set_mode(&file, mode).map_err(UpdateError::Swap)?;
    }
    #[cfg(not(unix))]
    {
        let _ = (new, canonical_exe);
    }
    Ok(())
}

/// A new file at `path`, created with mode 0755 on Unix (the umask may
/// reduce it; [`set_mode`] sets it after); an existing file is an error.
fn create_new_executable(path: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o755);
    }
    options.open(path)
}

/// Remove the files an interrupted run left in `dir` under [`NEW_PREFIX`]:
/// only `.crystalline-new-<pid>` files whose `<pid>` is this process or a
/// process that is no longer alive. A file of a run still going (another
/// update, under `sudo` with a lock of its own) and any other name are
/// left alone.
pub fn remove_new_leftovers(dir: &Path) {
    remove_new_leftovers_with(dir, crate::instance::process_alive);
}

fn remove_new_leftovers_with(dir: &Path, alive: impl Fn(u32) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let own = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix(NEW_PREFIX))
            .filter(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own || !alive(pid) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// How long the new binary may take to print its version before it is
/// killed and the update fails, the old exe untouched.
pub const VERSION_TIME_LIMIT: Duration = Duration::from_secs(30);

/// How long `update finish` may take: the old daemon gets up to about
/// twenty seconds to leave (the ask, then two signals) and its successor
/// fifteen to start, so two minutes is room for a slow machine. Past it
/// `update finish` is killed; the binary is in place by then.
pub const FINISH_TIME_LIMIT: Duration = Duration::from_secs(120);

/// Whether `bin --version` names exactly `want`: its first line must be
/// `crystalline <want>`, as clap prints it before the copyright lines. A
/// binary that does not answer within [`VERSION_TIME_LIMIT`] is killed and
/// fails the check.
pub fn version_matches(
    spawner: &dyn Spawner,
    bin: &Path,
    want: &Precedence,
) -> Result<(), UpdateError> {
    version_matches_within(spawner, bin, want, VERSION_TIME_LIMIT)
}

fn version_matches_within(
    spawner: &dyn Spawner,
    bin: &Path,
    want: &Precedence,
    limit: Duration,
) -> Result<(), UpdateError> {
    let program = bin.to_str().ok_or(UpdateError::VersionMismatch)?;
    let printed = spawner
        .output_within(program, &["--version"], limit)
        .map_err(|_| UpdateError::VersionMismatch)?;
    let expected = format!("crystalline {}", want.render());
    if printed.lines().next().map(str::trim_end) == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(UpdateError::VersionMismatch)
    }
}

/// Put `new` over `canonical_exe` with one plain `rename`. On an error the
/// new file is removed and the old exe is untouched.
pub fn swap(new: &Path, canonical_exe: &Path) -> Result<(), UpdateError> {
    std::fs::rename(new, canonical_exe).map_err(|err| {
        let _ = std::fs::remove_file(new);
        UpdateError::Swap(err)
    })
}

/// Give `new` the owner of `canonical_exe` when this process runs as root,
/// so a `sudo` update never leaves a root-owned binary in a folder that
/// belongs to someone else.
fn match_owner(new: &Path, canonical_exe: &Path) -> Result<(), UpdateError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid takes no arguments, cannot fail and touches no
        // memory.
        if unsafe { libc::geteuid() } == 0 {
            let old = std::fs::metadata(canonical_exe).map_err(UpdateError::Swap)?;
            std::os::unix::fs::chown(new, Some(old.uid()), Some(old.gid()))
                .map_err(UpdateError::Swap)?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (new, canonical_exe);
    }
    Ok(())
}

/// The arguments the old binary runs the new one with once it is in place
/// (S4).
pub const FINISH_ARGS: [&str; 2] = ["update", "finish"];

/// Run `<canonical_exe> update finish`, the new binary now in place, with
/// the terminal's input and output, and hand back its exit code; past
/// [`FINISH_TIME_LIMIT`] it is killed and this is an error. Never under
/// `sudo` (S5): the caller skips it there. Call it after letting go of the
/// update lock.
pub fn finish_after_swap(spawner: &dyn Spawner, canonical_exe: &Path) -> std::io::Result<i32> {
    spawner.run_within(
        &canonical_exe.to_string_lossy(),
        &FINISH_ARGS,
        FINISH_TIME_LIMIT,
    )
}

/// The tar.gz update after the lock and the lookup: download and verify
/// `asset` of `release` into `download_dir` ([`fetch_verified`]), extract
/// `member` beside `canonical_exe`, check its code signature when
/// `check_codesign` (macOS), check its `--version` against the release,
/// then [`swap`]. Leftovers of an interrupted run in the exe's folder are
/// removed first; call it under the update lock. On any failure the old
/// exe is untouched and the extracted file is gone.
///
/// On success the checks that ran.
pub async fn install_tarball(
    spawner: &dyn Spawner,
    release: &LatestRelease,
    asset: &str,
    member: &str,
    download_dir: &Path,
    canonical_exe: &Path,
    check_codesign: bool,
) -> Result<Checks, UpdateError> {
    let exe_dir = canonical_exe
        .parent()
        .ok_or_else(|| UpdateError::Swap(std::io::Error::other("the binary has no folder")))?;
    remove_new_leftovers(exe_dir);
    let (archive, mut checks) = fetch_verified(release, asset, download_dir).await?;
    let new = extract_member(&archive, member, exe_dir)?;
    let mut check_and_swap = || -> Result<(), UpdateError> {
        keep_mode(&new, canonical_exe)?;
        if check_codesign {
            codesign_ok(spawner, &new)?;
            checks.codesign = true;
        }
        version_matches(spawner, &new, &release.version)?;
        checks.version = true;
        match_owner(&new, canonical_exe)?;
        swap(&new, canonical_exe)
    };
    if let Err(err) = check_and_swap() {
        let _ = std::fs::remove_file(&new);
        return Err(err);
    }
    Ok(checks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::spawn::Scripted;

    /// A gzip-compressed tar of `entries`: (path, body, kind).
    fn archive(dir: &Path, entries: &[(&str, &[u8], tar::EntryType)]) -> PathBuf {
        let path = dir.join("release.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(gz);
        for (name, body, kind) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_size(if kind.is_file() { body.len() as u64 } else { 0 });
            header.set_mode(0o644);
            if kind.is_symlink() {
                header.set_link_name("/bin/sh").unwrap();
            }
            // `append_data` refuses `..`, so the name goes in by hand, as a
            // hostile archive would carry it.
            let bytes = name.as_bytes();
            header.as_old_mut().name[..bytes.len()].copy_from_slice(bytes);
            header.set_cksum();
            let body: &[u8] = if kind.is_file() { body } else { &[] };
            builder.append(&header, body).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        path
    }

    const MEMBER: &str = "crystalline-v0.34.0-linux-amd64/crystalline";

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn extracts_only_the_named_member() {
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("src");
        let into = work.path().join("bin");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&into).unwrap();
        let regular = tar::EntryType::Regular;
        let archive = archive(
            &src,
            &[
                ("../evil", b"escaped", regular),
                (
                    "crystalline-v0.34.0-linux-amd64/",
                    b"",
                    tar::EntryType::Directory,
                ),
                (
                    "crystalline-v0.34.0-linux-amd64/README",
                    b"read me",
                    regular,
                ),
                (MEMBER, b"the new binary", regular),
                ("crystalline", b"a binary at the top", regular),
            ],
        );
        let new = extract_member(&archive, MEMBER, &into).unwrap();
        assert_eq!(
            new,
            into.join(format!("{NEW_PREFIX}{}", std::process::id()))
        );
        assert_eq!(std::fs::read(&new).unwrap(), b"the new binary");
        assert_eq!(
            files_in(&into),
            vec![format!("{NEW_PREFIX}{}", std::process::id())],
            "only the one file appears in the target folder"
        );
        assert_eq!(files_in(&src), vec!["release.tar.gz"]);
        assert_eq!(files_in(work.path()), vec!["bin", "src"], "nothing escaped");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&new).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o755,
                "set on the handle, whatever the umask: {mode:o}"
            );
        }
    }

    /// The environment variable that makes [`umask_stand_in`] run, naming
    /// its work folder.
    #[cfg(unix)]
    const UMASK_STAND_IN: &str = "CRYSTALLINE_TEST_UMASK_STAND_IN";

    /// Not a test: the child [`a_strict_umask_keeps_the_old_mode`] starts,
    /// so the umask of 077 is set in a process of its own and never in the
    /// test process, whose other threads create files too. It extracts the
    /// member next to `<work>/bin/crystalline`, keeps the old mode and swaps.
    #[cfg(unix)]
    #[test]
    #[ignore = "a stand-in process for a test, started by that test"]
    fn umask_stand_in() {
        let Some(work) = std::env::var_os(UMASK_STAND_IN) else {
            return;
        };
        let work = PathBuf::from(work);
        // SAFETY: umask takes a mode and cannot fail.
        unsafe { libc::umask(0o077) };
        let archive = archive(
            &work,
            &[(MEMBER, b"the new binary", tar::EntryType::Regular)],
        );
        let bin = work.join("bin");
        let exe = bin.join("crystalline");
        let new = extract_member(&archive, MEMBER, &bin).unwrap();
        keep_mode(&new, &exe).unwrap();
        swap(&new, &exe).unwrap();
    }

    /// Under a umask of 077 an old exe at 0755 ends at 0755: the mode is
    /// set on the open handle, so a strict umask (sudo uses the person's)
    /// never leaves an install nobody else can run.
    #[cfg(unix)]
    #[test]
    fn a_strict_umask_keeps_the_old_mode() {
        use std::os::unix::fs::PermissionsExt;
        let work = tempfile::tempdir().unwrap();
        let bin = work.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("crystalline");
        std::fs::write(&exe, b"the old binary").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "update::swap::tests::umask_stand_in",
                "--exact",
                "--ignored",
                "--test-threads=1",
            ])
            .env(UMASK_STAND_IN, work.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(std::fs::read(&exe).unwrap(), b"the new binary", "swapped");
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "{mode:o}");
    }

    /// Any old mode is kept, group and other bits included.
    #[cfg(unix)]
    #[test]
    fn the_old_mode_is_copied() {
        use std::os::unix::fs::PermissionsExt;
        let work = tempfile::tempdir().unwrap();
        let exe = work.path().join("crystalline");
        std::fs::write(&exe, b"old").unwrap();
        let new = work.path().join(format!("{NEW_PREFIX}1"));
        std::fs::write(&new, b"new").unwrap();
        for old in [0o750, 0o711, 0o775] {
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(old)).unwrap();
            keep_mode(&new, &exe).unwrap();
            let mode = std::fs::metadata(&new).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, old, "{mode:o}");
        }
        // An old exe that cannot be read leaves 0755.
        keep_mode(&new, &work.path().join("gone")).unwrap();
        let mode = std::fs::metadata(&new).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "{mode:o}");
    }

    #[test]
    fn a_missing_twice_or_linked_member_is_refused_and_leaves_nothing() {
        let work = tempfile::tempdir().unwrap();
        let regular = tar::EntryType::Regular;
        let cases: [&[(&str, &[u8], tar::EntryType)]; 3] = [
            &[("crystalline", b"not the member", regular)],
            &[(MEMBER, b"one", regular), (MEMBER, b"two", regular)],
            &[(MEMBER, b"", tar::EntryType::Symlink)],
        ];
        for (at, entries) in cases.iter().enumerate() {
            let src = work.path().join(format!("src{at}"));
            let into = work.path().join(format!("bin{at}"));
            std::fs::create_dir_all(&src).unwrap();
            std::fs::create_dir_all(&into).unwrap();
            let archive = archive(&src, entries);
            let err = extract_member(&archive, MEMBER, &into).unwrap_err();
            assert!(matches!(err, UpdateError::NoMember), "case {at}: {err}");
            assert!(
                files_in(&into).is_empty(),
                "case {at}: {:?}",
                files_in(&into)
            );
        }
    }

    #[test]
    fn an_existing_target_file_is_never_opened() {
        let work = tempfile::tempdir().unwrap();
        let archive = archive(
            work.path(),
            &[(MEMBER, b"the new binary", tar::EntryType::Regular)],
        );
        let into = work.path().join("bin");
        std::fs::create_dir_all(&into).unwrap();
        let planted = into.join(format!("{NEW_PREFIX}{}", std::process::id()));
        std::fs::write(&planted, b"planted").unwrap();
        assert!(extract_member(&archive, MEMBER, &into).is_err());
        assert_eq!(
            std::fs::read(&planted).unwrap(),
            b"planted",
            "a file this run did not create is kept"
        );
        remove_new_leftovers(&into);
        assert!(files_in(&into).is_empty());
        extract_member(&archive, MEMBER, &into).unwrap();
    }

    /// A read-only folder after the extract: the rename fails, and the old
    /// exe keeps its bytes.
    #[cfg(unix)]
    #[test]
    fn a_failed_rename_keeps_the_old_binary() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid takes no arguments and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            // Root writes into a read-only folder, so there is no failure
            // to see.
            return;
        }
        let work = tempfile::tempdir().unwrap();
        let archive = archive(
            work.path(),
            &[(MEMBER, b"the new binary", tar::EntryType::Regular)],
        );
        let bin = work.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("crystalline");
        std::fs::write(&exe, b"the old binary").unwrap();
        let new = extract_member(&archive, MEMBER, &bin).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o555)).unwrap();
        let err = swap(&new, &exe).unwrap_err();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(err, UpdateError::Swap(_)), "{err}");
        assert!(
            err.to_string().starts_with("cannot replace the binary: "),
            "{err}"
        );
        assert_eq!(std::fs::read(&exe).unwrap(), b"the old binary");
    }

    #[test]
    fn a_rename_puts_the_new_binary_in_place() {
        let work = tempfile::tempdir().unwrap();
        let exe = work.path().join("crystalline");
        std::fs::write(&exe, b"the old binary").unwrap();
        let new = work.path().join(format!("{NEW_PREFIX}1"));
        std::fs::write(&new, b"the new binary").unwrap();
        swap(&new, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"the new binary");
        assert!(!new.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_folder_is_not_writable() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid takes no arguments and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let work = tempfile::tempdir().unwrap();
        let exe = work.path().join("crystalline");
        std::fs::write(&exe, b"bin").unwrap();
        assert!(writable_install_dir(&exe));
        assert!(!root_owned_install_dir(&exe));
        std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let writable = writable_install_dir(&exe);
        std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!writable);
        assert!(root_owned_install_dir(Path::new("/usr/bin/true")));
    }

    fn v(text: &str) -> Precedence {
        Precedence::parse(text).unwrap()
    }

    #[test]
    fn version_mismatch_refuses_the_swap() {
        let bin = Path::new("/install/.crystalline-new-7");
        for printed in [
            "crystalline 0.33.0\nCopyright\n",
            "crystalline 0.34.0-rc.1\n",
            "crystalline 0.34.00\n",
            "Crystalline 0.34.0\n",
            "",
        ] {
            let spawner = Scripted::new(|_, _| Ok(printed.to_string()));
            let err = version_matches(&spawner, bin, &v("0.34.0")).unwrap_err();
            assert!(matches!(err, UpdateError::VersionMismatch), "{printed:?}");
            assert_eq!(
                spawner.calls(),
                vec!["/install/.crystalline-new-7 --version"],
                "{printed:?}"
            );
        }
        let failing = Scripted::new(|_, _| Err(std::io::Error::other("exit 1")));
        assert!(version_matches(&failing, bin, &v("0.34.0")).is_err());
        let good =
            Scripted::new(|_, _| Ok("crystalline 0.34.0\nCopyright (c) Jordi\n".to_string()));
        version_matches(&good, bin, &v("0.34.0")).unwrap();
    }

    #[test]
    fn the_sudo_rule_needs_root_and_a_person() {
        assert!(under_sudo(0, Some("501")));
        assert!(!under_sudo(0, None));
        assert!(!under_sudo(0, Some("0")));
        assert!(!under_sudo(0, Some("")));
        assert!(!under_sudo(0, Some("someone")));
        assert!(!under_sudo(501, Some("501")));
    }

    #[test]
    fn only_own_or_dead_runs_leftovers_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let own = format!("{NEW_PREFIX}{}", std::process::id());
        for name in [
            own.as_str(),
            ".crystalline-new-111",
            ".crystalline-new-222",
            ".crystalline-new-",
            ".crystalline-new-12x",
            "crystalline",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        // 222 is a run still going (a sudo run with a lock of its own).
        remove_new_leftovers_with(dir.path(), |pid| pid == 222);
        assert_eq!(
            files_in(dir.path()),
            vec![
                ".crystalline-new-",
                ".crystalline-new-12x",
                ".crystalline-new-222",
                "crystalline"
            ]
        );
    }

    /// A live process's file survives the real liveness check: this test
    /// process's parent is alive.
    #[cfg(unix)]
    #[test]
    fn a_live_runs_file_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let parent = std::os::unix::process::parent_id();
        let name = format!("{NEW_PREFIX}{parent}");
        std::fs::write(dir.path().join(&name), b"x").unwrap();
        remove_new_leftovers(dir.path());
        assert_eq!(files_in(dir.path()), vec![name]);
    }

    /// A new binary that never answers `--version` is killed at the limit
    /// and fails the check, so nothing is swapped.
    #[cfg(unix)]
    #[test]
    fn a_hanging_new_binary_fails_the_version_check_in_time() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join(format!("{NEW_PREFIX}1"));
        std::fs::write(&bin, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let began = std::time::Instant::now();
        let err = version_matches_within(
            &crate::update::SystemSpawner,
            &bin,
            &v("0.34.0"),
            Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(matches!(err, UpdateError::VersionMismatch), "{err}");
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(VERSION_TIME_LIMIT, Duration::from_secs(30));
    }
}
