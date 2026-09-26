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

impl ProvisionReceipt {
    /// Move everything the receipt records under domain `old` to `new`: the
    /// `sources` entry and every harness file and MCP whose origin domain is
    /// `old`. A harness record left on the old name would read as an orphan
    /// to the next reconcile, which removes what it installed. Answers whether
    /// anything moved; a second call answers `false`.
    ///
    /// Stamps already under `new` win over the old name's for the same key,
    /// since they come from a later scan; the old name's other stamps join
    /// them.
    pub fn rename_domain(&mut self, old: &str, new: &str) -> bool {
        if old == new {
            return false;
        }
        let mut moved = false;
        if let Some(sources) = self.sources.remove(old) {
            let target = self.sources.entry(new.to_string()).or_default();
            for (key, stamp) in sources.files {
                target.files.entry(key).or_insert(stamp);
            }
            moved = true;
        }
        for harness in self.harnesses.values_mut() {
            let files = harness.files.values_mut().map(|f| &mut f.domain);
            let mcps = harness.mcps.values_mut().map(|m| &mut m.domain);
            for domain in files.chain(mcps).filter(|d| *d == old) {
                *domain = new.to_string();
                moved = true;
            }
        }
        moved
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

    fn stamp(text: &[u8]) -> SourceStamp {
        SourceStamp {
            mtime: 1_700_000_000,
            size: text.len() as u64,
            sha256: sha256_hex(text),
        }
    }

    /// A receipt with sources under `eng` and `ops`, and one harness holding a
    /// file and an MCP from each.
    fn two_domain_receipt() -> ProvisionReceipt {
        let mut receipt = ProvisionReceipt::default();
        for domain in ["eng", "ops"] {
            receipt.sources.insert(
                domain.to_string(),
                DomainSources {
                    files: BTreeMap::from([(format!("skills/{domain}/SKILL.md"), stamp(b"x"))]),
                },
            );
        }
        let file = |domain: &str| InstalledFile {
            domain: domain.to_string(),
            sha256: sha256_hex(b"x"),
        };
        let mcp = |domain: &str| InstalledMcp {
            domain: domain.to_string(),
            sha256: sha256_hex(b"server"),
        };
        receipt.harnesses.insert(
            "claude-code".to_string(),
            HarnessState {
                files: BTreeMap::from([
                    ("skills/eng/SKILL.md".to_string(), file("eng")),
                    ("skills/ops/SKILL.md".to_string(), file("ops")),
                ]),
                mcps: BTreeMap::from([
                    ("eng-mcp".to_string(), mcp("eng")),
                    ("ops-mcp".to_string(), mcp("ops")),
                ]),
            },
        );
        receipt
    }

    #[test]
    fn renaming_a_domain_moves_its_sources_and_every_harness_record() {
        let mut receipt = two_domain_receipt();
        assert!(receipt.rename_domain("eng", "platform"));

        assert!(!receipt.sources.contains_key("eng"));
        assert!(
            receipt.sources["platform"]
                .files
                .contains_key("skills/eng/SKILL.md")
        );
        assert!(receipt.sources.contains_key("ops"));
        let harness = &receipt.harnesses["claude-code"];
        assert_eq!(harness.files["skills/eng/SKILL.md"].domain, "platform");
        assert_eq!(harness.mcps["eng-mcp"].domain, "platform");
        assert_eq!(harness.files["skills/ops/SKILL.md"].domain, "ops");
        assert_eq!(harness.mcps["ops-mcp"].domain, "ops");

        assert!(
            !receipt.rename_domain("eng", "platform"),
            "a second call finds nothing under the old name"
        );
        assert!(!receipt.rename_domain("never", "seen"));
        assert!(!receipt.rename_domain("ops", "ops"));
    }

    /// Stamps already under the new name win over the old name's for the same
    /// key, since they are the later scan; the old name's other stamps join
    /// them.
    #[test]
    fn renaming_onto_existing_sources_keeps_the_new_names_stamps() {
        let mut receipt = two_domain_receipt();
        receipt.sources.insert(
            "platform".to_string(),
            DomainSources {
                files: BTreeMap::from([("skills/eng/SKILL.md".to_string(), stamp(b"newer"))]),
            },
        );
        receipt
            .sources
            .get_mut("eng")
            .unwrap()
            .files
            .insert("agents/extra.md".to_string(), stamp(b"extra"));
        assert!(receipt.rename_domain("eng", "platform"));
        assert!(!receipt.sources.contains_key("eng"));
        let files = &receipt.sources["platform"].files;
        assert_eq!(files["skills/eng/SKILL.md"], stamp(b"newer"));
        assert_eq!(files["agents/extra.md"], stamp(b"extra"));
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
