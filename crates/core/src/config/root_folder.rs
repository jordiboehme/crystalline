//! Creating the domains root before the first domain lands in it.
//!
//! A domain created without an explicit folder lands at
//! `<domains_root>/<name>`. [`prepare_domains_root`] runs right before that
//! folder is created and makes sure its parent, the root, is a folder:
//!
//! - an existing folder (a symlink to one is fine) is used as it is, and its
//!   mode is never changed;
//! - something else at that path (a file, a broken link) refuses the add with
//!   [`RootProblem::NotAFolder`];
//! - a missing root is created. The default root `~/.crystalline` is created
//!   owner only (`0700`) on Unix, so other accounts on the machine cannot read
//!   a domain, as they could not under the old default inside `~/Documents`,
//!   and hidden on Windows, where a dot alone does not hide a folder in the
//!   Explorer. Only the root itself, never its parent (the home). A configured
//!   root is created with the umask, as before.
//!
//! The domain folders inside keep the umask ([`crate::owner_only`]).

use std::path::{Path, PathBuf};

use super::{DEFAULT_DOMAINS_ROOT, expand_tilde};

/// Why the domains root cannot take a new domain.
#[derive(Debug, thiserror::Error)]
pub enum RootProblem {
    /// Something that is not a folder sits at the root's path.
    #[error(
        "The domains root {} is a file, not a folder. Move it away, or choose another place with `crystalline config set domains_root <folder>`.",
        .0.display()
    )]
    NotAFolder(PathBuf),
    /// Creating the root failed.
    #[error("cannot create the domains root {}: {source}{}", .path.display(), crate::config::io_hint_suffix(&.path.to_string_lossy(), .source))]
    Io {
        /// The root.
        path: PathBuf,
        /// The failure.
        source: std::io::Error,
    },
}

/// What [`prepare_domains_root`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Prepared {
    /// The root was missing and is created now.
    pub created: bool,
    /// A best effort step that failed (the hidden attribute on Windows), for
    /// the caller to log. The root is usable all the same.
    pub warning: Option<String>,
}

/// Make sure `root` is a folder a new domain can be created in, creating it
/// when missing. The default root is created owner only and hidden (see the
/// module doc); it is the default when it equals `~/.crystalline` expanded,
/// whatever set it.
pub fn prepare_domains_root(root: &Path) -> Result<Prepared, RootProblem> {
    prepare_root(root, &expand_tilde(DEFAULT_DOMAINS_ROOT))
}

/// [`prepare_domains_root`] against a named default root, so a test can use
/// a temporary home.
pub fn prepare_root(root: &Path, default_root: &Path) -> Result<Prepared, RootProblem> {
    match std::fs::metadata(root) {
        Ok(meta) if meta.is_dir() => return Ok(Prepared::default()),
        Ok(_) => return Err(RootProblem::NotAFolder(root.to_path_buf())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // A dangling link is not missing: creating through it would
            // build a folder somewhere the person never named.
            if std::fs::symlink_metadata(root).is_ok() {
                return Err(RootProblem::NotAFolder(root.to_path_buf()));
            }
        }
        Err(source) => {
            return Err(RootProblem::Io {
                path: root.to_path_buf(),
                source,
            });
        }
    }
    let io = |source| RootProblem::Io {
        path: root.to_path_buf(),
        source,
    };
    if !is_default(root, default_root) {
        std::fs::create_dir_all(root).map_err(io)?;
        return Ok(Prepared {
            created: true,
            warning: None,
        });
    }
    if let Some(parent) = root.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    match create_private(root) {
        Ok(()) => {}
        // Another process created it in between: use it as it is.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && root.is_dir() => {
            return Ok(Prepared::default());
        }
        Err(e) => return Err(io(e)),
    }
    Ok(Prepared {
        created: true,
        warning: hide(root).err().map(|e| {
            format!(
                "cannot mark the domains root {} hidden: {e}",
                root.display()
            )
        }),
    })
}

/// Whether `root` is the default root, compared by text with the case rule
/// of this platform.
fn is_default(root: &Path, default_root: &Path) -> bool {
    let fold = cfg!(any(windows, target_os = "macos"));
    crate::path::lies_within(root, default_root, fold)
        && crate::path::lies_within(default_root, root, fold)
}

#[cfg(unix)]
fn create_private(root: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(root)
}

#[cfg(not(unix))]
fn create_private(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir(root)
}

#[cfg(windows)]
fn hide(root: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SetFileAttributesW,
    };
    let wide: Vec<u16> = root
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives both calls.
    unsafe {
        let attrs = GetFileAttributesW(wide.as_ptr());
        if attrs == INVALID_FILE_ATTRIBUTES {
            return Err(std::io::Error::last_os_error());
        }
        if SetFileAttributesW(wide.as_ptr(), attrs | FILE_ATTRIBUTE_HIDDEN) == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn hide(_root: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Whether the folder at `path` carries the hidden attribute (Windows).
#[cfg(windows)]
pub fn is_hidden(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;
    std::fs::metadata(path).is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let default_root = tmp.path().join("home").join(".crystalline");
        std::fs::create_dir_all(default_root.parent().unwrap()).unwrap();
        (tmp, default_root)
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_missing_default_root_is_created_owner_only_and_hidden() {
        let (_tmp, root) = setup();
        let done = prepare_root(&root, &root).unwrap();
        assert!(done.created);
        assert!(done.warning.is_none(), "{done:?}");
        assert!(root.is_dir());
        #[cfg(unix)]
        assert_eq!(mode(&root), 0o700);
        #[cfg(windows)]
        assert!(is_hidden(&root));
    }

    #[test]
    fn a_configured_root_spelled_as_the_default_is_the_default() {
        let (_tmp, root) = setup();
        let spelled = root.parent().unwrap().join(".").join(".crystalline");
        prepare_root(&spelled, &root).unwrap();
        #[cfg(unix)]
        assert_eq!(mode(&root), 0o700);
        #[cfg(windows)]
        assert!(is_hidden(&root));
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_root_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, root) = setup();
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("foreign.txt"), "x").unwrap();
        let done = prepare_root(&root, &root).unwrap();
        assert!(!done.created);
        assert_eq!(mode(&root), 0o755);
    }

    #[test]
    fn a_file_at_the_root_is_refused_with_the_sentence() {
        let (_tmp, root) = setup();
        std::fs::write(&root, "x").unwrap();
        let err = prepare_root(&root, &root).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "The domains root {} is a file, not a folder. Move it away, or choose another place with `crystalline config set domains_root <folder>`.",
                root.display()
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_at_the_root_is_refused() {
        let (tmp, root) = setup();
        std::os::unix::fs::symlink(tmp.path().join("nowhere"), &root).unwrap();
        assert!(matches!(
            prepare_root(&root, &root),
            Err(RootProblem::NotAFolder(_))
        ));
        assert!(!tmp.path().join("nowhere").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_configured_root_is_created_with_the_umask() {
        let (tmp, default_root) = setup();
        let root = tmp.path().join("kb").join("roots");
        let done = prepare_root(&root, &default_root).unwrap();
        assert!(done.created);
        assert!(root.is_dir());
        let probe = tmp.path().join("probe");
        std::fs::create_dir(&probe).unwrap();
        assert_eq!(mode(&root), mode(&probe), "the umask applies");
        assert!(!default_root.exists());
    }
}
