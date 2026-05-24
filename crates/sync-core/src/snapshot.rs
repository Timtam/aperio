//! `Snapshot` — a full dump of the app's synchronisable state at
//! a given timestamp, folded from all log entries up to that
//! point.
//!
//! New devices pull the snapshot first (one HTTP GET / file copy)
//! and then replay only the more recent logs — much cheaper than
//! replaying years of history from event one. The compaction
//! algorithm (Phase Sg) regenerates the snapshot periodically and
//! GCs the logs it folds in.
//!
//! ## What's in a snapshot
//!
//! Everything the event log can produce. Concretely:
//!
//! - Local-adapter events / tasks / task_lists / calendars
//! - Color labels
//! - Plugin metadata (not binaries)
//! - Shortcut overrides
//! - Whitelisted settings (per §19.2.1)
//! - Sound-file hash references (the binaries live under
//!   `assets/sounds/` and are referenced by sha256 — see §19.2.2)
//!
//! ## What's NOT in a snapshot
//!
//! External-adapter data (Google / iCloud / Graph / EWS / CardDAV
//! events, tasks, contacts) — those sync via their own provider
//! APIs and aren't part of the event log. Local-only secrets
//! (keychain) and device-specific settings (window position, …)
//! per the §19.2.1 always-local list.
//!
//! ## Structure
//!
//! The snapshot body is loosely typed (`serde_json::Value`) at
//! this layer because the source tables have their own evolution
//! and the generator/applier own the strong typing. Same logic as
//! the `EventPayload` shape — keep the schema bumps localised.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::SyncResult;
use crate::meta::SCHEMA_VERSION;

/// Snapshot metadata — written as a small header alongside the
/// body so we can read just the metadata cheaply without
/// deserialising the whole state dump.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotMetadata {
    /// Wire-format version — same field as in `meta.json`.
    /// Snapshots and meta must agree; the compaction algorithm
    /// ensures that.
    pub schema_version: u32,
    /// Inclusive timestamp — the snapshot reflects every event
    /// whose `timestamp <= snapshot_timestamp` AND no events
    /// after. Devices use this as the cursor when deciding which
    /// logs to apply on top.
    pub snapshot_timestamp: DateTime<Utc>,
    /// App version that generated the snapshot. Diagnostic only —
    /// the schema gate is `schema_version` in `meta.json`.
    pub app_version: String,
}

/// A full snapshot bundle.
///
/// The body lives as an opaque JSON `Value` rather than a typed
/// struct. The generator (in `src-tauri`) knows the cross-table
/// shape and lays it out; the applier knows how to walk that
/// shape and insert rows. Keeping the body untyped at the
/// sync-core layer means we can grow the snapshot schema with
/// every local-adapter migration without bumping `schema_version`
/// — only structural breaks bump it.
///
/// ## On-disk shape
///
/// Stored as a single JSON document at `sync/snapshot.json`:
///
/// ```json
/// {
///   "metadata": {
///     "schema_version": 1,
///     "snapshot_timestamp": "2025-05-12T09:14:22Z",
///     "app_version": "1.0.0"
///   },
///   "body": { …full state dump… }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub metadata: SnapshotMetadata,
    /// State dump. Conventional top-level keys are
    /// `events`, `tasks`, `task_lists`, `calendars`, `color_labels`,
    /// `shortcuts`, `settings`, `plugins`, `sound_refs` — but the
    /// sync-core layer doesn't enforce any of that.
    pub body: serde_json::Value,
}

impl Snapshot {
    /// Construct a fresh snapshot from a typed body.
    pub fn new(
        snapshot_timestamp: DateTime<Utc>,
        app_version: impl Into<String>,
        body: serde_json::Value,
    ) -> Self {
        Self {
            metadata: SnapshotMetadata {
                schema_version: SCHEMA_VERSION,
                snapshot_timestamp,
                app_version: app_version.into(),
            },
            body,
        }
    }

    /// Decode from raw bytes. The storage adapter reads bytes;
    /// this helper hides the serde_json detail.
    pub fn from_bytes(bytes: &[u8]) -> SyncResult<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// Encode to pretty-printed JSON bytes. We pretty-print
    /// snapshots so that operators inspecting the sync folder by
    /// hand have a chance of making sense of them; the size cost
    /// is negligible compared to the actual state contents.
    pub fn to_bytes(&self) -> SyncResult<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn snapshot_round_trips_through_json() {
        let stamp = Utc.with_ymd_and_hms(2025, 5, 12, 9, 14, 22).unwrap();
        let snap = Snapshot::new(
            stamp,
            "1.0.0",
            serde_json::json!({
                "events": [{ "id": "e1", "title": "x" }],
                "tasks": [],
            }),
        );
        let bytes = snap.to_bytes().unwrap();
        let decoded = Snapshot::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, snap);
        assert_eq!(decoded.metadata.schema_version, SCHEMA_VERSION);
    }
}
