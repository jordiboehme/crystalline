//! Verification of a downloaded release asset, fail closed: the asset digest
//! GitHub lists (when its API answered), the asset's line in `SHA256SUMS`
//! and the minisign signature over `SHA256SUMS`, whose trusted comment must
//! name the tag being installed so an old valid signature cannot install an
//! old release. Every refusal is a [`VerifyError`] that names the check.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Why a downloaded release was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    /// The download's sha256 differs from the digest GitHub lists for the
    /// asset, or that digest is not a `sha256:<hex>` value.
    #[error("the download does not match the sha256 digest GitHub lists for the asset")]
    DigestMismatch,
    /// `SHA256SUMS` has no line for the exact asset file name.
    #[error("SHA256SUMS has no line for the downloaded asset")]
    SumMissing,
    /// The asset's line in `SHA256SUMS` names a different sha256.
    #[error("the download does not match its line in SHA256SUMS")]
    SumMismatch,
    /// The signature over `SHA256SUMS` is malformed or does not verify.
    #[error("the signature over SHA256SUMS is not valid")]
    BadSignature,
    /// The signature was made by a key this build does not trust.
    #[error("SHA256SUMS is signed by a key this build does not trust")]
    UnknownKey,
    /// The signature is valid but was made for another release.
    #[error("the signature over SHA256SUMS is for {signed:?}, not for tag {wanted}")]
    WrongTag {
        /// The signed trusted comment, cut to printable ASCII.
        signed: String,
        /// The tag being installed.
        wanted: String,
    },
}

/// Which checks ran on a download, so the output can say so.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Checks {
    /// The asset digest from the GitHub API.
    pub digest: bool,
    /// The asset's line in `SHA256SUMS`.
    pub sums: bool,
    /// The minisign signature over `SHA256SUMS`, with its tag.
    pub signature: bool,
    /// The macOS code signature requirement.
    pub codesign: bool,
    /// The new binary's `--version` output.
    pub version: bool,
}

/// Checks `bytes` against the digest GitHub lists for the asset,
/// `sha256:<hex>`. Anything else in `api_digest` is a mismatch.
pub fn verify_digest(bytes: &[u8], api_digest: &str) -> Result<(), VerifyError> {
    match api_digest.strip_prefix("sha256:") {
        Some(hex) if is_sha256_hex(hex) && hex.eq_ignore_ascii_case(&sha256_hex(bytes)) => Ok(()),
        _ => Err(VerifyError::DigestMismatch),
    }
}

/// Checks `bytes` against the line of `sums` (sha256sum format: 64 hex
/// digits, a space, then a space or `*`, then the file name) whose file name
/// equals `asset` exactly. Every line naming `asset` must match.
pub fn verify_sums_line(sums: &str, asset: &str, bytes: &[u8]) -> Result<(), VerifyError> {
    let actual = sha256_hex(bytes);
    let mut found = false;
    for line in sums.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some((hex, name)) = sums_line(line) else {
            continue;
        };
        if name != asset {
            continue;
        }
        if !hex.eq_ignore_ascii_case(&actual) {
            return Err(VerifyError::SumMismatch);
        }
        found = true;
    }
    if found {
        Ok(())
    } else {
        Err(VerifyError::SumMissing)
    }
}

/// One `SHA256SUMS` line split into its hex digest and file name, or `None`
/// when it is not in sha256sum form.
fn sums_line(line: &str) -> Option<(&str, &str)> {
    let hex = line.get(..64)?;
    let rest = line.get(64..)?;
    let name = rest
        .strip_prefix("  ")
        .or_else(|| rest.strip_prefix(" *"))?;
    (is_sha256_hex(hex) && !name.is_empty()).then_some((hex, name))
}

/// Checks the minisign signature `minisig` over `sums` with any key in
/// `keys` (each the full text of a `.pub` file), then that its trusted
/// comment is exactly `tag:<tag>`. Legacy (not pre-hashed) signatures are
/// refused.
pub fn verify_signature(
    sums: &[u8],
    minisig: &str,
    keys: &[&str],
    tag: &str,
) -> Result<(), VerifyError> {
    let signature =
        minisign_verify::Signature::decode(minisig).map_err(|_| VerifyError::BadSignature)?;
    // Every key is tried, so two keys that share an id cannot hide the one
    // that verifies. A key text that does not parse trusts nothing.
    let mut key_matched = false;
    let mut verified = false;
    for key in keys {
        let Ok(key) = minisign_verify::PublicKey::decode(key) else {
            continue;
        };
        match key.verify(sums, &signature, false) {
            Ok(()) => {
                verified = true;
                break;
            }
            Err(minisign_verify::Error::UnexpectedKeyId) => {}
            Err(_) => key_matched = true,
        }
    }
    if !verified {
        return Err(if key_matched {
            VerifyError::BadSignature
        } else {
            VerifyError::UnknownKey
        });
    }
    let comment = signature.trusted_comment();
    if comment != format!("tag:{tag}") {
        return Err(VerifyError::WrongTag {
            signed: printable(comment),
            wanted: printable(tag),
        });
    }
    Ok(())
}

/// The longest text a [`VerifyError::WrongTag`] carries.
const SHOWN_CHARS: usize = 64;

/// `text` cut to [`SHOWN_CHARS`] characters, with anything but printable
/// ASCII and the space shown as `?`, so a signed comment cannot steer a
/// terminal.
fn printable(text: &str) -> String {
    text.chars()
        .take(SHOWN_CHARS)
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect()
}

/// The lowercase hex sha256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether `text` is 64 hex digits, either case.
fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &str = include_str!("../../tests/fixtures/update/test.key");
    const TEST_PUB: &str = include_str!("../../tests/fixtures/update/test.pub");
    const ASSET: &str = "crystalline-v0.34.0-linux-amd64.tar.gz";
    const TAG: &str = "v0.34.0";
    const BYTES: &[u8] = b"the release archive";

    /// A `SHA256SUMS` text in the release workflow's shape.
    fn sums_for(asset: &str, bytes: &[u8]) -> String {
        format!(
            "{}  crystalline-v0.34.0-darwin-arm64.tar.gz\n{}  {asset}\n",
            sha256_hex(b"another asset"),
            sha256_hex(bytes),
        )
    }

    /// Signs `data` with the fixture test key and `trusted_comment`.
    fn sign_with(secret: &str, data: &[u8], trusted_comment: &str) -> String {
        let sk = minisign::SecretKeyBox::from_string(secret)
            .unwrap()
            .into_unencrypted_secret_key()
            .unwrap();
        minisign::sign(None, &sk, data, Some(trusted_comment), None)
            .unwrap()
            .into_string()
    }

    /// A second throwaway key pair, made fresh: its `.pub` text and its
    /// secret key text.
    fn other_key() -> (String, String) {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        (
            pair.pk.to_box().unwrap().into_string(),
            pair.sk.to_box(None).unwrap().into_string(),
        )
    }

    #[test]
    fn the_fixture_key_says_it_is_a_test_key() {
        let first = "untrusted comment: test key, never used for a release";
        assert_eq!(TEST_KEY.lines().next(), Some(first));
        assert_eq!(TEST_PUB.lines().next(), Some(first));
    }

    #[test]
    fn good_release_verifies() {
        let sums = sums_for(ASSET, BYTES);
        let sig = sign_with(TEST_KEY, sums.as_bytes(), "tag:v0.34.0");
        let digest = format!("sha256:{}", sha256_hex(BYTES));
        assert_eq!(verify_digest(BYTES, &digest), Ok(()));
        // GitHub writes lowercase; an uppercase digest is the same value.
        assert_eq!(
            verify_digest(
                BYTES,
                &digest.to_uppercase().replacen("SHA256", "sha256", 1)
            ),
            Ok(())
        );
        assert_eq!(verify_sums_line(&sums, ASSET, BYTES), Ok(()));
        assert_eq!(
            verify_signature(sums.as_bytes(), &sig, &[TEST_PUB], TAG),
            Ok(())
        );
        // The binary marker of sha256sum and CRLF line ends count too.
        let binary = format!("{} *{ASSET}\r\n", sha256_hex(BYTES));
        assert_eq!(verify_sums_line(&binary, ASSET, BYTES), Ok(()));
    }

    #[test]
    fn wrong_digest() {
        let other = format!("sha256:{}", sha256_hex(b"something else"));
        assert_eq!(
            verify_digest(BYTES, &other),
            Err(VerifyError::DigestMismatch)
        );
        let hex = sha256_hex(BYTES);
        for malformed in [
            hex.clone(),
            format!("sha512:{hex}"),
            format!("sha256:{}", &hex[..63]),
            format!("sha256:{hex}0"),
            format!("sha256: {hex}"),
            String::new(),
        ] {
            assert_eq!(
                verify_digest(BYTES, &malformed),
                Err(VerifyError::DigestMismatch),
                "{malformed:?}"
            );
        }
        let sums = sums_for(ASSET, b"something else");
        assert_eq!(
            verify_sums_line(&sums, ASSET, BYTES),
            Err(VerifyError::SumMismatch)
        );
    }

    #[test]
    fn missing_sum() {
        let sums = sums_for("crystalline-v0.34.0-linux-arm64.tar.gz", BYTES);
        assert_eq!(
            verify_sums_line(&sums, ASSET, BYTES),
            Err(VerifyError::SumMissing)
        );
        assert_eq!(
            verify_sums_line("", ASSET, BYTES),
            Err(VerifyError::SumMissing)
        );
        // A line that is not in sha256sum form does not count.
        let loose = format!("{} {ASSET}\n", sha256_hex(BYTES));
        assert_eq!(
            verify_sums_line(&loose, ASSET, BYTES),
            Err(VerifyError::SumMissing)
        );
    }

    #[test]
    fn a_sums_line_for_a_similar_name_does_not_count() {
        let sums = format!("{}  {ASSET}.bak\n", sha256_hex(BYTES));
        assert_eq!(
            verify_sums_line(&sums, ASSET, BYTES),
            Err(VerifyError::SumMissing)
        );
        let prefixed = format!("{}  old-{ASSET}\n", sha256_hex(BYTES));
        assert_eq!(
            verify_sums_line(&prefixed, ASSET, BYTES),
            Err(VerifyError::SumMissing)
        );
    }

    #[test]
    fn a_second_line_for_the_asset_must_match_too() {
        let sums = format!(
            "{}  {ASSET}\n{}  {ASSET}\n",
            sha256_hex(BYTES),
            sha256_hex(b"something else")
        );
        assert_eq!(
            verify_sums_line(&sums, ASSET, BYTES),
            Err(VerifyError::SumMismatch)
        );
    }

    #[test]
    fn bad_signature() {
        let sums = sums_for(ASSET, BYTES);
        let sig = sign_with(TEST_KEY, sums.as_bytes(), "tag:v0.34.0");
        // The same signature over changed sums.
        let changed = sums_for(ASSET, b"a swapped archive");
        assert_eq!(
            verify_signature(changed.as_bytes(), &sig, &[TEST_PUB], TAG),
            Err(VerifyError::BadSignature)
        );
        // A trusted comment edited after signing.
        let edited = sig.replace("tag:v0.34.0", "tag:v0.34.1");
        assert_eq!(
            verify_signature(sums.as_bytes(), &edited, &[TEST_PUB], "v0.34.1"),
            Err(VerifyError::BadSignature)
        );
        // Not a signature at all.
        for garbage in ["", "untrusted comment: x\nnot base64\n"] {
            assert_eq!(
                verify_signature(sums.as_bytes(), garbage, &[TEST_PUB], TAG),
                Err(VerifyError::BadSignature)
            );
        }
    }

    #[test]
    fn wrong_key() {
        let sums = sums_for(ASSET, BYTES);
        let sig = sign_with(TEST_KEY, sums.as_bytes(), "tag:v0.34.0");
        let (other_pub, _) = other_key();
        assert_eq!(
            verify_signature(sums.as_bytes(), &sig, &[&other_pub], TAG),
            Err(VerifyError::UnknownKey)
        );
        assert_eq!(
            verify_signature(sums.as_bytes(), &sig, &[], TAG),
            Err(VerifyError::UnknownKey)
        );
        // A key text that does not parse trusts nothing.
        assert_eq!(
            verify_signature(sums.as_bytes(), &sig, &["not a key"], TAG),
            Err(VerifyError::UnknownKey)
        );
    }

    #[test]
    fn a_valid_signature_for_another_tag_is_refused() {
        let sums = sums_for(ASSET, BYTES);
        let sig = sign_with(TEST_KEY, sums.as_bytes(), "tag:v0.33.0");
        let err = verify_signature(sums.as_bytes(), &sig, &[TEST_PUB], TAG).unwrap_err();
        assert_eq!(
            err,
            VerifyError::WrongTag {
                signed: "tag:v0.33.0".to_string(),
                wanted: "v0.34.0".to_string(),
            }
        );
        assert_eq!(
            err.to_string(),
            "the signature over SHA256SUMS is for \"tag:v0.33.0\", not for tag v0.34.0"
        );
        // The default minisign comment names no tag at all.
        let sig = sign_with(TEST_KEY, sums.as_bytes(), "timestamp:1763337600");
        assert!(matches!(
            verify_signature(sums.as_bytes(), &sig, &[TEST_PUB], TAG),
            Err(VerifyError::WrongTag { .. })
        ));
        // A tag that only starts the comment is not the tag.
        let sig = sign_with(TEST_KEY, sums.as_bytes(), "tag:v0.34.0 extra");
        assert!(matches!(
            verify_signature(sums.as_bytes(), &sig, &[TEST_PUB], TAG),
            Err(VerifyError::WrongTag { .. })
        ));
    }

    #[test]
    fn a_wrong_tag_shows_only_printable_text() {
        let sums = sums_for(ASSET, BYTES);
        let long = format!("tag:\u{1b}[31m{}", "x".repeat(200));
        let sig = sign_with(TEST_KEY, sums.as_bytes(), &long);
        let Err(VerifyError::WrongTag { signed, .. }) =
            verify_signature(sums.as_bytes(), &sig, &[TEST_PUB], TAG)
        else {
            panic!("expected WrongTag");
        };
        assert!(signed.starts_with("tag:?[31m"), "{signed}");
        assert!(signed.chars().count() <= 64, "{signed}");
        assert!(signed.bytes().all(|b| b.is_ascii_graphic() || b == b' '));
    }

    #[test]
    fn either_compiled_key_is_accepted() {
        let sums = sums_for(ASSET, BYTES);
        let (first_pub, first_secret) = other_key();
        let keys = [first_pub.as_str(), TEST_PUB];
        let by_second = sign_with(TEST_KEY, sums.as_bytes(), "tag:v0.34.0");
        assert_eq!(
            verify_signature(sums.as_bytes(), &by_second, &keys, TAG),
            Ok(())
        );
        let by_first = sign_with(&first_secret, sums.as_bytes(), "tag:v0.34.0");
        assert_eq!(
            verify_signature(sums.as_bytes(), &by_first, &keys, TAG),
            Ok(())
        );
    }
}
