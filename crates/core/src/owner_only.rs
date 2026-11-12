//! Owner-only permissions for this machine's private state.
//!
//! The state folder holds the search index (engram text, private domains
//! included), the accounts database (password and token hashes) and the
//! daemon log (which can carry a setup token). On Unix none of it is anyone
//! else's business, so the folder is kept at `0700` and those files at `0600`.
//! A folder at `0700` is the real barrier: another account cannot reach a file
//! inside it whatever the file's own mode says. The file modes are the second
//! layer, for the day the folder is copied or its mode is widened by hand.
//!
//! Nothing here sets the process umask. The daemon also writes engrams into
//! domain folders, often a bind mount or a git checkout other people read, and
//! those files must keep the modes their owner expects.
//!
//! Windows is left alone: its permissions are ACLs on the user profile, and
//! every function here is a no-op there.

use std::fs::File;
use std::io;
use std::path::Path;

/// The files directly in the state folder that are tightened to `0600` when
/// they exist: both databases with their write-ahead log and shared-memory
/// sidecars, the daemon log and the daemon record.
pub const PRIVATE_STATE_FILES: &[&str] = &[
    "index.db",
    "index.db-wal",
    "index.db-shm",
    "web-auth.db",
    "web-auth.db-wal",
    "web-auth.db-shm",
    "daemon.log",
    "service.json",
];

/// Create `dir` as an owner-only folder, or tighten it to one when it exists
/// with group or other bits set. Its parents are created with the usual mode.
///
/// An existing folder is changed only when it has a bit to lose, so a folder
/// that is already `0700` (or narrower) costs one `stat`.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if let Some(parent) = dir.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let mode = std::fs::metadata(dir)?.permissions().mode();
                if mode & 0o077 != 0 {
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Tighten an existing file to `0600` when it has group or other bits set.
/// A file that does not exist is not an error, and nothing is created.
pub fn tighten_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = match std::fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        // A link is followed by `set_permissions`, so it would change a file
        // somewhere else; only a regular file of our own is touched.
        if !meta.is_file() {
            return Ok(());
        }
        if meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Tighten a database file and its `-wal` and `-shm` sidecars to `0600`,
/// whichever of them exist. Called right after a database is opened, which is
/// when a new one has just been created at the umask's mode.
pub fn tighten_database(path: &Path) -> io::Result<()> {
    tighten_file(path)?;
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        tighten_file(Path::new(&sidecar))?;
    }
    Ok(())
}

/// Open `path` for appending, creating it owner-only when it is new and
/// tightening it when it already existed with a wider mode.
pub fn open_private_append(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    tighten_file(path)?;
    Ok(file)
}

/// Make `dir` an owner-only state folder and tighten every file in
/// [`PRIVATE_STATE_FILES`] that it already holds.
///
/// Best effort by design: it returns one line per thing it could not do, for
/// the caller to log, and never fails. A permission is a hardening step, and a
/// folder this process cannot change (one somebody else owns, a read-only
/// mount) must not keep the daemon from starting.
pub fn secure_state_dir(dir: &Path) -> Vec<String> {
    // Nothing to do on Windows, and nothing created there ahead of time either.
    if cfg!(not(unix)) {
        return Vec::new();
    }
    let mut problems = Vec::new();
    if let Err(e) = ensure_private_dir(dir) {
        problems.push(format!(
            "could not make the state folder {} owner-only: {e}",
            dir.display()
        ));
    }
    for name in PRIVATE_STATE_FILES {
        let path = dir.join(name);
        if let Err(e) = tighten_file(&path) {
            problems.push(format!("could not make {} owner-only: {e}", path.display()));
        }
    }
    problems
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_new_state_folder_is_created_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("state").join("crystalline");
        ensure_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn an_existing_wide_folder_and_its_files_are_tightened() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("crystalline");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        for name in ["index.db", "web-auth.db-wal", "daemon.log", "notes.txt"] {
            let file = dir.join(name);
            std::fs::write(&file, b"x").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        assert!(secure_state_dir(&dir).is_empty());

        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("index.db")), 0o600);
        assert_eq!(mode(&dir.join("web-auth.db-wal")), 0o600);
        assert_eq!(mode(&dir.join("daemon.log")), 0o600);
        assert_eq!(
            mode(&dir.join("notes.txt")),
            0o644,
            "a file that is not on the list keeps its mode"
        );
    }

    #[test]
    fn a_database_is_tightened_with_its_sidecars() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("web-auth.db");
        for name in ["web-auth.db", "web-auth.db-wal", "web-auth.db-shm"] {
            let file = tmp.path().join(name);
            std::fs::write(&file, b"x").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        tighten_database(&db).unwrap();
        for name in ["web-auth.db", "web-auth.db-wal", "web-auth.db-shm"] {
            assert_eq!(mode(&tmp.path().join(name)), 0o600, "{name}");
        }
    }

    #[test]
    fn a_narrower_mode_is_left_as_it_is() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("index.db");
        std::fs::write(&file, b"x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400)).unwrap();
        tighten_file(&file).unwrap();
        assert_eq!(mode(&file), 0o400);
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        tighten_file(&tmp.path().join("absent.db")).unwrap();
        assert!(!tmp.path().join("absent.db").exists());
    }

    #[test]
    fn the_log_is_created_owner_only_and_an_old_one_is_tightened() {
        let tmp = tempfile::tempdir().unwrap();
        let fresh = tmp.path().join("daemon.log");
        drop(open_private_append(&fresh).unwrap());
        assert_eq!(mode(&fresh), 0o600);

        let old = tmp.path().join("old.log");
        std::fs::write(&old, b"x").unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o644)).unwrap();
        drop(open_private_append(&old).unwrap());
        assert_eq!(mode(&old), 0o600);
    }
}
