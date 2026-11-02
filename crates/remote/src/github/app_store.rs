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

/// The marker a keychain save leaves beside the file store: it holds the
/// App's host and no secret. A keychain that later fails to answer reads the
/// same as one that holds no App; the marker is how the engine tells them
/// apart, and refuses rather than fall back to the instance token.
const MARKER_FILE_NAME: &str = "github-app.present";

/// What an Environment-backed credential tells the person who tries to change
/// it: the variables are the source of truth until they are unset.
const ENV_READ_ONLY: &str = "the GitHub App credentials come from the CRYSTALLINE_GITHUB_APP_* environment variables and are read-only; unset them to manage saved credentials";

/// The lock file beside the file store that every save of an App takes, so
/// a create that must not overwrite and a set from another process do not
/// interleave between their look and their write.
const LOCK_FILE_NAME: &str = "github-app.lock";

/// An exclusive lock on a state directory's App store, held until dropped.
/// It holds no secret; the file it locks stays empty.
#[derive(Debug)]
pub struct AppStoreLock {
    file: std::fs::File,
}

impl AppStoreLock {
    /// Waits for and takes the lock of the App store in `dir`. Blocking: the
    /// holders are short (a store read and a write).
    pub fn acquire(dir: &Path) -> Result<AppStoreLock, RemoteError> {
        std::fs::create_dir_all(dir)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(LOCK_FILE_NAME))?;
        fs4::FileExt::lock(&file)?;
        Ok(AppStoreLock { file })
    }
}

impl Drop for AppStoreLock {
    fn drop(&mut self) {
        let _ = fs4::FileExt::unlock(&self.file);
    }
}

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

impl AppCredentials {
    /// The `host` argument [`AppStore::resolve_and_load`] takes: `None` for
    /// GitHub.com, the Enterprise Server host otherwise. The one place the
    /// credentials' `"github.com"` string becomes the store's `None`.
    pub fn store_host(&self) -> Option<&str> {
        (self.host != "github.com").then_some(self.host.as_str())
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

/// What [`AppStore::lookup`] found, and where.
#[derive(Debug)]
pub struct AppLookup {
    /// The backend the credentials live in, or would be saved to.
    pub store: AppStore,
    /// The saved App, or `None` when that backend holds none.
    pub credentials: Option<AppCredentials>,
    /// Why the keychain could not be read, when it could not and the file
    /// fallback answered instead. `None` after a clean keychain read and
    /// under the no-keychain test seam.
    pub keychain_failed: Option<String>,
    /// The host the marker names, read only when the keychain failed: an
    /// App was saved in that keychain for this host (see
    /// [`AppStore::save_marked`]). `None` when the keychain answered.
    pub marker: Option<String>,
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
        AppStore::lookup(host, fallback_dir).map(|found| (found.store, found.credentials))
    }

    /// [`AppStore::resolve_and_load`], also saying whether the keychain read
    /// failed and the answer came from the file fallback instead. A caller
    /// that caches "no App" must not cache it then: an App saved in a
    /// keychain that was locked or slow at that moment would read as none.
    pub fn lookup(host: Option<&str>, fallback_dir: &Path) -> Result<AppLookup, RemoteError> {
        // The test seam: never construct the real keychain backend, see
        // `token::refuse_real_keychain`. The file is then the only store
        // there is, so nothing failed.
        if refuse_real_keychain() {
            let store = AppStore::file_fallback(fallback_dir);
            let credentials = store.load()?;
            return Ok(AppLookup {
                store,
                credentials,
                keychain_failed: None,
                marker: None,
            });
        }
        Self::lookup_bounded(host, fallback_dir, |account| {
            keyring_read(account, KEYRING_TIMEOUT)
        })
    }

    /// [`AppStore::lookup`] with the keychain read injected, so a test can
    /// drive each branch without touching the real keychain.
    fn lookup_bounded(
        host: Option<&str>,
        fallback_dir: &Path,
        read: impl FnOnce(&str) -> KeyringRead,
    ) -> Result<AppLookup, RemoteError> {
        let account = account_for(host);
        match read(&account) {
            KeyringRead::Found(json) => Ok(AppLookup {
                store: AppStore::Keyring { account },
                credentials: Some(from_json(&json)?),
                keychain_failed: None,
                marker: None,
            }),
            KeyringRead::Empty => Ok(AppLookup {
                store: AppStore::Keyring { account },
                credentials: None,
                keychain_failed: None,
                marker: None,
            }),
            KeyringRead::Failed(reason) => {
                let store = AppStore::file_fallback(fallback_dir);
                let credentials = store.load()?;
                Ok(AppLookup {
                    store,
                    credentials,
                    keychain_failed: Some(reason),
                    marker: read_marker(fallback_dir),
                })
            }
        }
    }

    /// The keychain backend for `host` (`None` for GitHub.com), without
    /// reading it: for a delete that is tried while a read failed.
    pub fn keyring(host: Option<&str>) -> AppStore {
        AppStore::Keyring {
            account: account_for(host),
        }
    }

    /// The file backend under `dir`, never the keychain: what
    /// [`AppStore::resolve_and_load`] falls back to, for a caller (a test
    /// seam) that must not reach the OS credential store at all.
    pub fn file_fallback(dir: &Path) -> AppStore {
        AppStore::File {
            path: dir.join(APP_FILE_NAME),
        }
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

    /// [`AppStore::save`], plus the marker for a keychain save: written into
    /// `marker_dir` (the directory the file store lives in) before the
    /// keychain item, and a save whose marker cannot be written saves
    /// nothing. A file save writes no marker, since the file is read
    /// whatever the keychain does.
    pub fn save_marked(
        &self,
        credentials: &AppCredentials,
        marker_dir: &Path,
    ) -> Result<(), RemoteError> {
        match self {
            AppStore::Keyring { account } => {
                save_keyring_marked(account, credentials, marker_dir, |account, json| {
                    keyring_write(account, json, KEYRING_TIMEOUT)
                })
            }
            other => other.save(credentials),
        }
    }

    /// [`AppStore::delete`], then the marker in `marker_dir`, in that order:
    /// a marker outliving its keychain item only refuses until the next
    /// remove, while an item outliving its marker would be missed. Deleting
    /// the file copy takes the marker too: a remove means no App at all, and
    /// a marker left behind would keep refusing origin operations.
    pub fn delete_marked(&self, marker_dir: &Path) -> Result<(), RemoteError> {
        match self {
            AppStore::Keyring { account } => {
                delete_keyring_marked(account, marker_dir, |account| {
                    keyring_delete(account, KEYRING_TIMEOUT)
                })
            }
            AppStore::File { .. } => {
                self.delete()?;
                delete_marker(marker_dir)
            }
            AppStore::Env => self.delete(),
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
/// GitHub Enterprise Server host. `Some("github.com")` is the same account as
/// `None`, so the two spellings of GitHub.com never split one App in two. Namespaced away from the token accounts
/// (`github`, `github:<host>`) so an App never overwrites a sign-in.
fn account_for(host: Option<&str>) -> String {
    match host {
        None | Some("github.com") => "github-app".to_string(),
        Some(host) => format!("github-app:{host}"),
    }
}

/// The marker's path within `dir`.
fn marker_path(dir: &Path) -> PathBuf {
    dir.join(MARKER_FILE_NAME)
}

/// The host the marker in `dir` names, when one was written. An unreadable
/// or empty marker still says an App was saved, so it reads as an unnamed
/// host rather than none.
pub fn read_marker(dir: &Path) -> Option<String> {
    let path = marker_path(dir);
    match std::fs::read_to_string(&path) {
        Ok(host) => Some(host.trim().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => path.exists().then(String::new),
    }
}

/// Writes the marker for `host` into `dir`, through a temporary file and a
/// rename, so a crash never leaves a half-written marker behind.
fn write_marker(dir: &Path, host: &str) -> Result<(), RemoteError> {
    let path = marker_path(dir);
    let tmp = path.with_extension(format!("present.tmp.{}", std::process::id()));
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, format!("{host}\n")))
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            RemoteError::Credential {
                detail: format!("could not write the GitHub App marker: {e}"),
            }
        })
}

/// Deletes the marker in `dir`; a missing one is not an error.
fn delete_marker(dir: &Path) -> Result<(), RemoteError> {
    match std::fs::remove_file(marker_path(dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(RemoteError::Credential {
            detail: format!("could not delete the GitHub App marker: {e}"),
        }),
    }
}

/// The keychain half of [`AppStore::save_marked`], with the keychain write
/// injected so a test drives it without the real keychain.
fn save_keyring_marked(
    account: &str,
    credentials: &AppCredentials,
    marker_dir: &Path,
    write: impl FnOnce(&str, String) -> Result<(), RemoteError>,
) -> Result<(), RemoteError> {
    let json = to_json(credentials)?;
    // A marker already there stands for an item saved earlier, which a
    // failed write leaves in place; only a marker this save wrote is undone.
    let existed = marker_path(marker_dir).exists();
    write_marker(marker_dir, &credentials.host)?;
    write(account, json).inspect_err(|_| {
        if !existed {
            let _ = delete_marker(marker_dir);
        }
    })
}

/// The keychain half of [`AppStore::delete_marked`], with the keychain
/// delete injected.
fn delete_keyring_marked(
    account: &str,
    marker_dir: &Path,
    delete: impl FnOnce(&str) -> Result<(), RemoteError>,
) -> Result<(), RemoteError> {
    delete(account)?;
    delete_marker(marker_dir)
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
        assert_eq!(account_for(Some("github.com")), "github-app");
        let mut app = sample();
        assert_eq!(app.store_host(), None);
        assert_eq!(account_for(app.store_host()), "github-app");
        app.host = "ghes.example".to_string();
        assert_eq!(account_for(app.store_host()), "github-app:ghes.example");
    }

    #[test]
    fn app_keyring_branches_pick_the_backend_from_one_read() {
        let dir = tempfile::tempdir().unwrap();
        let found = AppStore::lookup_bounded(Some("ghes.example"), dir.path(), |account| {
            assert_eq!(account, "github-app:ghes.example");
            KeyringRead::Found(to_json(&sample()).unwrap())
        })
        .unwrap();
        assert_eq!(found.store.kind(), "keyring");
        assert_eq!(found.credentials, Some(sample()));
        assert_eq!(found.keychain_failed, None);

        let empty = AppStore::lookup_bounded(None, dir.path(), |_| KeyringRead::Empty).unwrap();
        assert_eq!(empty.store.kind(), "keyring");
        assert_eq!(empty.credentials, None);
        assert_eq!(
            empty.keychain_failed, None,
            "an empty keychain is an answer"
        );

        let failed = AppStore::lookup_bounded(None, dir.path(), |_| {
            KeyringRead::Failed("the keychain is locked".to_string())
        })
        .unwrap();
        assert_eq!(failed.store.kind(), "file");
        assert_eq!(failed.credentials, None);
        assert_eq!(
            failed.keychain_failed.as_deref(),
            Some("the keychain is locked"),
            "a failed read says so, so nobody caches its empty file as no App"
        );
    }

    #[test]
    fn app_lookup_under_the_no_keychain_seam_reads_the_file_and_reports_no_failure() {
        let dir = tempfile::tempdir().unwrap();
        let found = AppStore::lookup(Some("ghes.example"), dir.path()).unwrap();
        assert_eq!(found.store.kind(), "file");
        assert_eq!(found.keychain_failed, None);
    }

    #[test]
    fn a_keychain_save_writes_the_marker_before_the_item_and_a_delete_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = sample();
        app.host = "ghes.example".to_string();
        let mut written = None;
        save_keyring_marked(
            "github-app:ghes.example",
            &app,
            dir.path(),
            |account, json| {
                assert!(
                    marker_path(dir.path()).exists(),
                    "the marker is written before the keychain item"
                );
                written = Some((account.to_string(), json));
                Ok(())
            },
        )
        .unwrap();
        let (account, json) = written.unwrap();
        assert_eq!(account, "github-app:ghes.example");
        assert_eq!(from_json(&json).unwrap(), app);
        assert_eq!(read_marker(dir.path()).as_deref(), Some("ghes.example"));
        let marker = std::fs::read_to_string(marker_path(dir.path())).unwrap();
        for secret in ["CLIENTSECRETVALUE", "PRIVATEKEYBODY", "WEBHOOKSECRETVALUE"] {
            assert!(!marker.contains(secret), "the marker holds no secret");
        }

        delete_keyring_marked("github-app:ghes.example", dir.path(), |_| Ok(())).unwrap();
        assert_eq!(read_marker(dir.path()), None);
    }

    #[test]
    fn a_keychain_save_whose_marker_cannot_be_written_saves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the marker file would go: the write fails.
        std::fs::create_dir_all(marker_path(dir.path())).unwrap();
        let mut reached = false;
        let err = save_keyring_marked("github-app", &sample(), dir.path(), |_, _| {
            reached = true;
            Ok(())
        });
        assert!(err.is_err());
        assert!(!reached, "no keychain item without its marker");
    }

    #[test]
    fn a_failed_keychain_write_takes_back_the_marker_it_wrote_and_only_that() {
        let dir = tempfile::tempdir().unwrap();
        let refuse = |_: &str, _: String| {
            Err(RemoteError::Credential {
                detail: "the keychain is locked".to_string(),
            })
        };
        assert!(save_keyring_marked("github-app", &sample(), dir.path(), refuse).is_err());
        assert_eq!(read_marker(dir.path()), None, "no item, so no marker");

        // A marker from an earlier save stands for that item and stays.
        write_marker(dir.path(), "github.com").unwrap();
        assert!(save_keyring_marked("github-app", &sample(), dir.path(), refuse).is_err());
        assert_eq!(read_marker(dir.path()).as_deref(), Some("github.com"));
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(
            leftovers,
            vec!["github-app.present".to_string()],
            "no temp file left"
        );
    }

    #[test]
    fn a_failed_keychain_read_carries_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let failed = |dir: &Path| {
            AppStore::lookup_bounded(None, dir, |_| {
                KeyringRead::Failed("the keychain is locked".to_string())
            })
            .unwrap()
        };
        assert_eq!(failed(dir.path()).marker, None, "no marker, nothing saved");
        write_marker(dir.path(), "github.com").unwrap();
        assert_eq!(failed(dir.path()).marker.as_deref(), Some("github.com"));
        let empty = AppStore::lookup_bounded(None, dir.path(), |_| KeyringRead::Empty).unwrap();
        assert_eq!(
            empty.marker, None,
            "a keychain that answered needs no marker"
        );
    }

    #[test]
    fn a_file_save_writes_no_marker() {
        let dir = tempfile::tempdir().unwrap();
        let store = AppStore::file_fallback(dir.path());
        store.save_marked(&sample(), dir.path()).unwrap();
        assert_eq!(read_marker(dir.path()), None);
        assert_eq!(store.load().unwrap(), Some(sample()));
        write_marker(dir.path(), "github.com").unwrap();
        store.delete_marked(dir.path()).unwrap();
        assert_eq!(store.load().unwrap(), None);
        assert_eq!(read_marker(dir.path()), None, "a remove leaves no marker");
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
