//! Where a GitHub access token lives once a machine has one: the OS
//! keychain when it works, a permissions-locked file on disk when it does
//! not, or a read-only view over a value the environment already supplied.
//!
//! [`TokenStore::resolve_and_load`] fuses the backend choice with the one
//! read that decides it: a single keychain `get_password` both picks the
//! store and hands back whatever token was already there, so a caller never
//! reads twice (once to probe, once to load) and a machine that has granted
//! keychain access is prompted at most once. [`TokenStore::save_resolving`]
//! is its write-side twin, writing straight through the keychain with no
//! pre-save probe read and landing in the file store only when the keychain
//! write itself fails. Keeping the chosen backend in the returned
//! `TokenStore` value means a single sign-in session always reads back what
//! it just wrote, even on a machine whose keychain the read judged unusable.
//!
//! [`TokenStore::env`] builds the third backend directly rather than through
//! a keychain read: this crate never reads the process environment itself
//! (`CRYSTALLINE_GITHUB_TOKEN` is read exactly once, in
//! `crystalline_service::overlay::EnvOverlay::from_process_env`), so a caller
//! that already has the value in hand constructs the store explicitly.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::RemoteError;

/// The keyring service name every Crystalline credential is stored under.
const KEYRING_SERVICE: &str = "crystalline";

/// The file name the file-backed store writes within its state directory for
/// the instance credential. Byte-identical to what every existing install
/// already carries: a personal token gets its own name beside it rather than
/// changing this one.
const TOKEN_FILE_NAME: &str = "github-token.json";

/// The longest personal identity name a credential is addressed by. Account
/// names come from the auth layer already trimmed and lowercased, so this is a
/// sanity ceiling rather than a policy: a keyring account name and a file name
/// both stay comfortably short.
const MAX_IDENTITY_NAME_BYTES: usize = 128;

/// Which credential a store addresses: the instance-wide token, or one
/// person's personal token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenIdentity {
    /// The machine-wide credential `crystalline connect github` manages.
    Instance,
    /// A personal credential bound to a crystalline account name (already
    /// trimmed and lowercased by the account layer) - or the fixed local
    /// name `owner` for the CLI/stdio machine owner.
    Personal(String),
}

/// A saved GitHub access token, together with who it belongs to and where.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl StoredToken {
    /// The signed-in login for display, or `None` when it is unknown. Only
    /// [`TokenStore::Env`] ever produces an empty `user` (an
    /// environment-supplied token with no login lookup behind it); every
    /// other store fills it in at connect time, so this is the one place
    /// that distinction needs making, rather than every caller checking
    /// `user.is_empty()` itself.
    pub fn user_display(&self) -> Option<&str> {
        if self.user.is_empty() {
            None
        } else {
            Some(self.user.as_str())
        }
    }
}

/// Redacts `access_token`: a `StoredToken` reaches `Debug` output in a log
/// line, a panic message or a test failure far more easily than it reaches
/// `Display` (which this type has none of), so the derive is deliberately not
/// used here. Every other field is plain, non-secret metadata and prints as
/// normal.
impl std::fmt::Debug for StoredToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredToken")
            .field("access_token", &"<redacted>")
            .field("host", &self.host)
            .field("user", &self.user)
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// Where a GitHub access token is persisted between runs. `Clone` so the
/// engine can cache a resolved store for the process lifetime and hand out
/// copies without re-resolving; the manual redacting `Debug` below stays.
#[derive(Clone)]
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
    /// A token supplied directly by the `CRYSTALLINE_GITHUB_TOKEN`
    /// environment variable: read-only, since the environment is the source
    /// of truth and there is nothing here for `save` or `delete` to change.
    /// Never produced by [`TokenStore::resolve_and_load`] or
    /// [`TokenStore::save_resolving`]; built directly with [`TokenStore::env`]
    /// by a caller that already holds the value (see the module docs).
    ///
    /// Instance-only, always: the environment supplies the machine's own
    /// credential and never a personal one. One process serves everybody who
    /// reaches it, so a single environment variable cannot mean "alice's
    /// token" for one request and "bob's" for the next -
    /// [`TokenIdentity::Personal`] is resolved through the keyring or the
    /// file store and never through here.
    Env {
        /// The token value, exactly as `CRYSTALLINE_GITHUB_TOKEN` carries it.
        token: String,
        /// The GitHub host this token authenticates against: `github.com`,
        /// or a GitHub Enterprise Server hostname.
        host: String,
    },
}

/// Redacts the `token` field of [`TokenStore::Env`] the same way
/// [`StoredToken`]'s manual `Debug` redacts `access_token`: the other two
/// variants carry no secret, so their fields print as a derive would.
impl std::fmt::Debug for TokenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenStore::Keyring { account } => {
                f.debug_struct("Keyring").field("account", account).finish()
            }
            TokenStore::File { path } => f.debug_struct("File").field("path", path).finish(),
            TokenStore::Env { host, .. } => f
                .debug_struct("Env")
                .field("token", &"<redacted>")
                .field("host", host)
                .finish(),
        }
    }
}

impl TokenStore {
    /// Picks a backend for `host` and loads whatever token it already holds,
    /// in exactly one keychain read. `host` is `None` for GitHub.com,
    /// `Some(host)` for a GitHub Enterprise Server host; `fallback_dir` is the
    /// origins state directory the file store lives under.
    ///
    /// The single `get_password` both chooses the store and returns its
    /// contents: `Ok(json)` means the keychain works and already holds a
    /// token, so `(Keyring, Some(token))`; `Err(NoEntry)` means the keychain
    /// works but is empty, so `(Keyring, None)` (an absent item never prompts
    /// on macOS, so a machine that has not connected yet is re-read freely);
    /// anything else - including failing to even build the entry - means the
    /// backend itself is unusable (headless Linux with no session bus, most CI
    /// runners), so the file fallback plus whatever that file holds. Callers
    /// that need both a token and the backend choice - every origin operation
    /// and the offline connection probe - get both here without the old
    /// probe-then-load double read that made every such call two keychain
    /// touches, one dialog each until the user grants "Always Allow".
    pub fn resolve_and_load(
        host: Option<&str>,
        fallback_dir: &Path,
    ) -> Result<(TokenStore, Option<StoredToken>), RemoteError> {
        TokenStore::resolve_and_load_for(&TokenIdentity::Instance, host, fallback_dir)
    }

    /// [`TokenStore::resolve_and_load`] for one identity: the same single
    /// keychain read, against the account [`account_for_identity`] names and
    /// with the file fallback [`file_fallback_for`] names, so the instance
    /// credential and every personal one are separate credentials rather than
    /// one credential several callers overwrite in turn.
    ///
    /// A personal name that could escape the fallback directory is refused
    /// here, before any backend is touched.
    pub fn resolve_and_load_for(
        identity: &TokenIdentity,
        host: Option<&str>,
        fallback_dir: &Path,
    ) -> Result<(TokenStore, Option<StoredToken>), RemoteError> {
        let account = account_for_identity_checked(identity, host)?;
        let read = keyring::Entry::new(KEYRING_SERVICE, &account)
            .ok()
            .map(|entry| entry.get_password());
        match read {
            Some(Ok(json)) => Ok((TokenStore::Keyring { account }, Some(from_json(&json)?))),
            Some(Err(keyring::Error::NoEntry)) => Ok((TokenStore::Keyring { account }, None)),
            _ => {
                let store = file_fallback_for(identity, fallback_dir)?;
                let token = store.load()?;
                Ok((store, token))
            }
        }
    }

    /// Saves `token` for `host` and returns the store that now holds it,
    /// writing through the keychain when it works and the file under
    /// `fallback_dir` when it does not - without the probe read
    /// [`TokenStore::resolve_and_load`] does, since a save has the value in
    /// hand and learns the same thing from the write itself.
    ///
    /// The keychain is tried first with a direct `set_password`; any failure
    /// (or failing to build the entry, or serialize the token) lands the token
    /// in the file store instead. The trade-off is deliberate: a keychain that
    /// reads fine but fails this one write puts the token in the file, and the
    /// user simply retries connect - judged far rarer and more recoverable
    /// than re-probing on every save, which is the prompt storm this module
    /// exists to avoid.
    pub fn save_resolving(
        host: Option<&str>,
        fallback_dir: &Path,
        token: &StoredToken,
    ) -> Result<TokenStore, RemoteError> {
        TokenStore::save_resolving_for(&TokenIdentity::Instance, host, fallback_dir, token)
    }

    /// [`TokenStore::save_resolving`] for one identity: the same keychain-first
    /// write, against the per-identity account and file names, so saving
    /// alice's personal token never disturbs the instance credential or bob's.
    ///
    /// A personal name that could escape the fallback directory is refused
    /// here, before any backend is touched: the refusal names the account and
    /// nothing is written.
    pub fn save_resolving_for(
        identity: &TokenIdentity,
        host: Option<&str>,
        fallback_dir: &Path,
        token: &StoredToken,
    ) -> Result<TokenStore, RemoteError> {
        let account = account_for_identity_checked(identity, host)?;
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &account)
            && let Ok(json) = to_json(token)
            && entry.set_password(&json).is_ok()
        {
            return Ok(TokenStore::Keyring { account });
        }
        let store = file_fallback_for(identity, fallback_dir)?;
        store.save(token)?;
        Ok(store)
    }

    /// Builds the read-only [`TokenStore::Env`] backend for a token the
    /// caller already holds (from `CRYSTALLINE_GITHUB_TOKEN`, read once by
    /// `crystalline_service::overlay::EnvOverlay`). `host` defaults to
    /// `"github.com"` when `None`, matching every other host-defaulting site
    /// in this module (see [`account_for`]).
    pub fn env(token: impl Into<String>, host: Option<&str>) -> TokenStore {
        TokenStore::Env {
            token: token.into(),
            host: host
                .map(str::to_string)
                .unwrap_or_else(|| "github.com".to_string()),
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
            TokenStore::Env { .. } => Err(env_read_only_error()),
        }
    }

    /// Loads the saved token, or `None` if nothing has been saved yet. The
    /// `Env` variant never has "nothing saved": it always synthesizes a
    /// token from the value it was built with, an empty `user` (unknown
    /// offline - nothing in this crate ever calls GitHub just to look up who
    /// an environment-supplied token belongs to) and a fresh `created_at`
    /// (never displayed anywhere; only `user`, `kind` and the fact that a
    /// token exists at all ever surface).
    pub fn load(&self) -> Result<Option<StoredToken>, RemoteError> {
        match self {
            TokenStore::Keyring { account } => match keyring_entry(account)?.get_password() {
                Ok(json) => from_json(&json).map(Some),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(credential_error("load", e)),
            },
            TokenStore::File { path } => load_file(path),
            TokenStore::Env { token, host } => Ok(Some(StoredToken {
                access_token: token.clone(),
                host: host.clone(),
                user: String::new(),
                created_at: chrono::Utc::now(),
            })),
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
            TokenStore::Env { .. } => Err(env_read_only_error()),
        }
    }

    /// `"keyring"`, `"file"` or `"environment"`, for doctor and status to
    /// report which backend is in play.
    pub fn kind(&self) -> &'static str {
        match self {
            TokenStore::Keyring { .. } => "keyring",
            TokenStore::File { .. } => "file",
            TokenStore::Env { .. } => "environment",
        }
    }
}

/// The keyring account name for `identity` at `host`, the single site the
/// account naming scheme lives:
///
/// - the instance credential keeps exactly the names it has always had,
///   `"github"` for GitHub.com and `"github:<host>"` for a GitHub Enterprise
///   Server host, so an install that connected before personal tokens existed
///   still resolves its token;
/// - a personal credential is namespaced away from those,
///   `"github-personal:<name>"` and `"github-personal:<name>:<host>"`.
///
/// Callers that may be handed an unvalidated name go through
/// [`account_for_identity_checked`] instead; this function assumes the name is
/// already sound.
fn account_for_identity(identity: &TokenIdentity, host: Option<&str>) -> String {
    match (identity, host) {
        (TokenIdentity::Instance, None) => "github".to_string(),
        (TokenIdentity::Instance, Some(host)) => format!("github:{host}"),
        (TokenIdentity::Personal(name), None) => format!("github-personal:{name}"),
        (TokenIdentity::Personal(name), Some(host)) => format!("github-personal:{name}:{host}"),
    }
}

/// [`account_for_identity`] with the name checked first, for the two resolving
/// constructors: they take an identity from a caller and must refuse a bad one
/// before a keychain entry is opened or a path is joined.
fn account_for_identity_checked(
    identity: &TokenIdentity,
    host: Option<&str>,
) -> Result<String, RemoteError> {
    check_identity(identity)?;
    Ok(account_for_identity(identity, host))
}

/// Whether `name` is safe to address a credential by. Account names reach here
/// already lowercased and `[a-z0-9._-]`-safe from the auth layer, so this is
/// the belt to that layer's braces: a name that is empty, over-long or carries
/// a path separator or a NUL would otherwise decide where a token file lands.
fn valid_identity_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_IDENTITY_NAME_BYTES
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// Refuses an identity whose name could address something other than its own
/// credential. [`RemoteError::Refused`] rather than a credential error: the
/// caller's own request is at fault, and the message names the account it
/// asked for so the caller can see which one. The token itself is never part
/// of this - or of any other - message.
fn check_identity(identity: &TokenIdentity) -> Result<(), RemoteError> {
    match identity {
        TokenIdentity::Instance => Ok(()),
        TokenIdentity::Personal(name) if valid_identity_name(name) => Ok(()),
        TokenIdentity::Personal(name) => Err(RemoteError::Refused(format!(
            "\"{name}\" is not a usable account name for a personal GitHub token; use the account name Crystalline knows this person by."
        ))),
    }
}

/// Opens a keyring entry, mapping the (rare) failure to build one at all to
/// [`RemoteError::Credential`].
fn keyring_entry(account: &str) -> Result<keyring::Entry, RemoteError> {
    keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| credential_error("open", e))
}

/// The file-backed store for `identity` under `fallback_dir`, the single place
/// a token file's location is derived so [`TokenStore::resolve_and_load_for`]
/// and [`TokenStore::save_resolving_for`] never disagree about where the
/// fallback lives. The instance file keeps its long-standing
/// `github-token.json`; a personal one is `github-token-personal-<name>.json`
/// beside it.
///
/// Public because a host process that redirects the token directory - the
/// service engine's store override - maps identities through here rather than
/// rebuilding the naming scheme. It returns a `Result` for the same reason:
/// this is where a name becomes a path, so this is where a name that could
/// escape `fallback_dir` is refused.
pub fn file_fallback_for(
    identity: &TokenIdentity,
    fallback_dir: &Path,
) -> Result<TokenStore, RemoteError> {
    check_identity(identity)?;
    let name = match identity {
        TokenIdentity::Instance => TOKEN_FILE_NAME.to_string(),
        TokenIdentity::Personal(name) => format!("github-token-personal-{name}.json"),
    };
    Ok(TokenStore::File {
        path: fallback_dir.join(name),
    })
}

/// Builds a [`RemoteError::Credential`] naming the attempted `operation`.
fn credential_error(operation: &str, source: impl std::fmt::Display) -> RemoteError {
    RemoteError::Credential {
        detail: format!("could not {operation} the GitHub token: {source}"),
    }
}

/// The refusal [`TokenStore::save`] and [`TokenStore::delete`] return for the
/// `Env` variant: the environment is the source of truth for this token, so
/// there is nothing here to save or delete until the variable is unset.
fn env_read_only_error() -> RemoteError {
    RemoteError::Credential {
        detail: "the GitHub token comes from the CRYSTALLINE_GITHUB_TOKEN environment variable and is read-only; unset it to manage a saved token".to_string(),
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

    // The keyring arms of `resolve_and_load` and `save_resolving` are
    // intentionally never exercised here: proving them would mean reading,
    // writing or prompting the real platform keychain from the test suite,
    // which is exactly what this crate's tests must never do. Their file
    // fallback is covered by driving the `File` variant directly below and by
    // `file_fallback_store_round_trips_under_the_fallback_dir`, which exercises
    // the exact store both functions build on a machine with no usable keyring
    // backend.

    #[test]
    fn personal_and_instance_tokens_live_in_separate_files() {
        let dir = tempfile::tempdir().unwrap();
        let instance = file_fallback_for(&TokenIdentity::Instance, dir.path()).unwrap();
        let alice =
            file_fallback_for(&TokenIdentity::Personal("alice".to_string()), dir.path()).unwrap();

        // The instance file name is the one existing installs already carry;
        // a personal token gets its own file beside it.
        match (&instance, &alice) {
            (TokenStore::File { path: instance }, TokenStore::File { path: alice }) => {
                assert_eq!(instance, &dir.path().join("github-token.json"));
                assert_eq!(
                    alice,
                    &dir.path().join("github-token-personal-alice.json"),
                    "a personal token lands in its own file"
                );
            }
            other => panic!("expected two file stores, got {other:?}"),
        }

        // Saving one leaves the other untouched: the two are separate
        // credentials, not two views of one.
        alice.save(&sample_token()).unwrap();
        assert_eq!(alice.load().unwrap(), Some(sample_token()));
        assert_eq!(
            instance.load().unwrap(),
            None,
            "the instance token is not what alice just saved"
        );
    }

    #[test]
    fn account_names_are_identity_and_host_scoped() {
        assert_eq!(
            account_for_identity(&TokenIdentity::Instance, None),
            "github"
        );
        assert_eq!(
            account_for_identity(&TokenIdentity::Instance, Some("ghes.example")),
            "github:ghes.example"
        );
        assert_eq!(
            account_for_identity(&TokenIdentity::Personal("alice".to_string()), None),
            "github-personal:alice"
        );
        assert_eq!(
            account_for_identity(
                &TokenIdentity::Personal("alice".to_string()),
                Some("ghes.example")
            ),
            "github-personal:alice:ghes.example"
        );
    }

    #[test]
    fn a_path_escaping_identity_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let err = TokenStore::save_resolving_for(
            &TokenIdentity::Personal("../x".to_string()),
            None,
            dir.path(),
            &sample_token(),
        )
        .unwrap_err();

        let msg = err.to_string();
        assert!(matches!(err, RemoteError::Refused(_)), "{err:?}");
        assert!(msg.contains("../x"), "the refusal names the account: {msg}");
        assert!(
            !msg.contains("gho_"),
            "a refusal never carries the token: {msg}"
        );

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(entries.is_empty(), "nothing was written: {entries:?}");
    }

    #[test]
    fn file_fallback_store_round_trips_under_the_fallback_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = file_fallback_for(&TokenIdentity::Instance, dir.path()).unwrap();
        assert_eq!(store.kind(), "file");
        // The fallback lands at the documented file name under the given dir,
        // so a save here is found again by a later resolve on the same machine.
        match &store {
            TokenStore::File { path } => assert_eq!(path, &dir.path().join(TOKEN_FILE_NAME)),
            other => panic!("expected a file store, got {other:?}"),
        }

        assert_eq!(store.load().unwrap(), None, "nothing saved yet");
        let token = sample_token();
        store.save(&token).unwrap();
        assert_eq!(store.load().unwrap(), Some(token));
    }

    #[test]
    fn token_store_clone_preserves_backend_and_redaction() {
        // The engine caches a cloned store, so a clone must keep each variant's
        // identity and, for the secret-carrying variant, its redaction.
        let file = TokenStore::File {
            path: PathBuf::from("/nonexistent/github-token.json"),
        };
        assert_eq!(file.clone().kind(), "file");

        let keyring = TokenStore::Keyring {
            account: "github".to_string(),
        };
        assert_eq!(keyring.clone().kind(), "keyring");

        let env = TokenStore::env("gho_SECRETSECRET", Some("ghe.example.com"));
        let cloned = env.clone();
        assert_eq!(cloned.kind(), "environment");
        let debugged = format!("{cloned:?}");
        assert!(!debugged.contains("SECRET"), "{debugged}");
        assert!(debugged.contains("<redacted>"), "{debugged}");
    }

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
    fn debug_redacts_the_access_token_but_keeps_the_other_fields() {
        let debugged = format!("{:?}", sample_token());
        assert!(!debugged.contains("gho_examplesecret"), "{debugged}");
        assert!(debugged.contains("<redacted>"), "{debugged}");
        assert!(debugged.contains("octocat"), "{debugged}");
    }

    #[test]
    fn kind_reports_keyring_for_the_keyring_variant() {
        let store = TokenStore::Keyring {
            account: "github".to_string(),
        };
        assert_eq!(store.kind(), "keyring");
    }

    // --- the Env variant ------------------------------------------------------

    #[test]
    fn env_store_round_trips_the_token_and_host_with_an_empty_user() {
        let store = TokenStore::env("gho_SECRETSECRET", Some("ghe.example.com"));
        assert_eq!(store.kind(), "environment");

        let token = store
            .load()
            .unwrap()
            .expect("the env store always has a token");
        assert_eq!(token.access_token, "gho_SECRETSECRET");
        assert_eq!(token.host, "ghe.example.com");
        assert_eq!(token.user, "", "the login is unknown offline");
        assert_eq!(token.user_display(), None);
    }

    #[test]
    fn env_store_defaults_the_host_to_github_com() {
        let store = TokenStore::env("gho_SECRETSECRET", None);
        let token = store.load().unwrap().unwrap();
        assert_eq!(token.host, "github.com");
    }

    #[test]
    fn env_store_save_returns_the_guidance_error() {
        let store = TokenStore::env("gho_SECRETSECRET", None);
        let err = store.save(&sample_token()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("CRYSTALLINE_GITHUB_TOKEN"), "{msg}");
        assert!(msg.contains("read-only"), "{msg}");
        assert!(matches!(err, RemoteError::Credential { .. }));
    }

    #[test]
    fn env_store_delete_returns_the_guidance_error() {
        let store = TokenStore::env("gho_SECRETSECRET", None);
        let err = store.delete().unwrap_err();
        assert!(
            err.to_string().contains("CRYSTALLINE_GITHUB_TOKEN"),
            "{err}"
        );
        assert!(matches!(err, RemoteError::Credential { .. }));
    }

    #[test]
    fn env_store_debug_never_shows_the_token_or_a_prefix_of_it() {
        let store = TokenStore::env("gho_SECRETSECRET", Some("ghe.example.com"));
        let debugged = format!("{store:?}");
        assert!(!debugged.contains("SECRET"), "{debugged}");
        assert!(debugged.contains("<redacted>"), "{debugged}");
        assert!(debugged.contains("ghe.example.com"), "{debugged}");

        // The synthesized `StoredToken` redacts the same way, through the
        // existing manual `Debug` impl.
        let token = store.load().unwrap().unwrap();
        let token_debugged = format!("{token:?}");
        assert!(!token_debugged.contains("SECRET"), "{token_debugged}");
        assert!(token_debugged.contains("<redacted>"), "{token_debugged}");
    }
}
