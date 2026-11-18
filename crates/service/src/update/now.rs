//! `crystalline update now`'s service side: the update lock, the download
//! folder and the download of a release asset with its full verification.
//!
//! The download folder is `<cache_dir>/updates/<tag>/`, persistent and
//! never a system temp folder; older tag folders are removed on the next
//! run (S12). Every run, whatever the install method, holds the lock file
//! `<cache_dir>/updates/.lock` (U6), so two runs never download into the
//! same folder or act at the same time.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crystalline_core::update::{
    Checks, LatestRelease, Sha256Digest, verify_digest, verify_signature,
};
use crystalline_core::version::release_version;
use fs4::FileExt;

use super::client::{UpdateError, download};

/// The folder under the cache folder that holds the lock and the downloads.
const UPDATES_DIR: &str = "updates";

/// The lock file's name in [`UPDATES_DIR`].
const LOCK_FILE: &str = ".lock";

/// The checksum list every release publishes.
const SUMS: &str = "SHA256SUMS";

/// Its minisign signature.
const SUMS_SIG: &str = "SHA256SUMS.minisig";

/// The update lock: held while the value lives. The file stays; only the
/// lock on its open handle is released, so no run can remove a lock another
/// run just took.
#[derive(Debug)]
pub struct UpdateLock {
    _file: File,
}

/// Take the update lock, `<cache_dir>/updates/.lock`, creating the folder
/// when it is missing. [`UpdateError::Locked`] when another run holds it.
/// The lock belongs to the open handle (`flock` on Unix, `LockFileEx` on
/// Windows), so the system releases it when a run dies: nothing is ever
/// left behind to time out.
pub fn take_lock(cache_dir: &Path) -> Result<UpdateLock, UpdateError> {
    let dir = cache_dir.join(UPDATES_DIR);
    std::fs::create_dir_all(&dir)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))?;
    match FileExt::try_lock(&file) {
        Ok(()) => Ok(UpdateLock { _file: file }),
        Err(fs4::TryLockError::WouldBlock) => Err(UpdateError::Locked),
        Err(fs4::TryLockError::Error(err)) => Err(err.into()),
    }
}

/// The download folder of a release, `<cache_dir>/updates/<tag>/`.
pub fn download_dir(cache_dir: &Path, tag: &str) -> PathBuf {
    cache_dir.join(UPDATES_DIR).join(tag)
}

/// Remove every release folder under `<cache_dir>/updates/` but `keep`'s:
/// only folders named by an exact release tag (`v<x.y.z>`) are touched, so
/// the lock file and anything else there stays. Call it under the lock. A
/// folder that cannot be removed is left for the next run.
pub fn remove_older_tags(cache_dir: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(cache_dir.join(UPDATES_DIR)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == keep || release_version(name).is_none() {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Download `asset` of `release` with `SHA256SUMS` and its signature into
/// `dir`, and verify it, fail closed, in the spec's order: the digest
/// GitHub's API lists for the asset (only when the API answered; an API
/// answer without a digest for it is refused), then the minisign signature
/// over `SHA256SUMS` with the trusted keys and the release's tag, then the
/// asset's line in those very signed bytes. One tag value, `release.tag`,
/// names every download and the signature, and every check runs on the
/// bytes that were written, never on a second read.
///
/// On success the asset's path and the checks that ran; on any failure the
/// three files are removed, so nothing unverified is left in `dir`.
pub async fn fetch_verified(
    release: &LatestRelease,
    asset: &str,
    dir: &Path,
) -> Result<(PathBuf, Checks), UpdateError> {
    std::fs::create_dir_all(dir)?;
    let paths = [dir.join(asset), dir.join(SUMS), dir.join(SUMS_SIG)];
    let verified = fetch_and_verify(release, asset, &paths).await;
    if verified.is_err() {
        for path in &paths {
            let _ = std::fs::remove_file(path);
        }
    }
    verified.map(|checks| (paths[0].clone(), checks))
}

async fn fetch_and_verify(
    release: &LatestRelease,
    asset: &str,
    [asset_path, sums_path, sig_path]: &[PathBuf; 3],
) -> Result<Checks, UpdateError> {
    let tag = release.tag.as_str();
    let bytes = download(tag, asset, asset_path).await?;
    let sums = download(tag, SUMS, sums_path).await?;
    let sig = download(tag, SUMS_SIG, sig_path).await?;
    let digest = Sha256Digest::of(&bytes);
    let mut checks = Checks::default();
    if release.from_api {
        let listed = release.digests.get(asset).ok_or(UpdateError::NoDigest)?;
        verify_digest(&digest, listed).map_err(UpdateError::Verify)?;
        checks.digest = true;
    }
    let sig = std::str::from_utf8(&sig)
        .map_err(|_| UpdateError::Verify(crystalline_core::update::VerifyError::BadSignature))?;
    let keys = crystalline_core::update::trusted_keys()?;
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let signed = verify_signature(&sums, sig, &keys, tag).map_err(UpdateError::Verify)?;
    checks.signature = true;
    signed.line(asset, &digest).map_err(UpdateError::Verify)?;
    checks.sums = true;
    Ok(checks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_run_finds_the_lock_taken() {
        let cache = tempfile::tempdir().unwrap();
        let first = take_lock(cache.path()).expect("free");
        let second = take_lock(cache.path()).expect_err("held");
        assert!(matches!(second, UpdateError::Locked), "{second:?}");
        assert_eq!(second.to_string(), "another update is running");
        drop(first);
        let again = take_lock(cache.path()).expect("free again once dropped");
        drop(again);
        assert!(
            cache.path().join("updates/.lock").is_file(),
            "the lock file stays"
        );
    }

    #[test]
    fn the_download_folder_is_per_tag_under_the_cache() {
        let cache = Path::new("/c");
        assert_eq!(
            download_dir(cache, "v0.34.0"),
            Path::new("/c/updates/v0.34.0")
        );
    }

    #[test]
    fn older_tag_folders_are_removed() {
        let cache = tempfile::tempdir().unwrap();
        let updates = cache.path().join("updates");
        for tag in ["v0.33.0", "v0.34.0", "v0.35.0"] {
            std::fs::create_dir_all(updates.join(tag)).unwrap();
            std::fs::write(updates.join(tag).join("asset"), b"x").unwrap();
        }
        // Neither a file nor a folder that is not a release tag is touched.
        std::fs::create_dir_all(updates.join("keep-me")).unwrap();
        std::fs::create_dir_all(updates.join("v0.36.0-rc.1")).unwrap();
        std::fs::write(updates.join("v0.32.0"), b"a file").unwrap();
        let _lock = take_lock(cache.path()).unwrap();

        remove_older_tags(cache.path(), "v0.35.0");

        let mut left: Vec<String> = std::fs::read_dir(&updates)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [".lock", "keep-me", "v0.32.0", "v0.35.0", "v0.36.0-rc.1"]
        );
        assert!(updates.join("v0.35.0/asset").is_file(), "kept whole");
        // No updates folder at all is no error.
        remove_older_tags(&cache.path().join("missing"), "v0.35.0");
    }
}
