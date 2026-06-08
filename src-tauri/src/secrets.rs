//! Secret storage backed by the platform's native credential store
//! (DESIGN.md §6.6). Aperio never persists access tokens, OAuth refresh
//! tokens, or basic-auth passwords in the SQLite database; they all
//! go through this module and end up in:
//!
//!   - **Windows** — Windows Credential Manager
//!   - **macOS** — Keychain Services
//!   - **Linux** — Secret Service via libsecret / GNOME Keyring
//!
//! The `keyring` crate handles the per-platform plumbing; this module
//! is the thin wall in front of it so the rest of the backend has one
//! type to talk to and one place to instrument logging or future
//! crypto-at-rest behaviour against.
//!
//! ## Naming convention
//!
//! Each credential is addressed by an opaque `account_id` (typically a
//! UUID from the `accounts` table) plus a `slot` name. The slot lets a
//! single account own several distinct secrets — e.g. OAuth accounts
//! carry `access_token`, `refresh_token`, and sometimes `id_token` —
//! without smashing them into one JSON blob. The on-disk service name
//! follows `Aperio:<slot>` so a user inspecting the credential store
//! sees one logical entry per slot per account.

use keyring::Entry;
use thiserror::Error;
use tracing::warn;

/// Service prefix used in the platform credential store. Keeping the
/// "Aperio:" prefix lets a user (or a sysadmin) spot Aperio's entries
/// at a glance and clean them up by hand if needed.
const SERVICE_PREFIX: &str = "Aperio";

/// One logical slot per account. Adding a new slot is a code change
/// rather than a string, so typos can't cause a silent migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretSlot {
    /// Short-lived OAuth2 access token.
    AccessToken,
    /// OAuth2 refresh token (long-lived).
    RefreshToken,
    /// Generic password — used by Basic Auth (CalDAV, WebDAV, …).
    Password,
    /// API token (Vikunja, Todoist, …).
    ApiToken,
    /// 32-byte AES-256 key for cross-device sync E2E encryption
    /// (Phase Sk). Stored as base64 so the keychain backend
    /// doesn't choke on null bytes; the value is `KEY_LEN`
    /// bytes after decode.
    SyncEncryptionKey,
}

impl SecretSlot {
    fn as_str(self) -> &'static str {
        match self {
            SecretSlot::AccessToken => "access_token",
            SecretSlot::RefreshToken => "refresh_token",
            SecretSlot::Password => "password",
            SecretSlot::ApiToken => "api_token",
            SecretSlot::SyncEncryptionKey => "sync_encryption_key",
        }
    }

    /// Public wire name for the slot — the same string used as the
    /// keychain service suffix. The credential-sync emit path names the
    /// slot with this when building a `credential.set` event.
    pub fn wire_name(self) -> &'static str {
        self.as_str()
    }

    /// Map a wire slot name back to a slot, but ONLY for the slots that
    /// may travel through cross-device credential sync. The short-lived
    /// [`SecretSlot::AccessToken`] (each device re-derives its own from
    /// the refresh token) and the E2E key itself
    /// ([`SecretSlot::SyncEncryptionKey`] — syncing it would defeat
    /// end-to-end encryption) are deliberately rejected here, so a
    /// malformed or hostile event can never smuggle them into the
    /// keychain. This allowlist is the single place that decides what a
    /// received credential event is allowed to write.
    pub fn syncable_from_wire(name: &str) -> Option<SecretSlot> {
        match name {
            "password" => Some(SecretSlot::Password),
            "refresh_token" => Some(SecretSlot::RefreshToken),
            "api_token" => Some(SecretSlot::ApiToken),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("no secret stored for this account/slot")]
    NotFound,
    #[error("keychain error: {0}")]
    Backend(String),
}

impl From<keyring::Error> for SecretError {
    fn from(err: keyring::Error) -> Self {
        match err {
            keyring::Error::NoEntry => SecretError::NotFound,
            other => SecretError::Backend(other.to_string()),
        }
    }
}

/// Persist `value` for `(account_id, slot)`. Overwrites any previous
/// value in that slot; the platform credential store does not stack
/// multiple entries for the same (service, user) pair.
pub fn store(account_id: &str, slot: SecretSlot, value: &str) -> Result<(), SecretError> {
    let entry = entry_for(account_id, slot)?;
    entry.set_password(value).map_err(Into::into)
}

/// Read the previously stored value for `(account_id, slot)`. Returns
/// `SecretError::NotFound` when no entry exists — callers translate
/// that into "user needs to re-authenticate".
pub fn retrieve(account_id: &str, slot: SecretSlot) -> Result<String, SecretError> {
    let entry = entry_for(account_id, slot)?;
    entry.get_password().map_err(Into::into)
}

/// Best-effort secret removal. A missing entry is treated as success
/// — calling code shouldn't need to know whether a slot was ever
/// populated before account deletion.
pub fn delete(account_id: &str, slot: SecretSlot) -> Result<(), SecretError> {
    let entry = entry_for(account_id, slot)?;
    match entry.delete_password() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Clear every slot tied to `account_id`. Called when an account is
/// removed from Aperio so the credential store doesn't accumulate
/// stale tokens. Returns `Ok(())` even when some slots were absent;
/// the goal is "after this call, no Aperio secret exists for that
/// account", regardless of starting state.
pub fn delete_all(account_id: &str) -> Result<(), SecretError> {
    let slots = [
        SecretSlot::AccessToken,
        SecretSlot::RefreshToken,
        SecretSlot::Password,
        SecretSlot::ApiToken,
    ];
    let mut first_err: Option<SecretError> = None;
    for slot in slots {
        if let Err(err) = delete(account_id, slot) {
            warn!(?err, slot = ?slot, "failed to delete secret slot");
            if first_err.is_none() {
                first_err = Some(err);
            }
        }
    }
    match first_err {
        None => Ok(()),
        Some(err) => Err(err),
    }
}

fn entry_for(account_id: &str, slot: SecretSlot) -> Result<Entry, SecretError> {
    let service = format!("{SERVICE_PREFIX}:{}", slot.as_str());
    Entry::new(&service, account_id).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keychain backend is unavailable in many CI environments
    /// (no D-Bus on Linux runners, no Keychain on macOS bare runners).
    /// The integration test below only runs when the platform's
    /// keychain is actually reachable; otherwise we'd be testing
    /// the env, not us.
    fn keychain_reachable() -> bool {
        let probe = Entry::new("Aperio:probe", "test");
        let Ok(probe) = probe else { return false };
        // A get on a non-existent entry should error with NoEntry on
        // a reachable backend; if it errors with anything else the
        // backend is up but unhappy, so we still skip.
        matches!(probe.get_password(), Err(keyring::Error::NoEntry) | Ok(_))
    }

    #[test]
    fn round_trip_when_keychain_is_reachable() {
        if !keychain_reachable() {
            eprintln!("skipping: platform keychain not reachable");
            return;
        }
        let account = format!("test-{}", uuid::Uuid::new_v4());
        store(&account, SecretSlot::AccessToken, "hunter2").unwrap();
        let read = retrieve(&account, SecretSlot::AccessToken).unwrap();
        assert_eq!(read, "hunter2");

        // Overwrite works.
        store(&account, SecretSlot::AccessToken, "hunter3").unwrap();
        let read = retrieve(&account, SecretSlot::AccessToken).unwrap();
        assert_eq!(read, "hunter3");

        // Delete works and is idempotent.
        delete(&account, SecretSlot::AccessToken).unwrap();
        delete(&account, SecretSlot::AccessToken).unwrap();
        assert!(matches!(
            retrieve(&account, SecretSlot::AccessToken),
            Err(SecretError::NotFound)
        ));
    }

    #[test]
    fn syncable_allowlist_admits_only_durable_slots() {
        // The slots that may travel through cross-device credential sync.
        assert_eq!(
            SecretSlot::syncable_from_wire("password"),
            Some(SecretSlot::Password)
        );
        assert_eq!(
            SecretSlot::syncable_from_wire("refresh_token"),
            Some(SecretSlot::RefreshToken)
        );
        assert_eq!(
            SecretSlot::syncable_from_wire("api_token"),
            Some(SecretSlot::ApiToken)
        );
    }

    #[test]
    fn syncable_allowlist_rejects_access_token_and_e2e_key() {
        // The short-lived access token is re-derived per device, and the
        // E2E key itself must NEVER ride the sync (it would defeat E2E).
        // Both — and any unknown name — are refused so a received
        // credential event can't smuggle them into the keychain.
        assert_eq!(SecretSlot::syncable_from_wire("access_token"), None);
        assert_eq!(SecretSlot::syncable_from_wire("sync_encryption_key"), None);
        assert_eq!(SecretSlot::syncable_from_wire(""), None);
        assert_eq!(SecretSlot::syncable_from_wire("password "), None);
        // The round-trip names line up with `wire_name`.
        assert_eq!(SecretSlot::Password.wire_name(), "password");
        assert_eq!(SecretSlot::RefreshToken.wire_name(), "refresh_token");
    }

    #[test]
    fn delete_all_clears_every_slot() {
        if !keychain_reachable() {
            eprintln!("skipping: platform keychain not reachable");
            return;
        }
        let account = format!("test-{}", uuid::Uuid::new_v4());
        store(&account, SecretSlot::AccessToken, "a").unwrap();
        store(&account, SecretSlot::RefreshToken, "r").unwrap();
        store(&account, SecretSlot::Password, "p").unwrap();
        delete_all(&account).unwrap();
        assert!(matches!(
            retrieve(&account, SecretSlot::AccessToken),
            Err(SecretError::NotFound)
        ));
        assert!(matches!(
            retrieve(&account, SecretSlot::RefreshToken),
            Err(SecretError::NotFound)
        ));
        assert!(matches!(
            retrieve(&account, SecretSlot::Password),
            Err(SecretError::NotFound)
        ));
    }
}
