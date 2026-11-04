//! What `crystalline install claude-desktop` did last, in
//! `<state_dir>/claude-desktop.json`.
//!
//! Crystalline cannot see what Claude Desktop has installed, so this record
//! is all it knows: the extension it built (version, pinned binary, launcher
//! hash, file, time, whether it was opened) and the skill zip it saved.
//! `doctor` and `uninstall` read it back.
//!
//! It is a file of its own, not a row in the install receipt
//! (`installs.json`), because an older binary rewriting the receipt would
//! drop fields it does not know. For the same reason unknown fields load
//! here; only an unknown `format` makes the file unreadable, and a caller
//! treats that as no record.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crystalline_core::config;

/// The record format this binary writes.
pub(crate) const FORMAT: u32 = 1;

/// The whole record file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DesktopRecord {
    /// Always [`FORMAT`] when written by this binary.
    pub format: u32,
    /// The extension built last, if any.
    #[serde(default)]
    pub extension: Option<ExtensionRecord>,
    /// The skill zip saved last, if any.
    #[serde(default)]
    pub skill: Option<SkillRecord>,
}

impl Default for DesktopRecord {
    fn default() -> Self {
        DesktopRecord {
            format: FORMAT,
            extension: None,
            skill: None,
        }
    }
}

/// The extension a run built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ExtensionRecord {
    /// The Crystalline version the extension was built for.
    pub version: String,
    /// The binary path pinned in its manifest.
    pub binary: String,
    /// The sha256 of the launcher it carries.
    pub shim_sha256: String,
    /// Where the `.mcpb` was written.
    pub file: String,
    /// When it was built, RFC 3339 in UTC.
    pub built_at: String,
    /// Whether it was handed to Claude Desktop.
    pub opened: bool,
}

/// The skill zip a run saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SkillRecord {
    /// The skill's name.
    pub name: String,
    /// The sha256 of its `SKILL.md` text.
    pub sha256: String,
    /// Where the zip was saved.
    pub file: String,
    /// When it was saved, RFC 3339 in UTC.
    pub saved_at: String,
}

/// The record's location, `<state_dir>/claude-desktop.json`.
pub(crate) fn record_path() -> anyhow::Result<PathBuf> {
    Ok(config::state_dir()?.join("claude-desktop.json"))
}

/// Where the built extension goes, `<state_dir>/claude-desktop/crystalline.mcpb`.
pub(crate) fn bundle_path() -> anyhow::Result<PathBuf> {
    Ok(config::state_dir()?
        .join("claude-desktop")
        .join("crystalline.mcpb"))
}

/// Load the record. A missing file is `None`; an unreadable file, bad JSON
/// or an unknown format is an error the caller treats as no record.
pub(crate) fn load(path: &Path) -> anyhow::Result<Option<DesktopRecord>> {
    let bytes = match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => anyhow::bail!("could not read {}: {e}", path.display()),
        Ok(bytes) => bytes,
    };
    let record: DesktopRecord = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("{} is not a valid record: {e}", path.display()))?;
    if record.format != FORMAT {
        anyhow::bail!(
            "{} carries unknown record format {}",
            path.display(),
            record.format
        );
    }
    Ok(Some(record))
}

/// Write the record to `<path>.partial`, then rename it over `path`, so a
/// reader never sees half a file. Pretty JSON with a trailing newline.
pub(crate) fn save(path: &Path, record: &DesktopRecord) -> anyhow::Result<()> {
    let mut text = serde_json::to_string_pretty(record)?;
    text.push('\n');
    write_replacing(path, text.as_bytes())
}

/// Write `bytes` to `<path>.partial` and rename it over `path`, creating the
/// parent folder first.
pub(crate) fn write_replacing(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("could not create {}: {e}", parent.display()))?;
    }
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    std::fs::write(&partial, bytes)
        .map_err(|e| anyhow::anyhow!("could not write {}: {e}", partial.display()))?;
    if let Err(e) = std::fs::rename(&partial, path) {
        let _ = std::fs::remove_file(&partial);
        anyhow::bail!("could not write {}: {e}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DesktopRecord {
        DesktopRecord {
            format: FORMAT,
            extension: Some(ExtensionRecord {
                version: "0.32.0".to_string(),
                binary: "/opt/homebrew/bin/crystalline".to_string(),
                shim_sha256: "ab".repeat(32),
                file: "/state/claude-desktop/crystalline.mcpb".to_string(),
                built_at: "2026-11-04T10:00:00Z".to_string(),
                opened: false,
            }),
            skill: Some(SkillRecord {
                name: "crystalline-intelligence".to_string(),
                sha256: "cd".repeat(32),
                file: "/home/Downloads/crystalline-intelligence.zip".to_string(),
                saved_at: "2026-11-04T10:00:01Z".to_string(),
            }),
        }
    }

    #[test]
    fn desktop_record_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("claude-desktop.json");
        save(&path, &sample()).unwrap();
        assert_eq!(load(&path).unwrap(), Some(sample()));
        let partial = dir
            .path()
            .join("nested")
            .join("claude-desktop.json.partial");
        assert!(!partial.exists(), "no partial file is left");
    }

    #[test]
    fn a_missing_desktop_record_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("claude-desktop.json")).unwrap(), None);
    }

    #[test]
    fn a_desktop_record_of_an_unknown_format_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude-desktop.json");
        std::fs::write(&path, r#"{"format": 2}"#).unwrap();
        assert!(load(&path).is_err());
        std::fs::write(&path, "not json").unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn a_desktop_record_with_an_unknown_field_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude-desktop.json");
        std::fs::write(&path, r#"{"format": 1, "later": {"x": true}}"#).unwrap();
        assert_eq!(load(&path).unwrap(), Some(DesktopRecord::default()));
    }
}
