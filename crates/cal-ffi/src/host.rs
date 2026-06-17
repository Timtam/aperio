//! The mobile `Host` — the on-device counterpart to the desktop
//! `src-tauri` backend, assembled from the shared `host-core` crate.
//!
//! Where [`crate::LocalStore`] serves only the local SQLite task store,
//! `Host` owns the full account + adapter surface: it opens the same
//! migrated database, statically links + registers all 17 bundled
//! adapter plugins (no dlopen — iOS forbids it; see `host-plugins`),
//! and drives the same [`host_core::registry::AdapterRegistry`] the
//! desktop uses to route per-account reads/writes through external
//! services (CalDAV, EWS, …).
//!
//! ## Secret seam
//!
//! Credentials never touch the SQLite file. The desktop stores them in
//! the OS keyring; mobile implements the [`KeychainBridge`] foreign
//! trait over the iOS Keychain / Android Keystore. [`BridgeSecretStore`]
//! adapts that bridge to the engine-side [`sync_engine::SecretStore`]
//! the registry already routes through.
//!
//! ## Scope (this increment)
//!
//! Account CRUD only (`accounts_json` / `create_account_json` /
//! `delete_account`), all synchronous — opening a plugin instance is a
//! sync call, so no async runtime is needed yet. The pre-persist
//! credential smoke-test, the cross-device credential push, and the
//! `AccountCreated` sync-log event (all desktop `create_account`
//! behaviour) ride on the event-log + a tokio runtime and land with the
//! read/write/sync phases.

use std::sync::Arc;

use host_core::accounts::{AccountsRepo, AdapterKind};
use host_core::registry::AdapterRegistry;
use host_core::DbHandle;
use plugin_core::PluginManager;
use sync_engine::{SecretError, SecretSlot, SecretStore};

use crate::{from_json, to_json, StoreError};

/// Errors the foreign keychain implementation can raise. Mirrors
/// [`sync_engine::SecretError`] so the `NotFound` distinction the
/// registry branches on (e.g. an absent optional iCal password) survives
/// the round-trip.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum KeychainError {
    /// No secret stored for this `(account, slot)`.
    #[error("secret not found")]
    NotFound,
    /// The platform keychain/keystore backend failed.
    #[error("keychain backend error: {detail}")]
    Backend { detail: String },
}

/// Platform credential store, implemented on the foreign side (iOS
/// Keychain / Android Keystore). `slot` is the stable wire name from
/// [`SecretSlot::wire_name`] (`"password"`, `"access_token"`, …) so the
/// foreign code can key its store without depending on the Rust enum.
#[uniffi::export(with_foreign)]
pub trait KeychainBridge: Send + Sync {
    /// Persist `value` for `(account_id, slot)`, overwriting any prior.
    fn store(&self, account_id: String, slot: String, value: String) -> Result<(), KeychainError>;
    /// Read the value for `(account_id, slot)`; `NotFound` when absent.
    fn retrieve(&self, account_id: String, slot: String) -> Result<String, KeychainError>;
    /// Best-effort removal; a missing entry is `Ok(())`.
    fn delete(&self, account_id: String, slot: String) -> Result<(), KeychainError>;
    /// Clear every slot tied to `account_id`.
    fn delete_all(&self, account_id: String) -> Result<(), KeychainError>;
}

/// Adapts a foreign [`KeychainBridge`] to the engine-side
/// [`SecretStore`] the [`AdapterRegistry`] routes credentials through.
struct BridgeSecretStore {
    bridge: Arc<dyn KeychainBridge>,
}

fn to_secret_err(e: KeychainError) -> SecretError {
    match e {
        KeychainError::NotFound => SecretError::NotFound,
        KeychainError::Backend { detail } => SecretError::Backend(detail),
    }
}

impl SecretStore for BridgeSecretStore {
    fn store(&self, account_id: &str, slot: SecretSlot, value: &str) -> Result<(), SecretError> {
        self.bridge
            .store(
                account_id.to_string(),
                slot.wire_name().to_string(),
                value.to_string(),
            )
            .map_err(to_secret_err)
    }

    fn retrieve(&self, account_id: &str, slot: SecretSlot) -> Result<String, SecretError> {
        self.bridge
            .retrieve(account_id.to_string(), slot.wire_name().to_string())
            .map_err(to_secret_err)
    }

    fn delete(&self, account_id: &str, slot: SecretSlot) -> Result<(), SecretError> {
        self.bridge
            .delete(account_id.to_string(), slot.wire_name().to_string())
            .map_err(to_secret_err)
    }

    fn delete_all(&self, account_id: &str) -> Result<(), SecretError> {
        self.bridge
            .delete_all(account_id.to_string())
            .map_err(to_secret_err)
    }
}

/// Account-creation request from the foreign side. Same wire shape as the
/// desktop's `CreateAccountRequest` (snake-case `adapter_kind`), so the
/// shared TS domain logic emits one payload for both backends.
#[derive(serde::Deserialize)]
struct NewAccountRequest {
    adapter_kind: AdapterKind,
    display_name: String,
    #[serde(default = "default_config_json")]
    config_json: String,
    /// Secret half of the credentials (CalDAV password, API token, …).
    /// Stored only via the keychain bridge, never in the SQLite file.
    #[serde(default)]
    secret: Option<String>,
}

fn default_config_json() -> String {
    "{}".into()
}

/// The mobile app's handle to the full on-device engine.
#[derive(uniffi::Object)]
pub struct Host {
    db: DbHandle,
    registry: Arc<AdapterRegistry>,
    secret_store: Arc<dyn SecretStore>,
    // Kept alive for the lifetime of the host; the registry holds an
    // `Arc` clone and looks plugins up against it per account.
    _plugin_manager: Arc<PluginManager>,
}

#[uniffi::export]
impl Host {
    /// Open the on-device database at `db_path`, register every bundled
    /// adapter plugin statically, build the adapter registry over the
    /// supplied keychain bridge, and materialise adapters for the
    /// persisted accounts.
    #[uniffi::constructor]
    pub fn open(
        db_path: String,
        keychain: Arc<dyn KeychainBridge>,
    ) -> Result<Arc<Self>, StoreError> {
        let db = DbHandle::open(&db_path).map_err(|e| StoreError::Open {
            detail: e.to_string(),
        })?;

        // Static plugin embedding: no dlopen (iOS forbids it) — the 17
        // `-plugin` rlibs are linked into this library and registered by id.
        let plugin_manager = Arc::new(PluginManager::new(env!("CARGO_PKG_VERSION")));
        host_plugins::register_all_static(&plugin_manager).map_err(|e| StoreError::Open {
            detail: format!("plugin registration failed: {e}"),
        })?;

        let secret_store: Arc<dyn SecretStore> = Arc::new(BridgeSecretStore { bridge: keychain });

        // Per-account plugin state (EWS sync cookies, caches) persists next
        // to the database file.
        let data_dir = std::path::Path::new(&db_path)
            .parent()
            .map(|p| p.to_path_buf());
        let registry = Arc::new(AdapterRegistry::with_data_dir(
            Arc::clone(&plugin_manager),
            Arc::clone(&secret_store),
            data_dir,
        ));
        {
            let shared = db.shared();
            let repo = AccountsRepo::new(&shared);
            registry.bootstrap(&repo);
        }

        Ok(Arc::new(Self {
            db,
            registry,
            secret_store,
            _plugin_manager: plugin_manager,
        }))
    }

    /// All persisted accounts as JSON (the `cal_core`/desktop wire shape).
    pub fn accounts_json(&self) -> Result<String, StoreError> {
        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        let accounts = repo.list().map_err(acc_err)?;
        to_json(&accounts)
    }

    /// Create an external (or local) account: persist the row, store the
    /// secret via the keychain bridge, and register the adapter so
    /// subsequent reads/writes route through it. Returns the created
    /// account as JSON.
    ///
    /// Mirrors the desktop `create_account` minus the pre-persist
    /// credential smoke-test + the cross-device credential/account
    /// sync-log events (those need the event log + a tokio runtime and
    /// arrive with the read/write/sync phases). Like the desktop, a
    /// secret-store or registration failure tears the row back down so
    /// the DB, keychain, and registry never drift.
    pub fn create_account_json(&self, request_json: String) -> Result<String, StoreError> {
        let req: NewAccountRequest = from_json("account", &request_json)?;

        // Only the kinds with a non-OAuth construction path. Google /
        // Microsoft Graph / the VC kinds need the interactive OAuth flow
        // (a later phase); reject them here rather than persisting a row
        // that can never authenticate.
        if !matches!(
            req.adapter_kind,
            AdapterKind::Local
                | AdapterKind::Caldav
                | AdapterKind::Ical
                | AdapterKind::Ews
                | AdapterKind::Vikunja
                | AdapterKind::Todoist
        ) {
            return Err(StoreError::InvalidField {
                field: "adapter_kind".to_string(),
                detail: format!(
                    "adapter '{}' needs the interactive OAuth flow (a later phase)",
                    req.adapter_kind.as_str()
                ),
            });
        }

        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        let created = repo
            .create(req.adapter_kind, req.display_name.trim(), &req.config_json)
            .map_err(acc_err)?;

        // Persist the secret right after the row so the keychain and DB
        // stay aligned. The slot mirrors what the registry's register_*
        // path reads back: API token for Vikunja/Todoist, password
        // otherwise. A write failure is fatal — tear the row down.
        if let Some(secret) = req.secret {
            let slot = match req.adapter_kind {
                AdapterKind::Vikunja | AdapterKind::Todoist => SecretSlot::ApiToken,
                _ => SecretSlot::Password,
            };
            if let Err(err) = self.secret_store.store(&created.id, slot, &secret) {
                let _ = repo.delete(&created.id);
                return Err(StoreError::Storage {
                    detail: format!("failed to store credential: {err}"),
                });
            }
        }

        // Register the freshly created external adapter. A failure is
        // fatal: drop the secrets + row so keychain/DB/registry stay in step.
        if req.adapter_kind != AdapterKind::Local {
            if let Err(err) = self.registry.register(&created) {
                let _ = self.secret_store.delete_all(&created.id);
                let _ = repo.delete(&created.id);
                return Err(StoreError::Storage {
                    detail: format!("adapter registration failed: {err}"),
                });
            }
        }

        to_json(&created)
    }

    /// Delete an account: unregister its adapter, clear its secrets, and
    /// remove the row. The local account cannot be deleted
    /// ([`StoreError::InvalidField`]).
    pub fn delete_account(&self, account_id: String) -> Result<(), StoreError> {
        self.registry.unregister(&account_id);
        let _ = self.secret_store.delete_all(&account_id);
        let shared = self.db.shared();
        let repo = AccountsRepo::new(&shared);
        repo.delete(&account_id).map_err(acc_err)
    }
}

/// Map an accounts-repo error to the FFI store error, preserving the
/// NotFound / forbidden-local-delete distinctions for the UI.
fn acc_err(e: host_core::accounts::AccountsError) -> StoreError {
    use host_core::accounts::AccountsError;
    match e {
        AccountsError::NotFound(_) => StoreError::NotFound,
        AccountsError::DeleteLocalForbidden => StoreError::InvalidField {
            field: "account_id".to_string(),
            detail: "the local account cannot be deleted".to_string(),
        },
        AccountsError::UnknownKind(detail) => StoreError::InvalidField {
            field: "adapter_kind".to_string(),
            detail,
        },
        AccountsError::Sqlite(e) => StoreError::Storage {
            detail: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory keychain for tests — the Rust stand-in for the iOS/
    /// Android bridge.
    #[derive(Default)]
    struct FakeKeychain {
        map: Mutex<HashMap<(String, String), String>>,
    }

    impl KeychainBridge for FakeKeychain {
        fn store(
            &self,
            account_id: String,
            slot: String,
            value: String,
        ) -> Result<(), KeychainError> {
            self.map.lock().unwrap().insert((account_id, slot), value);
            Ok(())
        }
        fn retrieve(&self, account_id: String, slot: String) -> Result<String, KeychainError> {
            self.map
                .lock()
                .unwrap()
                .get(&(account_id, slot))
                .cloned()
                .ok_or(KeychainError::NotFound)
        }
        fn delete(&self, account_id: String, slot: String) -> Result<(), KeychainError> {
            self.map.lock().unwrap().remove(&(account_id, slot));
            Ok(())
        }
        fn delete_all(&self, account_id: String) -> Result<(), KeychainError> {
            self.map
                .lock()
                .unwrap()
                .retain(|(acc, _), _| acc != &account_id);
            Ok(())
        }
    }

    fn open_host() -> (tempfile::TempDir, Arc<Host>, Arc<FakeKeychain>) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("aperio.sqlite");
        let keychain = Arc::new(FakeKeychain::default());
        let host = Host::open(
            db_path.to_string_lossy().into_owned(),
            keychain.clone() as Arc<dyn KeychainBridge>,
        )
        .unwrap();
        (dir, host, keychain)
    }

    #[test]
    fn fresh_host_lists_only_the_seeded_local_account() {
        let (_dir, host, _kc) = open_host();
        let json = host.accounts_json().unwrap();
        // Migration 0003 seeds the implicit local account.
        assert!(json.contains("\"adapter_kind\":\"local\""), "got: {json}");
    }

    #[test]
    fn create_caldav_account_persists_row_and_secret_and_registers() {
        let (_dir, host, kc) = open_host();
        let req = r#"{
            "adapter_kind": "caldav",
            "display_name": "Work CalDAV",
            "config_json": "{\"server_url\":\"https://dav.example.invalid/\",\"username\":\"alice\",\"auth_kind\":\"basic\"}",
            "secret": "hunter2"
        }"#;
        let created = host.create_account_json(req.to_string()).unwrap();
        assert!(created.contains("\"adapter_kind\":\"caldav\""));
        assert!(created.contains("\"display_name\":\"Work CalDAV\""));

        // The account is listed.
        let listed = host.accounts_json().unwrap();
        assert!(listed.contains("Work CalDAV"));

        // The secret reached the keychain bridge under the password slot.
        let stored = kc.map.lock().unwrap();
        assert!(stored
            .iter()
            .any(|((_, slot), v)| slot == "password" && v == "hunter2"));
    }

    #[test]
    fn create_then_delete_account_round_trips() {
        let (_dir, host, kc) = open_host();
        let req = r#"{
            "adapter_kind": "vikunja",
            "display_name": "Tasks",
            "config_json": "{\"server_url\":\"https://vikunja.example.invalid/\"}",
            "secret": "tok_123"
        }"#;
        let created = host.create_account_json(req.to_string()).unwrap();
        let id: serde_json::Value = serde_json::from_str(&created).unwrap();
        let account_id = id["id"].as_str().unwrap().to_string();

        // API-token slot for Vikunja.
        assert!(kc
            .map
            .lock()
            .unwrap()
            .keys()
            .any(|(_, slot)| slot == "api_token"));

        host.delete_account(account_id.clone()).unwrap();
        let listed = host.accounts_json().unwrap();
        assert!(!listed.contains("Tasks"));
        // Secrets cleared for the account.
        assert!(kc
            .map
            .lock()
            .unwrap()
            .keys()
            .all(|(acc, _)| acc != &account_id));
    }

    #[test]
    fn oauth_kind_is_rejected_until_a_later_phase() {
        let (_dir, host, _kc) = open_host();
        let req = r#"{"adapter_kind":"google","display_name":"G"}"#;
        let err = host.create_account_json(req.to_string()).unwrap_err();
        assert!(matches!(err, StoreError::InvalidField { .. }));
    }

    #[test]
    fn deleting_the_local_account_is_forbidden() {
        let (_dir, host, _kc) = open_host();
        let err = host.delete_account("local".to_string()).unwrap_err();
        assert!(matches!(err, StoreError::InvalidField { .. }));
    }
}
