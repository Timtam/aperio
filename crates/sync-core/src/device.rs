//! `DeviceId` — opaque per-install identity used in log file names
//! and the `meta.json.devices` map.
//!
//! Generated on first run as a UUID v4, persisted via the user_prefs
//! repository (key [`DEVICE_ID_PREF_KEY`]). The id never changes for
//! the lifetime of the install — re-installing Aperio on the same
//! machine produces a new id, which is intentional: two installs
//! with the same dataset are conceptually different "devices" in
//! the §19 sense.
//!
//! Two reasons we wrap UUID in a newtype:
//!
//! 1. Domain clarity — a `DeviceId` is not interchangeable with the
//!    `event_id`s or contact UIDs that also pass through this crate.
//!    The newtype keeps the boundary tight.
//! 2. Stability across schema bumps — if a future spec needs to
//!    extend the id (e.g. to carry a key-version suffix for E2E),
//!    the type can grow without ripping through every call site.

use serde::{Deserialize, Serialize};

/// `user_prefs` key under which the device id is stored.
///
/// Living next to the other `sync.*` prefs (`sync.intervalMinutes`,
/// `sync.adapter`, etc.) keeps the namespace coherent.
pub const DEVICE_ID_PREF_KEY: &str = "sync.deviceId";

/// Opaque per-install identifier.
///
/// Wraps a UUID v4 hex string. The wire format is just the bare
/// string — serde uses `transparent` so devices on different
/// platforms can match by string equality without worrying about
/// our wrapper type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(String);

impl DeviceId {
    /// Mint a fresh device id from a UUID v4.
    pub fn new() -> Self {
        // `simple()` formats the UUID without dashes — 32 hex
        // chars. Smaller and slightly less typo-prone than the
        // dashed form, and irrelevant for matching since we treat
        // the value as opaque.
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    /// Construct from a string. Caller is responsible for
    /// uniqueness — used by the loader path that round-trips from
    /// `user_prefs` and by tests that need stable ids.
    pub fn from_string(raw: String) -> Self {
        Self(raw)
    }

    /// Borrow the underlying string. Useful for path concatenation
    /// in log-file naming and for the `meta.json.devices` key.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Take ownership of the underlying string.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Default for DeviceId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for DeviceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_generates_unique_ids() {
        let a = DeviceId::new();
        let b = DeviceId::new();
        assert_ne!(a, b);
        // UUID simple form is 32 hex chars.
        assert_eq!(a.as_str().len(), 32);
    }

    #[test]
    fn serde_uses_transparent_string() {
        let dev = DeviceId::from_string("abc123".into());
        let encoded = serde_json::to_string(&dev).unwrap();
        assert_eq!(encoded, r#""abc123""#);
        let decoded: DeviceId = serde_json::from_str(r#""abc123""#).unwrap();
        assert_eq!(decoded, dev);
    }

    #[test]
    fn display_and_as_ref_match_the_underlying_string() {
        let dev = DeviceId::from_string("device-x".into());
        assert_eq!(format!("{dev}"), "device-x");
        let s: &str = dev.as_ref();
        assert_eq!(s, "device-x");
    }
}
