//! The file substrate: what every whole-file write of a document repeats.
//! The per-file lock, an optional read, the SHA-256 compare-and-swap, the
//! LF write, the on-disk stamp a reindex records and the removal of a file
//! that may already be gone. One copy, so a fix to any of them reaches
//! every kind of document.

use tokio::sync::OwnedMutexGuard;

use super::*;

/// What a write compares the file against before it writes.
#[derive(Debug)]
pub(super) enum Swap<'a> {
    /// The file must be there and hash to this checksum.
    Expect(&'a str),
    /// The file must not be there: a create only.
    Absent,
    /// No comparison: create or replace.
    Any,
}

/// Why a compare-and-swap refused.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SwapRefused {
    /// `Expect`, and there is no file.
    Missing,
    /// `Expect`, and the file hashes to `found`.
    Stale { found: String },
    /// `Absent`, and there is a file.
    Exists,
}

/// The per-file write lock of one path, held for as long as this value
/// lives. See [`Engine::write_lock`].
pub(super) struct FileLock {
    _guard: OwnedMutexGuard<()>,
}

/// The file as text, or `None` when it is not there. Any other failure,
/// a file that is not UTF-8 included, is an error.
pub(super) fn read_opt(abs: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(abs) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(text_read_error(abs, source)),
    }
}

/// Whether `current` (the file as read under its lock) satisfies `swap`.
pub(super) fn check_swap(
    current: Option<&str>,
    swap: &Swap<'_>,
) -> std::result::Result<(), SwapRefused> {
    match (swap, current) {
        (Swap::Any, _) | (Swap::Absent, None) => Ok(()),
        (Swap::Absent, Some(_)) => Err(SwapRefused::Exists),
        (Swap::Expect(_), None) => Err(SwapRefused::Missing),
        (Swap::Expect(expected), Some(text)) => {
            let found = sha256_hex(text.as_bytes());
            if found == *expected {
                Ok(())
            } else {
                Err(SwapRefused::Stale { found })
            }
        }
    }
}

/// The file's bytes and the stamp a reindex records for them, so the
/// watcher does not process a file Crystalline just wrote.
pub(super) fn read_stamped(abs: &Path) -> Result<(Vec<u8>, FileStamp)> {
    let io = |source| EngineError::Io {
        path: abs.display().to_string(),
        source,
    };
    let bytes = std::fs::read(abs).map_err(io)?;
    let meta = std::fs::metadata(abs).map_err(io)?;
    let stamp = FileStamp {
        mtime: mtime_secs(&meta),
        size: meta.len(),
        sha256: sha256_hex(&bytes),
    };
    Ok((bytes, stamp))
}

/// Whether moving `src` to `dst` only changes the letter case of one regular
/// file: the same domain-relative path under case folding, the same canonical
/// file on disk, and neither end a symlink. True only on a disk that folds
/// case; a symlink to the file, or another file, is never the same one. Both
/// paths must be inside one domain (a cross-domain move is never case-only).
/// A change only in Unicode normal form (NFC to NFD) is not case-only: it
/// answers a conflict.
pub(super) fn is_case_only_rename(src: &Path, dst: &Path, from_rel: &str, to_rel: &str) -> bool {
    let plain_file = |p: &Path| p.symlink_metadata().is_ok_and(|m| m.file_type().is_file());
    src != dst
        && crystalline_core::path::fold_path_case(from_rel)
            == crystalline_core::path::fold_path_case(to_rel)
        && plain_file(src)
        && plain_file(dst)
        && matches!((src.canonicalize(), dst.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// Remove the file; `false` when it was not there.
pub(super) fn remove_if_there(abs: &Path) -> Result<bool> {
    match std::fs::remove_file(abs) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(EngineError::Io {
            path: abs.display().to_string(),
            source,
        }),
    }
}

impl Engine {
    /// Take the write lock of `abs` and hold it until the value drops.
    pub(super) async fn lock_file(&self, abs: &Path) -> FileLock {
        FileLock {
            _guard: self.write_lock(abs).lock_owned().await,
        }
    }

    /// Take the write locks of several files at once, in the order of their
    /// lock keys, so two callers that each want two of the same files take
    /// them in one order and cannot wait on each other. Two paths that name
    /// one file take its lock once. Held until the value drops.
    ///
    /// **The mutexes are resolved once, up front, and those are what is locked.**
    /// While this waits for the first, a missing file can appear under another
    /// spelling, after which a fresh lookup of the second path on a disk that
    /// folds case answers with the first: the lock this already holds. A mutex
    /// keeps the key it was made under, so the order between any two mutexes
    /// never changes, however the paths resolve later.
    pub(super) async fn lock_files(&self, paths: &[&Path]) -> Vec<FileLock> {
        let mut keyed: Vec<_> = paths.iter().map(|abs| self.keyed_write_lock(abs)).collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        keyed.dedup_by(|a, b| Arc::ptr_eq(&a.1, &b.1));
        let mut locks = Vec::with_capacity(keyed.len());
        for (_, lock) in keyed {
            locks.push(FileLock {
                _guard: lock.lock_owned().await,
            });
        }
        locks
    }

    /// Lock, read, compare and write `text` (as LF). On success the lock
    /// comes back, so the caller reindexes under it, with the text the write
    /// replaced, read under the same lock, for a caller that may have to put
    /// it back; on a refusal nothing was written.
    pub(super) async fn write_checked(
        &self,
        abs: &Path,
        swap: Swap<'_>,
        text: &str,
    ) -> Result<std::result::Result<(FileLock, Option<String>), SwapRefused>> {
        let lock = self.lock_file(abs).await;
        let current = read_opt(abs)?;
        if let Err(refused) = check_swap(current.as_deref(), &swap) {
            return Ok(Err(refused));
        }
        write_file(abs, text)?;
        Ok(Ok((lock, current)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swap_names_what_it_found() {
        let text = "# A\n";
        let sum = sha256_hex(text.as_bytes());
        assert_eq!(check_swap(Some(text), &Swap::Expect(&sum)), Ok(()));
        assert_eq!(
            check_swap(Some(text), &Swap::Expect("0000")),
            Err(SwapRefused::Stale { found: sum.clone() })
        );
        assert_eq!(
            check_swap(None, &Swap::Expect(&sum)),
            Err(SwapRefused::Missing)
        );
        assert_eq!(
            check_swap(Some(text), &Swap::Absent),
            Err(SwapRefused::Exists)
        );
        assert_eq!(check_swap(None, &Swap::Absent), Ok(()));
        assert_eq!(check_swap(Some(text), &Swap::Any), Ok(()));
    }

    #[test]
    fn an_absent_file_reads_as_none_and_removes_as_false() {
        let tmp = tempfile::tempdir().unwrap();
        let abs = tmp.path().join("a.md");
        assert_eq!(read_opt(&abs).unwrap(), None);
        assert!(!remove_if_there(&abs).unwrap());
        std::fs::write(&abs, "x").unwrap();
        assert_eq!(read_opt(&abs).unwrap().as_deref(), Some("x"));
        let (_, stamp) = read_stamped(&abs).unwrap();
        assert_eq!(
            (stamp.size, stamp.sha256.as_str()),
            (1, sha256_hex(b"x").as_str())
        );
        assert!(remove_if_there(&abs).unwrap());
    }
}
