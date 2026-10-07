//! Where a local Crystalline keeps its credential for each Crystalline
//! server `crystalline connect <url>` signed in to.
//!
//! The same three backends as [`crate::token`]'s GitHub credential, over the
//! same bounded keychain calls and the same test seam: the OS keychain under
//! the account `crystalline-server:<key>` beside the `github` accounts (one
//! per connected server: a machine may hold several), a
//! permissions-locked `credential.json` in the host's folder under
//! `<state_dir>/remote/<key>/` when the keychain cannot be used, and a
//! read-only value the environment supplied (`CRYSTALLINE_REMOTE_TOKEN`, read
//! by [`crate::sources::env_source`] and handed in).
//!
//! What differs is the payload. A server credential is a pasted personal MCP
//! token (`cmt_`, no expiry, nothing to refresh with) or an OAuth pair (`coa_`
//! access for an hour, `cor_` refresh for thirty days) with the registration
//! it belongs to and the resource it was minted for.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::error::RemoteError;
use crate::token::{
    KEYRING_SERVICE, KEYRING_TIMEOUT, KeyringRead, keyring_call_bounded, keyring_read,
    refuse_real_keychain, refuse_real_keychain_under_test, set_owner_only,
};

/// The keychain account prefix, namespaced away from the `github` accounts.
pub const SERVER_ACCOUNT_PREFIX: &str = "crystalline-server:";

/// The file the fallback store writes inside the host's folder.
pub const CREDENTIAL_FILE: &str = "credential.json";

/// How long before its expiry an access token is refreshed. A request that
/// starts a few seconds before the hour ends must not arrive after it.
pub const REFRESH_MARGIN_SECS: i64 = 60;

/// How this machine signed in to the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialKind {
    /// Through the server's OAuth: an access token and the refresh token that
    /// mints the next pair.
    Oauth,
    /// A pasted personal MCP token.
    Token,
}

impl CredentialKind {
    /// `"oauth"` or `"token"`, the word `status` and `doctor` print.
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialKind::Oauth => "oauth",
            CredentialKind::Token => "token",
        }
    }
}

/// The credential for one server.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCredential {
    /// How it was obtained.
    pub kind: CredentialKind,
    /// The bearer token every request carries.
    pub access_token: String,
    /// The OAuth refresh token; `None` for a pasted token.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// When the access token stops working, on THIS machine's clock. `None`
    /// for a pasted token, which lives until it is revoked.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    /// The OAuth registration a refresh is made under.
    #[serde(default)]
    pub client_id: Option<String>,
    /// The resource the token was minted for: the server's origin as its
    /// protected-resource document names it.
    pub resource: String,
    /// The account the server resolved the token to.
    pub account: String,
    /// When this credential was saved.
    pub created_at: DateTime<Utc>,
}

impl ServerCredential {
    /// A pasted personal MCP token.
    pub fn token(
        access_token: String,
        resource: String,
        account: String,
        now: DateTime<Utc>,
    ) -> ServerCredential {
        ServerCredential {
            kind: CredentialKind::Token,
            access_token,
            refresh_token: None,
            expires_at: None,
            client_id: None,
            resource,
            account,
            created_at: now,
        }
    }

    /// An OAuth pair as the token endpoint answered it. The expiry is `now`
    /// plus `expires_in`, measured here when the answer arrived, so the two
    /// machines' clocks never have to agree.
    pub fn oauth(
        access_token: String,
        refresh_token: String,
        expires_in: u64,
        client_id: String,
        resource: String,
        account: String,
        now: DateTime<Utc>,
    ) -> ServerCredential {
        let lifetime = i64::try_from(expires_in).unwrap_or(i64::MAX / 2);
        ServerCredential {
            kind: CredentialKind::Oauth,
            access_token,
            refresh_token: Some(refresh_token),
            expires_at: Some(now + TimeDelta::seconds(lifetime)),
            client_id: Some(client_id),
            resource,
            account,
            created_at: now,
        }
    }

    /// Whether the access token should be refreshed before the next request:
    /// an OAuth token within [`REFRESH_MARGIN_SECS`] of its expiry. A pasted
    /// token never is.
    pub fn needs_refresh(&self, now: DateTime<Utc>) -> bool {
        self.kind == CredentialKind::Oauth
            && self
                .expires_at
                .is_some_and(|at| at - TimeDelta::seconds(REFRESH_MARGIN_SECS) <= now)
    }
}

/// Redacts both tokens: a credential reaches a log line or a test failure far
/// more easily than anything else.
impl std::fmt::Debug for ServerCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerCredential")
            .field("kind", &self.kind)
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .field("client_id", &self.client_id)
            .field("resource", &self.resource)
            .field("account", &self.account)
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// The keychain key for a server: the authority, lower case, with every
/// character that is not a letter, a digit, `.` or `-` replaced by `_` (so
/// `host:port` and an IPv6 literal become one token), then the path of the
/// base with its slashes kept: `example.com/crystalline`. A server without a
/// path keeps exactly the 0.23.0 key, so its saved credential and cache are
/// found after an upgrade. An authority of dots alone, or none, is `_`, and
/// so is a path segment of dots alone, so no key can ever name a parent
/// folder.
pub fn server_key(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = &rest[..rest.find(['?', '#']).unwrap_or(rest.len())];
    let (authority, path) = match rest.find(['/', '\\']) {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let mut key: String = authority
        .to_ascii_lowercase()
        .chars()
        .filter(|c| *c != '[' && *c != ']')
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if key.trim_matches('.').is_empty() {
        key = "_".to_string();
    }
    for segment in path.split(['/', '\\']).filter(|s| !s.is_empty()) {
        let safe: String = segment
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        key.push('/');
        key.push_str(if safe.trim_matches('.').is_empty() {
            "_"
        } else {
            &safe
        });
    }
    key
}

/// The folder name for a key: each `/` of the path becomes `~`, which no
/// prefix may contain, so two servers never share a folder and the name is
/// one path segment on every platform.
pub fn server_folder(key: &str) -> String {
    key.replace('/', "~")
}

/// The keychain account for a server key.
pub fn server_account(key: &str) -> String {
    format!("{SERVER_ACCOUNT_PREFIX}{key}")
}

/// Where one server's credential is kept.
#[derive(Clone)]
pub enum ServerCredentialStore {
    /// The OS keychain, under the shared `crystalline` service.
    Keyring {
        /// `crystalline-server:<key>`.
        account: String,
    },
    /// `credential.json` in the host's folder.
    File {
        /// The file's path.
        path: PathBuf,
    },
    /// A token the environment supplied: read-only.
    Env {
        /// The token, as `CRYSTALLINE_REMOTE_TOKEN` carries it.
        token: String,
        /// The server origin it is for.
        resource: String,
    },
}

impl std::fmt::Debug for ServerCredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerCredentialStore::Keyring { account } => {
                f.debug_struct("Keyring").field("account", account).finish()
            }
            ServerCredentialStore::File { path } => {
                f.debug_struct("File").field("path", path).finish()
            }
            ServerCredentialStore::Env { resource, .. } => f
                .debug_struct("Env")
                .field("token", &"<redacted>")
                .field("resource", resource)
                .finish(),
        }
    }
}

impl ServerCredentialStore {
    /// Pick the backend for `key` and load what it holds, in one keychain
    /// read: [`crate::token::TokenStore::resolve_and_load`]'s rule.
    pub fn resolve_and_load(
        key: &str,
        host_dir: &Path,
    ) -> Result<(ServerCredentialStore, Option<ServerCredential>), RemoteError> {
        if refuse_real_keychain() {
            let store = ServerCredentialStore::file(host_dir);
            let credential = store.load()?;
            return Ok((store, credential));
        }
        Self::resolve_and_load_with(key, host_dir, |account| {
            keyring_read(account, KEYRING_TIMEOUT)
        })
    }

    /// [`ServerCredentialStore::resolve_and_load`] with the keychain read
    /// injected, so a test drives the unusable-backend branch without the
    /// real keychain. The kill switch is the public entry point's job, so this
    /// seam always reaches the closure.
    pub(crate) fn resolve_and_load_with(
        key: &str,
        host_dir: &Path,
        read: impl FnOnce(&str) -> KeyringRead,
    ) -> Result<(ServerCredentialStore, Option<ServerCredential>), RemoteError> {
        let account = server_account(key);
        match read(&account) {
            KeyringRead::Found(json) => Ok((
                ServerCredentialStore::Keyring { account },
                Some(from_json(&json)?),
            )),
            KeyringRead::Empty => Ok((ServerCredentialStore::Keyring { account }, None)),
            KeyringRead::Failed(_) => {
                let store = ServerCredentialStore::file(host_dir);
                let credential = store.load()?;
                Ok((store, credential))
            }
        }
    }

    /// Save `credential` for `key`: through the keychain when it takes the
    /// write, the file otherwise. Answers the store that now holds it.
    pub fn save_resolving(
        key: &str,
        host_dir: &Path,
        credential: &ServerCredential,
    ) -> Result<ServerCredentialStore, RemoteError> {
        if refuse_real_keychain() {
            let store = ServerCredentialStore::file(host_dir);
            store.save(credential)?;
            return Ok(store);
        }
        Self::save_resolving_with(key, host_dir, credential, keyring_write)
    }

    /// [`ServerCredentialStore::save_resolving`] with the keychain write
    /// injected.
    pub(crate) fn save_resolving_with(
        key: &str,
        host_dir: &Path,
        credential: &ServerCredential,
        write: impl FnOnce(&str, String) -> Result<(), String>,
    ) -> Result<ServerCredentialStore, RemoteError> {
        let account = server_account(key);
        if let Ok(json) = to_json(credential)
            && write(&account, json).is_ok()
        {
            return Ok(ServerCredentialStore::Keyring { account });
        }
        let store = ServerCredentialStore::file(host_dir);
        store.save(credential)?;
        Ok(store)
    }

    /// The file store in `host_dir`.
    pub fn file(host_dir: &Path) -> ServerCredentialStore {
        ServerCredentialStore::File {
            path: host_dir.join(CREDENTIAL_FILE),
        }
    }

    /// The read-only store over a token the environment supplied.
    pub fn env(token: String, resource: String) -> ServerCredentialStore {
        ServerCredentialStore::Env { token, resource }
    }

    /// The saved credential, or `None` when nothing is saved.
    pub fn load(&self) -> Result<Option<ServerCredential>, RemoteError> {
        match self {
            ServerCredentialStore::Keyring { account } => {
                match keyring_read(account, KEYRING_TIMEOUT) {
                    KeyringRead::Found(json) => from_json(&json).map(Some),
                    KeyringRead::Empty => Ok(None),
                    KeyringRead::Failed(e) => Err(store_error("load", e)),
                }
            }
            ServerCredentialStore::File { path } => match std::fs::read_to_string(path) {
                Ok(contents) => from_json(&contents).map(Some),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
            ServerCredentialStore::Env { token, resource } => Ok(Some(ServerCredential::token(
                token.clone(),
                resource.clone(),
                String::new(),
                Utc::now(),
            ))),
        }
    }

    /// Save `credential`, replacing what was there.
    pub fn save(&self, credential: &ServerCredential) -> Result<(), RemoteError> {
        match self {
            ServerCredentialStore::Keyring { account } => {
                keyring_write(account, to_json(credential)?).map_err(|e| store_error("save", e))
            }
            ServerCredentialStore::File { path } => {
                let json = to_json(credential)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
                std::fs::write(&tmp, json.as_bytes())?;
                set_owner_only(&tmp)?;
                std::fs::rename(&tmp, path)?;
                Ok(())
            }
            ServerCredentialStore::Env { .. } => Err(env_read_only()),
        }
    }

    /// Delete the saved credential. Deleting nothing is not an error.
    pub fn delete(&self) -> Result<(), RemoteError> {
        match self {
            ServerCredentialStore::Keyring { account } => {
                keyring_delete(account).map_err(|e| store_error("delete", e))
            }
            ServerCredentialStore::File { path } => match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
            ServerCredentialStore::Env { .. } => Err(env_read_only()),
        }
    }

    /// `"keyring"`, `"file"` or `"environment"`.
    pub fn kind(&self) -> &'static str {
        match self {
            ServerCredentialStore::Keyring { .. } => "keyring",
            ServerCredentialStore::File { .. } => "file",
            ServerCredentialStore::Env { .. } => "environment",
        }
    }
}

/// One bounded keychain write, framed for the server credential.
fn keyring_write(account: &str, json: String) -> Result<(), String> {
    refuse_real_keychain_under_test("save");
    let owned = account.to_string();
    keyring_call_bounded(KEYRING_TIMEOUT, "save", move || {
        match keyring::Entry::new(KEYRING_SERVICE, &owned) {
            Ok(entry) => entry.set_password(&json).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    })?
}

/// One bounded keychain delete. An entry that is not there is not an error.
fn keyring_delete(account: &str) -> Result<(), String> {
    refuse_real_keychain_under_test("delete");
    let owned = account.to_string();
    keyring_call_bounded(
        KEYRING_TIMEOUT,
        "delete",
        move || match keyring::Entry::new(KEYRING_SERVICE, &owned) {
            Ok(entry) => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            Err(e) => Err(e.to_string()),
        },
    )?
}

fn store_error(operation: &str, detail: impl std::fmt::Display) -> RemoteError {
    RemoteError::ServerCredential {
        detail: format!("could not {operation} the server credential: {detail}"),
    }
}

fn env_read_only() -> RemoteError {
    RemoteError::ServerCredential {
        detail: "the server credential comes from the CRYSTALLINE_REMOTE_TOKEN environment \
                 variable and is read-only; unset it to sign in with crystalline connect"
            .to_string(),
    }
}

fn to_json(credential: &ServerCredential) -> Result<String, RemoteError> {
    serde_json::to_string(credential).map_err(|e| store_error("serialize", e))
}

fn from_json(json: &str) -> Result<ServerCredential, RemoteError> {
    serde_json::from_str(json).map_err(|e| RemoteError::ServerCredential {
        detail: format!("the saved server credential is not valid: {e}"),
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn sample_oauth() -> ServerCredential {
        ServerCredential::oauth(
            "coa_access".to_string(),
            "cor_refresh".to_string(),
            3600,
            "coc_client".to_string(),
            "https://kb.example".to_string(),
            "ada".to_string(),
            now(),
        )
    }

    #[test]
    fn a_root_server_keeps_its_0_23_0_key_account_and_folder() {
        // Golden values from 0.23.0: a saved source and its keychain entry
        // must resolve unchanged after the upgrade.
        for (url, key) in [
            ("https://Team.Example.com", "team.example.com"),
            ("https://team.example.com/", "team.example.com"),
            ("http://127.0.0.1:7411/", "127.0.0.1_7411"),
            ("http://[::1]:7411", "__1_7411"),
            ("http://localhost:7411", "localhost_7411"),
        ] {
            assert_eq!(server_key(url), key, "{url}");
            assert_eq!(server_folder(key), key, "{url}");
            assert_eq!(server_account(key), format!("crystalline-server:{key}"));
        }
    }

    #[test]
    fn two_paths_on_one_host_get_two_keys_two_accounts_and_two_folders() {
        let a = server_key("https://example.com/crystalline/");
        let b = server_key("https://example.com/team/kb");
        assert_eq!(a, "example.com/crystalline");
        assert_eq!(b, "example.com/team/kb");
        assert_eq!(
            server_account(&a),
            "crystalline-server:example.com/crystalline"
        );
        assert_eq!(server_folder(&a), "example.com~crystalline");
        assert_eq!(server_folder(&b), "example.com~team~kb");
        assert_ne!(server_key("https://example.com"), a);
    }

    #[test]
    fn a_key_can_never_climb_out_of_the_remote_folder() {
        for hostile in [
            "http://..",
            "http://.",
            "http://",
            "https://../x",
            "https://x/../..",
            "https://x/a\\b",
        ] {
            let folder = server_folder(&server_key(hostile));
            assert!(!folder.is_empty(), "{hostile}");
            assert!(!folder.trim_matches('.').is_empty(), "{hostile}: {folder}");
            assert!(
                !folder.contains('/') && !folder.contains('\\'),
                "{hostile}: {folder}"
            );
        }
    }

    #[test]
    fn the_keychain_account_is_namespaced_away_from_github() {
        assert_eq!(
            server_account("kb.example"),
            "crystalline-server:kb.example"
        );
    }

    /// The expiry is this machine's clock plus the server's `expires_in`, taken
    /// when the answer arrived, so a clock that disagrees with the server's
    /// never makes a fresh token look expired or an expired one look fresh.
    #[test]
    fn the_expiry_is_measured_on_this_machine() {
        let credential = sample_oauth();
        assert_eq!(
            credential.expires_at,
            Some(now() + TimeDelta::seconds(3600)),
            "the expiry is now plus expires_in"
        );
        assert!(
            !credential.needs_refresh(now()),
            "a token just issued is fresh"
        );
        assert!(
            !credential.needs_refresh(now() + TimeDelta::seconds(3600 - REFRESH_MARGIN_SECS - 1)),
            "fresh until the margin"
        );
        assert!(
            credential.needs_refresh(now() + TimeDelta::seconds(3600 - REFRESH_MARGIN_SECS)),
            "refreshed a margin before it runs out"
        );
        let pasted = ServerCredential::token(
            "cmt_x".to_string(),
            "https://kb.example".to_string(),
            "ada".to_string(),
            now(),
        );
        assert!(
            !pasted.needs_refresh(now() + TimeDelta::days(400)),
            "a pasted token has no expiry and nothing to refresh with"
        );
    }

    #[test]
    fn debug_output_never_carries_a_token() {
        let printed = format!("{:?}", sample_oauth());
        assert!(!printed.contains("coa_access"), "{printed}");
        assert!(!printed.contains("cor_refresh"), "{printed}");
        assert!(
            printed.contains("ada") && printed.contains("kb.example"),
            "{printed}"
        );
        let env =
            ServerCredentialStore::env("cmt_secret".to_string(), "https://kb.example".to_string());
        assert!(!format!("{env:?}").contains("cmt_secret"));
    }

    #[test]
    fn the_file_store_round_trips_and_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = ServerCredentialStore::file(dir.path());
        assert_eq!(store.load().unwrap(), None, "nothing saved yet");
        store.save(&sample_oauth()).unwrap();
        assert_eq!(store.load().unwrap(), Some(sample_oauth()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(CREDENTIAL_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "no other local account reads the token"
            );
        }
        store.delete().unwrap();
        store.delete().unwrap();
        assert_eq!(
            store.load().unwrap(),
            None,
            "deleting twice is deleting once"
        );
    }

    /// A keychain that cannot be used - wedged, or absent on a headless box -
    /// lands the credential in the file beside the remote cache, and the
    /// returned store says where. The injected closures are the keychain; the
    /// flags prove the fallback was reached through them and not around them.
    #[test]
    fn an_unusable_keychain_falls_back_to_the_file_on_save_and_on_load() {
        use std::cell::Cell;
        let dir = tempfile::tempdir().unwrap();
        let wrote = Cell::new(false);
        let saved = ServerCredentialStore::save_resolving_with(
            "kb.example",
            dir.path(),
            &sample_oauth(),
            |_account, _json| {
                wrote.set(true);
                Err("no session bus".to_string())
            },
        )
        .unwrap();
        assert!(wrote.get(), "the keychain write was attempted first");
        assert_eq!(saved.kind(), "file");
        assert!(
            dir.path().join(CREDENTIAL_FILE).is_file(),
            "the file holds it"
        );
        let read = Cell::new(false);
        let (store, loaded) =
            ServerCredentialStore::resolve_and_load_with("kb.example", dir.path(), |account| {
                read.set(true);
                assert_eq!(account, "crystalline-server:kb.example");
                KeyringRead::Failed("no session bus".to_string())
            })
            .unwrap();
        assert!(read.get(), "the keychain read was attempted first");
        assert_eq!(store.kind(), "file");
        assert_eq!(loaded, Some(sample_oauth()));
    }

    /// The other half of the seam: a keychain that answers is the store, and
    /// nothing is written to the file.
    #[test]
    fn a_working_keychain_is_the_store_and_the_file_stays_empty() {
        let dir = tempfile::tempdir().unwrap();
        let saved = ServerCredentialStore::save_resolving_with(
            "kb.example",
            dir.path(),
            &sample_oauth(),
            |account, json| {
                assert_eq!(account, "crystalline-server:kb.example");
                assert!(json.contains("coa_access"));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(saved.kind(), "keyring");
        assert!(!dir.path().join(CREDENTIAL_FILE).exists());
        let json = serde_json::to_string(&sample_oauth()).unwrap();
        let (store, loaded) =
            ServerCredentialStore::resolve_and_load_with("kb.example", dir.path(), |_| {
                KeyringRead::Found(json)
            })
            .unwrap();
        assert_eq!(store.kind(), "keyring");
        assert_eq!(loaded, Some(sample_oauth()));
        let (empty, none) =
            ServerCredentialStore::resolve_and_load_with("kb.example", dir.path(), |_| {
                KeyringRead::Empty
            })
            .unwrap();
        assert_eq!(empty.kind(), "keyring");
        assert_eq!(none, None);
    }

    /// Under the test kill switch the public entry points never touch the
    /// keychain at all: they answer the file store.
    #[test]
    fn the_kill_switch_keeps_the_public_entry_points_off_the_keychain() {
        if !refuse_real_keychain() {
            return; // only meaningful with CRYSTALLINE_TEST_NO_KEYCHAIN set
        }
        let dir = tempfile::tempdir().unwrap();
        let saved =
            ServerCredentialStore::save_resolving("kb.example", dir.path(), &sample_oauth())
                .unwrap();
        assert_eq!(saved.kind(), "file");
        let (store, loaded) =
            ServerCredentialStore::resolve_and_load("kb.example", dir.path()).unwrap();
        assert_eq!(store.kind(), "file");
        assert_eq!(loaded, Some(sample_oauth()));
    }

    #[test]
    fn the_environment_store_is_read_only_and_a_token() {
        let store =
            ServerCredentialStore::env("cmt_env".to_string(), "https://kb.example".to_string());
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.kind, CredentialKind::Token);
        assert_eq!(loaded.access_token, "cmt_env");
        assert_eq!(store.kind(), "environment");
        assert!(store.save(&sample_oauth()).is_err());
        assert!(store.delete().is_err());
    }

    #[test]
    fn a_corrupt_file_is_a_credential_error_naming_the_server() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(CREDENTIAL_FILE), "{not json").unwrap();
        let err = ServerCredentialStore::file(dir.path()).load().unwrap_err();
        assert!(
            matches!(err, RemoteError::ServerCredential { .. }),
            "{err:?}"
        );
        assert!(err.to_string().contains("Crystalline server"), "{err}");
        assert!(
            !err.to_string().contains("GitHub"),
            "not the GitHub sentence: {err}"
        );
        assert!(
            !err.is_temporary(),
            "a corrupt file does not heal on its own"
        );
    }
}
