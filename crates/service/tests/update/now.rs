//! `update now`'s download and verification against the fake GitHub: a
//! release whose `SHA256SUMS` is signed with the test key, trusted through
//! the debug-only key seam. Every refusal leaves nothing in the folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use crystalline_core::update::{Checks, LatestRelease};
use crystalline_core::version::release_version;
use crystalline_service::update::{UpdateError, fetch_verified};
use sha2::{Digest, Sha256};

use super::{ENV, Fake, respond, use_fake};

const TAG: &str = "v0.34.0";
const ASSET: &str = "crystalline_0.34.0_amd64.deb";
const BYTES: &str = "the release package";

pub(super) fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../core/tests/fixtures/update")
        .join(name)
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `SHA256SUMS` in the release workflow's shape, listing `asset` with the
/// hash of `bytes`.
fn sums_for(asset: &str, bytes: &[u8]) -> String {
    format!(
        "{}  crystalline-v0.34.0-linux-amd64.tar.gz\n{}  {asset}\n",
        sha256_hex(b"another asset"),
        sha256_hex(bytes),
    )
}

/// `data` signed with the fixture test key and `trusted_comment`.
pub(super) fn sign(data: &[u8], trusted_comment: &str) -> String {
    let secret = std::fs::read_to_string(fixture("test.key")).unwrap();
    let sk = minisign::SecretKeyBox::from_string(&secret)
        .unwrap()
        .into_unencrypted_secret_key()
        .unwrap();
    minisign::sign(None, &sk, data, Some(trusted_comment), None)
        .unwrap()
        .into_string()
}

/// What the fake serves under the release's download folder.
#[derive(Clone)]
struct Served {
    asset: String,
    sums: String,
    sig: String,
}

impl Served {
    fn good() -> Served {
        let sums = sums_for(ASSET, BYTES.as_bytes());
        Served {
            asset: BYTES.to_string(),
            sig: sign(sums.as_bytes(), &format!("tag:{TAG}")),
            sums,
        }
    }
}

async fn serve(served: Served) -> Fake {
    let prefix = format!("/jordiboehme/crystalline/releases/download/{TAG}/");
    let fake = Fake::start(move |_, path, _| match path.strip_prefix(&prefix) {
        Some(ASSET) => respond(StatusCode::OK, &[], &served.asset),
        Some("SHA256SUMS") => respond(StatusCode::OK, &[], &served.sums),
        Some("SHA256SUMS.minisig") => respond(StatusCode::OK, &[], &served.sig),
        _ => respond(StatusCode::NOT_FOUND, &[], ""),
    })
    .await;
    use_fake(&fake);
    fake
}

/// The release as the lookup found it: from the API with `digest` listed
/// for the asset, or from the fallback with no digests.
fn release(digest: Option<String>, from_api: bool) -> LatestRelease {
    LatestRelease {
        tag: TAG.to_string(),
        version: release_version(TAG).unwrap(),
        published_at: None,
        digests: digest
            .map(|digest| BTreeMap::from([(ASSET.to_string(), digest)]))
            .unwrap_or_default(),
        from_api,
    }
}

fn good_digest() -> Option<String> {
    Some(format!("sha256:{}", sha256_hex(BYTES.as_bytes())))
}

/// The test key trusted through the debug-only seam while this lives
/// (with `on`), and never after: dropped also when a test panics, so no
/// later test in the same process trusts it by accident.
pub(super) struct TestKey;

pub(super) fn trust_test_key(on: bool) -> TestKey {
    // SAFETY: every test that touches the environment holds `ENV`.
    unsafe {
        if on {
            std::env::set_var("CRYSTALLINE_TEST_UPDATE_KEY", fixture("test.pub"));
        } else {
            std::env::remove_var("CRYSTALLINE_TEST_UPDATE_KEY");
        }
    }
    TestKey
}

impl Drop for TestKey {
    fn drop(&mut self) {
        // SAFETY: as above; the guard is dropped while `ENV` is still held.
        unsafe { std::env::remove_var("CRYSTALLINE_TEST_UPDATE_KEY") };
    }
}

pub(super) fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn a_signed_release_verifies_with_every_check() {
    let _env = ENV.lock().await;
    let fake = serve(Served::good()).await;
    let _key = trust_test_key(true);
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("updates").join(TAG);

    let (path, checks) = fetch_verified(&release(good_digest(), true), ASSET, &dir)
        .await
        .expect("verified");

    assert_eq!(path, dir.join(ASSET));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), BYTES);
    assert_eq!(
        checks,
        Checks {
            digest: true,
            sums: true,
            signature: true,
            codesign: false,
            version: false,
        }
    );
    assert_eq!(files_in(&dir), ["SHA256SUMS", "SHA256SUMS.minisig", ASSET]);
    // One tag value names every download.
    let paths: Vec<String> = fake.seen().into_iter().map(|seen| seen.path).collect();
    let base = format!("/jordiboehme/crystalline/releases/download/{TAG}");
    assert_eq!(
        paths,
        [
            format!("{base}/{ASSET}"),
            format!("{base}/SHA256SUMS"),
            format!("{base}/SHA256SUMS.minisig"),
        ]
    );
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn the_fallback_has_no_digest_to_check() {
    let _env = ENV.lock().await;
    let _fake = serve(Served::good()).await;
    let _key = trust_test_key(true);
    let dir = tempfile::tempdir().unwrap();
    let (_, checks) = fetch_verified(&release(None, false), ASSET, dir.path())
        .await
        .expect("verified without a digest");
    assert!(!checks.digest, "only the API carries a digest");
    assert!(checks.sums && checks.signature);
}

/// Each way a download is refused, and that nothing is left behind.
#[cfg(debug_assertions)]
#[tokio::test]
async fn a_download_that_fails_a_check_is_removed() {
    let _env = ENV.lock().await;
    let good = Served::good();
    let tampered = Served {
        asset: "a changed package".to_string(),
        ..good.clone()
    };
    let other_tag = Served {
        sig: sign(good.sums.as_bytes(), "tag:v0.33.0"),
        ..good.clone()
    };
    let unlisted = {
        let sums = sums_for("crystalline_0.34.0_arm64.deb", BYTES.as_bytes());
        Served {
            sig: sign(sums.as_bytes(), &format!("tag:{TAG}")),
            sums,
            ..good.clone()
        }
    };
    let cases: Vec<(&str, Served, LatestRelease, bool, &str)> = vec![
        (
            "wrong digest",
            good.clone(),
            release(Some(format!("sha256:{}", "0".repeat(64))), true),
            true,
            "verification failed: the download does not match the sha256 digest GitHub lists for the asset",
        ),
        (
            "a changed asset",
            tampered,
            release(None, false),
            true,
            "verification failed: the download does not match its line in SHA256SUMS",
        ),
        (
            "a signature for another tag",
            other_tag,
            release(good_digest(), true),
            true,
            "verification failed: the signature over SHA256SUMS is for \"tag:v0.33.0\", not for tag v0.34.0",
        ),
        (
            "no line for the asset",
            unlisted,
            release(good_digest(), true),
            true,
            "verification failed: SHA256SUMS has no line for the downloaded asset",
        ),
        (
            "a key this build does not trust",
            good.clone(),
            release(good_digest(), true),
            false,
            "verification failed: SHA256SUMS is signed by a key this build does not trust",
        ),
    ];
    for (name, served, release, trusted, message) in cases {
        let _fake = serve(served).await;
        let key = trust_test_key(trusted);
        let dir = tempfile::tempdir().unwrap();
        let err = fetch_verified(&release, ASSET, dir.path())
            .await
            .expect_err(name);
        drop(key);
        assert_eq!(err.to_string(), message, "{name}");
        assert!(
            files_in(dir.path()).is_empty(),
            "{name}: left {:?}",
            files_in(dir.path())
        );
    }
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn a_missing_signature_is_an_error_and_leaves_nothing() {
    let _env = ENV.lock().await;
    let served = Served::good();
    let fake = Fake::start(move |_, path, _| {
        if path.ends_with("SHA256SUMS.minisig") {
            respond(StatusCode::NOT_FOUND, &[], "")
        } else if path.ends_with("SHA256SUMS") {
            respond(StatusCode::OK, &[], &served.sums)
        } else {
            respond(StatusCode::OK, &[], &served.asset)
        }
    })
    .await;
    use_fake(&fake);
    let dir = tempfile::tempdir().unwrap();
    let err = fetch_verified(&release(good_digest(), true), ASSET, dir.path())
        .await
        .expect_err("no signature");
    assert!(matches!(err, UpdateError::Status(404)), "{err:?}");
    assert!(files_in(dir.path()).is_empty());
}

/// An API answer without a digest for the asset is refused before any
/// download.
#[cfg(debug_assertions)]
#[tokio::test]
async fn no_digest_from_the_api_downloads_nothing() {
    let _env = ENV.lock().await;
    let fake = serve(Served::good()).await;
    let _key = trust_test_key(true);
    let dir = tempfile::tempdir().unwrap();
    let err = fetch_verified(&release(None, true), ASSET, dir.path())
        .await
        .expect_err("no digest");
    assert_eq!(
        err.to_string(),
        "GitHub lists no sha256 digest for the download"
    );
    assert!(fake.seen().is_empty(), "{:?}", fake.seen());
    assert!(files_in(dir.path()).is_empty());
}

/// A refused download never replaces a copy an earlier run verified.
#[cfg(debug_assertions)]
#[tokio::test]
async fn a_refused_run_keeps_an_earlier_verified_copy() {
    let _env = ENV.lock().await;
    let good = Served::good();
    let _fake = serve(Served {
        asset: "a changed package".to_string(),
        ..good.clone()
    })
    .await;
    let _key = trust_test_key(true);
    let dir = tempfile::tempdir().unwrap();
    for (name, bytes) in [
        (ASSET, BYTES),
        ("SHA256SUMS", good.sums.as_str()),
        ("SHA256SUMS.minisig", good.sig.as_str()),
    ] {
        std::fs::write(dir.path().join(name), bytes).unwrap();
    }
    fetch_verified(&release(None, false), ASSET, dir.path())
        .await
        .expect_err("a changed asset");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(ASSET)).unwrap(),
        BYTES
    );
    assert_eq!(
        files_in(dir.path()),
        ["SHA256SUMS", "SHA256SUMS.minisig", ASSET]
    );
}

/// A run interrupted between the download and the verification (here: the
/// signature never arrives and the run is dropped) leaves the downloaded
/// bytes only under a staging name; an earlier verified copy keeps its
/// name and bytes, and the next run clears the leftovers.
#[cfg(debug_assertions)]
#[tokio::test]
async fn an_interrupted_run_leaves_nothing_under_the_final_name() {
    use axum::body::Body;
    use axum::http::Response;

    let _env = ENV.lock().await;
    let good = Served::good();
    let sums = good.sums.clone();
    let fake = Fake::start(move |_, path, _| {
        if path.ends_with("/SHA256SUMS.minisig") {
            // Headers, then a body that never ends.
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from_stream(futures::stream::pending::<
                    Result<String, std::io::Error>,
                >()))
                .unwrap()
        } else if path.ends_with("/SHA256SUMS") {
            respond(StatusCode::OK, &[], &sums)
        } else {
            respond(StatusCode::OK, &[], "unverified bytes")
        }
    })
    .await;
    use_fake(&fake);
    let _key = trust_test_key(true);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(ASSET), BYTES).unwrap();

    let fallback = release(None, false);
    let run = fetch_verified(&fallback, ASSET, dir.path());
    let interrupted = tokio::time::timeout(std::time::Duration::from_secs(1), run).await;
    assert!(interrupted.is_err(), "the run was still waiting");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(ASSET)).unwrap(),
        BYTES,
        "the earlier verified copy is untouched"
    );
    let staged = format!(".{ASSET}.unverified");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(&staged)).unwrap(),
        "unverified bytes",
        "the download sits under its staging name only"
    );
    assert!(!files_in(dir.path()).contains(&"SHA256SUMS".to_string()));

    // The next run removes the leftovers and puts the verified files in place.
    let _fake = serve(good).await;
    fetch_verified(&release(None, false), ASSET, dir.path())
        .await
        .expect("verified");
    assert_eq!(
        files_in(dir.path()),
        ["SHA256SUMS", "SHA256SUMS.minisig", ASSET]
    );
}
