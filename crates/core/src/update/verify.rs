//! Verification of a downloaded release asset, fail closed: the asset digest
//! GitHub lists (when its API answered), the minisign signature over
//! `SHA256SUMS`, whose trusted comment must name the tag being installed so
//! an old valid signature cannot install an old release, and the asset's
//! line in that signed `SHA256SUMS`. Every refusal is a [`VerifyError`] that
//! names the check.
//!
//! The line check exists only on [`SignedSums`], which only
//! [`verify_signature`] makes, so no caller can read a line from a
//! `SHA256SUMS` it did not verify, or from other bytes than it verified.
//! The download is hashed once, into a [`Sha256Digest`], for both checks.

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
    /// `SHA256SUMS` names the asset on more than one line, identical or not.
    #[error("SHA256SUMS names the downloaded asset on more than one line")]
    SumDuplicate,
    /// The signed `SHA256SUMS` is not UTF-8 text.
    #[error("SHA256SUMS is not text")]
    SumsNotText,
    /// The signature over `SHA256SUMS` is malformed or does not verify.
    #[error("the signature over SHA256SUMS is not valid")]
    BadSignature,
    /// The signature was made by a key this build does not trust.
    #[error("SHA256SUMS is signed by a key this build does not trust")]
    UnknownKey,
    /// A trusted public key does not parse, or there is none.
    #[error("no usable public key: a trusted key does not parse or none is set")]
    NoUsableKey,
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

/// The sha256 of a download, computed once and handed to every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// The sha256 of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(sha256_hex(bytes))
    }

    /// The digest as lowercase hex.
    pub fn hex(&self) -> &str {
        &self.0
    }
}

/// `SHA256SUMS` whose signature verified for the wanted tag. Only
/// [`verify_signature`] makes one, from the very bytes it verified.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "check the asset's line with `SignedSums::line`"]
pub struct SignedSums<'a> {
    text: &'a str,
}

impl SignedSums<'_> {
    /// Checks the download against its line in these sums: the line (in
    /// sha256sum format: 64 hex digits, a space, then a space or `*`, then
    /// the file name) whose file name equals `asset` exactly. A second line
    /// naming `asset` is refused, even with the same digest.
    pub fn line(&self, asset: &str, download: &Sha256Digest) -> Result<(), VerifyError> {
        verify_sums_line(self.text, asset, download)
    }
}

/// Checks the download against the digest GitHub lists for the asset,
/// `sha256:<hex>`. Anything else in `api_digest` is a mismatch.
pub fn verify_digest(download: &Sha256Digest, api_digest: &str) -> Result<(), VerifyError> {
    match api_digest.strip_prefix("sha256:") {
        Some(hex) if is_sha256_hex(hex) && hex.eq_ignore_ascii_case(download.hex()) => Ok(()),
        _ => Err(VerifyError::DigestMismatch),
    }
}

/// The line check behind [`SignedSums::line`], on text not yet known to be
/// signed; private so only signed text reaches it outside the tests.
fn verify_sums_line(sums: &str, asset: &str, download: &Sha256Digest) -> Result<(), VerifyError> {
    let mut listed = None;
    for line in sums.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some((hex, name)) = sums_line(line) else {
            continue;
        };
        if name != asset {
            continue;
        }
        if listed.replace(hex).is_some() {
            return Err(VerifyError::SumDuplicate);
        }
    }
    match listed {
        None => Err(VerifyError::SumMissing),
        Some(hex) if hex.eq_ignore_ascii_case(download.hex()) => Ok(()),
        Some(_) => Err(VerifyError::SumMismatch),
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
/// comment is exactly `tag:<tag>`, and hands back the sums as
/// [`SignedSums`] for the line check. Legacy (not pre-hashed) signatures are
/// refused, and so is a key list that is empty or holds a key text that does
/// not parse ([`VerifyError::NoUsableKey`]), so a broken build never reads as
/// an unknown signer. `tag` should come from
/// [`release_tag`](crate::version::release_tag).
///
/// The trusted comment binds the tag, not the kind of file, so a release
/// key must sign nothing but `SHA256SUMS`.
pub fn verify_signature<'a>(
    sums: &'a [u8],
    minisig: &str,
    keys: &[&str],
    tag: &str,
) -> Result<SignedSums<'a>, VerifyError> {
    let keys = keys
        .iter()
        .map(|key| minisign_verify::PublicKey::decode(key))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| VerifyError::NoUsableKey)?;
    if keys.is_empty() {
        return Err(VerifyError::NoUsableKey);
    }
    let signature =
        minisign_verify::Signature::decode(minisig).map_err(|_| VerifyError::BadSignature)?;
    // Every key is tried, so two keys that share an id cannot hide the one
    // that verifies.
    let mut key_matched = false;
    let mut verified = false;
    for key in &keys {
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
    let text = std::str::from_utf8(sums).map_err(|_| VerifyError::SumsNotText)?;
    Ok(SignedSums { text })
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

    fn of(bytes: &[u8]) -> Sha256Digest {
        Sha256Digest::of(bytes)
    }

    /// The signature check alone, its proof dropped.
    fn sig_check(sums: &[u8], minisig: &str, keys: &[&str], tag: &str) -> Result<(), VerifyError> {
        verify_signature(sums, minisig, keys, tag).map(drop)
    }

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
        let download = Sha256Digest::of(BYTES);
        let digest = format!("sha256:{}", sha256_hex(BYTES));
        // The order a caller runs them in: digest, signature, then the line
        // in the signed sums.
        assert_eq!(verify_digest(&download, &digest), Ok(()));
        let signed = verify_signature(sums.as_bytes(), &sig, &[TEST_PUB], TAG).unwrap();
        assert_eq!(signed.line(ASSET, &download), Ok(()));
        assert_eq!(
            signed.line("crystalline-v0.34.0-linux-arm64.tar.gz", &download),
            Err(VerifyError::SumMissing)
        );
        // GitHub writes lowercase; an uppercase digest is the same value.
        let upper = digest.to_uppercase().replacen("SHA256", "sha256", 1);
        assert_eq!(verify_digest(&download, &upper), Ok(()));
        // The binary marker of sha256sum and CRLF line ends count too.
        let binary = format!("{} *{ASSET}\r\n", sha256_hex(BYTES));
        assert_eq!(verify_sums_line(&binary, ASSET, &download), Ok(()));
    }

    #[test]
    fn signed_sums_that_are_not_text_are_refused() {
        let mut sums = sums_for(ASSET, BYTES).into_bytes();
        sums.extend_from_slice(&[0xff, 0xfe, b'\n']);
        let sig = sign_with(TEST_KEY, &sums, "tag:v0.34.0");
        assert_eq!(
            sig_check(&sums, &sig, &[TEST_PUB], TAG),
            Err(VerifyError::SumsNotText)
        );
    }

    #[test]
    fn wrong_digest() {
        let other = format!("sha256:{}", sha256_hex(b"something else"));
        assert_eq!(
            verify_digest(&of(BYTES), &other),
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
                verify_digest(&of(BYTES), &malformed),
                Err(VerifyError::DigestMismatch),
                "{malformed:?}"
            );
        }
        let sums = sums_for(ASSET, b"something else");
        assert_eq!(
            verify_sums_line(&sums, ASSET, &of(BYTES)),
            Err(VerifyError::SumMismatch)
        );
    }

    #[test]
    fn missing_sum() {
        let sums = sums_for("crystalline-v0.34.0-linux-arm64.tar.gz", BYTES);
        assert_eq!(
            verify_sums_line(&sums, ASSET, &of(BYTES)),
            Err(VerifyError::SumMissing)
        );
        assert_eq!(
            verify_sums_line("", ASSET, &of(BYTES)),
            Err(VerifyError::SumMissing)
        );
        // A line that is not in sha256sum form does not count.
        let loose = format!("{} {ASSET}\n", sha256_hex(BYTES));
        assert_eq!(
            verify_sums_line(&loose, ASSET, &of(BYTES)),
            Err(VerifyError::SumMissing)
        );
    }

    #[test]
    fn a_sums_line_for_a_similar_name_does_not_count() {
        let sums = format!("{}  {ASSET}.bak\n", sha256_hex(BYTES));
        assert_eq!(
            verify_sums_line(&sums, ASSET, &of(BYTES)),
            Err(VerifyError::SumMissing)
        );
        let prefixed = format!("{}  old-{ASSET}\n", sha256_hex(BYTES));
        assert_eq!(
            verify_sums_line(&prefixed, ASSET, &of(BYTES)),
            Err(VerifyError::SumMissing)
        );
    }

    #[test]
    fn a_second_line_for_the_asset_is_refused() {
        let good = sha256_hex(BYTES);
        let other = sha256_hex(b"something else");
        for (first, second) in [(&good, &other), (&other, &good), (&good, &good)] {
            let sums = format!("{first}  {ASSET}\n{second} *{ASSET}\n");
            assert_eq!(
                verify_sums_line(&sums, ASSET, &of(BYTES)),
                Err(VerifyError::SumDuplicate)
            );
        }
    }

    #[test]
    fn bad_signature() {
        let sums = sums_for(ASSET, BYTES);
        let sig = sign_with(TEST_KEY, sums.as_bytes(), "tag:v0.34.0");
        // The same signature over changed sums.
        let changed = sums_for(ASSET, b"a swapped archive");
        assert_eq!(
            sig_check(changed.as_bytes(), &sig, &[TEST_PUB], TAG),
            Err(VerifyError::BadSignature)
        );
        // A trusted comment edited after signing.
        let edited = sig.replace("tag:v0.34.0", "tag:v0.34.1");
        assert_eq!(
            sig_check(sums.as_bytes(), &edited, &[TEST_PUB], "v0.34.1"),
            Err(VerifyError::BadSignature)
        );
        // Not a signature at all.
        for garbage in ["", "untrusted comment: x\nnot base64\n"] {
            assert_eq!(
                sig_check(sums.as_bytes(), garbage, &[TEST_PUB], TAG),
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
            sig_check(sums.as_bytes(), &sig, &[&other_pub], TAG),
            Err(VerifyError::UnknownKey)
        );
    }

    #[test]
    fn a_key_that_does_not_parse_is_no_usable_key() {
        let sums = sums_for(ASSET, BYTES);
        let sig = sign_with(TEST_KEY, sums.as_bytes(), "tag:v0.34.0");
        // The bare base64 line is not a `.pub` file text.
        let bare = TEST_PUB.lines().nth(1).unwrap();
        for keys in [
            &[][..],
            &["not a key"][..],
            &[bare][..],
            // A good key beside a broken one still refuses: the build is
            // broken, which must not read as a good or an unknown signer.
            &[TEST_PUB, "not a key"][..],
        ] {
            assert_eq!(
                sig_check(sums.as_bytes(), &sig, keys, TAG),
                Err(VerifyError::NoUsableKey),
                "{keys:?}"
            );
        }
    }

    #[test]
    fn a_legacy_signature_is_refused() {
        // Made with `minisign -S -l` and the test key: valid, but not
        // pre-hashed.
        let sums = include_str!("../../tests/fixtures/update/legacy-SHA256SUMS");
        let sig = include_str!("../../tests/fixtures/update/legacy-SHA256SUMS.minisig");
        let key = minisign_verify::PublicKey::decode(TEST_PUB).unwrap();
        let decoded = minisign_verify::Signature::decode(sig).unwrap();
        assert!(
            key.verify(sums.as_bytes(), &decoded, true).is_ok(),
            "the fixture is a genuine legacy signature"
        );
        assert_eq!(
            sig_check(sums.as_bytes(), sig, &[TEST_PUB], TAG),
            Err(VerifyError::BadSignature)
        );
    }

    #[test]
    fn a_valid_signature_for_another_tag_is_refused() {
        let sums = sums_for(ASSET, BYTES);
        let sig = sign_with(TEST_KEY, sums.as_bytes(), "tag:v0.33.0");
        let err = sig_check(sums.as_bytes(), &sig, &[TEST_PUB], TAG).unwrap_err();
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
            sig_check(sums.as_bytes(), &sig, &[TEST_PUB], TAG),
            Err(VerifyError::WrongTag { .. })
        ));
        // Only the exact tag counts, in either direction of a prefix.
        for (signed, wanted) in [
            ("tag:v0.34.0 extra", TAG),
            ("tag:v0.34.0-rc1", TAG),
            ("tag:v0.34.01", TAG),
            ("tag:v0.34.0", "v0.34.0-rc1"),
            ("tag:v0.34.0", "v0.34"),
        ] {
            let sig = sign_with(TEST_KEY, sums.as_bytes(), signed);
            assert_eq!(
                sig_check(sums.as_bytes(), &sig, &[TEST_PUB], wanted),
                Err(VerifyError::WrongTag {
                    signed: signed.to_string(),
                    wanted: wanted.to_string(),
                }),
                "{signed} for {wanted}"
            );
        }
    }

    #[test]
    fn a_wrong_tag_shows_only_printable_text() {
        let sums = sums_for(ASSET, BYTES);
        let long = format!("tag:\u{1b}[31m{}", "x".repeat(200));
        let sig = sign_with(TEST_KEY, sums.as_bytes(), &long);
        let Err(VerifyError::WrongTag { signed, .. }) =
            sig_check(sums.as_bytes(), &sig, &[TEST_PUB], TAG)
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
        assert_eq!(sig_check(sums.as_bytes(), &by_second, &keys, TAG), Ok(()));
        let by_first = sign_with(&first_secret, sums.as_bytes(), "tag:v0.34.0");
        assert_eq!(sig_check(sums.as_bytes(), &by_first, &keys, TAG), Ok(()));
    }
}
