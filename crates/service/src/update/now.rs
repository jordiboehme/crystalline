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

use super::client::{UpdateError, download_base, download_from};

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
/// On success the asset's path, the bytes that were checked and the checks
/// that ran ([`Verified`]).
pub async fn fetch_verified(
    release: &LatestRelease,
    asset: &str,
    dir: &Path,
) -> Result<Verified, UpdateError> {
    fetch_verified_from(&ReleaseSource::here(), release, asset, dir).await
}

/// A download that passed every check of [`fetch_verified`].
#[derive(Debug)]
pub struct Verified {
    /// Where the asset now lies under its final name, for a command the
    /// person runs (`apt install`, the MSI steps).
    pub path: PathBuf,
    /// The very bytes the checks ran on. Whatever this process takes out
    /// of the asset, it takes from these, never from `path` again: the
    /// file can be replaced between the check and a second read (a temp
    /// folder the person owns under `sudo -E`), the bytes in memory
    /// cannot.
    pub bytes: Vec<u8>,
    /// The checks that ran.
    pub checks: Checks,
}

/// Where [`fetch_verified_from`] downloads from and which keys it trusts.
/// A release build has only [`ReleaseSource::here`]: GitHub and the
/// compiled-in keys. A debug build's test can name a local server and an
/// extra key without touching the process environment (S11).
#[derive(Debug, Clone, Default)]
pub struct ReleaseSource {
    #[cfg(debug_assertions)]
    test: Option<(String, String)>,
}

impl ReleaseSource {
    /// GitHub and the trusted keys (in a debug build also the
    /// environment seams).
    pub fn here() -> ReleaseSource {
        ReleaseSource::default()
    }

    /// A debug build's test source: the web root `base` and the extra
    /// trusted public key `key` (the text of a `.pub` file).
    #[cfg(debug_assertions)]
    pub fn for_test(base: &str, key: &str) -> ReleaseSource {
        ReleaseSource {
            test: Some((base.trim_end_matches('/').to_string(), key.to_string())),
        }
    }

    fn base(&self) -> String {
        #[cfg(debug_assertions)]
        if let Some((base, _)) = &self.test {
            return base.clone();
        }
        download_base()
    }

    fn keys(&self) -> std::io::Result<Vec<String>> {
        #[allow(unused_mut)]
        let mut keys = crystalline_core::update::trusted_keys()?;
        #[cfg(debug_assertions)]
        if let Some((_, key)) = &self.test {
            keys.push(key.clone());
        }
        Ok(keys)
    }
}

/// [`fetch_verified`] from `source`.
pub async fn fetch_verified_from(
    source: &ReleaseSource,
    release: &LatestRelease,
    asset: &str,
    dir: &Path,
) -> Result<Verified, UpdateError> {
    if release.from_api && !release.digests.contains_key(asset) {
        return Err(UpdateError::NoDigest);
    }
    std::fs::create_dir_all(dir)?;
    remove_staging_leftovers(dir);
    let names = [asset, SUMS, SUMS_SIG];
    let staging = names.map(|name| staged(dir, name));
    let verified = fetch_and_verify(source, release, asset, &staging).await;
    let (checks, bytes) = match verified {
        Ok(verified) => verified,
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
    Ok(Verified {
        path: dir.join(asset),
        bytes,
        checks,
    })
}

/// Extract exactly `member` (`crystalline.exe`) of the verified zip
/// `archive` (its bytes, [`Verified::bytes`]) into `dir`, under that name,
/// and hand back its path. Nothing
/// else of the archive is ever written: every other entry is read past, and
/// the name is matched byte for byte, so a `../` path or a folder prefix
/// never matches. The member must be one plain file and appear once; a
/// folder, a link, a second copy or no copy at all is
/// [`UpdateError::NoMember`], and so is an archive that repeats any name.
/// It is written under a staging name
/// (`.<member>.part-<pid>`) and takes its final name only when complete, so
/// the name the printed steps copy from never holds a partial file. Call it
/// under the update lock.
pub fn extract_zip_member(
    archive: &[u8],
    member: &str,
    dir: &Path,
) -> Result<PathBuf, UpdateError> {
    let staging = dir.join(format!(".{member}.part-{}", std::process::id()));
    let _ = std::fs::remove_file(&staging);
    let result = extract_zip_into(archive, member, &staging);
    if let Err(err) = result {
        let _ = std::fs::remove_file(&staging);
        return Err(err);
    }
    let target = dir.join(member);
    if let Err(err) = std::fs::rename(&staging, &target) {
        let _ = std::fs::remove_file(&staging);
        return Err(UpdateError::Extract(err));
    }
    Ok(target)
}

fn extract_zip_into(archive: &[u8], member: &str, staging: &Path) -> Result<(), UpdateError> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .map_err(|err| UpdateError::Extract(std::io::Error::other(err.to_string())))?;
    // The reader keeps one entry per name, so a second copy is seen only
    // here: an archive with any repeated name is refused.
    if zip.has_duplicate_names() {
        return Err(UpdateError::NoMember);
    }
    let mut found = None;
    for index in 0..zip.len() {
        let entry = zip
            .by_index_raw(index)
            .map_err(|err| UpdateError::Extract(std::io::Error::other(err.to_string())))?;
        if entry.name_raw() != member.as_bytes() {
            continue;
        }
        if found.is_some() || !entry.is_file() {
            return Err(UpdateError::NoMember);
        }
        found = Some(index);
    }
    let index = found.ok_or(UpdateError::NoMember)?;
    let mut entry = zip
        .by_index(index)
        .map_err(|err| UpdateError::Extract(std::io::Error::other(err.to_string())))?;
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging)
        .map_err(UpdateError::Extract)?;
    let copied = std::io::copy(
        &mut std::io::Read::take(&mut entry, super::swap::MAX_BINARY_BYTES + 1),
        &mut out,
    )
    .map_err(UpdateError::Extract)?;
    if copied > super::swap::MAX_BINARY_BYTES {
        return Err(UpdateError::Extract(std::io::Error::other(
            "the binary is larger than 1 GiB",
        )));
    }
    out.sync_all().map_err(UpdateError::Extract)?;
    Ok(())
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
    source: &ReleaseSource,
    release: &LatestRelease,
    asset: &str,
    [asset_path, sums_path, sig_path]: &[PathBuf; 3],
) -> Result<(Checks, Vec<u8>), UpdateError> {
    let tag = release.tag.as_str();
    let base = source.base();
    let bytes = download_from(&base, tag, asset, asset_path).await?;
    let sums = download_from(&base, tag, SUMS, sums_path).await?;
    let sig = download_from(&base, tag, SUMS_SIG, sig_path).await?;
    let digest = Sha256Digest::of(&bytes);
    let mut checks = Checks::default();
    if release.from_api {
        let listed = release.digests.get(asset).ok_or(UpdateError::NoDigest)?;
        verify_digest(&digest, listed).map_err(UpdateError::Verify)?;
        checks.digest = true;
    }
    let sig = std::str::from_utf8(&sig)
        .map_err(|_| UpdateError::Verify(crystalline_core::update::VerifyError::BadSignature))?;
    let keys = source.keys()?;
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let signed = verify_signature(&sums, sig, &keys, tag).map_err(UpdateError::Verify)?;
    checks.signature = true;
    signed.line(asset, &digest).map_err(UpdateError::Verify)?;
    checks.sums = true;
    Ok((checks, bytes))
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

    /// A zip of `entries`: (name, body); a name ending in `/` is a folder.
    fn zip_of(path: &Path, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            if name.ends_with('/') {
                writer.add_directory(*name, options).unwrap();
            } else {
                writer.start_file(*name, options).unwrap();
                writer.write_all(body).unwrap();
            }
        }
        writer.finish().unwrap();
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn only_the_exe_is_extracted_from_the_zip() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("crystalline-v0.34.0-windows-amd64.zip");
        zip_of(
            &archive,
            &[
                ("../crystalline.exe", b"escape"),
                ("sub/crystalline.exe", b"nested"),
                ("README.md", b"readme"),
                ("crystalline.exe", b"the new binary"),
                ("LICENSE", b"license"),
            ],
        );
        let path = extract_zip_member(
            &std::fs::read(&archive).unwrap(),
            "crystalline.exe",
            dir.path(),
        )
        .unwrap();
        assert_eq!(path, dir.path().join("crystalline.exe"));
        assert_eq!(std::fs::read(&path).unwrap(), b"the new binary");
        assert_eq!(
            names_in(dir.path()),
            ["crystalline-v0.34.0-windows-amd64.zip", "crystalline.exe"],
            "nothing else is written, no staging file stays"
        );
        assert!(
            !dir.path()
                .parent()
                .unwrap()
                .join("crystalline.exe")
                .exists()
        );
    }

    #[test]
    fn a_zip_without_one_plain_exe_is_refused_and_leaves_nothing() {
        let cases: [&[(&str, &[u8])]; 3] = [
            &[("README.md", b"readme")],
            &[("crystalline.exe", b"one"), ("crystalline.exe", b"two")],
            &[("crystalline.exe/", b"")],
        ];
        for entries in cases {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join("a.zip");
            // The writer refuses a duplicate name, so that archive is
            // written by hand from two single-entry ones.
            if entries.len() == 2 {
                duplicate_zip(&archive);
            } else {
                zip_of(&archive, entries);
            }
            let err = extract_zip_member(
                &std::fs::read(&archive).unwrap(),
                "crystalline.exe",
                dir.path(),
            )
            .expect_err("refused");
            assert!(matches!(err, UpdateError::NoMember), "{err:?}");
            assert_eq!(names_in(dir.path()), ["a.zip"], "{entries:?}");
        }
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("not.zip");
        std::fs::write(&archive, b"not a zip").unwrap();
        let err = extract_zip_member(
            &std::fs::read(&archive).unwrap(),
            "crystalline.exe",
            dir.path(),
        )
        .expect_err("bad");
        assert!(matches!(err, UpdateError::Extract(_)), "{err:?}");
        assert_eq!(names_in(dir.path()), ["not.zip"]);
    }

    /// A zip holding `crystalline.exe` twice.
    fn duplicate_zip(path: &Path) {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("crystalline.exe", options).unwrap();
        writer.write_all(b"one").unwrap();
        writer.start_file("crystalline.exf", options).unwrap();
        writer.write_all(b"two").unwrap();
        writer.finish().unwrap();
        // Same length, so every offset stays: rename the second entry in
        // both its local header and its central directory record.
        let bytes = std::fs::read(path).unwrap();
        let from = b"crystalline.exf";
        let to = b"crystalline.exe";
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i..].starts_with(from) {
                out.extend_from_slice(to);
                i += from.len();
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn older_tag_folders_go_and_everything_else_stays() {
        let cache = tempfile::tempdir().unwrap();
        let updates = cache.path().join(UPDATES_DIR);
        for tag in ["v0.33.0", "v0.34.0", "v0.34.1"] {
            std::fs::create_dir_all(updates.join(tag)).unwrap();
            std::fs::write(updates.join(tag).join("steps.txt"), b"x").unwrap();
        }
        std::fs::create_dir_all(updates.join("notes")).unwrap();
        std::fs::write(updates.join("v0.32.0"), b"a file, not a folder").unwrap();
        std::fs::write(updates.join(LOCK_FILE), b"").unwrap();
        remove_older_tags(cache.path(), "v0.34.0");
        let mut left: Vec<String> = std::fs::read_dir(&updates)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, [".lock", "notes", "v0.32.0", "v0.34.0"]);
        assert!(updates.join("v0.34.0/steps.txt").is_file());
        // No updates folder at all is nothing to do.
        remove_older_tags(&cache.path().join("missing"), "v0.34.0");
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
