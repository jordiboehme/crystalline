//! An in-memory sealed vault for the engine's tests, and the guard every
//! test that installs a vault holds: the registry is process-global, so the
//! guard serializes those tests and clears the registry on both ends.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crystalline_remote::sealed::SealKey;
use crystalline_remote::{RemoteError, SealedRow, SecretVault, VaultSummary, vault};

static SERIAL: Mutex<()> = Mutex::new(());

/// Holds the registry for one test and clears it when dropped.
pub(crate) struct RegistryGuard {
    _serial: MutexGuard<'static, ()>,
}

pub(crate) fn guard() -> RegistryGuard {
    let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    vault::clear();
    RegistryGuard { _serial: serial }
}

impl Drop for RegistryGuard {
    fn drop(&mut self) {
        vault::clear();
    }
}

/// A fixed test key.
pub(crate) fn key(byte: u8) -> SealKey {
    SealKey::from_bytes([byte; 32])
}

/// Rows in a map, with every call counted.
#[derive(Default)]
pub(crate) struct MemoryVault {
    rows: Mutex<BTreeMap<String, SealedRow>>,
    calls: AtomicUsize,
}

impl MemoryVault {
    pub(crate) fn has(&self, name: &str) -> bool {
        self.rows.lock().unwrap().contains_key(name)
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl SecretVault for MemoryVault {
    fn load(&self, name: &str) -> Result<Option<SealedRow>, RemoteError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.rows.lock().unwrap().get(name).cloned())
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
    vault::install(vault.clone(), key);
    vault
}
