//! Reading, backing up and saving the harness files install edits.
//!
//! The text itself is edited by `crystalline_core::jsonc_edit`; this
//! module does the IO around it: read the file, keep one copy of a file the
//! person had before the first change, and write the result atomically. A
//! harness file is the person's, so the write keeps what it is on disk: a
//! symbolic link (a dotfiles setup) is written through, never replaced by a
//! plain file, and the file keeps its permissions.

use std::path::{Path, PathBuf};

use crystalline_core::HarnessKind;

use crate::receipt;

/// The text of a harness file, or `None` when it does not exist.
pub(crate) fn read_text(path: &Path) -> anyhow::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
    }
}

/// Keep `bytes`, the text an edit of `path` started from, as
/// `<state_dir>/backups/<harness id>/<path key>/<file name>.<UTC stamp>`
/// unless that folder already holds a backup (spec decision 12). The path
/// key is a short hash of the full path, so two files with one name in two
/// folders each get their own backup. The copy is kept out of the harness
/// folder on purpose, because some harnesses load every file in their
/// folders. Returns the new backup, or `None` when this path was backed up
/// before.
pub(crate) fn backup_once(
    state_dir: &Path,
    harness: HarnessKind,
    path: &Path,
    bytes: &[u8],
) -> anyhow::Result<Option<PathBuf>> {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Ok(None);
    };
    let dir = state_dir
        .join("backups")
        .join(harness.id())
        .join(path_key(path));
    if has_backup(&dir, &name)? {
        return Ok(None);
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let backup = dir.join(format!("{name}.{stamp}"));
    write_private(&backup, bytes)?;
    Ok(Some(backup))
}

/// Write a backup that only the user can read: the file is 0600 and every
/// folder created on the way is 0700 on unix, whatever the mode of the file
/// it copies, because an MCP file can hold tokens.
fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let io = |e: std::io::Error| anyhow::anyhow!("could not write {}: {e}", path.display());
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(dir).map_err(io)?;
    }
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        Some(std::fs::Permissions::from_mode(0o600))
    };
    #[cfg(not(unix))]
    let permissions = None;
    replace_atomically(path, bytes, permissions).map_err(io)
}

/// The first twelve hex digits of the sha256 of the full path.
fn path_key(path: &Path) -> String {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut key = receipt::sha256_hex(full.as_os_str().as_encoded_bytes());
    key.truncate(12);
    key
}

/// Whether `dir` already holds a file named `<name>.<stamp>`.
fn has_backup(dir: &Path, name: &str) -> anyhow::Result<bool> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(anyhow::anyhow!("could not read {}: {e}", dir.display())),
    };
    let prefix = format!("{name}.");
    for entry in entries {
        let entry = entry.map_err(|e| anyhow::anyhow!("could not read {}: {e}", dir.display()))?;
        let file_name = entry.file_name();
        if file_name
            .to_string_lossy()
            .strip_prefix(&prefix)
            .is_some_and(is_stamp)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `20261002T101500Z`: the shape `backup_once` writes.
fn is_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 16
        && b[8] == b'T'
        && b[15] == b'Z'
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[9..15].iter().all(u8::is_ascii_digit)
}

/// What `save_edited` did.
#[derive(Debug)]
pub(crate) struct Written {
    /// The file was written.
    pub changed: bool,
    /// A backup taken before this write, to name in the report.
    pub backup: Option<PathBuf>,
}

/// Write `after` to `path` atomically unless it equals `before`. When the
/// file existed (`before` is `Some`) and a state dir is given, `before` is
/// backed up once first. Install cannot tell a file it created in an earlier
/// run from the person's own, so the first change of any existing file is
/// backed up; that is the harmless direction.
pub(crate) fn save_edited(
    state_dir: Option<&Path>,
    harness: HarnessKind,
    path: &Path,
    before: Option<&str>,
    after: &str,
) -> anyhow::Result<Written> {
    if before == Some(after) {
        return Ok(Written {
            changed: false,
            backup: None,
        });
    }
    let backup = match (state_dir, before) {
        (Some(state_dir), Some(before)) => {
            backup_once(state_dir, harness, path, before.as_bytes())?
        }
        _ => None,
    };
    write_in_place(path, after.as_bytes())?;
    Ok(Written {
        changed: true,
        backup,
    })
}

/// Write `bytes` to the file `path` names, atomically: a temporary file
/// beside the real file, given the real file's permissions, then renamed
/// over it. A link is followed to its target, so the link itself survives.
fn write_in_place(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let target = link_target(path);
    let io = |e: std::io::Error| anyhow::anyhow!("could not write {}: {e}", target.display());
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let permissions = std::fs::metadata(&target).ok().map(|m| m.permissions());
    replace_atomically(&target, bytes, permissions).map_err(io)
}

/// Write `bytes` to a temporary file beside `target` with `permissions`
/// (when given), then rename it over `target`.
fn replace_atomically(
    target: &Path,
    bytes: &[u8],
    permissions: Option<std::fs::Permissions>,
) -> std::io::Result<()> {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = target.with_file_name(format!(".{name}.crystalline-{}.tmp", std::process::id()));
    let result = write_new(&tmp, bytes, permissions).and_then(|()| std::fs::rename(&tmp, target));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Create `tmp` with `bytes`, private while it is written when it replaces
/// a file, then with the permissions of that file.
fn write_new(
    tmp: &Path,
    bytes: &[u8],
    permissions: Option<std::fs::Permissions>,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if permissions.is_some() {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if let Some(permissions) = permissions {
        std::fs::set_permissions(tmp, permissions)?;
    }
    Ok(())
}

/// The file a path names once every symbolic link on its last component is
/// followed (a relative link resolves against the link's folder). A path
/// that is not a link comes back as it is.
fn link_target(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    // A bound, so a link loop ends instead of spinning.
    for _ in 0..40 {
        let is_link = std::fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink());
        if !is_link {
            break;
        }
        let Ok(next) = std::fs::read_link(&current) else {
            break;
        };
        current = if next.is_absolute() {
            next
        } else {
            match current.parent() {
                Some(dir) => dir.join(&next),
                None => next,
            }
        };
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decision 12: the backup lands once, under the state dir, never in the
    /// harness folder a harness scans.
    #[test]
    fn a_backup_is_taken_once_per_path_outside_the_harness_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let file = tmp.path().join("home/.kiro/settings/mcp.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"{ \"mcpServers\": {} }").unwrap();
        let first = backup_once(&state, HarnessKind::Kiro, &file, b"{ \"mcpServers\": {} }")
            .unwrap()
            .expect("taken");
        assert!(
            first.starts_with(state.join("backups/kiro")),
            "{}",
            first.display()
        );
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("mcp.json.")
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"{ \"mcpServers\": {} }");
        assert_eq!(
            backup_once(&state, HarnessKind::Kiro, &file, b"{}").unwrap(),
            None,
            "once per path"
        );
        assert_eq!(
            std::fs::read_dir(file.parent().unwrap()).unwrap().count(),
            1,
            "nothing beside the file"
        );
    }

    #[test]
    fn save_edited_writes_atomically_and_backs_up_only_a_file_we_did_not_create() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let file = tmp.path().join("h/.cursor/mcp.json");
        let w = save_edited(Some(&state), HarnessKind::Cursor, &file, None, "{}\n").unwrap();
        assert!(
            w.changed && w.backup.is_none(),
            "created, nothing to back up"
        );
        let w = save_edited(
            Some(&state),
            HarnessKind::Cursor,
            &file,
            Some("{}\n"),
            "{\"a\": 1}\n",
        )
        .unwrap();
        assert!(
            w.backup.is_some(),
            "the first change to an existing file is backed up"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"a\": 1}\n");
    }

    /// A dotfiles setup links the harness file into a repository: the edit
    /// goes into the linked file and the link stays a link.
    #[cfg(unix)]
    #[test]
    fn a_linked_file_is_written_through_the_link() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("dotfiles/cursor/mcp.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "{}\n").unwrap();
        let link = tmp.path().join("home/.cursor/mcp.json");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../../dotfiles/cursor/mcp.json", &link).unwrap();
        let w = save_edited(
            None,
            HarnessKind::Cursor,
            &link,
            Some("{}\n"),
            "{\"a\": 1}\n",
        )
        .unwrap();
        assert!(w.changed);
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link is kept"
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"a\": 1}\n");
    }

    /// A file that can hold tokens is often private: the edit keeps its
    /// permissions.
    #[cfg(unix)]
    #[test]
    fn the_file_mode_is_kept() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".cursor/mcp.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "{}\n").unwrap();
        // Not 0600, which the temporary file starts with: only a copied mode
        // can make this pass.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        save_edited(
            None,
            HarnessKind::Cursor,
            &file,
            Some("{}\n"),
            "{\"a\": 1}\n",
        )
        .unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"a\": 1}\n");
    }

    /// An MCP file can hold tokens: its backup is private whatever the
    /// file's own mode, in folders only the user can enter.
    #[cfg(unix)]
    #[test]
    fn a_backup_is_private_and_so_are_its_folders() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let file = tmp.path().join("h/.cursor/mcp.json");
        let body = "{\"mcpServers\": {\"other\": {\"env\": {\"TOKEN\": \"x\"}}}}\n";
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, body).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let w = save_edited(Some(&state), HarnessKind::Cursor, &file, Some(body), "{}\n").unwrap();
        let backup = w.backup.expect("backed up");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&backup), 0o600, "the backup");
        for dir in [
            backup.parent().unwrap().to_path_buf(),
            state.join("backups/cursor"),
            state.join("backups"),
        ] {
            assert_eq!(mode(&dir), 0o700, "{}", dir.display());
        }
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), body);
    }

    /// Two files with one name in two folders each get their own backup.
    #[test]
    fn a_backup_is_taken_once_per_full_path_not_per_file_name() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let first = tmp.path().join("home/.gemini/settings.json");
        let second = tmp.path().join("repo/.gemini/settings.json");
        for (file, body) in [(&first, "{\"a\": 1}"), (&second, "{\"b\": 2}")] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, body).unwrap();
        }
        let a = backup_once(&state, HarnessKind::Gemini, &first, b"{\"a\": 1}")
            .unwrap()
            .expect("first path backed up");
        let b = backup_once(&state, HarnessKind::Gemini, &second, b"{\"b\": 2}")
            .unwrap()
            .expect("second path backed up too");
        assert_ne!(a, b);
        assert_eq!(std::fs::read(&a).unwrap(), b"{\"a\": 1}");
        assert_eq!(std::fs::read(&b).unwrap(), b"{\"b\": 2}");
        assert_eq!(
            backup_once(&state, HarnessKind::Gemini, &second, b"{\"b\": 3}").unwrap(),
            None
        );
    }

    /// The backup holds the text the edit was planned from, not whatever is
    /// on disk by the time it is written.
    #[test]
    fn the_backup_holds_the_bytes_the_edit_started_from() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let file = tmp.path().join("h/.cursor/hooks.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "{\"changed\": true}\n").unwrap();
        let w = save_edited(
            Some(&state),
            HarnessKind::Cursor,
            &file,
            Some("{}\n"),
            "{\"a\": 1}\n",
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(w.backup.unwrap()).unwrap(), "{}\n");
    }

    #[test]
    fn an_unchanged_file_is_not_written_and_a_missing_one_reads_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("settings.json");
        assert_eq!(read_text(&file).unwrap(), None);
        std::fs::write(&file, "{}\n").unwrap();
        assert_eq!(read_text(&file).unwrap().as_deref(), Some("{}\n"));
        let w = save_edited(None, HarnessKind::Gemini, &file, Some("{}\n"), "{}\n").unwrap();
        assert!(!w.changed && w.backup.is_none());
    }
}
