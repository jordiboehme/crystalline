#![allow(dead_code)] // removed by Task 8, which wires the callers
//! Reading, backing up and saving the harness files install edits.
//!
//! The text itself is edited by `crystalline_core::jsonc_edit`; this
//! module does the IO around it: read the file, keep
//! one copy of a file the person had before the first change, and write the
//! result atomically.

use std::path::{Path, PathBuf};

use crystalline_core::HarnessKind;
use crystalline_core::config;

/// The text of a harness file, or `None` when it does not exist.
pub(crate) fn read_text(path: &Path) -> anyhow::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
    }
}

/// Copy `path` to `<state_dir>/backups/<harness id>/<file name>.<UTC stamp>`
/// unless a backup of that file name is already there (spec decision 12).
/// The copy is kept out of the harness folder on purpose, because some
/// harnesses load every file in their folders. Returns the new backup, or
/// `None` when the file does not exist or was backed up before.
pub(crate) fn backup_once(
    state_dir: &Path,
    harness: HarnessKind,
    path: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Ok(None);
    };
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
    };
    let dir = state_dir.join("backups").join(harness.id());
    if has_backup(&dir, &name)? {
        return Ok(None);
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let backup = dir.join(format!("{name}.{stamp}"));
    config::save_bytes(&backup, &bytes)?;
    Ok(Some(backup))
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
/// file existed (`before` is `Some`) and a state dir is given, the file is
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
        (Some(state_dir), Some(_)) => backup_once(state_dir, harness, path)?,
        _ => None,
    };
    config::save_bytes(path, after.as_bytes())?;
    Ok(Written {
        changed: true,
        backup,
    })
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
        let first = backup_once(&state, HarnessKind::Kiro, &file)
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
            backup_once(&state, HarnessKind::Kiro, &file).unwrap(),
            None,
            "once per path"
        );
        assert_eq!(
            std::fs::read_dir(file.parent().unwrap()).unwrap().count(),
            1,
            "nothing beside the file"
        );
        let missing = tmp.path().join("home/.kiro/nope.json");
        assert_eq!(
            backup_once(&state, HarnessKind::Kiro, &missing).unwrap(),
            None,
            "nothing to keep"
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
