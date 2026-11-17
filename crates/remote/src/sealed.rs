//! Sealing: the secrets kept in Postgres are stored as XChaCha20-Poly1305
//! blobs under a key the operator supplies in `CRYSTALLINE_SECRET_KEY` or
//! `CRYSTALLINE_SECRET_KEY_FILE`.
//!
//! Format: `0x01 (format version) || 24-byte random nonce || ciphertext+tag`.
//! The associated data is the version byte followed by the row name, so a
//! sealed value copied to another row does not open there.

use std::fmt;

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// The variable holding the key itself.
pub const KEY_ENV: &str = "CRYSTALLINE_SECRET_KEY";
/// The variable naming a file that holds the key.
pub const KEY_FILE_ENV: &str = "CRYSTALLINE_SECRET_KEY_FILE";

const VERSION: u8 = 1;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;

/// Why the secret key could not be read.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// Both variables are set.
    #[error("Both {KEY_ENV} and {KEY_FILE_ENV} are set. Set only one of them.")]
    BothSet,
    /// The key file could not be read.
    #[error("{KEY_FILE_ENV} points at a file that cannot be read ({0}).")]
    Unreadable(String),
    /// The key is not 32 bytes in hex or base64.
    #[error(
        "{var} must hold a 32 byte key as 64 hex characters or as base64 (44 characters). Generate one with: openssl rand -hex 32"
    )]
    Malformed {
        /// The variable the key came from.
        var: &'static str,
    },
}

/// Why a sealed value did not open.
#[derive(Debug, thiserror::Error)]
pub enum SealError {
    /// The value is shorter than a version, a nonce and a tag.
    #[error("The sealed value is too short to be valid.")]
    Truncated,
    /// The value carries a format version this build does not know.
    #[error("The sealed value has an unknown format version ({0}).")]
    UnknownVersion(u8),
    /// The tag did not verify: wrong key, a changed value or another row name.
    #[error(
        "The sealed value did not open. The secret key is not the one it was sealed with, or the value was changed or moved."
    )]
    Failed,
}

/// The 32 byte key that seals and opens secrets. Never printed, wiped on
/// drop, and not `Clone`: share it behind an `Arc`.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SealKey([u8; 32]);

impl fmt::Debug for SealKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SealKey(<redacted>)")
    }
}

impl SealKey {
    /// Reads the key from [`KEY_ENV`] or [`KEY_FILE_ENV`]. `Ok(None)` when
    /// neither is set; a variable that is empty or only whitespace counts as
    /// unset. Surrounding whitespace (such as a file's trailing newline) is
    /// trimmed before the key is decoded. A value that is set but not valid
    /// Unicode is refused as malformed.
    pub fn from_env() -> Result<Option<SealKey>, KeyError> {
        Self::from_vars(std::env::var_os(KEY_ENV), std::env::var_os(KEY_FILE_ENV))
    }

    fn from_vars(
        plain: Option<std::ffi::OsString>,
        file: Option<std::ffi::OsString>,
    ) -> Result<Option<SealKey>, KeyError> {
        let blank = |v: &std::ffi::OsString| v.to_str().is_some_and(|s| s.trim().is_empty());
        let plain = plain.filter(|v| !blank(v));
        let file = file.filter(|v| !blank(v));
        match (plain, file) {
            (Some(_), Some(_)) => Err(KeyError::BothSet),
            (Some(text), None) => {
                let text = Zeroizing::new(
                    text.into_string()
                        .map_err(|_| KeyError::Malformed { var: KEY_ENV })?,
                );
                Self::parse(&text, KEY_ENV).map(Some)
            }
            (None, Some(path)) => {
                let bytes = Zeroizing::new(
                    std::fs::read(&path).map_err(|e| KeyError::Unreadable(e.kind().to_string()))?,
                );
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| KeyError::Malformed { var: KEY_FILE_ENV })?;
                Self::parse(text, KEY_FILE_ENV).map(Some)
            }
            (None, None) => Ok(None),
        }
    }

    fn parse(text: &str, var: &'static str) -> Result<SealKey, KeyError> {
        let text = text.trim();
        let bad = || KeyError::Malformed { var };
        let bytes: Zeroizing<Vec<u8>> = Zeroizing::new(
            if text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
                (0..32)
                    .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|_| bad()))
                    .collect::<Result<Vec<u8>, _>>()?
            } else if text.len() == 44 {
                base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .map_err(|_| bad())?
            } else {
                return Err(bad());
            },
        );
        let key: [u8; 32] = bytes.as_slice().try_into().map_err(|_| bad())?;
        Ok(SealKey(key))
    }

    /// The first 16 hex characters of the key's SHA-256: names the key
    /// without revealing it.
    pub fn key_id(&self) -> String {
        Sha256::digest(self.0)
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

fn aad(name: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(1 + name.len());
    aad.push(VERSION);
    aad.extend_from_slice(name.as_bytes());
    aad
}

/// Seals `plain` for the row `name`.
pub fn seal(key: &SealKey, name: &str, plain: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new((&key.0).into());
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).expect("the OS CSPRNG is available");
    let body = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plain,
                aad: &aad(name),
            },
        )
        .expect("XChaCha20-Poly1305 encryption of an in-memory buffer cannot fail");
    let mut out = Vec::with_capacity(1 + NONCE_LEN + body.len());
    out.push(VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&body);
    out
}

/// Opens a value sealed for the row `name`.
pub fn open(key: &SealKey, name: &str, sealed: &[u8]) -> Result<Vec<u8>, SealError> {
    let (&version, rest) = sealed.split_first().ok_or(SealError::Truncated)?;
    if version != VERSION {
        return Err(SealError::UnknownVersion(version));
    }
    if rest.len() < NONCE_LEN + TAG_LEN {
        return Err(SealError::Truncated);
    }
    let (nonce, body) = rest.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| SealError::Truncated)?;
    XChaCha20Poly1305::new((&key.0).into())
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: body,
                aad: &aad(name),
            },
        )
        .map_err(|_| SealError::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    fn zs(v: &str) -> std::ffi::OsString {
        v.into()
    }

    fn key() -> SealKey {
        SealKey::parse(HEX, KEY_ENV).unwrap()
    }

    #[test]
    fn seal_open_round_trip() {
        let sealed = seal(&key(), "github.token", b"ghp_secret");
        assert_eq!(sealed[0], 1);
        assert_eq!(sealed.len(), 1 + 24 + 10 + 16);
        assert_eq!(
            open(&key(), "github.token", &sealed).unwrap(),
            b"ghp_secret"
        );
        assert_ne!(sealed, seal(&key(), "github.token", b"ghp_secret"));
    }

    #[test]
    fn renamed_row_fails_to_open() {
        let sealed = seal(&key(), "a", b"x");
        assert!(matches!(open(&key(), "b", &sealed), Err(SealError::Failed)));
    }

    #[test]
    fn unknown_format_version_refused() {
        let mut sealed = seal(&key(), "a", b"x");
        sealed[0] = 2;
        assert!(matches!(
            open(&key(), "a", &sealed),
            Err(SealError::UnknownVersion(2))
        ));
        assert!(matches!(open(&key(), "a", &[]), Err(SealError::Truncated)));
        assert!(matches!(
            open(&key(), "a", &[1, 2]),
            Err(SealError::Truncated)
        ));
    }

    #[test]
    fn short_or_malformed_key_refused_naming_variable() {
        for bad in ["abcd", &HEX[..62], &format!("{HEX}00"), &"z".repeat(64)] {
            let err = SealKey::parse(bad, KEY_ENV).unwrap_err().to_string();
            assert!(err.contains("CRYSTALLINE_SECRET_KEY"), "{err}");
            assert!(!err.contains(bad), "{err}");
        }
        let err = SealKey::from_vars(Some(zs("short")), None).unwrap_err();
        assert!(!err.to_string().contains("short"));
        assert!(err.to_string().contains("CRYSTALLINE_SECRET_KEY"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k");
        std::fs::write(&path, "short").unwrap();
        let err = SealKey::from_vars(None, Some(path.clone().into_os_string())).unwrap_err();
        assert!(err.to_string().contains("CRYSTALLINE_SECRET_KEY_FILE"));
        assert!(SealKey::from_vars(None, None).unwrap().is_none());
    }

    #[test]
    fn a_tampered_value_and_a_wrong_key_fail_to_open() {
        let sealed = seal(&key(), "a", b"secret");
        for at in [1 + 24, sealed.len() - 1] {
            let mut bad = sealed.clone();
            bad[at] ^= 1;
            assert!(matches!(open(&key(), "a", &bad), Err(SealError::Failed)));
        }
        let other = SealKey::parse(&"11".repeat(32), KEY_ENV).unwrap();
        assert!(matches!(open(&other, "a", &sealed), Err(SealError::Failed)));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_unicode_value_is_malformed_not_unset() {
        use std::os::unix::ffi::OsStringExt;
        let bad = std::ffi::OsString::from_vec(vec![0xff, 0xfe]);
        let err = SealKey::from_vars(Some(bad), None).unwrap_err();
        assert!(matches!(err, KeyError::Malformed { var: KEY_ENV }));
        let blank = SealKey::from_vars(Some(zs("  ")), Some("".into())).unwrap();
        assert!(blank.is_none());
    }

    #[test]
    fn key_and_file_both_set_refused() {
        let err = SealKey::from_vars(Some(zs(HEX)), Some("/x".into())).unwrap_err();
        assert!(matches!(err, KeyError::BothSet));
        assert!(err.to_string().contains("CRYSTALLINE_SECRET_KEY_FILE"));
    }

    #[test]
    fn hex_and_base64_accepted() {
        let raw: Vec<u8> = (0u8..32).collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        assert_eq!(b64.len(), 44);
        let a = SealKey::parse(HEX, KEY_ENV).unwrap();
        let b = SealKey::parse(&b64, KEY_ENV).unwrap();
        assert_eq!(a.0, b.0);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k");
        std::fs::write(&path, format!("{HEX}\n")).unwrap();
        let c = SealKey::from_vars(None, Some(path.clone().into_os_string()))
            .unwrap()
            .unwrap();
        assert_eq!(a.0, c.0);
    }

    #[test]
    fn key_id_is_16_hex_of_sha256() {
        let id = key().key_id();
        assert_eq!(id.len(), 16);
        let full: String = Sha256::digest(key().0)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(id, full[..16]);
        assert!(!format!("{:?}", key()).contains("0001"));
    }
}
