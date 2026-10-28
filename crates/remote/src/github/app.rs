//! Signing the GitHub App JWT.
//!
//! A GitHub App proves who it is by sending a short-lived RS256 token signed
//! with the App's private key; GitHub trades that token for an installation
//! token. This module is only the signing primitive: it makes no request and
//! keeps nothing between calls.
//!
//! The signing goes through aws-lc-rs, which the HTTP client's rustls already
//! pulls in, and never through the `rsa` crate: `deny.toml` ignores the Marvin
//! advisory on the ground that Crystalline never signs with a private key
//! through `rsa`, and that has to stay true.

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{PrivatePkcs1KeyDer, PrivatePkcs8KeyDer};
use serde::Serialize;

use crate::error::RemoteError;

/// The header of every App JWT. A literal, so the bytes never depend on a
/// serializer's field order.
const HEADER: &str = r#"{"alg":"RS256","typ":"JWT"}"#;

/// How far before `now` the token claims to be issued. GitHub rejects a token
/// issued in its future, and a local clock a little ahead of GitHub's is the
/// common failure.
const BACKDATE: Duration = Duration::seconds(60);

/// How long after `now` the token expires. GitHub allows at most ten minutes
/// from the issue time; with the backdate this stays a minute inside it.
const LIFETIME: Duration = Duration::seconds(540);

/// A GitHub App's RSA private key, parsed and ready to sign.
///
/// `Debug` prints no key material.
pub struct AppKey {
    pair: RsaKeyPair,
}

impl std::fmt::Debug for AppKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AppKey(<redacted>)")
    }
}

impl AppKey {
    /// Parses a PEM private key: PKCS#1 (`RSA PRIVATE KEY`, what GitHub's
    /// download gives) or PKCS#8 (`PRIVATE KEY`).
    ///
    /// The error names the problem and never echoes the input.
    pub fn from_pem(pem: &str) -> Result<AppKey, RemoteError> {
        let pair = if let Ok(der) = PrivatePkcs1KeyDer::from_pem_slice(pem.as_bytes()) {
            RsaKeyPair::from_der(der.secret_pkcs1_der())
        } else if let Ok(der) = PrivatePkcs8KeyDer::from_pem_slice(pem.as_bytes()) {
            RsaKeyPair::from_pkcs8(der.secret_pkcs8_der())
        } else {
            return Err(RemoteError::AppCredential(
                "the private key is not a PEM RSA key (expected RSA PRIVATE KEY or PRIVATE KEY)"
                    .to_string(),
            ));
        };
        pair.map(|pair| AppKey { pair }).map_err(|_| {
            RemoteError::AppCredential("the private key is not a usable RSA key".to_string())
        })
    }
}

#[derive(Serialize)]
struct Claims<'a> {
    iat: i64,
    exp: i64,
    iss: &'a str,
}

/// Signs the JWT that identifies the App to GitHub, as `client_id`.
///
/// `now` is a parameter so a test can pin the whole token: PKCS#1 v1.5
/// signing is deterministic.
pub fn app_jwt(key: &AppKey, client_id: &str, now: DateTime<Utc>) -> Result<String, RemoteError> {
    let claims = Claims {
        iat: (now - BACKDATE).timestamp(),
        exp: (now + LIFETIME).timestamp(),
        iss: client_id,
    };
    let claims = serde_json::to_vec(&claims)
        .map_err(|_| RemoteError::AppCredential("the claims could not be encoded".to_string()))?;
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(HEADER),
        URL_SAFE_NO_PAD.encode(claims)
    );
    let mut signature = vec![0; key.pair.public_modulus_len()];
    key.pair
        .sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            signing_input.as_bytes(),
            &mut signature,
        )
        .map_err(|_| RemoteError::AppCredential("signing failed".to_string()))?;
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::signature::{KeyPair, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
    use chrono::TimeZone;

    /// A throwaway 2048-bit key generated for these tests only. It protects
    /// nothing and belongs to no App.
    const PKCS1: &str = include_str!("../../tests/fixtures/app-key-pkcs1.pem");
    /// The same test key as PKCS#8.
    const PKCS8: &str = include_str!("../../tests/fixtures/app-key-pkcs8.pem");

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap()
    }

    fn parts(token: &str) -> Vec<String> {
        token.split('.').map(str::to_string).collect()
    }

    fn decode(part: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(part).unwrap()
    }

    #[test]
    fn jwt_header_and_claims_are_exact() {
        let key = AppKey::from_pem(PKCS1).unwrap();
        let token = app_jwt(&key, "Iv1.test", now()).unwrap();
        let p = parts(&token);
        assert_eq!(p.len(), 3);
        assert_eq!(decode(&p[0]), br#"{"alg":"RS256","typ":"JWT"}"#);
        let t = now().timestamp();
        let expected = format!(r#"{{"iat":{},"exp":{},"iss":"Iv1.test"}}"#, t - 60, t + 540);
        assert_eq!(String::from_utf8(decode(&p[1])).unwrap(), expected);
    }

    #[test]
    fn jwt_signature_is_a_known_answer() {
        let key = AppKey::from_pem(PKCS1).unwrap();
        let token = app_jwt(&key, "Iv1.test", now()).unwrap();
        assert_eq!(token, KNOWN_TOKEN);
    }

    #[test]
    fn jwt_verifies_with_the_public_key() {
        let key = AppKey::from_pem(PKCS8).unwrap();
        let token = app_jwt(&key, "Iv1.test", now()).unwrap();
        let (input, sig) = token.rsplit_once('.').unwrap();
        UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, key.pair.public_key().as_ref())
            .verify(input.as_bytes(), &decode(sig))
            .unwrap();
    }

    #[test]
    fn pkcs8_and_pkcs1_fixtures_sign_identically() {
        let a = app_jwt(&AppKey::from_pem(PKCS1).unwrap(), "Iv1.test", now()).unwrap();
        let b = app_jwt(&AppKey::from_pem(PKCS8).unwrap(), "Iv1.test", now()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn a_garbage_pem_is_refused_without_echoing_it() {
        let garbage =
            "-----BEGIN RSA PRIVATE KEY-----\nSECRETGARBAGE\n-----END RSA PRIVATE KEY-----\n";
        for input in [garbage, "SECRETGARBAGE", ""] {
            let err = AppKey::from_pem(input).unwrap_err();
            assert!(matches!(err, RemoteError::AppCredential(_)));
            assert!(!err.to_string().contains("SECRETGARBAGE"), "{err}");
        }
    }

    #[test]
    fn app_key_debug_prints_no_key_material() {
        let key = AppKey::from_pem(PKCS1).unwrap();
        assert_eq!(format!("{key:?}"), "AppKey(<redacted>)");
    }

    /// Signed with the PKCS#1 fixture and cross-checked against `openssl dgst
    /// -sha256 -sign`, which is how the pin was made independent of this code.
    const KNOWN_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJpYXQiOjE3OTMxODg3NDAsImV4cCI6MTc5MzE4OTM0MCwiaXNzIjoiSXYxLnRlc3QifQ.UF-sWdgVSSoleYMWIqVq6IAxAvAN5FgTZ1qdqWxDW3yYruaE1d3gKCAFtlKdlGnlwmWmlX_bVCUzivNrdOYLZYysOwO93BEsmOGI6srn9semEfXqUOFbCp2mr4dfHqEYW6Clvn4vox9yZivu4DrOdhDvoy1GrOIjE8rawfHFJ8rEnwBiTBYs7V1ingfGp9CpGJIuDd0axbAtcGOqPmTwp6VK0hZY10xYeGCvNvFGuXAUGuwgHj_K2zMBtG7k1szxkRXvf-GXZUsPtwQTAC48_6vJuXYr_rWaLzxeSH3mfNlueEsSUkKmX2QFPtIwuutUrO2mPtKnY-R-AILRk89qrw";
}
