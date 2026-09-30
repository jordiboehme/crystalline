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
    /// The Hugging Face commit the files are fetched at. A full 40-character
    /// hash, never a branch: hf-hub answers a hash from
    /// `snapshots/<hash>/` in the cache without any network call, and a
    /// download gets exactly the files this build was tested with. Moving it
    /// is a deliberate change: an install whose cache holds only another
    /// commit starts on that one (see [`choose_snapshot`]) and switches at a
    /// later start, once the pinned commit is cached and carries the same
    /// weights and tokenizer.
    ///
    /// The index keys stored vectors by [`LocalModel::id`] alone, never by
    /// commit. So a pin bump that changes the content of `model.safetensors`
    /// or `tokenizer.json` must also change `id`: that is what sends every
    /// chunk through the existing model-change path to be embedded again.
    /// A bump that leaves both files as they were keeps the id. Today every
    /// loadable commit of both entries carries the same two files, so any
    /// older snapshot a cache holds switches over without re-embedding.
    pub revision: &'static str,
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
        revision: "835ad14087e140460703cf0fae09f97d469d65c2",
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
        revision: "5c38ec7c405ec4b44b94cc5a9bb96e735b38267a",
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

/// Which cached snapshot of a model a start loads, decided from the cache
/// directory alone with no network call. See [`choose_snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotChoice {
    /// The pinned commit is complete in the cache. `others` names every other
    /// snapshot of the repo there, which [`retire_older_snapshots`] removes
    /// once the pinned one has loaded.
    Pinned { others: Vec<String> },
    /// An older complete snapshot is used instead of the pinned commit.
    Older {
        /// The commit in use.
        commit: String,
        /// False: the pinned commit is not complete in the cache yet, and a
        /// `crystalline model download` (or the daemon's background fetch)
        /// brings it. True: the pinned commit is complete but its weights or
        /// tokenizer differ from `commit`'s, so switching would leave the
        /// index holding vectors from other weights, and the start stays on
        /// `commit`.
        pinned_differs: bool,
    },
    /// No complete snapshot: the pinned commit is downloaded.
    Missing,
}

/// The files whose content decides whether two snapshots embed alike: the
/// weights and the tokenizer. `config.json` is left out on purpose - granite's
/// commits differ there only in `classifier_pooling` (unused, the pooling is
/// in `local.rs`) and `max_position_embeddings` (the rope table size, far
/// above the input cap).
const CONTENT_FILES: [&str; 2] = ["model.safetensors", "tokenizer.json"];

/// Decide which snapshot of `model` a start loads, from `models_dir` alone.
///
/// 1. The pinned commit, when every file the loader needs is under
///    `snapshots/<revision>/`.
/// 2. Otherwise another complete snapshot of the same repository: the commit
///    `refs/main` names, else the newest by modification time (ties by
///    name, so the choice is deterministic). A snapshot missing any file of
///    [`LocalModel::files`] is never chosen.
/// 3. Otherwise [`SnapshotChoice::Missing`].
///
/// When the pinned commit is complete and an older complete snapshot is
/// also cached, the two are compared by the content of
/// [`CONTENT_FILES`]: equal content switches to the pinned one, different
/// content stays on the older one, because the index keys its vectors by
/// model id and could not tell the weights apart.
pub fn choose_snapshot(models_dir: &Path, model: &LocalModel) -> SnapshotChoice {
    let repo_dir = models_dir.join(model.cache_dir_name());
    let snapshots = repo_dir.join("snapshots");
    let mut others: Vec<String> = snapshot_commits(&snapshots)
        .into_iter()
        .filter(|c| c != model.revision)
        .collect();
    others.sort();
    let pinned_complete = snapshot_complete(&snapshots.join(model.revision), model);
    let older = older_snapshot(&repo_dir, &others, model);
    match (pinned_complete, older) {
        (true, None) => SnapshotChoice::Pinned { others },
        (true, Some(commit)) => {
            let pinned = snapshots.join(model.revision);
            let old = snapshots.join(&commit);
            if CONTENT_FILES
                .iter()
                .all(|f| same_content(&pinned.join(f), &old.join(f)))
            {
                SnapshotChoice::Pinned { others }
            } else {
                SnapshotChoice::Older {
                    commit,
                    pinned_differs: true,
                }
            }
        }
        (false, Some(commit)) => SnapshotChoice::Older {
            commit,
            pinned_differs: false,
        },
        (false, None) => SnapshotChoice::Missing,
    }
}

/// The directory names under `snapshots/`. A missing directory lists nothing.
fn snapshot_commits(snapshots: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(snapshots) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Every file the loader needs is present (a symlink counts when its blob is
/// there, which `is_file` follows).
fn snapshot_complete(dir: &Path, model: &LocalModel) -> bool {
    model.files.iter().all(|f| dir.join(f).is_file())
}

/// The older complete snapshot a start would use: the one `refs/main` names,
/// else the newest by modification time, ties by name.
fn older_snapshot(repo_dir: &Path, others: &[String], model: &LocalModel) -> Option<String> {
    let snapshots = repo_dir.join("snapshots");
    let complete: Vec<&String> = others
        .iter()
        .filter(|c| snapshot_complete(&snapshots.join(c), model))
        .collect();
    if let Ok(main) = std::fs::read_to_string(repo_dir.join("refs").join("main")) {
        let main = main.trim();
        if let Some(c) = complete.iter().find(|c| c.as_str() == main) {
            return Some((*c).clone());
        }
    }
    complete
        .into_iter()
        .map(|c| {
            let modified = std::fs::metadata(snapshots.join(c))
                .and_then(|m| m.modified())
                .ok();
            (modified, c)
        })
        .max()
        .map(|(_, c)| c.clone())
}

/// True when two cached files hold the same bytes. hf-hub stores a file as
/// `blobs/<etag>` and points the snapshot entry at it with a symlink, and the
/// etag of an LFS file is the sha256 of its content, so two links are
/// compared by their blob names without reading 220 MB. A plain file (a
/// pre-seeded copy, or a Windows cache, which copies instead of linking) is
/// compared by the sha256 of both files' content instead. A file that cannot
/// be read counts as different: staying on the older snapshot is the safe
/// side.
fn same_content(a: &Path, b: &Path) -> bool {
    if let (Some(x), Some(y)) = (blob_name(a), blob_name(b)) {
        return x == y;
    }
    match (sha256_file(a), sha256_file(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// The blob a snapshot symlink points at, by file name, or `None` for a file
/// that is not a symlink.
fn blob_name(path: &Path) -> Option<String> {
    let meta = path.symlink_metadata().ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    let target = std::fs::read_link(path).ok()?;
    target.file_name().map(|n| n.to_string_lossy().into_owned())
}

fn sha256_file(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(crate::hex_lower(&hasher.finalize()))
}

/// Remove the snapshots of `model` other than the pinned commit, once a start
/// has loaded the pinned one, returning the commits that went.
///
/// It acts only when [`choose_snapshot`] answers
/// [`SnapshotChoice::Pinned`]: a start that stayed on an older snapshot
/// (not downloaded yet, or different weights) never loses it. Beside each
/// removed snapshot directory, a `refs/` file naming it and every blob only
/// its links pointed at go too; a blob a remaining snapshot still links to
/// stays. Only this model's repository directory is ever touched. A removal
/// that fails (a read-only pre-seeded `CRYSTALLINE_MODELS_DIR`) is logged
/// and skipped: an untidy cache costs disk and nothing else.
pub fn retire_older_snapshots(models_dir: &Path, model: &LocalModel) -> Vec<String> {
    let SnapshotChoice::Pinned { others } = choose_snapshot(models_dir, model) else {
        return Vec::new();
    };
    let repo_dir = models_dir.join(model.cache_dir_name());
    let snapshots = repo_dir.join("snapshots");
    let blobs_of = |commit: &str| -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(snapshots.join(commit)) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| blob_name(&e.path()))
            .collect()
    };
    let mut removed = Vec::new();
    let mut freed_blobs: Vec<String> = Vec::new();
    for commit in &others {
        let blobs = blobs_of(commit);
        let dir = snapshots.join(commit);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {
                freed_blobs.extend(blobs);
                removed.push(commit.clone());
            }
            Err(e) => tracing::warn!(
                model = model.id,
                commit = %commit,
                path = %dir.display(),
                "leaving an older snapshot of the embedding model in place: {e}"
            ),
        }
    }
    // Blobs a remaining snapshot still links to are shared, and stay.
    let kept: Vec<String> = snapshot_commits(&snapshots)
        .iter()
        .flat_map(|c| blobs_of(c))
        .collect();
    for blob in freed_blobs {
        if kept.contains(&blob) {
            continue;
        }
        let path = repo_dir.join("blobs").join(&blob);
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), "leaving a model blob in place: {e}");
        }
    }
    // A ref naming a removed commit would point at nothing.
    if let Ok(entries) = std::fs::read_dir(repo_dir.join("refs")) {
        for entry in entries.flatten() {
            let names_removed = std::fs::read_to_string(entry.path())
                .map(|c| removed.iter().any(|r| r == c.trim()))
                .unwrap_or(false);
            if names_removed {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    if !removed.is_empty() {
        tracing::info!(
            model = model.id,
            commits = %removed.join(", "),
            "removed older snapshots of the embedding model now that the pinned commit is loaded"
        );
    }
    removed
}

/// The one warning a start on an older snapshot logs, shared with doctor so
/// both say the same. `None` for any other choice.
pub fn older_snapshot_warning(model: &LocalModel, choice: &SnapshotChoice) -> Option<String> {
    match choice {
        SnapshotChoice::Older {
            commit,
            pinned_differs: false,
        } => Some(format!(
            "embedding model {} is running on cached commit {commit}, not the pinned commit {}; \
             run `crystalline model download` to update (a daemon also fetches it in the \
             background and uses it from the next start)",
            model.id, model.revision
        )),
        SnapshotChoice::Older {
            commit,
            pinned_differs: true,
        } => Some(format!(
            "embedding model {} stays on cached commit {commit}: the pinned commit {} is cached \
             too, but its weights or tokenizer differ, and switching would need the index \
             re-embedded",
            model.id, model.revision
        )),
        _ => None,
    }
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

    /// Every model is fetched at a fixed commit, never at a branch: a full
    /// 40-character hash is what hf-hub resolves from the cache without a
    /// network call, and what makes a download the files this build was
    /// tested with.
    #[test]
    fn every_model_is_pinned_to_a_full_commit_hash() {
        for m in LOCAL_MODELS {
            assert_eq!(m.revision.len(), 40, "{}", m.id);
            assert!(
                m.revision
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
                "{}: {}",
                m.id,
                m.revision
            );
        }
        assert_eq!(
            lookup_local_model("granite-embedding-97m-multilingual-r2")
                .unwrap()
                .revision,
            "835ad14087e140460703cf0fae09f97d469d65c2"
        );
        assert_eq!(
            lookup_local_model("bge-small-en-v1.5").unwrap().revision,
            "5c38ec7c405ec4b44b94cc5a9bb96e735b38267a"
        );
    }

    const OLD_A: &str = "1111111111111111111111111111111111111111";
    const OLD_B: &str = "2222222222222222222222222222222222222222";

    /// A snapshot the way hf-hub writes one on unix: each file a relative
    /// symlink into `blobs/<etag>`, the etag here being `<tag>-<file>` so two
    /// snapshots share a blob exactly when they pass the same tag.
    #[cfg(unix)]
    fn linked_snapshot(root: &Path, model: &LocalModel, commit: &str, tag: &str) {
        let repo = root.join(model.cache_dir_name());
        let blobs = repo.join("blobs");
        let snap = repo.join("snapshots").join(commit);
        std::fs::create_dir_all(&blobs).unwrap();
        std::fs::create_dir_all(&snap).unwrap();
        for file in model.files {
            let etag = format!("{tag}-{file}");
            std::fs::write(blobs.join(&etag), tag.as_bytes()).unwrap();
            std::os::unix::fs::symlink(format!("../../blobs/{etag}"), snap.join(file)).unwrap();
        }
    }

    /// A snapshot of plain files, the way a copied pre-seed looks.
    fn copied_snapshot(root: &Path, model: &LocalModel, commit: &str, content: &[u8]) {
        let snap = root
            .join(model.cache_dir_name())
            .join("snapshots")
            .join(commit);
        std::fs::create_dir_all(&snap).unwrap();
        for file in model.files {
            std::fs::write(snap.join(file), content).unwrap();
        }
    }

    fn set_main(root: &Path, model: &LocalModel, commit: &str) {
        let refs = root.join(model.cache_dir_name()).join("refs");
        std::fs::create_dir_all(&refs).unwrap();
        std::fs::write(refs.join("main"), commit).unwrap();
    }

    fn granite() -> &'static LocalModel {
        lookup_local_model("granite-embedding-97m-multilingual-r2").unwrap()
    }

    #[test]
    fn an_empty_cache_has_no_snapshot_to_choose() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            choose_snapshot(tmp.path(), granite()),
            SnapshotChoice::Missing
        );
        assert!(retire_older_snapshots(tmp.path(), granite()).is_empty());
    }

    /// `refs/main` decides between two older snapshots; without it the
    /// newest by modification time does.
    #[test]
    fn refs_main_picks_the_older_snapshot_and_else_the_newest_does() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        copied_snapshot(root, granite(), OLD_A, b"a");
        copied_snapshot(root, granite(), OLD_B, b"b");
        let snaps = root.join(granite().cache_dir_name()).join("snapshots");
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        let new = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000);
        let touch = |commit: &str, t: std::time::SystemTime| {
            std::fs::File::open(snaps.join(commit))
                .unwrap()
                .set_modified(t)
                .unwrap();
        };
        touch(OLD_A, new);
        touch(OLD_B, old);
        assert_eq!(
            choose_snapshot(root, granite()),
            SnapshotChoice::Older {
                commit: OLD_A.to_string(),
                pinned_differs: false
            },
            "without refs/main the newest snapshot is chosen"
        );
        set_main(root, granite(), OLD_B);
        assert_eq!(
            choose_snapshot(root, granite()),
            SnapshotChoice::Older {
                commit: OLD_B.to_string(),
                pinned_differs: false
            },
            "refs/main wins over modification time"
        );
        // refs/main naming an incomplete snapshot is passed over.
        std::fs::remove_file(snaps.join(OLD_B).join("tokenizer.json")).unwrap();
        assert_eq!(
            choose_snapshot(root, granite()),
            SnapshotChoice::Older {
                commit: OLD_A.to_string(),
                pinned_differs: false
            }
        );
    }

    /// Linked snapshots compare by blob name, so the same etag switches to
    /// the pinned commit and another etag stays on the older one.
    #[cfg(unix)]
    #[test]
    fn linked_snapshots_are_compared_by_their_blobs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        linked_snapshot(root, granite(), OLD_A, "same");
        linked_snapshot(root, granite(), granite().revision, "same");
        assert_eq!(
            choose_snapshot(root, granite()),
            SnapshotChoice::Pinned {
                others: vec![OLD_A.to_string()]
            }
        );

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        linked_snapshot(root, granite(), OLD_A, "old");
        linked_snapshot(root, granite(), granite().revision, "new");
        assert_eq!(
            choose_snapshot(root, granite()),
            SnapshotChoice::Older {
                commit: OLD_A.to_string(),
                pinned_differs: true
            }
        );
        // Nothing is retired while the start stays on the older snapshot.
        assert!(retire_older_snapshots(root, granite()).is_empty());
        assert!(
            root.join(granite().cache_dir_name())
                .join("snapshots")
                .join(OLD_A)
                .is_dir()
        );
    }

    /// A copied pre-seed beside a linked pinned download is compared by
    /// content: the blob name alone would never match a plain file.
    #[cfg(unix)]
    #[test]
    fn a_copied_snapshot_is_compared_by_content() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        linked_snapshot(root, granite(), granite().revision, "same");
        copied_snapshot(root, granite(), OLD_A, b"same");
        assert!(matches!(
            choose_snapshot(root, granite()),
            SnapshotChoice::Pinned { .. }
        ));
        copied_snapshot(root, granite(), OLD_A, b"other");
        assert_eq!(
            choose_snapshot(root, granite()),
            SnapshotChoice::Older {
                commit: OLD_A.to_string(),
                pinned_differs: true
            }
        );
    }

    /// Once the pinned commit is loaded, the other snapshots of that repo go,
    /// with their refs and the blobs only they used; a shared blob, the
    /// pinned snapshot and every other repository stay.
    #[cfg(unix)]
    #[test]
    fn retiring_removes_only_the_older_snapshots_of_that_model() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let g = granite();
        linked_snapshot(root, g, g.revision, "same");
        linked_snapshot(root, g, OLD_A, "same");
        // An incomplete leftover that only an older snapshot used a blob for.
        linked_snapshot(root, g, OLD_B, "stray");
        std::fs::remove_file(
            root.join(g.cache_dir_name())
                .join("snapshots")
                .join(OLD_B)
                .join("model.safetensors"),
        )
        .unwrap();
        set_main(root, g, OLD_A);
        let bge = lookup_local_model("bge-small-en-v1.5").unwrap();
        copied_snapshot(root, bge, OLD_A, b"bge");

        let removed = retire_older_snapshots(root, g);
        assert_eq!(removed, vec![OLD_A.to_string(), OLD_B.to_string()]);
        let repo = root.join(g.cache_dir_name());
        assert!(!repo.join("snapshots").join(OLD_A).exists());
        assert!(!repo.join("snapshots").join(OLD_B).exists());
        assert!(
            !repo.join("refs").join("main").exists(),
            "refs/main named a removed commit"
        );
        for file in g.files {
            assert!(
                repo.join("snapshots").join(g.revision).join(file).is_file(),
                "the pinned {file} still resolves"
            );
            // The stray snapshot's weights link was already gone, so only
            // the blobs it still linked to are its to take along.
            if *file != "model.safetensors" {
                assert!(
                    !repo.join("blobs").join(format!("stray-{file}")).exists(),
                    "a blob only a removed snapshot used goes: {file}"
                );
            }
            assert!(
                repo.join("blobs").join(format!("same-{file}")).is_file(),
                "a blob the pinned snapshot shares stays: {file}"
            );
        }
        assert!(
            root.join(bge.cache_dir_name())
                .join("snapshots")
                .join(OLD_A)
                .is_dir(),
            "another model's snapshots are never touched"
        );
        assert_eq!(
            choose_snapshot(root, g),
            SnapshotChoice::Pinned { others: vec![] }
        );
        assert!(retire_older_snapshots(root, g).is_empty());
    }

    /// A pre-seeded cache the process may not write keeps its older
    /// snapshot, and retiring it is not an error.
    #[cfg(unix)]
    #[test]
    fn retiring_in_a_read_only_cache_logs_and_keeps_going() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let g = granite();
        copied_snapshot(root, g, g.revision, b"same");
        copied_snapshot(root, g, OLD_A, b"same");
        let snaps = root.join(g.cache_dir_name()).join("snapshots");
        // Read-only the way a mounted pre-seed is: every directory, so not
        // even the files inside the older snapshot can go.
        for dir in [snaps.join(OLD_A), snaps.clone()] {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        }

        let removed = retire_older_snapshots(root, g);
        for dir in [snaps.clone(), snaps.join(OLD_A)] {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(removed.is_empty(), "{removed:?}");
        assert!(snaps.join(OLD_A).join("model.safetensors").is_file());
        assert!(snaps.join(g.revision).is_dir());
    }
}
