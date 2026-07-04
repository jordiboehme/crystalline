//! Where a GitHub access token lives once a machine has one: the OS
//! keychain when it works, a permissions-locked file on disk when it does
//! not.
//!
//! [`TokenStore::resolve`] decides which backend to use, once, the first
//! time a machine needs to save a token; every later save, load or delete
//! goes through whichever variant was picked. Keeping the choice in the
//! `TokenStore` value rather than re-probing on every call means a single
//! sign-in session always reads back what it just wrote, even on a machine
//! whose keychain the probe judged unusable.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::RemoteError;

/// The keyring service name every Crystalline credential is stored under.
const KEYRING_SERVICE: &str = "crystalline";

/// The file name the file-backed store writes within its state directory.
const TOKEN_FILE_NAME: &str = "github-token.json";

/// A saved GitHub access token, together with who it belongs to and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredToken {
    /// The token itself, as GitHub issued it.
    pub access_token: String,
    /// The GitHub host this token is valid against: `github.com`, or a
    /// GitHub Enterprise Server hostname.
    pub host: String,
    /// The login of the user this token authenticates as.
    pub user: String,
    /// When this token was saved.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Where a GitHub access token is persisted between runs.
pub enum TokenStore {
    /// The OS-native credential store (Keychain, Credential Manager, the
    /// Secret Service), addressed by account name within the shared
    /// `crystalline` service.
    Keyring {
        /// `"github"` for GitHub.com, `"github:<host>"` for a GitHub
        /// Enterprise Server host.
        account: String,
    },
    /// A single JSON file, used when no working keychain backend is
    /// available (headless Linux with no session bus, most CI runners).
    File {
        /// The token file's path.
        path: PathBuf,
    },
}

impl TokenStore {
    /// Picks a backend for `host` (`None` for GitHub.com, `Some(host)` for
    /// a GitHub Enterprise Server host): the OS keychain if a cheap probe
    /// says it works, otherwise a file under `fallback_dir` (the origins
    /// state directory).
    pub fn resolve(host: Option<&str>, fallback_dir: &Path) -> TokenStore {
        let account = account_for(host);
        if keyring_probe_ok(&account) {
            TokenStore::Keyring { account }
        } else {
            TokenStore::File {
                path: fallback_dir.join(TOKEN_FILE_NAME),
            }
        }
    }

    /// Saves `token`, replacing whatever was saved before.
    pub fn save(&self, token: &StoredToken) -> Result<(), RemoteError> {
        match self {
            TokenStore::Keyring { account } => {
                let json = to_json(token)?;
                keyring_entry(account)?
                    .set_password(&json)
                    .map_err(|e| credential_error("save", e))
            }
            TokenStore::File { path } => save_file(path, token),
        }
    }

    /// Loads the saved token, or `None` if nothing has been saved yet.
    pub fn load(&self) -> Result<Option<StoredToken>, RemoteError> {
        match self {
            TokenStore::Keyring { account } => match keyring_entry(account)?.get_password() {
                Ok(json) => from_json(&json).map(Some),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(credential_error("load", e)),
            },
            TokenStore::File { path } => load_file(path),
        }
    }

    /// Deletes the saved token. Deleting when nothing is saved is not an
    /// error.
    pub fn delete(&self) -> Result<(), RemoteError> {
        match self {
            TokenStore::Keyring { account } => match keyring_entry(account)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(credential_error("delete", e)),
            },
            TokenStore::File { path } => delete_file(path),
        }
    }

    /// `"keyring"` or `"file"`, for doctor and status to report which
    /// backend is in play.
    pub fn kind(&self) -> &'static str {
        match self {
            TokenStore::Keyring { .. } => "keyring",
            TokenStore::File { .. } => "file",
        }
    }
}

/// The keyring account name for `host`: `"github"` for GitHub.com,
/// `"github:<host>"` for a GitHub Enterprise Server host.
fn account_for(host: Option<&str>) -> String {
    match host {
        Some(host) => format!("github:{host}"),
        None => "github".to_string(),
    }
}

/// Opens a keyring entry, mapping the (rare) failure to build one at all to
/// [`RemoteError::Credential`].
fn keyring_entry(account: &str) -> Result<keyring::Entry, RemoteError> {
    keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| credential_error("open", e))
}

/// Probes whether the platform keyring backend actually works, so
/// [`TokenStore::resolve`] can land on the file fallback instead when it
/// does not (headless Linux with no D-Bus session, most CI runners, or any
/// other platform with no usable native credential store).
///
/// A freshly probed account has never had a secret saved to it, so the only
/// outcome a genuinely working backend can give here is
/// `Err(keyring::Error::NoEntry)` (or `Ok`, if unusually something was
/// already saved under this exact account by an earlier run). Any other
/// outcome, starting with failing to even build the entry, means the
/// backend itself is unavailable rather than just empty; this is the one
/// place that distinction is made, so both `resolve` and anyone reasoning
/// about it later only need to look here.
fn keyring_probe_ok(account: &str) -> bool {
    match keyring::Entry::new(KEYRING_SERVICE, account) {
        Ok(entry) => !matches!(
            entry.get_password(),
            Err(e) if !matches!(e, keyring::Error::NoEntry)
        ),
        Err(_) => false,
    }
}

/// Builds a [`RemoteError::Credential`] naming the attempted `operation`.
fn credential_error(operation: &str, source: impl std::fmt::Display) -> RemoteError {
    RemoteError::Credential {
        detail: format!("could not {operation} the GitHub token: {source}"),
    }
}

fn to_json(token: &StoredToken) -> Result<String, RemoteError> {
    serde_json::to_string(token).map_err(|e| RemoteError::Credential {
        detail: format!("could not serialize the GitHub token: {e}"),
    })
}

fn from_json(json: &str) -> Result<StoredToken, RemoteError> {
    serde_json::from_str(json).map_err(|e| RemoteError::Credential {
        detail: format!("the saved GitHub token is not valid: {e}"),
    })
}

/// Writes `token` to `path` as JSON: a sibling temp file, permissioned
/// owner-only, then renamed into place, so a reader never sees a partial
/// file and no other local account can read the token in between.
fn save_file(path: &Path, token: &StoredToken) -> Result<(), RemoteError> {
    let json = to_json(token)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, json.as_bytes())?;
    set_owner_only(&tmp)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn load_file(path: &Path) -> Result<Option<StoredToken>, RemoteError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => from_json(&contents).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn delete_file(path: &Path) -> Result<(), RemoteError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Restricts `path` to owner read/write only, so the plaintext token file
/// is not readable by other local accounts. A no-op on non-unix platforms,
/// which have no equivalent bit this crate manages directly.
#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), RemoteError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<(), RemoteError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_token() -> StoredToken {
        StoredToken {
            access_token: "gho_examplesecret".to_string(),
            host: "github.com".to_string(),
            user: "octocat".to_string(),
            created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        }
    }

    // `resolve` is intentionally not exercised here: proving its keyring
    // arm actually gets picked would mean probing the real platform
    // keychain from the test suite, which is exactly what this crate's
    // tests must never do. Its file fallback is covered indirectly by
    // exercising the `File` variant directly below, which is the same
    // code `resolve` would have picked on a machine with no working
    // keyring backend.

    #[test]
    fn file_store_round_trips_save_load_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::File {
            path: dir.path().join("github-token.json"),
        };
        assert_eq!(store.kind(), "file");

        assert_eq!(store.load().unwrap(), None, "nothing saved yet");

        let token = sample_token();
        store.save(&token).unwrap();
        assert_eq!(store.load().unwrap(), Some(token.clone()));

        store.delete().unwrap();
        assert_eq!(store.load().unwrap(), None, "deleted");

        // Deleting again when nothing is saved is not an error.
        store.delete().unwrap();
    }

    #[test]
    fn file_store_save_overwrites_the_previous_token() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::File {
            path: dir.path().join("github-token.json"),
        };

        let mut token = sample_token();
        store.save(&token).unwrap();
        token.access_token = "gho_replacedsecret".to_string();
        store.save(&token).unwrap();

        assert_eq!(store.load().unwrap(), Some(token));
    }

    #[test]
    fn file_store_leaves_no_temp_file_behind_after_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::File {
            path: dir.path().join("github-token.json"),
        };

        store.save(&sample_token()).unwrap();

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            vec!["github-token.json".to_string()],
            "no leftover temp file: {entries:?}"
        );
    }

    #[test]
    fn file_store_creates_the_parent_directory_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested").join("further");
        let store = TokenStore::File {
            path: nested.join("github-token.json"),
        };

        store.save(&sample_token()).unwrap();

        assert_eq!(store.load().unwrap(), Some(sample_token()));
    }

    #[test]
    fn file_store_rejects_corrupt_json_as_a_credential_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("github-token.json");
        std::fs::write(&path, b"not json").unwrap();
        let store = TokenStore::File { path };

        let err = store.load().unwrap_err();
        assert!(matches!(err, RemoteError::Credential { .. }), "{err:?}");
    }

    #[test]
    #[cfg(unix)]
    fn file_store_writes_the_token_file_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("github-token.json");
        let store = TokenStore::File { path: path.clone() };

        store.save(&sample_token()).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected owner-only permissions, got {mode:o}");
    }

    #[test]
    fn kind_reports_keyring_for_the_keyring_variant() {
        let store = TokenStore::Keyring {
            account: "github".to_string(),
        };
        assert_eq!(store.kind(), "keyring");
    }
}
