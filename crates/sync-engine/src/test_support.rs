//! In-memory test doubles for the platform seams. Compiled under
//! `cfg(test)` for the crate's own unit tests, and under the
//! `test-support` feature so the desktop crate's tests can reuse
//! `FakeSecrets` (its applier tests run against the real `DesktopSyncStore`
//! + an in-memory keychain). The DB-backed round-trip tests live in the
//! desktop crate against the real store.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

use cal_adapter_local::{SnapshotApplyReport, SnapshotDump};

use crate::whitelist::is_synced_key;
use crate::{
    NewConflict, SecretError, SecretSlot, SecretStore, SnapshotAccount, StoreError, SyncStore,
};

/// In-memory [`SyncStore`]. `prefs` backs `get_pref`/`set_pref` and the
/// whitelisted-settings dump; `accounts` backs the account dump/upsert;
/// `applied`/`conflicts` back the applier seam; `e2e` is a fixed flag. The
/// snapshot row dump/apply and the remote-plugin announcements are no-ops
/// (`LocalAdapter` / the desktop store own those — tested in the desktop
/// crate).
#[derive(Default)]
pub struct FakeStore {
    pub prefs: Mutex<BTreeMap<String, String>>,
    pub accounts: Mutex<Vec<SnapshotAccount>>,
    pub applied: Mutex<HashSet<String>>,
    pub conflicts: Mutex<Vec<NewConflict>>,
    pub e2e: bool,
    /// Keys whose `get_pref` returns `Err` instead of a value. Lets a test
    /// exercise the difference between "this pref is unset" and "the store
    /// could not be read" — the two used to be the same thing to callers.
    pub failing_pref_reads: Mutex<HashSet<String>>,
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
        if self.failing_pref_reads.lock().unwrap().contains(key) {
            return Err(StoreError::Backend(format!(
                "simulated read failure: {key}"
            )));
        }
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
        let mut accounts = self.accounts.lock().unwrap();
        accounts.retain(|a| a.id != account.id);
        accounts.push(account.clone());
        Ok(())
    }

    fn e2e_enabled(&self) -> bool {
        self.e2e
    }

    fn is_event_applied(&self, event_id: &str) -> Result<bool, StoreError> {
        Ok(self.applied.lock().unwrap().contains(event_id))
    }

    fn mark_event_applied(&self, event_id: &str) -> Result<(), StoreError> {
        self.applied.lock().unwrap().insert(event_id.to_string());
        Ok(())
    }

    fn record_conflict(&self, conflict: &NewConflict) -> Result<(), StoreError> {
        self.conflicts.lock().unwrap().push(conflict.clone());
        Ok(())
    }

    fn delete_pref(&self, key: &str) -> Result<(), StoreError> {
        self.prefs.lock().unwrap().remove(key);
        Ok(())
    }

    fn delete_account(&self, id: &str) -> Result<(), StoreError> {
        self.accounts.lock().unwrap().retain(|a| a.id != id);
        Ok(())
    }

    fn upsert_remote_plugin(
        &self,
        _id: &str,
        _name: Option<&str>,
        _version: &str,
        _plugin_type: Option<&str>,
        _source: Option<&str>,
        _announced_by_device: &str,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    fn delete_remote_plugin(&self, _id: &str) -> Result<(), StoreError> {
        Ok(())
    }
}

/// In-memory [`SecretStore`] keyed by `(account_id, slot wire name)`.
#[derive(Default)]
pub struct FakeSecrets {
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
