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
    std::fs::create_dir_all(&dir).map_err(UpdateError::Lock)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))
        .map_err(UpdateError::Lock)?;
    match FileExt::try_lock(&file) {
        Ok(()) => Ok(UpdateLock { _file: file }),
        Err(fs4::TryLockError::WouldBlock) => Err(UpdateError::Locked),
        Err(fs4::TryLockError::Error(err)) => Err(UpdateError::Lock(err)),
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

/// The suffix of a file downloaded but not yet verified. Such a file is
/// hidden (it starts with a dot) and never has a name a printed command
/// points at.
const UNVERIFIED: &str = ".unverified";

/// The staging name of `name` in `dir`: `.<name>.unverified`.
fn staged(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(".{name}{UNVERIFIED}"))
}

/// Download `asset` of `release` with `SHA256SUMS` and its signature into
/// `dir`, and verify it, fail closed: the digest GitHub's API lists for the
/// asset (only when the API answered; an API answer without a digest for it
/// is refused before anything is downloaded), then the minisign signature
/// over `SHA256SUMS` with the trusted keys and the release's tag, then the
/// asset's line in those very signed bytes. One tag value, `release.tag`,
/// names every download and the signature, and every check runs on the
/// bytes that were written, never on a second read.
///
/// The three files are downloaded under staging names
/// (`.<name>.unverified`) and take their final names only once every check
/// passed, the asset last. So a run that is interrupted, killed or refused
/// never leaves unverified bytes under the name a printed command points
/// at, and never replaces a copy an earlier run verified. Staging files an
/// earlier run left are removed first; call it under the update lock.
///
/// On success the asset's path and the checks that ran.
pub async fn fetch_verified(
    release: &LatestRelease,
    asset: &str,
    dir: &Path,
) -> Result<(PathBuf, Checks), UpdateError> {
    if release.from_api && !release.digests.contains_key(asset) {
        return Err(UpdateError::NoDigest);
    }
    std::fs::create_dir_all(dir)?;
    remove_staging_leftovers(dir);
    let names = [asset, SUMS, SUMS_SIG];
    let staging = names.map(|name| staged(dir, name));
    let verified = fetch_and_verify(release, asset, &staging).await;
    let checks = match verified {
        Ok(checks) => checks,
        Err(err) => {
            remove_all(&staging);
            return Err(err);
        }
    };
    // The signature files first, the asset last: its final name appears
    // only once everything beside it is in place.
    for (from, name) in staging.iter().zip(names).rev() {
        if let Err(err) = std::fs::rename(from, dir.join(name)) {
            remove_all(&staging);
            return Err(err.into());
        }
    }
    Ok((dir.join(asset), checks))
}

fn remove_all(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

/// Remove the staging and partial files a run that was interrupted left in
/// `dir`: hidden files ending in `.unverified` or holding `.part-`.
fn remove_staging_leftovers(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') && (name.ends_with(UNVERIFIED) || name.contains(".part-")) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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
    fn a_lock_that_cannot_be_made_says_so() {
        let cache = tempfile::tempdir().unwrap();
        let file = cache.path().join("a-file");
        std::fs::write(&file, b"").unwrap();
        let err = take_lock(&file).expect_err("no folder under a file");
        assert!(matches!(err, UpdateError::Lock(_)), "{err:?}");
        assert!(
            err.to_string().starts_with("cannot take the update lock: "),
            "{err}"
        );
    }

    #[test]
    fn only_staging_leftovers_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            ".crystalline_0.34.0_amd64.deb.unverified",
            ".SHA256SUMS.unverified",
            ".crystalline_0.34.0_amd64.deb.part-123",
            "crystalline_0.34.0_amd64.deb",
            "SHA256SUMS",
            "unverified",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        remove_staging_leftovers(dir.path());
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            ["SHA256SUMS", "crystalline_0.34.0_amd64.deb", "unverified"]
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
