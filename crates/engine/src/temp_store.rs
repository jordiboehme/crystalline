//! Spill containment: where the daemon's database scratch files live, and how
//! the ones a killed daemon left behind are reclaimed.
//!
//! The embedded database engine spills a sorter that outgrows its buffer to a
//! temp file. It creates that file through `tempfile::tempdir()`, which resolves
//! the process temp location (`TMPDIR` on unix, `TMP`/`TEMP` on Windows) on
//! every call, and deletes the directory in `Drop`. A graceful error unwinds and
//! cleans up; a `SIGKILL` - a macOS jetsam OOM-kill, for instance - does not,
//! and the spill file survives in the user's shared temp directory with a name
//! that says nothing about who wrote it. A crash loop leaves one per attempt.
//! That is the 2026-07-28 field incident (see
//! `research/2026-07-28-turso-sorter-spill.md`), where ten orphans totalling
//! ~450GB filled the disk.
//!
//! So the daemon points the process temp location at
//! [`config::temp_store_dir`] and sweeps that one directory on startup. This
//! bounds the collateral damage of a future spill and makes the leftovers
//! obviously ours; it is defence in depth, not the fix. The fix is that no query
//! feeds a wide projection into a sorter (see `crate::index::turso::search`).
//!
//! `TURSO_TMPDIR` and `SQLITE_TMPDIR` are set alongside because the engine
//! honors those for its temp *database* files; the sorter path reads neither, so
//! the plain temp location is the one that matters and all three are set
//! together.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crystalline_core::config;

/// How long a leftover must have sat untouched before the sweep reclaims it. A
/// live daemon writes to its spill file continuously, and only one daemon owns a
/// state directory at a time, so an hour is far beyond any legitimate age; the
/// check exists so a sweep can never race a spill that is still in use during a
/// takeover.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// What the startup sweep reclaimed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// How many stale entries were removed.
    pub removed: usize,
    /// How many bytes those entries held.
    pub bytes: u64,
    /// How many entries were left alone because they are too young to be sure
    /// they are orphans.
    pub kept: usize,
}

/// Point this process's temp location at the daemon's own scratch directory,
/// creating it if needed. Returns the directory.
///
/// # Safety and call site
///
/// This sets process-wide environment variables, which is only sound while the
/// process is single-threaded: `setenv` is not thread-safe against a concurrent
/// `getenv` anywhere in the process. It must therefore be called from `main`
/// before any runtime or worker thread starts, and only for `crystalline serve`,
/// because the daemon owns its state directory for its whole life. A CLI
/// one-shot must never call it: several of those can run at once against the
/// same state directory, and they would race each other's sweeps.
///
/// Every other temp-file user in the daemon (a model download, an atomic write)
/// is relocated along with the database engine. That is intended: they are all
/// the daemon's scratch, and putting them next to the index keeps them on the
/// volume the user already sized for it.
pub fn point_at_state_dir() -> anyhow::Result<PathBuf> {
    let dir = config::temp_store_dir()?;
    std::fs::create_dir_all(&dir)?;
    // Canonicalize so a relative or symlinked state directory still yields an
    // absolute path: a child process inheriting the variable may have a
    // different working directory.
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);

    // SAFETY: the caller guarantees this runs on the only thread in the process,
    // before any runtime is started (see the call site in the CLI's `serve`
    // arm), so no concurrent `getenv` can observe a torn environment.
    unsafe {
        #[cfg(windows)]
        {
            std::env::set_var("TMP", &dir);
            std::env::set_var("TEMP", &dir);
        }
        #[cfg(not(windows))]
        {
            std::env::set_var("TMPDIR", &dir);
        }
        std::env::set_var("TURSO_TMPDIR", &dir);
        std::env::set_var("SQLITE_TMPDIR", &dir);
    }
    Ok(dir)
}

/// Reclaim stale scratch left behind by a killed predecessor, in the daemon's
/// own directory only. Never touches the system temp directory: the daemon has
/// no way to tell a foreign process's files from its own there, and deleting
/// someone else's scratch is worse than leaking ours.
///
/// Safe to call after the runtime has started - it only touches the filesystem.
pub fn sweep(dir: &Path) -> SweepReport {
    let mut report = SweepReport::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return report;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let young = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_none_or(|age| age < STALE_AFTER);
        if young {
            report.kept += 1;
            continue;
        }
        let size = dir_size(&path, meta.is_dir());
        let removed = if meta.is_dir() {
            std::fs::remove_dir_all(&path).is_ok()
        } else {
            std::fs::remove_file(&path).is_ok()
        };
        if removed {
            report.removed += 1;
            report.bytes += size;
        }
    }
    report
}

/// The bytes an entry holds, summed one level deep and beyond. Best effort: an
/// unreadable child contributes zero rather than aborting the sweep.
fn dir_size(path: &Path, is_dir: bool) -> u64 {
    if !is_dir {
        return std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| {
            let child = e.path();
            let is_dir = e.metadata().map(|m| m.is_dir()).unwrap_or(false);
            dir_size(&child, is_dir)
        })
        .sum()
}

/// Create the scratch directory (if the daemon was started without the CLI's
/// `point_at_state_dir` call), sweep it and log what was reclaimed. Called once
/// at daemon startup, after tracing is initialized.
pub fn sweep_at_startup() {
    let Ok(dir) = config::temp_store_dir() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(
            "could not create the scratch directory {}: {e}",
            dir.display()
        );
        return;
    }
    let report = sweep(&dir);
    if report.removed > 0 {
        tracing::info!(
            "reclaimed {} stale scratch {} ({:.1} MB) from {}",
            report.removed,
            if report.removed == 1 {
                "entry"
            } else {
                "entries"
            },
            report.bytes as f64 / 1_048_576.0,
            dir.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set an entry's mtime far enough into the past that the sweep sees it as
    /// stale. `filetime` is not a dependency, so this goes through `utimensat`
    /// on unix and is skipped elsewhere.
    #[cfg(unix)]
    fn age(path: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let old = libc::timespec {
            tv_sec: (SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                - 7200) as libc::time_t,
            tv_nsec: 0,
        };
        let times = [old, old];
        assert_eq!(
            unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) },
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_sweep_reclaims_stale_scratch_and_spares_fresh_scratch() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();

        // A stale leftover shaped like the engine's: a directory holding one
        // spill file.
        let stale = dir.join(".tmpAAAAAA");
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(stale.join("tursodb_temp_file"), vec![0u8; 4096]).unwrap();
        age(&stale);

        // A fresh one, which a live query could still be writing.
        let fresh = dir.join(".tmpBBBBBB");
        std::fs::create_dir(&fresh).unwrap();
        std::fs::write(fresh.join("tursodb_temp_file"), vec![0u8; 16]).unwrap();

        let report = sweep(dir);
        assert_eq!(report.removed, 1, "only the stale entry is reclaimed");
        assert_eq!(report.kept, 1, "the fresh entry is left alone");
        assert_eq!(report.bytes, 4096, "the reclaimed bytes are reported");
        assert!(!stale.exists());
        assert!(fresh.exists());
    }

    #[test]
    fn sweeping_a_missing_directory_is_a_no_op() {
        let root = tempfile::tempdir().unwrap();
        let report = sweep(&root.path().join("absent"));
        assert_eq!(report, SweepReport::default());
    }
}
