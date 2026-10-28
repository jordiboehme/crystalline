//! Where the credentials of a GitHub App live: the OS keychain when it works,
//! a permissions-locked file when it does not, or a read-only view over the
//! `CRYSTALLINE_GITHUB_APP_*` environment variables.
//!
//! This is [`crate::token::TokenStore`]'s sibling and follows its rules: one
//! keychain read both picks the backend and returns what is stored, so a
//! machine that granted keychain access is prompted at most once; a backend
//! that cannot answer falls back to the file; the environment is the source
//! of truth when it supplies the credentials, so that backend is read-only.
//! The environment itself is read once, in the engine's overlay, never here.
//!
//! The private key and the secrets are stored as one JSON document, so a
//! single keychain item (or one file) holds a whole App.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::RemoteError;
use crate::token::{
    KEYRING_TIMEOUT, KeyringRead, delete_file, keyring_delete, keyring_read, keyring_write,
    read_secret_file, refuse_real_keychain, write_secret_file,
};

/// The file the file-backed store writes within its state directory.
const APP_FILE_NAME: &str = "github-app.json";

/// What an Environment-backed credential tells the person who tries to change
/// it: the variables are the source of truth until they are unset.
const ENV_READ_ONLY: &str = "the GitHub App credentials come from the CRYSTALLINE_GITHUB_APP_* environment variables and are read-only; unset them to manage saved credentials";

/// A GitHub App's identity and secrets, as GitHub shows them on the App's
/// settings page (and returns from the manifest flow).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppCredentials {
    /// The numeric App id.
    pub app_id: u64,
    /// The App's client id (`Iv1...` or `Iv23...`), the issuer of its JWTs.
    pub client_id: String,
    /// The App's client secret, when it was handed over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// The PEM private key the App signs its JWTs with.
    pub private_key: String,
    /// The secret GitHub signs webhook deliveries with, when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_secret: Option<String>,
    /// The App's URL slug, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// The GitHub host the App belongs to: `github.com`, or a GitHub
    /// Enterprise Server hostname.
    pub host: String,
}

/// Redacts the private key and both secrets: credentials reach `Debug` output
/// in a log line or a test failure far more easily than any deliberate print.
impl std::fmt::Debug for AppCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let masked = |set: bool| if set { "<redacted>" } else { "<none>" };
        f.debug_struct("AppCredentials")
            .field("app_id", &self.app_id)
            .field("client_id", &self.client_id)
            .field("client_secret", &masked(self.client_secret.is_some()))
            .field("private_key", &"<redacted>")
            .field("webhook_secret", &masked(self.webhook_secret.is_some()))
            .field("slug", &self.slug)
            .field("host", &self.host)
            .finish()
    }
}

/// Where the credentials of a GitHub App are persisted between runs.
#[derive(Clone, Debug)]
pub enum AppStore {
    /// The OS credential store, addressed by account within the shared
    /// `crystalline` service.
    Keyring {
        /// `"github-app"` for GitHub.com, `"github-app:<host>"` for a GitHub
        /// Enterprise Server host.
        account: String,
    },
    /// A single JSON file, used when no working keychain is available.
    File {
        /// The credentials file's path.
        path: PathBuf,
    },
    /// Credentials supplied by the `CRYSTALLINE_GITHUB_APP_*` variables:
    /// read-only. Never produced by [`AppStore::resolve_and_load`]; the
    /// caller that holds the overlay's credentials uses them directly, so
    /// this backend only exists to say "environment" and to refuse changes.
    Env,
}

impl AppStore {
    /// Picks a backend for `host` and loads whatever it already holds, in
    /// exactly one keychain read. `host` is `None` for GitHub.com,
    /// `Some(host)` for a GitHub Enterprise Server host (the same derivation
    /// the token store uses); `fallback_dir` is the state directory the file
    /// store lives under.
    ///
    /// The read follows [`crate::token::TokenStore::resolve_and_load`]: a
    /// found item is `(Keyring, Some)`, an empty keychain `(Keyring, None)`,
    /// and a backend that cannot answer (or answers too slowly) the file
    /// fallback plus whatever that file holds.
    pub fn resolve_and_load(
        host: Option<&str>,
        fallback_dir: &Path,
    ) -> Result<(AppStore, Option<AppCredentials>), RemoteError> {
        Self::resolve_and_load_bounded(host, fallback_dir, |account| {
            keyring_read(account, KEYRING_TIMEOUT)
        })
    }

    /// [`AppStore::resolve_and_load`] with the keychain read injected so a
    /// test can drive each branch without touching the real keychain.
    fn resolve_and_load_bounded(
        host: Option<&str>,
        fallback_dir: &Path,
        read: impl FnOnce(&str) -> KeyringRead,
    ) -> Result<(AppStore, Option<AppCredentials>), RemoteError> {
        // The test seam: never construct the real keychain backend, see
        // `token::refuse_real_keychain`.
        if refuse_real_keychain() {
            return Self::file_and_load(fallback_dir);
        }
        let account = account_for(host);
        match read(&account) {
            KeyringRead::Found(json) => {
                Ok((AppStore::Keyring { account }, Some(from_json(&json)?)))
            }
            KeyringRead::Empty => Ok((AppStore::Keyring { account }, None)),
            KeyringRead::Failed(_) => Self::file_and_load(fallback_dir),
        }
    }

    fn file_and_load(
        fallback_dir: &Path,
    ) -> Result<(AppStore, Option<AppCredentials>), RemoteError> {
        let store = AppStore::File {
            path: fallback_dir.join(APP_FILE_NAME),
        };
        let credentials = store.load()?;
        Ok((store, credentials))
    }

    /// Saves `credentials`, replacing whatever was saved before.
    pub fn save(&self, credentials: &AppCredentials) -> Result<(), RemoteError> {
        match self {
            AppStore::Keyring { account } => {
                keyring_write(account, to_json(credentials)?, KEYRING_TIMEOUT)
            }
            AppStore::File { path } => write_secret_file(path, &to_json(credentials)?),
            AppStore::Env => Err(env_read_only()),
        }
    }

    /// Loads the saved credentials, or `None` when nothing has been saved.
    /// The `Env` backend holds nothing of its own to load: the overlay owns
    /// those values.
    pub fn load(&self) -> Result<Option<AppCredentials>, RemoteError> {
        match self {
            AppStore::Keyring { account } => match keyring_read(account, KEYRING_TIMEOUT) {
                KeyringRead::Found(json) => from_json(&json).map(Some),
                KeyringRead::Empty => Ok(None),
                KeyringRead::Failed(e) => Err(RemoteError::Credential {
                    detail: format!("could not load the GitHub App credentials: {e}"),
                }),
            },
            AppStore::File { path } => match read_secret_file(path)? {
                Some(json) => from_json(&json).map(Some),
                None => Ok(None),
            },
            AppStore::Env => Ok(None),
        }
    }

    /// Deletes the saved credentials. Deleting when nothing is saved is not
    /// an error.
    pub fn delete(&self) -> Result<(), RemoteError> {
        match self {
            AppStore::Keyring { account } => keyring_delete(account, KEYRING_TIMEOUT),
            AppStore::File { path } => delete_file(path),
            AppStore::Env => Err(env_read_only()),
        }
    }

    /// `"keyring"`, `"file"` or `"environment"`, for doctor and status.
    pub fn kind(&self) -> &'static str {
        match self {
            AppStore::Keyring { .. } => "keyring",
            AppStore::File { .. } => "file",
            AppStore::Env => "environment",
        }
    }
}

/// The keyring account for `host`: `github-app`, or `github-app:<host>` for a
/// GitHub Enterprise Server host. Namespaced away from the token accounts
/// (`github`, `github:<host>`) so an App never overwrites a sign-in.
fn account_for(host: Option<&str>) -> String {
    match host {
        None => "github-app".to_string(),
        Some(host) => format!("github-app:{host}"),
    }
}

fn env_read_only() -> RemoteError {
    RemoteError::Credential {
        detail: ENV_READ_ONLY.to_string(),
    }
}

fn to_json(credentials: &AppCredentials) -> Result<String, RemoteError> {
    serde_json::to_string(credentials).map_err(|e| RemoteError::Credential {
        detail: format!("could not serialize the GitHub App credentials: {e}"),
    })
}

fn from_json(json: &str) -> Result<AppCredentials, RemoteError> {
    serde_json::from_str(json).map_err(|e| RemoteError::Credential {
        detail: format!("the saved GitHub App credentials are not valid: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AppCredentials {
        AppCredentials {
            app_id: 4242,
            client_id: "Iv23liSAMPLE".to_string(),
            client_secret: Some("CLIENTSECRETVALUE".to_string()),
            private_key:
                "-----BEGIN RSA PRIVATE KEY-----\nPRIVATEKEYBODY\n-----END RSA PRIVATE KEY-----\n"
                    .to_string(),
            webhook_secret: Some("WEBHOOKSECRETVALUE".to_string()),
            slug: Some("crystalline-team".to_string()),
            host: "github.com".to_string(),
        }
    }

    #[test]
    fn app_credentials_round_trip_through_the_file_store_with_mode_0600() {
        let dir = tempfile::tempdir().unwrap();
        // Through the resolver, under the no-keychain seam every test runs in.
        let (store, loaded) = AppStore::resolve_and_load(None, dir.path()).unwrap();
        assert_eq!(store.kind(), "file");
        assert_eq!(loaded, None, "nothing saved yet");

        store.save(&sample()).unwrap();
        let path = dir.path().join("github-app.json");
        assert!(path.exists(), "the file is named github-app.json");
        assert_eq!(store.load().unwrap(), Some(sample()));
        let (_, again) = AppStore::resolve_and_load(None, dir.path()).unwrap();
        assert_eq!(again, Some(sample()), "a later resolve finds it");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "owner-only, got {mode:o}");
        }

        store.delete().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.delete().unwrap();
    }

    #[test]
    fn debug_of_app_credentials_redacts_every_secret() {
        let debugged = format!("{:?}", sample());
        for secret in ["CLIENTSECRETVALUE", "PRIVATEKEYBODY", "WEBHOOKSECRETVALUE"] {
            assert!(!debugged.contains(secret), "{debugged}");
        }
        assert!(debugged.contains("4242"), "{debugged}");
        assert!(debugged.contains("Iv23liSAMPLE"), "{debugged}");
        assert!(debugged.contains("crystalline-team"), "{debugged}");
    }

    #[test]
    fn app_accounts_are_host_scoped_and_apart_from_token_accounts() {
        assert_eq!(account_for(None), "github-app");
        assert_eq!(account_for(Some("ghes.example")), "github-app:ghes.example");
    }

    #[test]
    fn app_keyring_branches_pick_the_backend_from_one_read() {
        // The test seam is bypassed on purpose by calling the bounded form
        // only when the process is not under it; under it the seam wins, so
        // this pins the account naming and the seam together.
        let dir = tempfile::tempdir().unwrap();
        let (store, _) =
            AppStore::resolve_and_load_bounded(Some("ghes.example"), dir.path(), |_| {
                panic!("the keychain is never read under the no-keychain seam")
            })
            .unwrap();
        assert_eq!(store.kind(), "file");
    }

    #[test]
    fn app_env_store_is_read_only() {
        let store = AppStore::Env;
        assert_eq!(store.kind(), "environment");
        let msg = store.save(&sample()).unwrap_err().to_string();
        assert!(msg.contains("CRYSTALLINE_GITHUB_APP_"), "{msg}");
        assert!(store.delete().is_err());
    }
}
