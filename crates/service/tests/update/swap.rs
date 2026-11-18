//! The tar.gz update end to end against the fake GitHub: a release whose
//! archive holds a script standing in for the new binary, signed with the
//! test key, installed over a binary in a temp install folder. Never the
//! real exe.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

use axum::body::Body;
use axum::http::{Response, StatusCode};
use crystalline_core::update::{Checks, InstallMethod, LatestRelease, archive_member, asset_name};
use crystalline_core::version::release_version;
use crystalline_service::update::swap::{NEW_PREFIX, finish_after_swap};
use crystalline_service::update::{Spawner, SystemSpawner, UpdateError, install_tarball};

use super::now::{files_in, sha256_hex, sign, trust_test_key};
use super::{ENV, Fake, use_fake};

const TAG: &str = "v0.34.0";

/// The stand-in for the new binary: it prints the release version for
/// `--version`, and records any other call in `finish-called` beside
/// itself.
const NEW_BINARY: &str = "#!/bin/sh\n\
    if [ \"$1\" = \"--version\" ]; then\n\
    \x20 echo \"crystalline $VERSION_TO_PRINT\"\n\
    \x20 echo \"Copyright (c) the test\"\n\
    \x20 exit 0\n\
    fi\n\
    echo \"$@\" > \"$(dirname \"$0\")/finish-called\"\n";

/// The release archive in the release workflow's shape: a folder named
/// after the release and platform, holding the binary and a readme.
fn tarball(member: &str, script: &str) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let folder = member.rsplit_once('/').unwrap().0;
    for (name, body, mode) in [
        (format!("{folder}/README.md"), "read me".as_bytes(), 0o644),
        (member.to_string(), script.as_bytes(), 0o755),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        builder.append_data(&mut header, name, body).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn bytes(status: StatusCode, body: Vec<u8>) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap()
}

/// Serve `archive` as `asset` with a `SHA256SUMS` that lists it, signed
/// for [`TAG`].
async fn serve(asset: &str, archive: Vec<u8>) -> (Fake, String) {
    let digest = format!("sha256:{}", sha256_hex(&archive));
    let sums = format!("{}  {asset}\n", sha256_hex(&archive));
    let sig = sign(sums.as_bytes(), &format!("tag:{TAG}"));
    let prefix = format!("/jordiboehme/crystalline/releases/download/{TAG}/");
    let asset = asset.to_string();
    let fake = Fake::start(move |_, path, _| match path.strip_prefix(&prefix) {
        Some(name) if name == asset => bytes(StatusCode::OK, archive.clone()),
        Some("SHA256SUMS") => bytes(StatusCode::OK, sums.clone().into_bytes()),
        Some("SHA256SUMS.minisig") => bytes(StatusCode::OK, sig.clone().into_bytes()),
        _ => bytes(StatusCode::NOT_FOUND, Vec::new()),
    })
    .await;
    use_fake(&fake);
    (fake, digest)
}

fn release(asset: &str, digest: String) -> LatestRelease {
    LatestRelease {
        tag: TAG.to_string(),
        version: release_version(TAG).unwrap(),
        published_at: None,
        digests: BTreeMap::from([(asset.to_string(), digest)]),
        from_api: true,
    }
}

/// The system spawner, except that `codesign` answers yes: a test script
/// carries no Developer ID signature. Every call is recorded.
struct RealButCodesign {
    calls: std::sync::Mutex<Vec<String>>,
}

impl RealButCodesign {
    fn new() -> Self {
        RealButCodesign {
            calls: std::sync::Mutex::default(),
        }
    }

    fn record(&self, program: &str, args: &[&str]) {
        let mut call = vec![program];
        call.extend_from_slice(args);
        self.calls.lock().unwrap().push(call.join(" "));
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Spawner for RealButCodesign {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<i32> {
        self.record(program, args);
        SystemSpawner.run(program, args)
    }

    fn output(&self, program: &str, args: &[&str]) -> std::io::Result<String> {
        self.record(program, args);
        if program == "/usr/bin/codesign" {
            return Ok(String::new());
        }
        SystemSpawner.output(program, args)
    }
}

/// An install folder holding the old binary.
fn install_folder() -> (tempfile::TempDir, std::path::PathBuf) {
    let install = tempfile::tempdir().unwrap();
    let exe = install.path().join("crystalline");
    std::fs::write(&exe, b"the old binary").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    (install, exe)
}

fn script_printing(version: &str) -> String {
    NEW_BINARY.replace("$VERSION_TO_PRINT", version)
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn update_now_tarball_end_to_end() {
    let _env = ENV.lock().await;
    let version = release_version(TAG).unwrap();
    let asset = asset_name(InstallMethod::Tarball, &version).expect("a tar.gz platform");
    let member = archive_member(&version).unwrap();
    let script = script_printing("0.34.0");
    let (_fake, digest) = serve(&asset, tarball(&member, &script)).await;
    let _key = trust_test_key(true);
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("updates").join(TAG);
    let (install, exe) = install_folder();
    // A leftover of an interrupted run is cleared.
    std::fs::write(install.path().join(format!("{NEW_PREFIX}1")), b"old run").unwrap();

    let spawner = RealButCodesign::new();
    let macos = cfg!(target_os = "macos");
    let checks = install_tarball(
        &spawner,
        &release(&asset, digest),
        &asset,
        &member,
        &dir,
        &exe,
        macos,
    )
    .await
    .expect("installed");

    assert_eq!(
        checks,
        Checks {
            digest: true,
            sums: true,
            signature: true,
            codesign: macos,
            version: true,
        }
    );
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), script);
    assert_eq!(
        std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
        0o755,
        "the old exe's mode"
    );
    assert_eq!(files_in(install.path()), ["crystalline"], "nothing else");
    let new = format!(
        "{}/{NEW_PREFIX}{}",
        install.path().display(),
        std::process::id()
    );
    let mut expected = Vec::new();
    if macos {
        expected.push(format!(
            "/usr/bin/codesign --verify --strict -R={} {new}",
            crystalline_service::update::codesign::codesign_requirement()
        ));
    }
    expected.push(format!("{new} --version"));
    assert_eq!(spawner.calls(), expected, "checked before the swap");

    // Then the old binary hands over to the new one.
    let code = finish_after_swap(&spawner, &exe).unwrap();
    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read_to_string(install.path().join("finish-called")).unwrap(),
        "update finish\n",
        "the new binary ran update finish"
    );
    assert_eq!(
        spawner.calls().last().unwrap(),
        &format!("{} update finish", exe.display())
    );
}

/// The new binary prints another version: nothing is replaced and the
/// extracted file is gone.
#[cfg(debug_assertions)]
#[tokio::test]
async fn a_new_binary_of_another_version_is_not_installed() {
    let _env = ENV.lock().await;
    let version = release_version(TAG).unwrap();
    let asset = asset_name(InstallMethod::Tarball, &version).unwrap();
    let member = archive_member(&version).unwrap();
    let (_fake, digest) = serve(&asset, tarball(&member, &script_printing("0.33.9"))).await;
    let _key = trust_test_key(true);
    let cache = tempfile::tempdir().unwrap();
    let (install, exe) = install_folder();
    let err = install_tarball(
        &RealButCodesign::new(),
        &release(&asset, digest),
        &asset,
        &member,
        &cache.path().join(TAG),
        &exe,
        cfg!(target_os = "macos"),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, UpdateError::VersionMismatch), "{err}");
    assert_eq!(std::fs::read(&exe).unwrap(), b"the old binary");
    assert_eq!(files_in(install.path()), ["crystalline"]);
}

/// The archive does not hold the member: refused after the checks of the
/// download, with nothing written beside the exe.
#[cfg(debug_assertions)]
#[tokio::test]
async fn an_archive_without_the_binary_is_refused() {
    let _env = ENV.lock().await;
    let version = release_version(TAG).unwrap();
    let asset = asset_name(InstallMethod::Tarball, &version).unwrap();
    let member = archive_member(&version).unwrap();
    let (_fake, digest) = serve(
        &asset,
        tarball("crystalline-v0.34.0-elsewhere/crystalline", "#!/bin/sh\n"),
    )
    .await;
    let _key = trust_test_key(true);
    let cache = tempfile::tempdir().unwrap();
    let (install, exe) = install_folder();
    let spawner = RealButCodesign::new();
    let err = install_tarball(
        &spawner,
        &release(&asset, digest),
        &asset,
        &member,
        &cache.path().join(TAG),
        &exe,
        true,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, UpdateError::NoMember), "{err}");
    assert!(spawner.calls().is_empty(), "nothing was run");
    assert_eq!(std::fs::read(&exe).unwrap(), b"the old binary");
    assert_eq!(files_in(install.path()), ["crystalline"]);
}
