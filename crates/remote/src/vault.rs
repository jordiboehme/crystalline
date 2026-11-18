//! The sealed vault: where the credential stores keep their secrets when the
//! instance runs on Postgres.
//!
//! [`SecretVault`] is the seam: it moves sealed bytes by row name and knows
//! nothing about what they hold. This crate never depends on the index, so
//! the implementation over the database lives in the engine, which installs
//! it here with [`install`]. The three credential stores
//! ([`crate::TokenStore`], [`crate::AppStore`] and
//! [`crate::ServerCredentialStore`]) each have a `Vault { name }` arm and ask
//! the registry on every call.
//!
//! The resolution order is Env > Vault > Keyring > File. A caller that holds
//! an environment-supplied credential builds the `Env` store itself, so the
//! vault is never asked. Otherwise an installed vault answers, and the
//! keychain and the file store are not touched at all.
//!
//! The backend decides where secrets live, never the key: a process on
//! Postgres calls [`require`], and from then on a store call with no
//! installed vault fails with [`RemoteError::VaultUnavailable`] instead of
//! falling back to the keychain or a file. A process without the key
//! therefore never reads, writes, deletes or checks a vault row, and never
//! writes a secret to local disk either.
//!
//! The registry is a resettable `RwLock` rather than a `OnceLock`, so one
//! test's vault does not leak into the next under `cargo test`.

use std::sync::{Arc, RwLock};

use zeroize::Zeroizing;

use crate::error::RemoteError;
use crate::sealed::{self, SealKey};

/// One sealed row as the vault keeps it.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedRow {
    /// `0x01 || nonce || ciphertext and tag`, see [`crate::sealed`].
    pub sealed: Vec<u8>,
    /// The id of the key the row was sealed with ([`SealKey::key_id`]).
    pub key_id: String,
}

/// Keeps the sealed bytes out of `Debug` output.
impl std::fmt::Debug for SealedRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedRow")
            .field("sealed", &format_args!("<{} bytes>", self.sealed.len()))
            .field("key_id", &self.key_id)
            .finish()
    }
}

/// What the vault holds, without a single row name: what `status` and
/// `doctor` report and what the start checks compare with the key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VaultSummary {
    /// How many rows the vault holds.
    pub rows: usize,
    /// The distinct key ids the rows were sealed with, sorted.
    pub key_ids: Vec<String>,
}

/// Sealed secrets by row name. The row names are the keychain account names:
/// `github`, `github:<host>`, `github-personal:<name>[:<host>]`,
/// `github-app[:<host>]` and `crystalline-server:<key>`.
///
/// Every method blocks; an implementation bounds how long.
pub trait SecretVault: Send + Sync {
    /// The row named `name`, or `None` when there is none.
    fn load(&self, name: &str) -> Result<Option<SealedRow>, RemoteError>;
    /// Writes the row named `name`, replacing what was there.
    fn save(&self, name: &str, sealed: &[u8], key_id: &str) -> Result<(), RemoteError>;
    /// Deletes the row named `name`; `true` when there was one.
    fn delete(&self, name: &str) -> Result<bool, RemoteError>;
    /// How many rows there are and which keys sealed them.
    fn summary(&self) -> Result<VaultSummary, RemoteError>;
}

/// An installed vault and the key it seals with.
type Installed = (Arc<dyn SecretVault>, Arc<SealKey>);

struct Registry {
    required: bool,
    installed: Option<Installed>,
}

static REGISTRY: RwLock<Registry> = RwLock::new(Registry {
    required: false,
    installed: None,
});

/// Installs `vault` with the key that seals its rows. Every store call from
/// now on goes to it.
pub fn install(vault: Arc<dyn SecretVault>, key: SealKey) {
    let mut registry = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    registry.installed = Some((vault, Arc::new(key)));
}

/// Marks this process as one whose secrets live in the vault: from now on a
/// store call with no installed vault fails with
/// [`RemoteError::VaultUnavailable`] and never falls back to the keychain or
/// a file.
pub fn require() {
    REGISTRY.write().unwrap_or_else(|e| e.into_inner()).required = true;
}

/// Removes the installed vault and the requirement.
pub fn clear() {
    let mut registry = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    registry.required = false;
    registry.installed = None;
}

/// Whether this process keeps its secrets in the vault: one is installed or
/// required. A caller that would otherwise read or write the keychain or the
/// state folder itself (a marker, a lock file, a file copy) asks this first
/// and leaves them alone.
pub fn engaged() -> bool {
    let registry = REGISTRY.read().unwrap_or_else(|e| e.into_inner());
    registry.required || registry.installed.is_some()
}

/// The id of the key the installed vault seals with ([`SealKey::key_id`]),
/// `None` when none is installed. Not secret: what a daemon tells a CLI
/// process that seals beside it, so a process with another key is refused.
pub fn installed_key_id() -> Option<String> {
    let registry = REGISTRY.read().unwrap_or_else(|e| e.into_inner());
    registry.installed.as_ref().map(|(_, key)| key.key_id())
}

/// The installed vault and its key; `None` when no vault is required and
/// none is installed, so the keychain and the file store answer as before.
pub(crate) fn active() -> Result<Option<Installed>, RemoteError> {
    let registry = REGISTRY.read().unwrap_or_else(|e| e.into_inner());
    match (&registry.installed, registry.required) {
        (Some(installed), _) => Ok(Some(installed.clone())),
        (None, true) => Err(RemoteError::VaultUnavailable),
        (None, false) => Ok(None),
    }
}

/// The installed vault for a `Vault { name }` store, which has no other
/// place to go.
fn installed() -> Result<Installed, RemoteError> {
    active()?.ok_or(RemoteError::VaultUnavailable)
}

/// Opens the row `name` with `key`. A row sealed with another key reads as
/// absent and is kept; so is a row that does not open although its key id
/// matches, which is logged by name.
pub(crate) fn open_row(
    vault: &dyn SecretVault,
    key: &SealKey,
    name: &str,
) -> Result<Option<Zeroizing<Vec<u8>>>, RemoteError> {
    let Some(row) = vault.load(name)? else {
        return Ok(None);
    };
    let key_id = key.key_id();
    if row.key_id != key_id {
        tracing::warn!(
            name,
            row_key_id = %row.key_id,
            key_id = %key_id,
            "a sealed secret was sealed with another key; it reads as absent and is kept"
        );
        return Ok(None);
    }
    match sealed::open(key, name, &row.sealed) {
        Ok(plain) => Ok(Some(Zeroizing::new(plain))),
        Err(e) => {
            tracing::warn!(name, error = %e, "a sealed secret did not open; it reads as absent");
            Ok(None)
        }
    }
}

/// Seals `plain` for the row `name` and writes it. A row already there that
/// was sealed with another key is refused, never replaced.
pub(crate) fn seal_row(
    vault: &dyn SecretVault,
    key: &SealKey,
    name: &str,
    plain: &[u8],
) -> Result<(), RemoteError> {
    let key_id = key.key_id();
    refuse_other_key(vault, name, &key_id)?;
    vault.save(name, &sealed::seal(key, name, plain), &key_id)
}

/// Refuses a change to the row `name` when it was sealed with a key other
/// than `key_id`: such a row is kept as it is until the key it was sealed
/// with is back.
fn refuse_other_key(vault: &dyn SecretVault, name: &str, key_id: &str) -> Result<(), RemoteError> {
    match vault.load(name)? {
        Some(row) if row.key_id != key_id => Err(RemoteError::VaultKeyMismatch {
            name: name.to_string(),
            row_key_id: row.key_id,
            key_id: key_id.to_string(),
        }),
        _ => Ok(()),
    }
}

/// `Ok` when this process keeps its secrets in the keychain or a file;
/// [`RemoteError::VaultUnavailable`] when a vault is installed or required,
/// so a keychain or file store built or kept around the vault touches
/// nothing.
pub(crate) fn refuse_local() -> Result<(), RemoteError> {
    match active()? {
        Some(_) => Err(RemoteError::VaultUnavailable),
        None => Ok(()),
    }
}

/// The plain text of the row `name` in the installed vault, for a
/// `Vault { name }` store's `load`.
pub(crate) fn load_text(name: &str) -> Result<Option<Zeroizing<String>>, RemoteError> {
    let (vault, key) = installed()?;
    open_row(vault.as_ref(), &key, name)?
        .map(|plain| {
            String::from_utf8(plain.to_vec())
                .map(Zeroizing::new)
                .map_err(|_| RemoteError::Credential {
                    detail: format!("the sealed secret {name} is not text"),
                })
        })
        .transpose()
}

/// Seals `text` into the row `name` of the installed vault, for a
/// `Vault { name }` store's `save`.
pub(crate) fn save_text(name: &str, text: &str) -> Result<(), RemoteError> {
    let (vault, key) = installed()?;
    seal_row(vault.as_ref(), &key, name, text.as_bytes())
}

/// Deletes the row `name` of the installed vault, for a `Vault { name }`
/// store's `delete`. A missing row is not an error; a row sealed with
/// another key is refused and kept.
pub(crate) fn delete_row(name: &str) -> Result<(), RemoteError> {
    let (vault, key) = installed()?;
    refuse_other_key(vault.as_ref(), name, &key.key_id())?;
    vault.delete(name).map(|_| ())
}

#[cfg(test)]
pub(crate) mod testing {
    //! An in-memory vault and the guard every test that installs a vault, or
    //! resolves a store, holds.

    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static SERIAL: Mutex<()> = Mutex::new(());

    /// Serializes the tests that touch the registry and clears it on both
    /// ends, so no test sees another's vault.
    pub(crate) struct RegistryGuard {
        _serial: std::sync::MutexGuard<'static, ()>,
    }

    pub(crate) fn guard() -> RegistryGuard {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        RegistryGuard { _serial: serial }
    }

    impl Drop for RegistryGuard {
        fn drop(&mut self) {
            clear();
        }
    }

    /// A fixed test key.
    pub(crate) fn key(byte: u8) -> SealKey {
        SealKey::from_bytes([byte; 32])
    }

    /// Rows in a map, with every call counted.
    #[derive(Default)]
    pub(crate) struct MemoryVault {
        pub(crate) rows: Mutex<BTreeMap<String, SealedRow>>,
        pub(crate) calls: AtomicUsize,
        pub(crate) deletes: AtomicUsize,
    }

    impl MemoryVault {
        pub(crate) fn row(&self, name: &str) -> Option<SealedRow> {
            self.rows.lock().unwrap().get(name).cloned()
        }

        pub(crate) fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl SecretVault for MemoryVault {
        fn load(&self, name: &str) -> Result<Option<SealedRow>, RemoteError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.row(name))
        }

        fn save(&self, name: &str, sealed: &[u8], key_id: &str) -> Result<(), RemoteError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.rows.lock().unwrap().insert(
                name.to_string(),
                SealedRow {
                    sealed: sealed.to_vec(),
                    key_id: key_id.to_string(),
                },
            );
            Ok(())
        }

        fn delete(&self, name: &str) -> Result<bool, RemoteError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.deletes.fetch_add(1, Ordering::SeqCst);
            Ok(self.rows.lock().unwrap().remove(name).is_some())
        }

        fn summary(&self) -> Result<VaultSummary, RemoteError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let rows = self.rows.lock().unwrap();
            let mut key_ids: Vec<String> = rows.values().map(|r| r.key_id.clone()).collect();
            key_ids.sort();
            key_ids.dedup();
            Ok(VaultSummary {
                rows: rows.len(),
                key_ids,
            })
        }
    }

    /// Installs a fresh in-memory vault under `key` and hands it back.
    pub(crate) fn install_memory(key: SealKey) -> Arc<MemoryVault> {
        let vault = Arc::new(MemoryVault::default());
        install(vault.clone(), key);
        vault
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{guard, install_memory, key};
    use super::*;

    #[test]
    fn registry_clear_isolates_tests() {
        let _guard = guard();
        assert!(active().unwrap().is_none());
        assert!(!engaged());
        require();
        assert!(matches!(active(), Err(RemoteError::VaultUnavailable)));
        assert!(engaged(), "required is engaged");
        install_memory(key(1));
        assert!(active().unwrap().is_some());
        clear();
        assert!(active().unwrap().is_none(), "clear drops the vault");
        assert!(!engaged());
        install_memory(key(1));
        assert!(engaged(), "installed is engaged");
        clear();
        // A test that leaves a vault installed and required behind does not
        // reach the next one.
        require();
        install_memory(key(1));
        drop(_guard);
        let _next = guard();
        assert!(active().unwrap().is_none(), "the next test starts clean");
    }

    #[test]
    fn a_row_round_trips_sealed_under_the_key_id() {
        let _guard = guard();
        let vault = install_memory(key(1));
        save_text("github", "{\"t\":1}").unwrap();
        let row = vault.row("github").unwrap();
        assert_eq!(row.key_id, key(1).key_id());
        assert!(!row.sealed.windows(5).any(|w| w == b"{\"t\":"));
        assert_eq!(load_text("github").unwrap().unwrap().as_str(), "{\"t\":1}");
        let summary = vault.summary().unwrap();
        assert_eq!(summary.rows, 1);
        assert_eq!(summary.key_ids, vec![key(1).key_id()]);
        delete_row("github").unwrap();
        assert!(load_text("github").unwrap().is_none());
        delete_row("github").unwrap();
    }

    #[test]
    fn a_row_that_does_not_open_reads_absent_and_is_kept() {
        let _guard = guard();
        let vault = install_memory(key(1));
        vault
            .save("github", b"\x01not a sealed value at all", &key(1).key_id())
            .unwrap();
        assert!(load_text("github").unwrap().is_none());
        assert!(vault.row("github").is_some());
        assert_eq!(vault.deletes.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn a_vault_store_call_without_a_vault_is_unavailable() {
        let _guard = guard();
        assert!(matches!(
            load_text("github"),
            Err(RemoteError::VaultUnavailable)
        ));
        assert!(matches!(
            save_text("github", "x"),
            Err(RemoteError::VaultUnavailable)
        ));
        assert!(matches!(
            delete_row("github"),
            Err(RemoteError::VaultUnavailable)
        ));
    }

    #[test]
    fn local_stores_refuse_in_vault_mode() {
        use crate::{AppStore, ServerCredentialStore, TokenIdentity, TokenStore};
        let _guard = guard();
        let dir = tempfile::tempdir().unwrap();
        let token: crate::StoredToken = serde_json::from_str(
            r#"{"access_token":"gho_x","host":"github.com","user":"u","created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        let app: crate::AppCredentials = serde_json::from_str(
            r#"{"app_id":1,"client_id":"Iv1","private_key":"k","host":"github.com"}"#,
        )
        .unwrap();
        let server = crate::ServerCredential::token(
            "cmt_x".to_string(),
            "https://kb.example".to_string(),
            "alice".to_string(),
            crystalline_core::time::now(),
        );
        let unavailable = |r: Result<(), RemoteError>, what: &str| {
            assert!(
                matches!(r, Err(RemoteError::VaultUnavailable)),
                "{what}: {r:?}"
            );
        };
        let check = |mode: &str| {
            let file = TokenStore::file_fallback_for(&TokenIdentity::Instance, dir.path()).unwrap();
            unavailable(file.save(&token), mode);
            unavailable(file.load().map(|_| ()), mode);
            unavailable(file.delete(), mode);
            let keyring = TokenStore::Keyring {
                account: "github".to_string(),
            };
            unavailable(keyring.save(&token), mode);
            unavailable(keyring.load().map(|_| ()), mode);
            unavailable(keyring.delete(), mode);
            for store in [AppStore::file_fallback(dir.path()), AppStore::keyring(None)] {
                unavailable(store.save(&app), mode);
                unavailable(store.save_marked(&app, dir.path()), mode);
                unavailable(store.load().map(|_| ()), mode);
                unavailable(store.delete(), mode);
                unavailable(store.delete_marked(dir.path()), mode);
            }
            for store in [
                ServerCredentialStore::file(dir.path()),
                ServerCredentialStore::Keyring {
                    account: "crystalline-server:kb.example".to_string(),
                },
            ] {
                unavailable(store.save(&server), mode);
                unavailable(store.load().map(|_| ()), mode);
                unavailable(store.delete(), mode);
            }
            // The App store lock is one inside the process: no lock file.
            drop(crate::AppStoreLock::acquire(dir.path()).unwrap());
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "{mode}");
        };
        require();
        check("required");
        clear();
        let vault = install_memory(key(1));
        check("installed");
        assert_eq!(vault.calls(), 0, "a local store never reaches the vault");
    }

    #[test]
    fn a_row_sealed_with_another_key_is_never_replaced_or_deleted() {
        let _guard = guard();
        let vault = install_memory(key(1));
        seal_row(vault.as_ref(), &key(2), "github", b"other").unwrap();
        let kept = vault.row("github").unwrap();

        let err = save_text("github", "mine").unwrap_err();
        let text = err.to_string();
        assert!(
            matches!(err, RemoteError::VaultKeyMismatch { .. }),
            "{err:?}"
        );
        assert!(text.contains(&key(1).key_id()), "{text}");
        assert!(text.contains(&key(2).key_id()), "{text}");
        assert_eq!(vault.row("github"), Some(kept.clone()), "never overwritten");

        let err = delete_row("github").unwrap_err();
        assert!(
            matches!(err, RemoteError::VaultKeyMismatch { .. }),
            "{err:?}"
        );
        assert_eq!(vault.row("github"), Some(kept), "never deleted");
        assert_eq!(vault.deletes.load(std::sync::atomic::Ordering::SeqCst), 0);

        // Through the stores too.
        let store = crate::TokenStore::Vault {
            name: "github".to_string(),
        };
        assert!(matches!(
            store.delete(),
            Err(RemoteError::VaultKeyMismatch { .. })
        ));
        assert!(vault.row("github").is_some());
    }
}
