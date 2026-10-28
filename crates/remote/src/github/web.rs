//! The GitHub App web authorization-code flow with PKCE: building the
//! authorize url, exchanging the returned code, rotating an expiring user
//! token with its refresh token and revoking the grant.
//!
//! Sits beside [`super::auth`], whose device flow stays as it is. The token
//! endpoint is the same `POST {auth_base}/login/oauth/access_token`, and
//! `auth_base` derives from the api url the same way
//! ([`super::auth::auth_base`]), so a GitHub Enterprise Server host needs no
//! special case here. Revoking a grant is a REST API call and so takes the
//! api url instead.
//!
//! GitHub answers a refused exchange or refresh with HTTP 200 and an
//! `error` field in the body, so every token response is read for that
//! before it is read for a token.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::RemoteError;

/// The RFC 3986 unreserved characters stay as they are; everything else in a
/// query value or path segment is percent-encoded.
const URL_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

const DEFAULT_API_URL: &str = "https://api.github.com";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The tokens one exchange or refresh produced.
#[derive(Clone, PartialEq, Eq)]
pub struct WebTokens {
    pub access_token: String,
    /// Present when the app has expiring user tokens turned on. GitHub
    /// rotates it on every use, so the caller must persist the new one.
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub refresh_token_expires_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for WebTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebTokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .field("refresh_token_expires_at", &self.refresh_token_expires_at)
            .finish()
    }
}

/// A fresh PKCE `(code_verifier, code_challenge)` pair, S256. The verifier
/// is 43 characters of base64url over 32 random bytes; the challenge is the
/// base64url SHA-256 of the verifier.
pub fn pkce_pair() -> (String, String) {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// The url to send the browser to so the person can authorize the app.
/// `auth_base` comes from [`super::auth::auth_base`]; a GitHub App takes its
/// permissions from its registration, so no scope is requested.
pub fn authorize_url(
    auth_base: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> String {
    let enc = |v: &str| utf8_percent_encode(v, URL_COMPONENT).to_string();
    format!(
        "{}/login/oauth/authorize?client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256",
        auth_base.trim_end_matches('/'),
        enc(client_id),
        enc(redirect_uri),
        enc(state),
        enc(code_challenge),
    )
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    refresh_token_expires_in: Option<i64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// Exchanges the `code` GitHub sent to the callback for the person's tokens.
/// `redirect_uri` must be the one the authorize url carried and
/// `code_verifier` the verifier of the pair whose challenge it carried.
pub async fn exchange_code(
    auth_base: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<WebTokens, RemoteError> {
    let body = serde_json::json!({
        "client_id": client_id,
        "client_secret": client_secret,
        "code": code,
        "redirect_uri": redirect_uri,
        "code_verifier": code_verifier,
    });
    token_request(auth_base, &body, "the authorization code exchange").await
}

/// Trades a refresh token for a new access token. GitHub rotates the refresh
/// token on every use: the returned [`WebTokens::refresh_token`] replaces the
/// one passed in, and the old one is dead. A refresh token GitHub no longer
/// accepts (expired, already used, grant revoked) is
/// [`RemoteError::AuthExpired`].
pub async fn refresh(
    auth_base: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<WebTokens, RemoteError> {
    let body = serde_json::json!({
        "client_id": client_id,
        "client_secret": client_secret,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    });
    token_request(auth_base, &body, "the token refresh").await
}

async fn token_request(
    auth_base: &str,
    body: &serde_json::Value,
    what: &str,
) -> Result<WebTokens, RemoteError> {
    let response = client()
        .post(format!(
            "{}/login/oauth/access_token",
            auth_base.trim_end_matches('/')
        ))
        .header("Accept", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|_| RemoteError::Offline)?;
    let status = response.status();
    if !status.is_success() {
        let message = response
            .text()
            .await
            .unwrap_or_else(|_| "no error message provided".to_string());
        return Err(RemoteError::Api {
            status: status.as_u16(),
            message,
        });
    }
    let parsed: TokenResponse = response.json().await.map_err(|e| RemoteError::Api {
        status: status.as_u16(),
        message: format!("could not parse the response body: {e}"),
    })?;
    if let Some(error) = parsed.error.as_deref() {
        if error == "bad_refresh_token" {
            return Err(RemoteError::AuthExpired);
        }
        let detail = parsed.error_description.as_deref().unwrap_or(error);
        return Err(RemoteError::Api {
            status: status.as_u16(),
            message: format!("GitHub refused {what} with {error}: {detail}"),
        });
    }
    let Some(access_token) = parsed.access_token else {
        return Err(RemoteError::Api {
            status: status.as_u16(),
            message: format!("{what} returned neither a token nor an error"),
        });
    };
    let now = Utc::now();
    let after = |secs: Option<i64>| secs.map(|s| now + chrono::Duration::seconds(s));
    Ok(WebTokens {
        access_token,
        refresh_token: parsed.refresh_token,
        expires_at: after(parsed.expires_in),
        refresh_token_expires_at: after(parsed.refresh_token_expires_in),
    })
}

/// Asks GitHub to forget the whole grant the app holds for the person
/// (`DELETE /applications/{client_id}/grant`), which invalidates every token
/// issued under it. `api_url` is the REST base as in
/// [`super::GitHubProvider::new`]. A grant that is already gone counts as
/// done. Callers treat a failure as best effort.
pub async fn revoke_grant(
    api_url: Option<&str>,
    client_id: &str,
    client_secret: &str,
    access_token: &str,
) -> Result<(), RemoteError> {
    let base = api_url
        .map(|u| u.trim_end_matches('/'))
        .filter(|u| !u.is_empty())
        .unwrap_or(DEFAULT_API_URL);
    let basic = BASE64.encode(format!("{client_id}:{client_secret}"));
    let response = client()
        .delete(format!(
            "{base}/applications/{}/grant",
            utf8_percent_encode(client_id, URL_COMPONENT)
        ))
        .header("Authorization", format!("Basic {basic}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "crystalline")
        .json(&serde_json::json!({ "access_token": access_token }))
        .send()
        .await
        .map_err(|_| RemoteError::Offline)?;
    let status = response.status();
    if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    Err(RemoteError::Api {
        status: status.as_u16(),
        message: response
            .text()
            .await
            .unwrap_or_else(|_| "no error message provided".to_string()),
    })
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("the client config here is static and always valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pkce_pair_is_s256_over_a_url_safe_verifier() {
        let (verifier, challenge) = pkce_pair();
        assert_eq!(verifier.len(), 43);
        assert!(
            verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{verifier}"
        );
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected);
        assert_ne!(pkce_pair().0, verifier, "every pair is fresh");
    }

    #[test]
    fn the_rfc_7636_example_verifier_hashes_to_the_documented_challenge() {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(
            b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        ));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn the_authorize_url_carries_every_parameter_encoded() {
        let url = authorize_url(
            "https://github.com",
            "Iv1.abc",
            "https://kb.example.com/api/v1/auth/github/callback",
            "st ate&1",
            "chal-lenge_x",
        );
        assert!(
            url.starts_with("https://github.com/login/oauth/authorize?"),
            "{url}"
        );
        assert!(url.contains("client_id=Iv1.abc"), "{url}");
        assert!(
            url.contains(
                "redirect_uri=https%3A%2F%2Fkb.example.com%2Fapi%2Fv1%2Fauth%2Fgithub%2Fcallback"
            ),
            "{url}"
        );
        assert!(url.contains("state=st%20ate%261"), "{url}");
        assert!(url.contains("code_challenge=chal-lenge_x"), "{url}");
        assert!(url.ends_with("&code_challenge_method=S256"), "{url}");
    }

    #[test]
    fn a_ghes_auth_base_is_used_as_given_without_a_double_slash() {
        let url = authorize_url("https://ghe.example.com/", "c", "r", "s", "h");
        assert!(
            url.starts_with("https://ghe.example.com/login/oauth/authorize?"),
            "{url}"
        );
    }

    #[test]
    fn the_token_debug_render_redacts_both_tokens() {
        let tokens = WebTokens {
            access_token: "ghu_ACCESSSECRET".to_string(),
            refresh_token: Some("ghr_REFRESHSECRET".to_string()),
            expires_at: None,
            refresh_token_expires_at: None,
        };
        let debugged = format!("{tokens:?}");
        assert!(!debugged.contains("SECRET"), "{debugged}");
    }
}
