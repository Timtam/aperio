//! `MetaJson` — the coordination file at the root of every sync
//! dataset.
//!
//! `meta.json` lives next to `log/` and `snapshot.json` (and the
//! optional `assets/sounds/`). It carries:
//!
//! - The schema version so devices on different app versions know
//!   when to refuse the dataset (§19.13).
//! - The current snapshot timestamp so devices can discover whether
//!   they're up-to-date and the compaction algorithm has its anchor.
//! - The per-device registry so compaction knows which logs are
//!   safe to delete (only when every device has read them).
//! - An `e2e_enabled` flag — the file itself is **always
//!   unencrypted** even when E2E is on, so devices can read this
//!   metadata before they've prompted for the key.
//!
//! Concurrent writes are managed by the storage adapter via the
//! usual atomic-rename pattern; this module just declares the
//! on-disk shape.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::crypto::EncryptionParams;
use crate::device::DeviceId;

/// Current schema version Aperio writes.
///
/// Increment ONLY on breaking changes (renamed required fields,
/// changed event-type catalogue, incompatible snapshot format).
/// Additive changes — new optional payload fields, new event
/// variants — are forward-compatible because we don't use
/// `#[serde(deny_unknown_fields)]` anywhere; older clients
/// silently ignore the additions.
pub const SCHEMA_VERSION: u32 = 1;

/// One row in the per-device registry.
///
/// Devices update their own record after every successful sync;
/// the compaction algorithm reads the whole registry to decide
/// which log files are safe to delete (only when every device's
/// `last_seen_log >= log_timestamp`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceRecord {
    /// Human-readable name set during the device-onboarding flow —
    /// "Desktop-PC", "MacBook", "Phone". Optional; falls back to a
    /// short prefix of the device id if unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Timestamp of the latest log file this device has read AND
    /// applied locally. Compaction can drop logs older than the
    /// minimum across all devices.
    ///
    /// NOT a heartbeat, despite the name: it is a CONTENT horizon. On a
    /// dataset where nothing is happening it does not move, however often the
    /// device syncs. Anything that wants to say "this device was here
    /// recently" has to read [`Self::last_seen`] instead.
    pub last_seen_log: DateTime<Utc>,
    /// Wall clock of the last round this device completed — the answer to
    /// "which of these entries is still a device and which is left over".
    ///
    /// [`Self::last_seen_log`] cannot answer it: a device that syncs every
    /// fifteen minutes against a quiet dataset holds the same content horizon
    /// for weeks, and a device abandoned in March holds the horizon it had in
    /// March. The two are indistinguishable, and telling them apart is the
    /// whole point of the device list.
    ///
    /// Optional, and absent rather than defaulted: a dataset written by a
    /// build that predates this field has no answer, and `None` says so. A
    /// zero timestamp would have claimed 1970 — an entry that looks like the
    /// most abandoned device on the list when it may be the one syncing right
    /// now. The stamp appears the first time each device completes a round on
    /// a build that writes it.
    ///
    /// Additive, so no [`SCHEMA_VERSION`] bump: nothing in this crate sets
    /// `deny_unknown_fields`, so an older Aperio reading this file ignores the
    /// key, and — because it round-trips through its own `DeviceRecord` — drops
    /// it again on the next write. Which is the honest outcome: while an old
    /// build is participating, it genuinely cannot report when it was last
    /// here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<DateTime<Utc>>,
    /// App version that most recently updated this record. Lets
    /// the schema-versioning check (§19.13) reason about whether
    /// devices are running compatible binaries.
    pub app_version: String,
    /// Set to `true` when the device fell behind the snapshot
    /// horizon — it needs to consume the snapshot before it can
    /// participate in the log-based merge again. Default is
    /// `false` so we can drop this field on writes for the common
    /// case.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stale: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

/// The root document at `meta.json`.
///
/// Order matters for human readability when someone opens the
/// file in a text editor: schema_version + min_app_version at the
/// top so the meaning is obvious; snapshot_timestamp + e2e_enabled
/// next; devices map last.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetaJson {
    /// Wire-format version of this dataset. See [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Minimum Aperio version that can safely read this dataset.
    /// Devices running an older version block with the update-
    /// required dialog (§19.13).
    pub min_app_version: String,
    /// `true` when log files and snapshot are encrypted with the
    /// user-supplied passphrase. `meta.json` itself stays
    /// unencrypted either way so we can read this flag before
    /// prompting for the key.
    #[serde(default)]
    pub e2e_enabled: bool,
    /// KDF parameters (salt + Argon2 cost) needed by any device
    /// joining this dataset. Set in lockstep with `e2e_enabled`:
    /// the pair (`true`, `Some(_)`) marks an encrypted dataset;
    /// (`false`, `None`) is plaintext. The mixed states are
    /// invalid and rejected by the adapter wrapper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e2e_params: Option<EncryptionParams>,
    /// Timestamp of the current snapshot — the content horizon the
    /// `snapshot.json` actually covers (`max(own newest log, fetch cursor)`
    /// of the compacting device). A joining device adopts this as its
    /// starting cursor.
    pub snapshot_timestamp: DateTime<Utc>,
    /// The GC high-water mark: every log file with `timestamp < gc_horizon`
    /// has been deleted from the remote and can no longer be fetched
    /// incrementally. Monotonic (a compaction only ever raises it). This is
    /// DISTINCT from `snapshot_timestamp`: the snapshot may cover more recent
    /// content than has been GC'd (recent logs are retained so a briefly-behind
    /// device can still catch up across them), so a device is "stale" — needs
    /// to consume the snapshot rather than replay logs — exactly when its held
    /// horizon `< gc_horizon`, NOT `< snapshot_timestamp`. `None`/absent means
    /// no compaction has ever GC'd a log (a fresh OR legacy dataset), so the
    /// stale gate can't fire — which is what keeps a legacy `now()`-baseline
    /// meta (real `snapshot_timestamp`, no `snapshot.json`) from wedging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gc_horizon: Option<DateTime<Utc>>,
    /// Per-device registry. Keyed by [`DeviceId`] (the bare string
    /// form). BTreeMap so the on-disk order is deterministic per
    /// device id — useful for git-style diffing if someone hosts
    /// the sync dataset in a version-controlled location.
    #[serde(default)]
    pub devices: BTreeMap<String, DeviceRecord>,
}

impl MetaJson {
    /// Construct an empty meta document for a brand-new dataset.
    /// Used by the onboarding "Neu beginnen" path (§19.11).
    ///
    /// `snapshot_timestamp` is the [`DateTime::<Utc>::MIN_UTC`] sentinel,
    /// not `Utc::now()`: a fresh dataset has no snapshot yet, and stamping
    /// "now" made every freshly-minted meta look like it carried a real
    /// snapshot a hair in the past. The compactor's age trigger and the
    /// §19.10 stale backstop both gate on [`Self::has_real_snapshot`] so
    /// the sentinel reads cleanly as "no compaction has happened".
    pub fn fresh(app_version: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            min_app_version: app_version.into(),
            e2e_enabled: false,
            e2e_params: None,
            snapshot_timestamp: DateTime::<Utc>::MIN_UTC,
            gc_horizon: None,
            devices: BTreeMap::new(),
        }
    }

    /// Whether `snapshot_timestamp` represents a real compaction rather
    /// than the [`Self::fresh`] "no snapshot yet" sentinel. A dataset that
    /// has never been compacted carries [`DateTime::<Utc>::MIN_UTC`]; any
    /// later value is a genuine snapshot horizon. Used to gate the age-based
    /// compaction trigger and the §19.10 stale-device backstop so neither
    /// fires against a dataset that has no `snapshot.json` to resume from.
    pub fn has_real_snapshot(&self) -> bool {
        self.snapshot_timestamp > DateTime::<Utc>::MIN_UTC
    }

    /// The GC high-water mark as a concrete timestamp — [`Self::gc_horizon`]
    /// or the `MIN_UTC` floor when no compaction has ever deleted a log. The
    /// §19.10 stale gate fires exactly when a device's held horizon is below
    /// this, so a dataset that has never GC'd (`None`) never flags anyone.
    pub fn gc_horizon_or_min(&self) -> DateTime<Utc> {
        self.gc_horizon.unwrap_or(DateTime::<Utc>::MIN_UTC)
    }

    /// Read a `MetaJson` from raw bytes (the JSON the storage
    /// adapter pulled). Plain wrapper around `serde_json` so the
    /// adapter doesn't have to pull serde_json directly.
    pub fn from_bytes(bytes: &[u8]) -> crate::error::SyncResult<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// Serialise to a pretty-printed JSON byte vector — the file
    /// is human-readable; the few extra bytes are worth the
    /// readability when debugging a sync issue against a remote.
    pub fn to_bytes(&self) -> crate::error::SyncResult<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Insert or overwrite a device's record. Convenience for the
    /// per-sync write-back step the scheduler performs.
    pub fn upsert_device(&mut self, id: &DeviceId, record: DeviceRecord) {
        self.devices.insert(id.as_str().to_string(), record);
    }

    /// Borrow a device's record, if it's registered.
    pub fn device(&self, id: &DeviceId) -> Option<&DeviceRecord> {
        self.devices.get(id.as_str())
    }

    /// Drop a device from the registry. `true` when there was one to drop.
    ///
    /// ## What this is not
    ///
    /// It is not a revocation and it is not a delete. A device that is still
    /// running re-registers itself on its very next round — `upsert_device`
    /// does not ask whether it used to be there — so the gesture means "stop
    /// counting this one", and the registry corrects itself if the user was
    /// wrong about it being gone.
    ///
    /// It also leaves the device's LOG FILES where they are. They are dataset
    /// content, not device state: the compactor deletes each one once the
    /// snapshot covers it, and removing them here would delete events that no
    /// device has folded in yet. The one thing this call is for — a registry
    /// entry holding the GC floor down at some timestamp from March — is
    /// achieved by dropping the record alone, because the floor is computed
    /// from the records.
    pub fn remove_device(&mut self, id: &DeviceId) -> bool {
        self.devices.remove(id.as_str()).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_meta_round_trips() {
        let dev = DeviceId::from_string("dev-a".into());
        let mut meta = MetaJson::fresh("1.0.0");
        meta.upsert_device(
            &dev,
            DeviceRecord {
                name: Some("Desktop".into()),
                last_seen_log: Utc::now(),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: false,
            },
        );
        let bytes = meta.to_bytes().unwrap();
        let decoded = MetaJson::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, meta);
    }

    #[test]
    fn stale_flag_is_omitted_on_serialise_when_false() {
        let meta = MetaJson {
            schema_version: SCHEMA_VERSION,
            min_app_version: "1.0.0".into(),
            e2e_enabled: false,
            e2e_params: None,
            snapshot_timestamp: Utc::now(),
            gc_horizon: None,
            devices: BTreeMap::from([(
                "dev-a".into(),
                DeviceRecord {
                    name: None,
                    last_seen_log: Utc::now(),
                    last_seen: None,
                    app_version: "1.0.0".into(),
                    stale: false,
                },
            )]),
        };
        let json = serde_json::to_string(&meta).unwrap();
        // `stale: false` is the default — we keep the on-disk
        // shape minimal so the file stays readable.
        assert!(!json.contains("\"stale\""));
    }

    /// The registry a dataset written before this field existed carries, and
    /// what happens when it is read by a build that has it.
    ///
    /// `None`, not "1970". The device list renders "unknown" for it, which is
    /// true; a zero timestamp would have sorted every pre-existing device to
    /// the top of the abandoned end of the list, including the one the user is
    /// sitting in front of.
    #[test]
    fn a_registry_without_the_stamp_reads_as_no_answer() {
        let raw = r#"{
            "schema_version": 1,
            "min_app_version": "1.0.0",
            "snapshot_timestamp": "2025-01-01T00:00:00Z",
            "devices": {
                "dev-a": {
                    "last_seen_log": "2025-01-01T00:00:00Z",
                    "app_version": "1.0.0"
                }
            }
        }"#;
        let meta = MetaJson::from_bytes(raw.as_bytes()).unwrap();
        assert_eq!(meta.devices["dev-a"].last_seen, None);
    }

    /// Absent when unset, so a dataset shared with an older build does not
    /// grow a null key that build then has to ignore.
    #[test]
    fn the_stamp_is_omitted_on_serialise_when_unset() {
        let mut meta = MetaJson::fresh("1.0.0");
        let dev = DeviceId::from_string("dev-a".into());
        meta.upsert_device(
            &dev,
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now(),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: false,
            },
        );
        let json = String::from_utf8(meta.to_bytes().unwrap()).unwrap();
        assert!(!json.contains("last_seen\""), "{json}");

        // And present the moment there is something to say.
        meta.upsert_device(
            &dev,
            DeviceRecord {
                name: None,
                last_seen_log: Utc::now(),
                last_seen: Some(Utc::now()),
                app_version: "1.0.0".into(),
                stale: false,
            },
        );
        let json = String::from_utf8(meta.to_bytes().unwrap()).unwrap();
        assert!(json.contains("\"last_seen\""), "{json}");
    }

    /// Removing a record takes that record and nothing else — in particular
    /// not the other devices, which is what a user pruning a list of eight
    /// test installs is trusting it not to do.
    #[test]
    fn removing_a_device_takes_that_device_only() {
        let mut meta = MetaJson::fresh("1.0.0");
        let keep = DeviceId::from_string("keep".into());
        let drop = DeviceId::from_string("drop".into());
        for id in [&keep, &drop] {
            meta.upsert_device(
                id,
                DeviceRecord {
                    name: None,
                    last_seen_log: Utc::now(),
                    last_seen: None,
                    app_version: "1.0.0".into(),
                    stale: false,
                },
            );
        }

        assert!(meta.remove_device(&drop));
        assert!(meta.device(&drop).is_none());
        assert!(meta.device(&keep).is_some());

        // Twice is not an error. Two devices can prune the same leftover in
        // the same minute, and the second one has simply found the job done.
        assert!(!meta.remove_device(&drop));
    }

    #[test]
    fn missing_devices_map_decodes_as_empty() {
        // A fresh meta with no devices section yet is valid JSON.
        let raw = r#"{
            "schema_version": 1,
            "min_app_version": "1.0.0",
            "snapshot_timestamp": "2025-01-01T00:00:00Z"
        }"#;
        let meta = MetaJson::from_bytes(raw.as_bytes()).unwrap();
        assert!(meta.devices.is_empty());
        assert!(!meta.e2e_enabled);
    }

    #[test]
    fn unknown_fields_are_silently_ignored() {
        // Forward compatibility: an older app version reading a
        // dataset written by a newer Aperio that added an
        // additive field must NOT reject the meta file. Without
        // `deny_unknown_fields` serde does the right thing by
        // default — this test pins that behaviour.
        let raw = r#"{
            "schema_version": 1,
            "min_app_version": "1.0.0",
            "snapshot_timestamp": "2025-01-01T00:00:00Z",
            "some_future_field": "value-aperio-1-doesnt-understand"
        }"#;
        let meta = MetaJson::from_bytes(raw.as_bytes()).unwrap();
        assert_eq!(meta.schema_version, 1);
    }
}
