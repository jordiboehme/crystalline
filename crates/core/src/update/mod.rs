//! The pure parts of `crystalline update`: how this binary was installed,
//! which release asset fits it, how a download is verified, the check's
//! cache, the release-age gate and the reading of GitHub's release answers.
//! Nothing here touches the network, spawns a process or needs a runtime; the
//! caller hands in the canonical exe path and an environment lookup. The one
//! exception is the debug-only key seam behind [`trusted_keys`].

pub mod cache;
pub mod facts;
pub mod install;
pub mod offer;
pub mod release;
pub mod verify;

pub use cache::{
    CheckOutcome, FAILED_REASON, Gate, KnownRelease, UPDATE_CACHE_FILE, UPDATE_CACHE_FORMAT,
    UpdateCache, age_gate,
};
pub use facts::{CheckState, OffReason, UpdateFacts};
pub use install::{
    CHANNEL_ENV, DetectedInstall, Detection, InstallMethod, MARKER_FILE, Refusal, archive_member,
    asset_name, detect, display_path, official_build, release_platform,
};
pub use offer::{offer, offer_stale_after};
pub use release::{LatestRelease, Lookup};
pub use verify::{Checks, Sha256Digest, SignedSums, VerifyError, verify_digest, verify_signature};

/// The minisign public keys a release's `SHA256SUMS` signature is checked
/// with, each the full text of its `.pub` file: the online key (id
/// AC457F93DB463C6F), which the release job signs with, and the offline key
/// (id 3E3AEE836F215E0C), kept for rotation. A signature by either verifies.
pub const RELEASE_KEYS: [&str; 2] = [
    include_str!("keys/online.pub"),
    include_str!("keys/offline.pub"),
];

/// The public keys an update signature is checked with: [`RELEASE_KEYS`]
/// and, in a debug build only, the key in the `.pub` file
/// `CRYSTALLINE_TEST_UPDATE_KEY` names, so a test can sign with a throwaway
/// key. A release build has no such seam: the environment never chooses a
/// trusted key there (S11). `Err` only when that file cannot be read.
pub fn trusted_keys() -> std::io::Result<Vec<String>> {
    #[allow(unused_mut)]
    let mut keys: Vec<String> = RELEASE_KEYS.iter().map(|key| key.to_string()).collect();
    #[cfg(debug_assertions)]
    if let Some(path) = test_update_key() {
        let path = std::path::PathBuf::from(path);
        let key = std::fs::read_to_string(&path).map_err(|err| {
            std::io::Error::new(err.kind(), format!("cannot read {}: {err}", path.display()))
        })?;
        keys.push(key);
    }
    Ok(keys)
}

/// The test-only seam behind [`trusted_keys`]: the path of an extra trusted
/// `.pub` file. Compiled into debug builds only.
#[cfg(debug_assertions)]
fn test_update_key() -> Option<std::ffi::OsString> {
    std::env::var_os("CRYSTALLINE_TEST_UPDATE_KEY")
}

/// The Apple Team ID that signs the macOS release binaries; public in every
/// signed binary, and part of the code signature requirement an update checks.
pub const APPLE_TEAM_ID: &str = "77PA95K95V";

/// The code signing identifier of the macOS release binaries.
pub const CODESIGN_IDENTIFIER: &str = "io.github.jordiboehme.crystalline";

#[cfg(test)]
mod tests {
    use super::*;

    /// The key id minisign prints for a `.pub` file, read from the decoded
    /// key (its keynum, little-endian), never from the untrusted comment.
    fn key_id(public: &str) -> String {
        let key = minisign::PublicKeyBox::from_string(public)
            .and_then(|key| key.into_public_key())
            .expect("a minisign public key");
        let keynum: [u8; 8] = key.keynum().try_into().expect("an 8-byte keynum");
        format!("{:016X}", u64::from_le_bytes(keynum))
    }

    #[test]
    fn key_id_reads_the_keynum_as_minisign_prints_it() {
        // Made by `minisign -G -W` (0.12), which printed this id.
        let cli = include_str!("../../tests/fixtures/update/cli-test.pub");
        assert_eq!(key_id(cli), "0B545B948F0D5530");
    }

    #[test]
    fn release_keys_parse() {
        for key in RELEASE_KEYS {
            minisign_verify::PublicKey::decode(key).expect("a release key decodes");
        }
        assert_ne!(RELEASE_KEYS[0], RELEASE_KEYS[1]);
        assert_eq!(key_id(RELEASE_KEYS[0]), "AC457F93DB463C6F", "online key");
        assert_eq!(key_id(RELEASE_KEYS[1]), "3E3AEE836F215E0C", "offline key");
    }

    #[test]
    fn a_signature_made_by_the_minisign_cli_verifies() {
        // A throwaway key pair made by `minisign -G -W` (0.12), as the online
        // key was, and `minisign -S -t tag:v0.0.0`: the release signer's shape.
        let sums = include_bytes!("../../tests/fixtures/update/v0.0.0-SHA256SUMS");
        let sig =
            include_str!("../../tests/fixtures/update/v0.0.0-SHA256SUMS.cli-test-key.minisig");
        let key = include_str!("../../tests/fixtures/update/cli-test.pub");
        let signed = verify_signature(sums, sig, &[key], "v0.0.0").expect("the signature verifies");
        signed
            .line(
                "crystalline-v0.0.0-linux-amd64.tar.gz",
                &Sha256Digest::of(b"test"),
            )
            .expect("the asset line matches");
        assert_eq!(
            verify_signature(sums, sig, &RELEASE_KEYS, "v0.0.0"),
            Err(VerifyError::UnknownKey),
            "the release keys do not trust a test key"
        );
    }

    #[test]
    fn the_codesign_constants_name_the_release_signer() {
        assert_eq!(APPLE_TEAM_ID, "77PA95K95V");
        assert_eq!(CODESIGN_IDENTIFIER, "io.github.jordiboehme.crystalline");
    }
}
