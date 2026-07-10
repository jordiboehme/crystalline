//! The provisioning receipt: a reconcile engine's (M5) memory of what it
//! last wrote into each harness's config directory, and the per-source
//! stamp it uses to tell "unchanged since the last scan" from "needs
//! rehashing" without reading every file on every run.
//!
//! One JSON file at `<state_dir>/provisions.json` holds every domain's
//! source stamps and every harness's installed state. The receipt is
//! disposable derived state in the same spirit as the search index: a
//! missing file means nothing has been provisioned yet, and a corrupt or
//! unknown-format file is an error the caller decides how to survive - a
//! reconcile run regenerates from empty, a read-only inspection skips
//! quietly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config;
use crate::provision::model::is_plain_component;

/// The receipt format this crate writes. Bumped only on an incompatible
/// shape change; a reader errors on an unknown format rather than guessing
/// at its meaning.
const FORMAT: u32 = 1;

/// The whole receipt file: every domain's source stamps and every harness's
/// installed state, as of the last reconcile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisionReceipt {
    /// Format marker, [`FORMAT`] today.
    pub format: u32,
    /// Domain name to its source stamps.
    #[serde(default)]
    pub sources: BTreeMap<String, DomainSources>,
    /// Harness id (the same spelling [`crate::harness::HarnessKind::id`]
    /// produces) to its installed state.
    #[serde(default)]
    pub harnesses: BTreeMap<String, HarnessState>,
}

impl Default for ProvisionReceipt {
    fn default() -> ProvisionReceipt {
        ProvisionReceipt {
            format: FORMAT,
            sources: BTreeMap::new(),
            harnesses: BTreeMap::new(),
        }
    }
}

/// One domain's source stamps, keyed the same way a scan keys an
/// [`crate::provision::model::ArtifactFile`]: `"<kind.id()>/<rel>"`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DomainSources {
    /// Key to stamp.
    #[serde(default)]
    pub files: BTreeMap<String, SourceStamp>,
}

/// A cheap fingerprint of a source file as last scanned: its modification
/// time and size, cross-checked with its content hash so a reconcile engine
/// can skip rehashing a file whose mtime and size have not moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStamp {
    /// Modification time, Unix seconds.
    pub mtime: i64,
    /// File size in bytes.
    pub size: u64,
    /// Lowercase hex sha256 of the file's bytes as last scanned.
    pub sha256: String,
}

/// One harness's installed state: every file and MCP a reconcile run wrote
/// for it, and which domain each came from.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarnessState {
    /// Desired-set key to installed file.
    #[serde(default)]
    pub files: BTreeMap<String, InstalledFile>,
    /// MCP name to installed MCP.
    #[serde(default)]
    pub mcps: BTreeMap<String, InstalledMcp>,
}

/// One installed file's origin domain and content hash as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledFile {
    /// The domain that provisioned this file.
    pub domain: String,
    /// Lowercase hex sha256 of the file's bytes as written.
    pub sha256: String,
}

/// One installed MCP's origin domain and content hash as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledMcp {
    /// The domain that provisioned this MCP.
    pub domain: String,
    /// Lowercase hex sha256 of the MCP's `server` object as written.
    pub sha256: String,
}

/// The receipt's fixed location, `<state_dir>/provisions.json`.
pub fn receipt_path() -> anyhow::Result<PathBuf> {
    Ok(config::state_dir()?.join("provisions.json"))
}

/// Load the receipt. A missing file is the empty receipt (nothing
/// provisioned yet); an unreadable, unparseable or unknown-format file is an
/// error.
pub fn load(path: &Path) -> anyhow::Result<ProvisionReceipt> {
    let bytes = match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProvisionReceipt::default());
        }
        Err(e) => return Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
        Ok(bytes) => bytes,
    };
    let receipt: ProvisionReceipt = serde_json::from_slice(&bytes).map_err(|e| {
        anyhow::anyhow!(
            "{} is not a valid provisioning receipt: {e}",
            path.display()
        )
    })?;
    if receipt.format != FORMAT {
        return Err(anyhow::anyhow!(
            "{} carries unknown receipt format {}",
            path.display(),
            receipt.format
        ));
    }
    Ok(receipt)
}

/// Write the receipt atomically, pretty-printed with a trailing newline,
/// matching how the cli's install receipt is written.
pub fn save(path: &Path, receipt: &ProvisionReceipt) -> anyhow::Result<()> {
    let mut text = serde_json::to_string_pretty(receipt)?;
    text.push('\n');
    config::save_bytes(path, text.as_bytes())?;
    Ok(())
}

/// Lowercase hex sha256 of raw bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Whether a `files` key read back from the receipt is safe to split and
/// join onto a real directory. The receipt is plain JSON under
/// `<state_dir>/provisions.json`, editable by anything that can write there,
/// so every key it hands back is hostile until proven otherwise: a caller
/// must run this check before joining any component of `key` onto a path,
/// the same discipline [`crate::provision::model::is_plain_component`]
/// documents for a freshly scanned artifact. A key is plain when it is
/// non-empty and every `/`-separated component passes
/// [`crate::provision::model::is_plain_component`].
pub fn plain_rel_key(key: &str) -> bool {
    !key.is_empty() && key.split('/').all(is_plain_component)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_loads_as_the_empty_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let receipt = load(&dir.path().join("missing.json")).unwrap();
        assert_eq!(receipt.format, 1);
        assert!(receipt.sources.is_empty());
        assert!(receipt.harnesses.is_empty());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provisions.json");
        let mut receipt = ProvisionReceipt::default();
        receipt.sources.insert(
            "harbor".to_string(),
            DomainSources {
                files: BTreeMap::from([(
                    "skills/tide-tables/SKILL.md".to_string(),
                    SourceStamp {
                        mtime: 1_700_000_000,
                        size: 42,
                        sha256: sha256_hex(b"tide tables"),
                    },
                )]),
            },
        );
        receipt.harnesses.insert(
            "claude-code".to_string(),
            HarnessState {
                files: BTreeMap::from([(
                    "skills/tide-tables/SKILL.md".to_string(),
                    InstalledFile {
                        domain: "harbor".to_string(),
                        sha256: sha256_hex(b"tide tables"),
                    },
                )]),
                mcps: BTreeMap::from([(
                    "lighthouse".to_string(),
                    InstalledMcp {
                        domain: "harbor".to_string(),
                        sha256: sha256_hex(b"lighthouse server"),
                    },
                )]),
            },
        );

        save(&path, &receipt).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.format, 1);
        let domain_sources = loaded.sources.get("harbor").unwrap();
        let stamp = domain_sources
            .files
            .get("skills/tide-tables/SKILL.md")
            .unwrap();
        assert_eq!(stamp.mtime, 1_700_000_000);
        assert_eq!(stamp.size, 42);
        let harness = loaded.harnesses.get("claude-code").unwrap();
        assert_eq!(
            harness.files["skills/tide-tables/SKILL.md"].domain,
            "harbor"
        );
        assert_eq!(harness.mcps["lighthouse"].domain, "harbor");

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'));
        assert!(text.contains("\n  "));
    }

    #[test]
    fn corrupt_json_and_unknown_format_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let corrupt = dir.path().join("corrupt.json");
        std::fs::write(&corrupt, "{ nope").unwrap();
        assert!(load(&corrupt).is_err(), "corrupt JSON is an error");

        let future = dir.path().join("future.json");
        std::fs::write(
            &future,
            r#"{ "format": 99, "sources": {}, "harnesses": {} }"#,
        )
        .unwrap();
        assert!(load(&future).is_err(), "an unknown format is an error");
    }

    #[test]
    fn sha256_hex_is_the_lowercase_hex_digest() {
        // Known vector: sha256 of the empty input.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn plain_rel_key_accepts_ordinary_keys() {
        assert!(plain_rel_key("skills/tide-tables/SKILL.md"));
        assert!(plain_rel_key("agents/quartermaster.md"));
        assert!(plain_rel_key("lighthouse"));
    }

    #[test]
    fn plain_rel_key_rejects_hostile_rows() {
        assert!(!plain_rel_key("../x"));
        assert!(!plain_rel_key("a:b"));
        assert!(!plain_rel_key("a/../b"));
        assert!(!plain_rel_key("/absolute"));
        assert!(!plain_rel_key(""));
        assert!(!plain_rel_key("a//b"));
        assert!(!plain_rel_key("skills/.hidden/SKILL.md"));
    }
}
