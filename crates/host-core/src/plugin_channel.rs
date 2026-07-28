//! What the host does when a plugin reports something it was not asked.
//!
//! The transport is [`plugin_core::host_channel`]; this is the other end of it.
//! One kind exists today: an adapter whose provider handed back a new
//! credential during a refresh. Persisting it is the whole point — without it
//! the refreshed value lives in the adapter's memory until the instance closes,
//! the stored copy goes stale, and the account dies quietly whenever the old
//! credential finally lapses.
//!
//! ## Two things this deliberately does NOT do
//!
//! **It does not sync from here.** Writing the keychain is what the plugin is
//! promised when it gets `ACCEPTED`, and it is what must be durable before that
//! answer goes back. Emitting the credential onto the sync log takes the
//! process-wide database mutex, and this runs on an arbitrary plugin thread
//! while other calls may be in flight — taking that lock here would create a
//! lock-ordering hazard with any host command holding it while awaiting a
//! plugin. The sync emit is queued and drained elsewhere; a device that misses
//! one still has the working credential locally, which is the part that matters.
//!
//! **It does not trust the slot name.** A plugin names the slot as a string,
//! and only the slots a credential may legitimately occupy are accepted. A
//! plugin asking to write, say, the E2E key is refused rather than obeyed.

use std::os::raw::c_int;
use std::sync::Arc;

use plugin_core::abi::{
    HOST_CHANNEL_ACCEPTED, HOST_CHANNEL_FAILED, HOST_CHANNEL_MALFORMED, HOST_CHANNEL_REFUSED,
    HOST_CHANNEL_UNKNOWN_KIND, KIND_CREDENTIAL_ROTATED,
};
use plugin_core::host_channel::{HostChannelHandler, ResolvedScope};
use sync_engine::{SecretSlot, SecretStore};
use tracing::{info, warn};

/// The host's implementation of the plugin channel.
pub struct HostChannel {
    secret_store: Arc<dyn SecretStore>,
}

impl HostChannel {
    pub fn new(secret_store: Arc<dyn SecretStore>) -> Self {
        Self { secret_store }
    }

    /// Install this as the process-wide handler.
    pub fn install(secret_store: Arc<dyn SecretStore>) {
        plugin_core::host_channel::install_handler(Arc::new(Self::new(secret_store)));
    }
}

/// Slots a plugin may ask the host to rewrite.
///
/// Deliberately narrower than [`SecretSlot`] as a whole. An adapter refreshing
/// its own OAuth credentials has business with these three and nothing else;
/// the E2E key in particular is not an adapter's to touch, and the client
/// secret comes from the build or the user's registration rather than from a
/// provider response.
fn writable_slot(name: &str) -> Option<SecretSlot> {
    match name {
        "refresh_token" => Some(SecretSlot::RefreshToken),
        "access_token" => Some(SecretSlot::AccessToken),
        "api_token" => Some(SecretSlot::ApiToken),
        _ => None,
    }
}

impl HostChannelHandler for HostChannel {
    fn handle(&self, scope: &ResolvedScope, kind: &str, payload: &serde_json::Value) -> c_int {
        if kind != KIND_CREDENTIAL_ROTATED {
            return HOST_CHANNEL_UNKNOWN_KIND;
        }
        let Some(slot_name) = payload.get("slot").and_then(|v| v.as_str()) else {
            return HOST_CHANNEL_MALFORMED;
        };
        let Some(value) = payload.get("value").and_then(|v| v.as_str()) else {
            return HOST_CHANNEL_MALFORMED;
        };
        if value.is_empty() {
            // An empty credential would overwrite a working one with nothing.
            return HOST_CHANNEL_MALFORMED;
        }
        let Some(slot) = writable_slot(slot_name) else {
            warn!(
                account_id = %scope.account_id,
                plugin_id = %scope.plugin_id,
                slot = slot_name,
                "a plugin asked to write a credential slot it has no business with"
            );
            return HOST_CHANNEL_REFUSED;
        };

        // Durable before ACCEPTED goes back: that answer is a promise.
        match self.secret_store.store(&scope.account_id, slot, value) {
            Ok(()) => {
                // The VALUE is never logged, here or anywhere. Which slot on
                // which account is exactly what a maintainer needs, and is
                // safe.
                info!(
                    account_id = %scope.account_id,
                    plugin_id = %scope.plugin_id,
                    slot = slot_name,
                    "a plugin reported a rotated credential; stored"
                );
                HOST_CHANNEL_ACCEPTED
            }
            Err(e) => {
                warn!(
                    account_id = %scope.account_id,
                    slot = slot_name,
                    error = %e,
                    "could not store a rotated credential"
                );
                HOST_CHANNEL_FAILED
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryStore {
        entries: Mutex<HashMap<(String, String), String>>,
        fail: bool,
    }

    impl SecretStore for MemoryStore {
        fn store(
            &self,
            account_id: &str,
            slot: SecretSlot,
            secret: &str,
        ) -> Result<(), sync_engine::SecretError> {
            if self.fail {
                return Err(sync_engine::SecretError::Backend("nope".into()));
            }
            self.entries.lock().unwrap().insert(
                (account_id.to_string(), slot.wire_name().to_string()),
                secret.to_string(),
            );
            Ok(())
        }

        fn retrieve(
            &self,
            account_id: &str,
            slot: SecretSlot,
        ) -> Result<String, sync_engine::SecretError> {
            self.entries
                .lock()
                .unwrap()
                .get(&(account_id.to_string(), slot.wire_name().to_string()))
                .cloned()
                .ok_or(sync_engine::SecretError::NotFound)
        }

        fn delete(
            &self,
            account_id: &str,
            slot: SecretSlot,
        ) -> Result<(), sync_engine::SecretError> {
            self.entries
                .lock()
                .unwrap()
                .remove(&(account_id.to_string(), slot.wire_name().to_string()));
            Ok(())
        }

        fn delete_all(&self, account_id: &str) -> Result<(), sync_engine::SecretError> {
            self.entries
                .lock()
                .unwrap()
                .retain(|(id, _), _| id != account_id);
            Ok(())
        }
    }

    fn scope() -> ResolvedScope {
        ResolvedScope {
            account_id: "account-1".into(),
            plugin_id: "com.aperio.vc-adapter-webex".into(),
            generation: 1,
            live: true,
        }
    }

    #[test]
    fn a_rotated_refresh_token_is_stored_before_the_plugin_is_told_yes() {
        let store = Arc::new(MemoryStore::default());
        let handler = HostChannel::new(store.clone());
        let status = handler.handle(
            &scope(),
            KIND_CREDENTIAL_ROTATED,
            &serde_json::json!({ "slot": "refresh_token", "value": "RT-new" }),
        );
        assert_eq!(status, HOST_CHANNEL_ACCEPTED);
        assert_eq!(
            store
                .retrieve("account-1", SecretSlot::RefreshToken)
                .unwrap(),
            "RT-new"
        );
    }

    #[test]
    fn a_slot_the_adapter_has_no_business_with_is_refused() {
        // An adapter refreshing its own OAuth credentials has business with
        // three slots. The E2E key is not one of them, and obeying would let a
        // plugin destroy the user's encrypted sync.
        let store = Arc::new(MemoryStore::default());
        let handler = HostChannel::new(store.clone());
        for slot in [
            "sync_encryption_key",
            "password",
            "oauth_client_secret",
            "nonsense",
        ] {
            let status = handler.handle(
                &scope(),
                KIND_CREDENTIAL_ROTATED,
                &serde_json::json!({ "slot": slot, "value": "x" }),
            );
            assert_eq!(status, HOST_CHANNEL_REFUSED, "slot {slot} must be refused");
        }
        assert!(store.entries.lock().unwrap().is_empty());
    }

    #[test]
    fn an_empty_value_never_overwrites_a_working_credential() {
        let store = Arc::new(MemoryStore::default());
        store
            .store("account-1", SecretSlot::RefreshToken, "still-good")
            .unwrap();
        let handler = HostChannel::new(store.clone());
        let status = handler.handle(
            &scope(),
            KIND_CREDENTIAL_ROTATED,
            &serde_json::json!({ "slot": "refresh_token", "value": "" }),
        );
        assert_eq!(status, HOST_CHANNEL_MALFORMED);
        assert_eq!(
            store
                .retrieve("account-1", SecretSlot::RefreshToken)
                .unwrap(),
            "still-good"
        );
    }

    #[test]
    fn an_unknown_kind_is_reported_rather_than_guessed_at() {
        let handler = HostChannel::new(Arc::new(MemoryStore::default()));
        assert_eq!(
            handler.handle(&scope(), "quota.exceeded", &serde_json::json!({})),
            HOST_CHANNEL_UNKNOWN_KIND
        );
    }

    #[test]
    fn a_failed_write_is_reported_as_failed_not_accepted() {
        // ACCEPTED is a promise that the value is durable. Saying it after a
        // failed write would make the plugin discard the only copy.
        let store = Arc::new(MemoryStore {
            fail: true,
            ..Default::default()
        });
        let handler = HostChannel::new(store);
        assert_eq!(
            handler.handle(
                &scope(),
                KIND_CREDENTIAL_ROTATED,
                &serde_json::json!({ "slot": "refresh_token", "value": "v" }),
            ),
            HOST_CHANNEL_FAILED
        );
    }

    #[test]
    fn a_payload_missing_its_fields_is_malformed() {
        let handler = HostChannel::new(Arc::new(MemoryStore::default()));
        for payload in [
            serde_json::json!({}),
            serde_json::json!({ "slot": "refresh_token" }),
            serde_json::json!({ "value": "v" }),
            serde_json::json!({ "slot": 1, "value": "v" }),
        ] {
            assert_eq!(
                handler.handle(&scope(), KIND_CREDENTIAL_ROTATED, &payload),
                HOST_CHANNEL_MALFORMED
            );
        }
    }
}
