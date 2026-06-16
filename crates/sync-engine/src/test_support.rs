//! Shared in-memory test doubles for the platform seams. Compiled only
//! under `cfg(test)`; used by the snapshot + compactor unit tests so the
//! engine's logic can be exercised without a real SQLite or keychain. The
//! DB-backed round-trip tests live in the desktop crate against the real
//! `DesktopSyncStore`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use cal_adapter_local::{SnapshotApplyReport, SnapshotDump};

use crate::whitelist::is_synced_key;
use crate::{SecretError, SecretSlot, SecretStore, SnapshotAccount, StoreError, SyncStore};

/// In-memory [`SyncStore`]. `prefs` backs both `get_pref`/`set_pref` and
/// the whitelisted-settings dump; `accounts` backs the account
/// dump/upsert; `e2e` is a fixed flag. The snapshot row dump/apply is
/// `LocalAdapter`'s job (tested in the desktop crate), so it's a no-op
/// here.
#[derive(Default)]
pub(crate) struct FakeStore {
    pub prefs: Mutex<BTreeMap<String, String>>,
    pub accounts: Mutex<Vec<SnapshotAccount>>,
    pub e2e: bool,
}

impl SyncStore for FakeStore {
    fn dump_for_snapshot(&self) -> Result<SnapshotDump, StoreError> {
        Ok(SnapshotDump::default())
    }

    fn apply_snapshot_dump(&self, _dump: &SnapshotDump) -> Result<SnapshotApplyReport, StoreError> {
        Ok(SnapshotApplyReport::default())
    }

    fn dump_synced_settings(&self) -> Result<BTreeMap<String, String>, StoreError> {
        Ok(self
            .prefs
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| is_synced_key(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }

    fn get_pref(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self.prefs.lock().unwrap().get(key).cloned())
    }

    fn set_pref(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.prefs
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn dump_accounts(&self) -> Result<Vec<SnapshotAccount>, StoreError> {
        Ok(self.accounts.lock().unwrap().clone())
    }

    fn upsert_account(&self, account: &SnapshotAccount) -> Result<(), StoreError> {
        self.accounts.lock().unwrap().push(account.clone());
        Ok(())
    }

    fn e2e_enabled(&self) -> bool {
        self.e2e
    }
}

/// In-memory [`SecretStore`] keyed by `(account_id, slot wire name)`.
#[derive(Default)]
pub(crate) struct FakeSecrets {
    map: Mutex<HashMap<(String, &'static str), String>>,
}

impl SecretStore for FakeSecrets {
    fn store(&self, account_id: &str, slot: SecretSlot, value: &str) -> Result<(), SecretError> {
        self.map.lock().unwrap().insert(
            (account_id.to_string(), slot.wire_name()),
            value.to_string(),
        );
        Ok(())
    }

    fn retrieve(&self, account_id: &str, slot: SecretSlot) -> Result<String, SecretError> {
        self.map
            .lock()
            .unwrap()
            .get(&(account_id.to_string(), slot.wire_name()))
            .cloned()
            .ok_or(SecretError::NotFound)
    }

    fn delete(&self, account_id: &str, slot: SecretSlot) -> Result<(), SecretError> {
        self.map
            .lock()
            .unwrap()
            .remove(&(account_id.to_string(), slot.wire_name()));
        Ok(())
    }

    fn delete_all(&self, account_id: &str) -> Result<(), SecretError> {
        self.map
            .lock()
            .unwrap()
            .retain(|(acc, _), _| acc != account_id);
        Ok(())
    }
}
