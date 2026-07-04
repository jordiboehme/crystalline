//! Extracting a repository tarball into the in-memory file map the rest of
//! this crate works with.
//!
//! GitHub's tarball endpoint (`Provider::tarball`) answers with a gzipped
//! tar archive that wraps everything in a single top-level directory named
//! `<owner>-<repo>-<sha>/`. [`extract_tarball`] strips that wrapper and,
//! when the domain lives in a subfolder of the repository rather than at
//! its root, strips the subfolder too, so callers always get plain
//! domain-relative paths regardless of where the domain sits in the
//! repository.

use std::collections::BTreeMap;
use std::io::Read;

use flate2::read::GzDecoder;
use tar::Archive;

use crate::changes::MAX_SHARED_FILE_BYTES;
use crate::error::RemoteError;

/// The files extracted from a tarball (forward-slash relative paths to
/// content) alongside any entries skipped for exceeding
/// [`MAX_SHARED_FILE_BYTES`], each with its size.
pub type ExtractedFiles = (BTreeMap<String, Vec<u8>>, Vec<(String, u64)>);

/// Extracts the files under `subpath` (`None` for the whole tree) from a
/// gzipped tarball, stripping the tarball's single top-level directory.
///
/// Returns the extracted files (forward-slash relative paths, domain-rooted)
/// alongside any entries skipped for exceeding [`MAX_SHARED_FILE_BYTES`],
/// each with its size, so a caller can warn about what did not come down
/// rather than silently dropping it.
///
/// Only regular files are extracted: directories, symlinks, hard links, pax
/// extended headers and any other non-regular entry are skipped outright. A
/// symlink or hard link entry is never followed or written, since its target
/// is just another path inside (or, if crafted maliciously, outside) the
/// archive rather than content to trust.
///
/// An entry whose normalized path would escape the archive root (any `..`
/// path component) is rejected with [`RemoteError::State`]: extraction never
/// writes outside the tree it is asked to produce.
pub fn extract_tarball(bytes: &[u8], subpath: Option<&str>) -> Result<ExtractedFiles, RemoteError> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = Archive::new(decoder);
    let mut files = BTreeMap::new();
    let mut skipped_large = Vec::new();

    let subpath_prefix = subpath.map(|s| format!("{}/", s.trim_matches('/')));

    let entries = archive
        .entries()
        .map_err(|e| RemoteError::State(format!("could not read the tarball: {e}")))?;
    for entry in entries {
        let mut entry = entry
            .map_err(|e| RemoteError::State(format!("could not read a tarball entry: {e}")))?;

        // Only regular files carry content worth extracting; directories,
        // symlinks, hard links and pax headers are all skipped here, never
        // followed and never written.
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry
            .path()
            .map_err(|e| RemoteError::State(format!("invalid tarball entry path: {e}")))?;
        let path_str = path.to_string_lossy().replace('\\', "/");

        if path_str.split('/').any(|component| component == "..") {
            return Err(RemoteError::State(format!(
                "tarball entry escapes its root: {path_str}"
            )));
        }

        // Strip the tarball's single top-level directory
        // (`<owner>-<repo>-<sha>/`); an entry with no directory component at
        // all belongs to no domain tree and is skipped.
        let Some((_top, rest)) = path_str.split_once('/') else {
            continue;
        };

        let rel = match &subpath_prefix {
            Some(prefix) => match rest.strip_prefix(prefix.as_str()) {
                Some(rel) => rel,
                None => continue,
            },
            None => rest,
        };
        if rel.is_empty() {
            continue;
        }

        let size = entry.size();
        if size > MAX_SHARED_FILE_BYTES {
            skipped_large.push((rel.to_string(), size));
            continue;
        }

        let mut content = Vec::with_capacity(size as usize);
        entry.read_to_end(&mut content)?;
        files.insert(rel.to_string(), content);
    }

    Ok((files, skipped_large))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{EntryType, Header};

    use super::*;

    /// Builds a gzipped tarball with every entry wrapped in a single
    /// top-level directory, the way GitHub's tarball endpoint does
    /// (`<owner>-<repo>-<sha>/...`).
    struct TarballBuilder {
        top: String,
        builder: tar::Builder<Vec<u8>>,
    }

    impl TarballBuilder {
        fn new(top: &str) -> Self {
            TarballBuilder {
                top: top.to_string(),
                builder: tar::Builder::new(Vec::new()),
            }
        }

        fn add_file(mut self, rel: &str, content: &[u8]) -> Self {
            let mut header = Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            self.builder
                .append_data(&mut header, format!("{}/{rel}", self.top), content)
                .unwrap();
            self
        }

        fn add_dir(mut self, rel: &str) -> Self {
            let mut header = Header::new_gnu();
            header.set_entry_type(EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            header.set_cksum();
            self.builder
                .append_data(
                    &mut header,
                    format!("{}/{rel}/", self.top),
                    std::io::empty(),
                )
                .unwrap();
            self
        }

        fn add_symlink(mut self, rel: &str, target: &str) -> Self {
            let mut header = Header::new_gnu();
            header.set_entry_type(EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            self.builder
                .append_link(&mut header, format!("{}/{rel}", self.top), target)
                .unwrap();
            self
        }

        /// Writes an entry with `raw_path` copied directly into the header's
        /// name field, bypassing `tar`'s own path validation (which already
        /// rejects `..` components on the safe `append_data` path). This is
        /// how a maliciously crafted tarball could look on the wire, so it
        /// is what the path-escape test needs to build.
        fn add_raw_path(mut self, raw_path: &str, content: &[u8]) -> Self {
            let mut header = Header::new_gnu();
            let name = raw_path.as_bytes();
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            self.builder.append(&header, content).unwrap();
            self
        }

        fn finish_gz(self) -> Vec<u8> {
            let tar_bytes = self.builder.into_inner().unwrap();
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&tar_bytes).unwrap();
            encoder.finish().unwrap()
        }
    }

    #[test]
    fn full_extraction_strips_the_top_level_directory() {
        let bytes = TarballBuilder::new("acme-brand-knowledge-abc123")
            .add_dir("")
            .add_file("MANIFEST.md", b"# Manifest")
            .add_file("notes/example.md", b"content")
            .finish_gz();

        let (files, skipped) = extract_tarball(&bytes, None).unwrap();
        assert!(skipped.is_empty());
        assert_eq!(
            files.get("MANIFEST.md").map(Vec::as_slice),
            Some(&b"# Manifest"[..])
        );
        assert_eq!(
            files.get("notes/example.md").map(Vec::as_slice),
            Some(&b"content"[..])
        );
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn subpath_extraction_maps_the_subfolder_to_the_domain_root() {
        let bytes = TarballBuilder::new("acme-brand-knowledge-abc123")
            .add_file("README.md", b"repo readme")
            .add_file("knowledge/MANIFEST.md", b"# Manifest")
            .add_file("knowledge/notes/example.md", b"content")
            .finish_gz();

        let (files, skipped) = extract_tarball(&bytes, Some("knowledge")).unwrap();
        assert!(skipped.is_empty());
        assert_eq!(files.len(), 2);
        assert_eq!(
            files.get("MANIFEST.md").map(Vec::as_slice),
            Some(&b"# Manifest"[..])
        );
        assert_eq!(
            files.get("notes/example.md").map(Vec::as_slice),
            Some(&b"content"[..])
        );
        assert!(!files.contains_key("README.md"));
    }

    #[test]
    fn non_file_entries_are_skipped() {
        let bytes = TarballBuilder::new("acme-brand-knowledge-abc123")
            .add_dir("notes")
            .add_file("notes/example.md", b"content")
            .add_symlink("notes/link.md", "example.md")
            .finish_gz();

        let (files, _skipped) = extract_tarball(&bytes, None).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files.contains_key("notes/example.md"));
        assert!(!files.contains_key("notes/link.md"));
    }

    #[test]
    fn path_escape_is_rejected() {
        let bytes = TarballBuilder::new("acme-brand-knowledge-abc123")
            .add_raw_path("acme-brand-knowledge-abc123/../../etc/passwd", b"nope")
            .finish_gz();

        let err = extract_tarball(&bytes, None).unwrap_err();
        assert!(matches!(err, crate::error::RemoteError::State(_)));
    }

    #[test]
    fn oversize_entry_is_skipped_and_reported() {
        let oversized = vec![0u8; (crate::changes::MAX_SHARED_FILE_BYTES + 1) as usize];
        let bytes = TarballBuilder::new("acme-brand-knowledge-abc123")
            .add_file("notes/huge.md", &oversized)
            .add_file("notes/normal.md", b"fine")
            .finish_gz();

        let (files, skipped) = extract_tarball(&bytes, None).unwrap();
        assert!(!files.contains_key("notes/huge.md"));
        assert!(files.contains_key("notes/normal.md"));
        assert_eq!(
            skipped,
            vec![("notes/huge.md".to_string(), oversized.len() as u64)]
        );
    }
}
