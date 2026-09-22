//! The local embedding models Crystalline knows, and the cache their weights
//! live in.
//!
//! Architecture, pooling and the query prefix are per-model facts: bge is a
//! BERT encoder that embeds documents bare and wants an instruction in front of
//! a search query; granite is a ModernBERT encoder that wants nothing in front
//! of anything. Both pool the `[CLS]` position. Code that does not know which
//! of those a model wants produces vectors that are quietly wrong rather than
//! an error, so `embeddings.model` for the local provider is a table lookup
//! and anything else is refused at load.
//!
//! This module is compiled unconditionally even though only the
//! `local-embeddings` provider runs a model: doctor reports the configured
//! model's repo and the cache contents, and the daemon prunes old weights,
//! on every build.

use std::path::{Path, PathBuf};

use crate::error::{IndexError, Result};

/// The encoder family a model's weights load into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    /// candle's stock `bert` module (`model_type: "bert"`).
    Bert,
    /// The vendored ModernBERT module (`model_type: "modernbert"`).
    ModernBert,
}

/// One local model the provider knows how to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalModel {
    /// The id `embeddings.model` names, and the id stored against every vector.
    pub id: &'static str,
    /// The Hugging Face repository the weights come from.
    pub repo: &'static str,
    /// The embedding width.
    pub dims: usize,
    /// Which encoder the weights load into.
    pub architecture: Architecture,
    /// The instruction prefix a search query carries. Empty means bare.
    pub query_prefix: &'static str,
    /// The files the downloader fetches, in fetch order. Per model, so a
    /// model that never needed a file is never made to dial out for it.
    pub files: &'static [&'static str],
    /// The approximate first-use download, in megabytes, for the notice and
    /// for doctor.
    pub download_mb: u64,
}

/// Every model the local provider can run. The first entry is the default.
pub const LOCAL_MODELS: [LocalModel; 2] = [
    LocalModel {
        id: "granite-embedding-97m-multilingual-r2",
        repo: "ibm-granite/granite-embedding-97m-multilingual-r2",
        dims: 384,
        architecture: Architecture::ModernBert,
        query_prefix: "",
        files: &[
            "config.json",
            "tokenizer.json",
            "tokenizer_config.json",
            "special_tokens_map.json",
            "model.safetensors",
        ],
        download_mb: 220,
    },
    LocalModel {
        id: "bge-small-en-v1.5",
        repo: "BAAI/bge-small-en-v1.5",
        dims: 384,
        architecture: Architecture::Bert,
        query_prefix: "Represent this sentence for searching relevant passages: ",
        files: &["config.json", "tokenizer.json", "model.safetensors"],
        download_mb: 130,
    },
];

impl LocalModel {
    /// The text handed to the model for a search query.
    pub fn query_text(&self, text: &str) -> String {
        format!("{}{}", self.query_prefix, text)
    }

    /// This model's directory name inside the hf-hub cache.
    pub fn cache_dir_name(&self) -> String {
        hub_dir_name(self.repo)
    }

    /// The `model_type` the downloaded `config.json` must declare.
    pub fn model_type(&self) -> &'static str {
        match self.architecture {
            Architecture::Bert => "bert",
            Architecture::ModernBert => "modernbert",
        }
    }
}

/// The table entry for an id, by the short id or by the full repository id.
/// Surrounding whitespace in a configured value is not a different model.
pub fn local_model(id: &str) -> Option<&'static LocalModel> {
    let id = id.trim();
    LOCAL_MODELS.iter().find(|m| m.id == id || m.repo == id)
}

/// [`local_model`], refusing an id the table does not know.
///
/// A guess is worse than a refusal here: the architecture, the pooling and the
/// query prefix are per-model facts, and a model run with the wrong ones loads
/// without complaint and returns vectors that are quietly wrong.
pub fn lookup_local_model(id: &str) -> Result<&'static LocalModel> {
    local_model(id).ok_or_else(|| {
        let known = LOCAL_MODELS
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>()
            .join(" and ");
        IndexError::Invalid(format!(
            "unknown local embedding model '{}'; the local provider knows {known}, because the \
             architecture, the pooling and the query prefix are per-model facts the code has to \
             know. Name one of those, or set embeddings.provider: openai-compatible to use \
             another model through an endpoint",
            id.trim()
        ))
    })
}

/// The hf-hub cache directory name for a repository id: `models--<org>--<name>`.
pub fn hub_dir_name(repo: &str) -> String {
    format!("models--{}", repo.replace('/', "--"))
}

/// Every model directory in the cache with its size in bytes, sorted by repo
/// id. Only the `models--<org>--<name>` layout hf-hub writes is listed, so
/// anything else living in that directory is invisible here. A cache that does
/// not exist yet lists nothing.
pub fn cached_model_dirs(models_dir: &Path) -> Vec<(String, u64)> {
    let mut found: Vec<(String, u64)> = hub_dirs(models_dir)
        .into_iter()
        .map(|(repo, path)| {
            let bytes = dir_size(&path);
            (repo, bytes)
        })
        .collect();
    found.sort();
    found
}

/// Remove every cached directory for a repository `LOCAL_MODELS` lists whose
/// id is not in `keep`, returning what went and how many bytes it freed,
/// sorted by repo id.
///
/// Only a repository the table knows is ever a candidate: `CRYSTALLINE_MODELS_DIR`
/// may point at a cache other Hugging Face tools share, and a directory that
/// table does not list - another tool's model, or one this build used to know
/// and has since dropped - is never touched, whatever `keep` says. `keep`
/// itself holds repository ids, not table ids, so a caller that cannot
/// resolve its configured model through the table must decline to prune
/// rather than pass an empty list: an empty `keep` removes every OTHER
/// table-known model.
///
/// Only the `models--<org>--<name>` directories hf-hub writes are considered
/// in the first place; anything else in that directory is left alone. A
/// directory that cannot be emptied (the `-with-model` image bakes a
/// read-only layer) is logged and skipped, never an error: a cache this
/// process may not tidy is not a reason to fail the start that called this.
pub fn prune_model_cache(models_dir: &Path, keep: &[&str]) -> Result<Vec<(String, u64)>> {
    let mut removed = Vec::new();
    let mut candidates: Vec<(String, PathBuf)> = hub_dirs(models_dir)
        .into_iter()
        .filter(|(repo, _)| LOCAL_MODELS.iter().any(|m| m.repo == repo))
        .collect();
    candidates.sort();
    for (repo, path) in candidates {
        if keep.iter().any(|k| k.trim() == repo) {
            continue;
        }
        let bytes = dir_size(&path);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                tracing::info!(
                    repo = %repo,
                    bytes,
                    "removed the cached weights of an embedding model this install no longer uses"
                );
                removed.push((repo, bytes));
            }
            Err(e) => {
                tracing::warn!(
                    repo = %repo,
                    path = %path.display(),
                    "leaving cached model weights in place: {e}"
                );
            }
        }
    }
    Ok(removed)
}

/// The `models--<org>--<name>` directories under a cache root, as
/// `(repo id, path)`. A missing or unreadable root yields nothing.
fn hub_dirs(models_dir: &Path) -> Vec<(String, PathBuf)> {
    let entries = match std::fs::read_dir(models_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        // `models--<org>--<name>`: exactly two segments after the marker, which
        // is what hub_dir_name writes and all that is ever restored from.
        let Some(rest) = name.strip_prefix("models--") else {
            continue;
        };
        let parts: Vec<&str> = rest.split("--").collect();
        if parts.len() != 2 || parts.iter().any(|p| p.is_empty()) {
            continue;
        }
        found.push((format!("{}/{}", parts[0], parts[1]), entry.path()));
    }
    found
}

/// The bytes a directory holds, following no symlink: hf-hub's snapshot files
/// are links into `blobs`, so counting the link targets would count every
/// weight twice.
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_looked_up_by_id_and_by_repo_id() {
        let by_id = lookup_local_model("granite-embedding-97m-multilingual-r2").unwrap();
        assert_eq!(
            by_id.repo,
            "ibm-granite/granite-embedding-97m-multilingual-r2"
        );
        assert_eq!(by_id.dims, 384);
        assert_eq!(by_id.architecture, Architecture::ModernBert);
        assert_eq!(by_id.model_type(), "modernbert");
        // The full repo id resolves to the same entry, so a config that names
        // the model the way Hugging Face does is not a refusal.
        let by_repo =
            lookup_local_model("ibm-granite/granite-embedding-97m-multilingual-r2").unwrap();
        assert_eq!(by_repo.id, "granite-embedding-97m-multilingual-r2");
        // And the model this default replaced is still in the table, so an
        // install that named it keeps working.
        let old = lookup_local_model("bge-small-en-v1.5").unwrap();
        assert_eq!(old.repo, "BAAI/bge-small-en-v1.5");
        assert_eq!(old.architecture, Architecture::Bert);
        assert_eq!(old.model_type(), "bert");
        assert_eq!(
            lookup_local_model("BAAI/bge-small-en-v1.5").unwrap().id,
            old.id
        );
        // Surrounding whitespace in a config value is not a different model.
        assert_eq!(
            lookup_local_model("  granite-embedding-97m-multilingual-r2 ")
                .unwrap()
                .id,
            by_id.id
        );
    }

    #[test]
    fn an_unknown_model_is_refused_naming_both_known_ids() {
        let err = lookup_local_model("multilingual-e5-small")
            .unwrap_err()
            .to_string();
        assert!(err.contains("multilingual-e5-small"), "{err}");
        assert!(
            err.contains("granite-embedding-97m-multilingual-r2"),
            "{err}"
        );
        assert!(err.contains("bge-small-en-v1.5"), "{err}");
        // The refusal says why the code has to know the model, and names the
        // way out for anything else.
        assert!(err.contains("openai-compatible"), "{err}");
    }

    #[test]
    fn only_bge_prefixes_a_query_and_nothing_prefixes_a_document() {
        let granite = lookup_local_model("granite-embedding-97m-multilingual-r2").unwrap();
        // No prefix on either side: the model's own convention, and what makes
        // the stored chunk vectors comparable to each other without a decision.
        assert_eq!(granite.query_text("wie baue ich"), "wie baue ich");
        assert_eq!(granite.query_prefix, "");
        let bge = lookup_local_model("bge-small-en-v1.5").unwrap();
        assert_eq!(
            bge.query_text("how do i build"),
            "Represent this sentence for searching relevant passages: how do i build"
        );
    }

    #[test]
    fn each_model_names_the_files_its_download_fetches() {
        let granite = lookup_local_model("granite-embedding-97m-multilingual-r2").unwrap();
        assert_eq!(
            granite.files,
            [
                "config.json",
                "tokenizer.json",
                "tokenizer_config.json",
                "special_tokens_map.json",
                "model.safetensors",
            ]
        );
        // bge keeps its three-file list, so an air-gapped install that kept bge
        // by config never dials out for a file it did not need before.
        let bge = lookup_local_model("bge-small-en-v1.5").unwrap();
        assert_eq!(
            bge.files,
            ["config.json", "tokenizer.json", "model.safetensors"]
        );
        for m in LOCAL_MODELS {
            assert!(m.files.contains(&"model.safetensors"), "{}", m.id);
            assert!(m.files.contains(&"config.json"), "{}", m.id);
            assert!(m.files.contains(&"tokenizer.json"), "{}", m.id);
        }
    }

    #[test]
    fn a_repo_id_maps_to_the_hub_cache_directory_name() {
        assert_eq!(
            hub_dir_name("ibm-granite/granite-embedding-97m-multilingual-r2"),
            "models--ibm-granite--granite-embedding-97m-multilingual-r2"
        );
        assert_eq!(
            lookup_local_model("bge-small-en-v1.5")
                .unwrap()
                .cache_dir_name(),
            "models--BAAI--bge-small-en-v1.5"
        );
    }

    /// The default id must be a table entry, or the first start after an
    /// upgrade refuses to load a model at all.
    #[test]
    fn the_default_model_id_is_the_first_table_entry() {
        assert_eq!(crate::embed::DEFAULT_MODEL_ID, LOCAL_MODELS[0].id);
        assert!(local_model(crate::embed::DEFAULT_MODEL_ID).is_some());
    }

    /// A minimal hf-hub-shaped directory: one blob under `blobs`, one snapshot
    /// file, so a removal has something to count and a size has something to
    /// measure.
    fn hub_dir(root: &std::path::Path, repo: &str, blob: &[u8]) -> std::path::PathBuf {
        let dir = root.join(hub_dir_name(repo));
        std::fs::create_dir_all(dir.join("blobs")).unwrap();
        std::fs::create_dir_all(dir.join("snapshots/abc")).unwrap();
        std::fs::write(dir.join("blobs/weights"), blob).unwrap();
        std::fs::write(dir.join("snapshots/abc/config.json"), b"{}").unwrap();
        dir
    }

    #[test]
    fn pruning_removes_only_table_known_hub_directories_the_caller_did_not_keep() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        hub_dir(
            root,
            "ibm-granite/granite-embedding-97m-multilingual-r2",
            &[7u8; 64],
        );
        hub_dir(root, "BAAI/bge-small-en-v1.5", &[3u8; 32]);
        // A directory `LOCAL_MODELS` does not list: another tool's weights, or
        // a model this build used to know and has since dropped from the
        // table. The pruner never touches it, whatever the keep list says -
        // that is the whole point of a shared `CRYSTALLINE_MODELS_DIR` being
        // safe to point Crystalline at.
        hub_dir(root, "sentence-transformers/all-MiniLM-L6-v2", &[1u8; 16]);
        // Something in the cache that is not ours, which the pruner never
        // touches whatever the keep list says.
        std::fs::write(root.join("version.txt"), b"1").unwrap();

        let removed =
            prune_model_cache(root, &["ibm-granite/granite-embedding-97m-multilingual-r2"])
                .unwrap();
        let repos: Vec<&str> = removed.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(
            repos,
            ["BAAI/bge-small-en-v1.5"],
            "only the table-known model the caller did not keep is removed"
        );
        // The byte count is the directory that was there.
        let bytes: Vec<u64> = removed.iter().map(|(_, b)| *b).collect();
        assert!(
            bytes[0] >= 32,
            "the removal reports what it freed: {bytes:?}"
        );

        assert!(
            root.join(hub_dir_name(
                "ibm-granite/granite-embedding-97m-multilingual-r2"
            ))
            .is_dir()
        );
        assert!(!root.join(hub_dir_name("BAAI/bge-small-en-v1.5")).exists());
        assert!(
            root.join(hub_dir_name("sentence-transformers/all-MiniLM-L6-v2"))
                .is_dir(),
            "a directory the table does not know survives, kept or not"
        );
        assert!(
            root.join("version.txt").is_file(),
            "an unrelated file is untouched"
        );

        // Idempotent: a second call with the same keep list removes nothing.
        assert!(
            prune_model_cache(root, &["ibm-granite/granite-embedding-97m-multilingual-r2"])
                .unwrap()
                .is_empty()
        );
    }

    /// A model the table knows is pruned exactly like before the table
    /// restriction landed: being known is not the same as being kept.
    #[test]
    fn a_known_model_not_in_the_keep_list_is_still_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        hub_dir(
            root,
            "ibm-granite/granite-embedding-97m-multilingual-r2",
            &[7u8; 64],
        );
        hub_dir(root, "BAAI/bge-small-en-v1.5", &[3u8; 32]);

        let removed =
            prune_model_cache(root, &["ibm-granite/granite-embedding-97m-multilingual-r2"])
                .unwrap();
        let repos: Vec<&str> = removed.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(
            repos,
            ["BAAI/bge-small-en-v1.5"],
            "bge is in LOCAL_MODELS, so not being kept is reason enough to remove it"
        );
        assert!(!root.join(hub_dir_name("BAAI/bge-small-en-v1.5")).exists());
    }

    #[test]
    fn pruning_a_missing_cache_directory_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("never-created");
        assert!(
            prune_model_cache(
                &missing,
                &["ibm-granite/granite-embedding-97m-multilingual-r2"]
            )
            .unwrap()
            .is_empty()
        );
    }

    /// A directory the process cannot empty is left alone and logged, never an
    /// error: the `-with-model` image layer is exactly this case. Unix only -
    /// the permission model this leans on is not Windows's, and Windows is a
    /// known CI blind spot here.
    #[cfg(unix)]
    #[test]
    fn a_directory_that_cannot_be_removed_is_skipped_rather_than_failing() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        hub_dir(
            root,
            "ibm-granite/granite-embedding-97m-multilingual-r2",
            &[7u8; 64],
        );
        let locked = hub_dir(root, "BAAI/bge-small-en-v1.5", &[3u8; 32]);
        std::fs::write(root.join("version.txt"), b"1").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

        // Ok, not Err: a cache the process may not tidy is not a reason to
        // fail the start that called this.
        let removed =
            prune_model_cache(root, &["ibm-granite/granite-embedding-97m-multilingual-r2"])
                .unwrap();
        assert!(
            removed.iter().all(|(r, _)| r != "BAAI/bge-small-en-v1.5"),
            "a directory that could not be emptied is not reported as removed: {removed:?}"
        );
        assert!(
            root.join(hub_dir_name(
                "ibm-granite/granite-embedding-97m-multilingual-r2"
            ))
            .is_dir()
        );
        assert!(root.join("version.txt").is_file());

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn the_cache_listing_reports_every_hub_directory_with_its_size() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        hub_dir(
            root,
            "ibm-granite/granite-embedding-97m-multilingual-r2",
            &[7u8; 64],
        );
        hub_dir(root, "BAAI/bge-small-en-v1.5", &[3u8; 32]);
        std::fs::write(root.join("version.txt"), b"1").unwrap();

        let cached = cached_model_dirs(root);
        let repos: Vec<&str> = cached.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(
            repos,
            [
                "BAAI/bge-small-en-v1.5",
                "ibm-granite/granite-embedding-97m-multilingual-r2"
            ]
        );
        assert!(
            cached[1].1 >= 64,
            "the bigger model measures bigger: {cached:?}"
        );
        assert_eq!(cached_model_dirs(&root.join("missing")), Vec::new());
    }
}
