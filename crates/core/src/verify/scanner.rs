//! Discovering Domain roots and collecting their markdown files.
//!
//! Every path given to [`super::verify_paths`] directly is treated as one
//! Domain root - never a directory to search for nested `MANIFEST.md`
//! files. All `.md` files found recursively under a root are parsed;
//! dotfiles and dot-directories are skipped.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::config::{self, DomainConfig};
use crate::engram::Engram;
use crate::parse::{self, ParseError};

use super::VerifyOptions;

/// A file discovered while scanning a Domain root.
pub(crate) struct ScannedFile {
    /// The path as constructed from the given root, used for display and as
    /// the [`super::Issue::path`].
    pub path: PathBuf,
    /// The path relative to the Domain root, used to match `required_files`
    /// entries and per-file token budgets.
    pub rel_path: PathBuf,
    /// The raw file contents.
    pub source: String,
    /// The parse outcome. `Err` short-circuits every rule except the
    /// format-family checks that report the parse failure itself.
    pub parsed: Result<Engram, ParseError>,
}

/// One Domain root and its scanned files.
pub(crate) struct Domain {
    /// The domain name, derived from the root's final path component.
    pub name: String,
    /// The root path, exactly as given.
    pub root: PathBuf,
    /// The index into `files` of `MANIFEST.md`, when present at the root.
    pub manifest_index: Option<usize>,
    /// Every `.md` file found under the root, sorted by path.
    pub files: Vec<ScannedFile>,
    /// The domain's `.crystalline.yaml`, or the default when absent or when
    /// `VerifyOptions::config_override` is set.
    pub config: DomainConfig,
}

/// An error scanning the given paths: a usage or IO failure, distinct from a
/// verify [`super::Issue`], which represents a finding in an otherwise
/// readable file. Maps to exit code 2 at the CLI layer.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// The given path does not exist.
    #[error("path does not exist: {}", .0.display())]
    NotFound(PathBuf),
    /// The given path is not a directory.
    #[error("not a directory: {}", .0.display())]
    NotADirectory(PathBuf),
    /// An IO error while walking or reading a file.
    #[error("io error at {}: {source}", .path.display())]
    Io {
        /// The path involved.
        path: PathBuf,
        /// The underlying IO error.
        source: std::io::Error,
    },
}

pub(crate) fn scan<I, P>(paths: I, options: &VerifyOptions) -> Result<Vec<Domain>, ScanError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut domains = Vec::new();
    let mut seen_roots: HashSet<PathBuf> = HashSet::new();

    for p in paths {
        let root = p.as_ref().to_path_buf();
        let meta = std::fs::metadata(&root).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ScanError::NotFound(root.clone())
            } else {
                ScanError::Io {
                    path: root.clone(),
                    source: e,
                }
            }
        })?;
        if !meta.is_dir() {
            return Err(ScanError::NotADirectory(root));
        }
        if !seen_roots.insert(root.clone()) {
            continue;
        }

        let name = domain_name(&root);
        let files = collect_markdown_files(&root)?;
        let mut manifest_index = None;
        let mut scanned = Vec::with_capacity(files.len());
        for (i, path) in files.into_iter().enumerate() {
            let rel_path = super::forward_slashes(path.strip_prefix(&root).unwrap_or(&path));
            if rel_path == Path::new("MANIFEST.md") {
                manifest_index = Some(i);
            }
            let source = std::fs::read_to_string(&path).map_err(|e| ScanError::Io {
                path: path.clone(),
                source: e,
            })?;
            let parsed = parse::parse_engram(&source);
            scanned.push(ScannedFile {
                path,
                rel_path,
                source,
                parsed,
            });
        }

        let domain_config = if let Some(over) = &options.config_override {
            over.clone()
        } else {
            let cfg_path = root.join(".crystalline.yaml");
            if cfg_path.is_file() {
                config::load_yaml(&cfg_path).unwrap_or_default()
            } else {
                DomainConfig::default()
            }
        };

        domains.push(Domain {
            name,
            root,
            manifest_index,
            files: scanned,
            config: domain_config,
        });
    }

    Ok(domains)
}

fn domain_name(root: &Path) -> String {
    root.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| root.display().to_string())
}

fn collect_markdown_files(root: &Path) -> Result<Vec<PathBuf>, ScanError> {
    let mut out = Vec::new();
    let walker = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_dotfile(e.file_name()));
    for entry in walker {
        let entry = entry.map_err(|e| {
            let path = e
                .path()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.to_path_buf());
            ScanError::Io {
                path,
                source: e
                    .into_io_error()
                    .unwrap_or_else(|| std::io::Error::other("directory walk failed")),
            }
        })?;
        if entry.file_type().is_file()
            && entry.path().extension().and_then(OsStr::to_str) == Some("md")
        {
            out.push(entry.path().to_path_buf());
        }
    }
    out.sort();
    Ok(out)
}

fn is_dotfile(name: &OsStr) -> bool {
    name.to_str().map(|s| s.starts_with('.')).unwrap_or(false)
}
