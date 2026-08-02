//! `SyncAdapter` — the storage-backend trait every sync adapter
//! implements.
//!
//! Adapters carry the on-disk shape from §19.2 to/from a remote
//! storage:
//!
//! ```text
//! sync/
//! ├── log/
//! │   ├── <timestamp>_<device_id>.jsonl
//! │   └── …
//! ├── snapshot.json
//! ├── assets/
//! │   └── sounds/<sha256>.{mp3,wav,…}
//! └── meta.json
//! ```
//!
//! Implementations:
//!
//! - `adapter-local` — local filesystem / NAS mount
//! - `adapter-webdav` — Nextcloud, ownCloud, generic WebDAV
//! - `adapter-sftp` — SFTP over SSH
//! - `adapter-ftp` — FTPS
//! - `adapter-dropbox` — Dropbox API v2
//! - `adapter-google::drive` — Google Drive API v3, the storage role
//!   of the same account that serves its calendars
//!
//! ## Layering: encryption sits ABOVE the adapter
//!
//! The adapter operates on raw bytes. When the user enables E2E
//! (§19.7), the bytes the adapter sees are already
//! AES-256-GCM-encrypted on push and still encrypted on fetch — a
//! decryption wrapper at the command layer is responsible for the
//! conversion. This keeps adapters dumb-and-fast and means new
//! adapters don't have to re-implement the crypto.
//!
//! Two files are explicitly **always unencrypted** even under E2E:
//! `meta.json` (the version/device registry that bootstrapping
//! needs before a password prompt) and any encryption probe blob.
//! Adapters don't know that — they push whatever bytes the layer
//! above hands them.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::SyncResult;
use crate::log::LogFile;
use crate::snapshot::Snapshot;

/// Cursor used by `fetch_new_logs` to ask "give me everything
/// strictly newer than this point". The adapter compares against
/// the encoded timestamp in each log file's name; it does NOT
/// peek inside the bytes.
///
/// Devices remember their cursor between sync runs and bump it
/// after a successful pull so the next run only pulls genuinely
/// new files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceCursor {
    /// Highest log-file timestamp we've already fetched.
    /// `fetch_new_logs(since)` returns every log whose
    /// `LogFileName.timestamp > since.last_seen_log`.
    pub last_seen_log: DateTime<Utc>,
    /// Device whose log files the adapter should SKIP before even
    /// fetching their bytes. The sync round sets this to the local
    /// device id: its own session files sit above the cursor (the
    /// cursor only advances on FOREIGN logs) and used to be fully
    /// re-downloaded every round just for the orchestrator to discard
    /// them post-fetch. `None` = no exclusion — the compactor's GC
    /// coverage scan and onboarding genuinely want every file.
    /// `serde(default)` keeps the plugin-ABI encoding compatible.
    #[serde(default)]
    pub exclude_device: Option<crate::device::DeviceId>,
    /// Byte lengths of already-APPLIED log files, keyed by filename.
    /// The growth-refetch signal: a peer keeps appending to its live
    /// session file after we first fetched it, but the file's name (and
    /// so its timestamp) never changes — the plain cursor filter would
    /// hide the appended events until the peer rotates its session.
    /// An adapter whose listing carries sizes re-fetches a file that is
    /// AT/BELOW the cursor when its listed size exceeds the recorded
    /// applied length (per-event idempotency makes the re-apply safe).
    /// Adapters without listing sizes ignore this (append-miss persists
    /// there until rotation). `serde(default)` keeps the wire compatible.
    #[serde(default)]
    pub known_lengths: Vec<KnownLogLength>,
}

/// One applied-length record for [`DeviceCursor::known_lengths`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnownLogLength {
    /// The log's exact filename (`LogFileName::to_filename`).
    pub name: String,
    /// Bytes of that file this device has fetched AND applied.
    pub len: u64,
}

impl DeviceCursor {
    /// Cursor that asks for "everything" — used by a brand-new
    /// install during onboarding before a snapshot has been
    /// pulled.
    pub fn epoch() -> Self {
        Self {
            last_seen_log: DateTime::<Utc>::MIN_UTC,
            exclude_device: None,
            known_lengths: Vec::new(),
        }
    }

    /// Whether a listed log file should be fetched under this cursor:
    /// strictly newer than the horizon and not from the excluded
    /// device. The single filter every adapter's listing loop applies,
    /// so the exclusion semantics can't drift between backends.
    pub fn wants(&self, name: &crate::log::LogFileName) -> bool {
        name.timestamp > self.last_seen_log && self.exclude_device.as_ref() != Some(&name.device_id)
    }

    /// Size-aware variant of [`Self::wants`] for adapters whose listing
    /// reports byte sizes: additionally re-fetches a file at/below the
    /// cursor whose listed size GREW past the recorded applied length
    /// (a peer's live session file gaining appended events). A file with
    /// no recorded length is never growth-refetched — it was applied
    /// before length tracking existed, and treating "unknown" as grown
    /// would re-download the whole history once per upgrade.
    pub fn wants_sized(
        &self,
        name: &crate::log::LogFileName,
        filename: &str,
        listed_len: Option<u64>,
    ) -> bool {
        if self.exclude_device.as_ref() == Some(&name.device_id) {
            return false;
        }
        if name.timestamp > self.last_seen_log {
            return true;
        }
        match listed_len {
            Some(listed) => self
                .known_lengths
                .iter()
                .any(|k| k.name == filename && listed > k.len),
            None => false,
        }
    }
}

/// The storage-backend trait every sync adapter implements.
///
/// Adapters are stateful (they hold connection pools, refresh
/// tokens, etc.) so the trait takes `&self`. Concurrent access is
/// safe — internal mutability lives in `Mutex` / `OnceLock`
/// inside the adapter.
#[async_trait]
pub trait SyncAdapter: Send + Sync {
    /// Quick health check. Used by the settings dialog's "Test
    /// connection" button and by the scheduler before its first
    /// real round-trip after the adapter is reconfigured.
    ///
    /// Should be cheap: a HEAD against the root URL, a SFTP
    /// `realpath` on the home directory, an open-close on a local
    /// path. NOT a full directory listing.
    async fn test_connection(&self) -> SyncResult<()>;

    /// Read `meta.json` from the remote root, parse it, return.
    /// Returns `None` when the file doesn't exist — that's the
    /// "fresh dataset, you're the first device" case the
    /// onboarding flow (§19.11) keys off.
    async fn fetch_meta(&self) -> SyncResult<Option<crate::meta::MetaJson>>;

    /// Write the entire `meta.json` content back atomically. The
    /// adapter is responsible for the write-temp + rename dance
    /// so a half-written meta never wins a race. Callers ensure
    /// only one device updates the registry at a time via
    /// optimistic-locking on the file's etag (where the backend
    /// supports it; local FS and FTPS don't, so the spec
    /// tolerates last-write-wins on this file specifically).
    async fn push_meta(&self, meta: &crate::meta::MetaJson) -> SyncResult<()>;

    /// Enumerate log files newer than `since`. The return order
    /// MAY be chronological (some backends already sort) but the
    /// caller treats it as unordered — applier sorts by
    /// timestamp + id before replay.
    ///
    /// Returns `Vec<LogFile>` with the actual bytes loaded. For
    /// large backlogs, the caller will set a sensible cursor
    /// (e.g. only fetch the last 90 days when bootstrapping) so
    /// the adapter doesn't have to stream.
    async fn fetch_new_logs(&self, since: &DeviceCursor) -> SyncResult<Vec<LogFile>>;

    /// Upload one log file. The adapter writes it to
    /// `sync/log/<filename>` exactly. Idempotent: if a file by
    /// that name already exists, return Ok — log filenames are
    /// timestamp + device id, collisions only happen on retries.
    async fn push_log(&self, log: &LogFile) -> SyncResult<()>;

    /// Read `snapshot.json` from the remote, decode, return.
    /// `Ok(None)` when there isn't one yet (brand-new dataset
    /// before the first compaction).
    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>>;

    /// Atomically replace the remote snapshot. Same write-temp +
    /// rename pattern as `push_meta`. After this lands, the
    /// caller updates `meta.json.snapshot_timestamp` to match.
    async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()>;

    /// Delete a log file that's been folded into the snapshot
    /// and is older than every device's cursor. The caller
    /// computes the list and issues one delete per file; we
    /// don't expose a bulk-delete because some backends don't
    /// implement it natively and the per-file pattern is
    /// universal.
    async fn delete_log(&self, name: &crate::log::LogFileName) -> SyncResult<()>;

    /// Upload a binary asset (sound file). Keyed by its sha256
    /// hash so the adapter writes to
    /// `assets/sounds/<hash>.<ext>`. Returns `Ok` if the file
    /// already existed — content-addressed paths are immutable
    /// once written, so a duplicate upload is a no-op.
    async fn push_sound_asset(&self, hash: &str, extension: &str, bytes: &[u8]) -> SyncResult<()>;

    /// Fetch a sound file by hash. `Ok(None)` when missing —
    /// callers fall back to silence for that particular sound
    /// reference without erroring the whole sync.
    async fn fetch_sound_asset(&self, hash: &str, extension: &str) -> SyncResult<Option<Vec<u8>>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceId;
    use crate::log::LogFileName;
    use chrono::TimeZone;

    fn name(ts_secs: i64, device: &str) -> LogFileName {
        LogFileName {
            timestamp: Utc.timestamp_opt(ts_secs, 0).unwrap(),
            device_id: DeviceId::from_string(device.into()),
        }
    }

    #[test]
    fn wants_applies_cursor_and_exclusion() {
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: Some(DeviceId::from_string("me".into())),
            known_lengths: Vec::new(),
        };
        // Newer + foreign → fetch.
        assert!(cursor.wants(&name(2_000, "peer")));
        // Newer but OWN → skipped at the listing stage (the round used to
        // download these in full just to discard them post-fetch).
        assert!(!cursor.wants(&name(2_000, "me")));
        // Older foreign → skipped by the horizon.
        assert!(!cursor.wants(&name(500, "peer")));
        // No exclusion (compactor GC scan) → own files still fetched.
        let epoch = DeviceCursor::epoch();
        assert!(epoch.wants(&name(2_000, "me")));
    }

    #[test]
    fn wants_sized_refetches_grown_files_only() {
        let file = name(1_000, "peer");
        let filename = "the-file.jsonl";
        let cursor = DeviceCursor {
            // Cursor AT the file's timestamp — the plain filter skips it.
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: Some(DeviceId::from_string("me".into())),
            known_lengths: vec![KnownLogLength {
                name: filename.into(),
                len: 100,
            }],
        };
        // Listed size grew past the applied length → refetch.
        assert!(cursor.wants_sized(&file, filename, Some(150)));
        // Unchanged → skip.
        assert!(!cursor.wants_sized(&file, filename, Some(100)));
        // No listed size (server omits it) → skip, plain semantics.
        assert!(!cursor.wants_sized(&file, filename, None));
        // Unknown filename (applied before tracking) → never refetched.
        assert!(!cursor.wants_sized(&file, "other.jsonl", Some(9_999)));
        // Own file → excluded even when grown.
        let own = name(1_000, "me");
        let own_cursor = DeviceCursor {
            known_lengths: vec![KnownLogLength {
                name: "own.jsonl".into(),
                len: 10,
            }],
            ..cursor.clone()
        };
        assert!(!own_cursor.wants_sized(&own, "own.jsonl", Some(20)));
        // Above the cursor → fetched regardless of sizes.
        assert!(cursor.wants_sized(&name(2_000, "peer"), "new.jsonl", None));
    }
}
