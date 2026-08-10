//! The store's other half: mirroring the dataset into a filesystem directory.
//!
//! It lived in its own crate, behind its own plugin, under its own adapter kind
//! (`local_folder`) — a name that existed only to dodge a collision with this
//! crate, which already owned `local`. Once folder sync folded into the
//! built-in store the collision was gone, and so was the reason for the split:
//! this is the same adapter, in its second role.
//!
//! It needs no plugin ABI either. [`LocalFsSyncAdapter`] implements
//! `sync_core::SyncAdapter` directly, so the host constructs it exactly as it
//! constructs [`crate::LocalAdapter`] — linked in, no vtable, no cdylib.
//!
//! Wraps a configurable directory path and maps the
//! [`sync_core::SyncAdapter`] trait onto plain `tokio::fs`
//! operations. The expected layout under `remote_root` matches
//! DESIGN.md §19:
//!
//! ```text
//! <remote_root>/
//! ├── log/
//! │   ├── <ts>_<device-a>.jsonl
//! │   └── <ts>_<device-b>.jsonl
//! ├── snapshot.json
//! ├── assets/
//! │   └── sounds/<sha256>.<ext>
//! └── meta.json
//! ```
//!
//! ## Use cases
//!
//! - **Local NAS / network share** — point `remote_root` at the
//!   mount point, e.g. `Z:\Aperio-Sync\` or `/mnt/nas/aperio/`.
//! - **USB stick passed between machines** — works without
//!   network entirely.
//! - **Development + automated tests** — two `LocalFsSyncAdapter`
//!   instances sharing the same path simulate a full multi-device
//!   sync without standing up a WebDAV / SFTP backend.
//!
//! ## Atomic writes
//!
//! `meta.json` and `snapshot.json` use the write-temp-then-rename
//! dance: a sibling `<name>.tmp` is written first, then renamed
//! over the destination. Rename on the same filesystem is
//! POSIX-atomic; on Windows we accept the slim race window
//! between the unlink and the rename — sync conflicts there are
//! already covered by the §19.3 conflict-resolution path.
//!
//! Log files don't use the temp-rename pattern because their
//! filenames embed the device id + timestamp and never collide
//! between devices. Two devices writing the same UID is by
//! construction impossible.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter, SyncError, SyncResult,
};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::debug;

/// SyncAdapter targeting an OS filesystem directory.
///
/// All adapter calls are cheap — no connection state to maintain,
/// no auth tokens to refresh. The struct is `Send + Sync` so a
/// single instance can serve the orchestrator across the
/// scheduler's async tasks.
#[derive(Debug, Clone)]
pub struct LocalFsSyncAdapter {
    remote_root: PathBuf,
    /// Layout directories already created this adapter lifetime.
    /// Directories persist on disk, so once ensured every later
    /// push can skip the `create_dir_all` triplet — on the
    /// documented SMB/NAS mount use case each call is a network
    /// metadata round trip. Shared across clones so the cache
    /// survives `.clone()`; cleared when a push IO error hints
    /// the tree vanished underneath us (USB remount, deleted
    /// folder), so the next attempt re-ensures.
    dirs_ensured: Arc<AtomicBool>,
}

impl LocalFsSyncAdapter {
    /// Construct an adapter rooted at `remote_root`. The
    /// directory does NOT have to exist yet — `test_connection`
    /// is responsible for creating the layout if needed.
    pub fn new(remote_root: impl Into<PathBuf>) -> Self {
        Self {
            remote_root: remote_root.into(),
            dirs_ensured: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Borrow the configured root. Useful for diagnostics + for
    /// the orchestrator to derive a per-adapter cursor key.
    pub fn remote_root(&self) -> &Path {
        &self.remote_root
    }

    fn log_dir(&self) -> PathBuf {
        self.remote_root.join("log")
    }

    fn meta_path(&self) -> PathBuf {
        self.remote_root.join("meta.json")
    }

    fn snapshot_path(&self) -> PathBuf {
        self.remote_root.join("snapshot.json")
    }

    fn sound_dir(&self) -> PathBuf {
        self.remote_root.join("assets").join("sounds")
    }

    fn sound_path(&self, hash: &str, extension: &str) -> PathBuf {
        self.sound_dir().join(format!("{hash}.{extension}"))
    }

    /// Create the layout directories, at most once per adapter
    /// lifetime (mirrors WebDAV's once-per-session MKCOL cache).
    async fn ensure_dirs(&self) -> SyncResult<()> {
        if self.dirs_ensured.load(Ordering::Relaxed) {
            return Ok(());
        }
        fs::create_dir_all(&self.remote_root).await?;
        fs::create_dir_all(self.log_dir()).await?;
        fs::create_dir_all(self.sound_dir()).await?;
        // Relaxed suffices for a cache hint: a concurrent first
        // touch racing here at worst runs a second harmless
        // `create_dir_all` triplet.
        self.dirs_ensured.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Pass a push result through, dropping the ensured-dirs
    /// cache on failure: an IO error mid-push may mean the layout
    /// vanished underneath us (USB stick remounted, folder
    /// deleted on the NAS), so the next attempt must re-run
    /// `ensure_dirs` instead of trusting the stale flag.
    fn invalidate_ensured_dirs_on_err<T>(&self, result: SyncResult<T>) -> SyncResult<T> {
        if result.is_err() {
            self.dirs_ensured.store(false, Ordering::Relaxed);
        }
        result
    }

    /// Atomic write of `bytes` to `path`. Write to a sibling temp file, then
    /// rename over the destination. Rename within the same filesystem is
    /// atomic.
    ///
    /// The temp name carries a fresh id per write, and that is the whole point.
    /// It used to be derived from the destination alone — one fixed
    /// `meta.json.tmp` — while this module's own docs name a shared network
    /// share, and two instances pointed at one path, as first-class uses. Two
    /// writers then truncated and interleaved into the SAME temp file before
    /// either renamed, and the file that survived was neither writer's: a
    /// torn `meta.json` that no device wrote and none can read back.
    ///
    /// A per-write name cannot prevent the second rename from winning — that is
    /// last-writer-wins, which the sync protocol already expects — but each
    /// rename now publishes a file exactly one writer produced.
    async fn atomic_write(path: &Path, bytes: &[u8]) -> SyncResult<()> {
        let unique = uuid::Uuid::new_v4();
        let tmp = path.with_extension(match path.extension() {
            Some(ext) => format!("{}.{unique}.tmp", ext.to_string_lossy()),
            None => format!("{unique}.tmp"),
        });
        {
            let mut file = fs::OpenOptions::new()
                // `create_new`, not `create`: with a fresh id a collision means
                // something is wrong, and clobbering it would be the very bug
                // this is here to stop.
                .create_new(true)
                .write(true)
                .open(&tmp)
                .await?;
            file.write_all(bytes).await?;
            file.flush().await?;
            file.sync_data().await.ok();
        }
        // A failed rename would otherwise leave the temp file behind for ever —
        // with a fixed name the next write reused it, with a fresh one it would
        // accumulate.
        if let Err(err) = fs::rename(&tmp, path).await {
            let _ = fs::remove_file(&tmp).await;
            return Err(err.into());
        }
        Ok(())
    }
}

#[async_trait]
impl SyncAdapter for LocalFsSyncAdapter {
    /// Verify the remote root exists (creating it if missing) and
    /// is writable. Writes a one-byte probe and removes it —
    /// catches read-only mounts, permission misconfigurations,
    /// and missing parent directories all in one go.
    async fn test_connection(&self) -> SyncResult<()> {
        // An explicit health check must not trust the ensured-dirs
        // cache — its whole job is verifying the layout really is
        // there (the user may have just replugged the USB stick).
        // The forced `ensure_dirs` re-seeds the cache on success.
        self.dirs_ensured.store(false, Ordering::Relaxed);
        self.ensure_dirs().await?;
        let probe = self.remote_root.join(".aperio-write-probe");
        match fs::write(&probe, b"ok").await {
            Ok(()) => {
                let _ = fs::remove_file(&probe).await;
                Ok(())
            }
            Err(err) => Err(SyncError::io(format!(
                "write probe failed at {}: {err}",
                probe.display(),
            ))),
        }
    }

    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        let path = self.meta_path();
        match fs::read(&path).await {
            Ok(bytes) => Ok(Some(MetaJson::from_bytes(&bytes)?)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(SyncError::io(err.to_string())),
        }
    }

    async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
        self.ensure_dirs().await?;
        let bytes = meta.to_bytes()?;
        self.invalidate_ensured_dirs_on_err(Self::atomic_write(&self.meta_path(), &bytes).await)
    }

    async fn fetch_new_logs(&self, since: &DeviceCursor) -> SyncResult<Vec<LogFile>> {
        let dir = self.log_dir();
        let mut entries = match fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // No log directory yet — the dataset has zero
                // sync history. Return empty; the caller's
                // subsequent push_log will create the dir.
                return Ok(Vec::new());
            }
            Err(err) => return Err(SyncError::io(err.to_string())),
        };

        let mut out = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            // Parse the filename through the canonical sync-core
            // helper. Anything that doesn't fit the
            // <ts>_<device>.jsonl shape (stray temp files,
            // editor backups) gets silently skipped.
            let parsed = match LogFileName::from_filename(&name_str) {
                Ok(n) => n,
                Err(err) => {
                    debug!(
                        name = %name_str,
                        ?err,
                        "skipping log directory entry: not a valid log filename",
                    );
                    continue;
                }
            };
            // Size-aware filter: the fs metadata length feeds the growth
            // check, so a peer's live session file with appended events is
            // re-fetched even though its timestamp sits at/below the cursor.
            let listed_len = entry.metadata().await.ok().map(|m| m.len());
            if !since.wants_sized(&parsed, &name_str, listed_len) {
                continue;
            }
            let bytes = match fs::read(entry.path()).await {
                Ok(b) => b,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    // Listed but gone: the compactor deleted it in
                    // the gap between the readdir and this read.
                    // Safe to skip silently — the file was folded
                    // into the snapshot and won't reappear.
                    debug!(
                        name = %name_str,
                        "log listed but no longer present; skipping",
                    );
                    continue;
                }
                Err(err) => {
                    // Any other failure (NAS/SMB hiccup, antivirus
                    // lock, permission error) must fail the WHOLE
                    // fetch: the orchestrator advances the cursor
                    // past every RETURNED file, so silently
                    // skipping this one would strand its events
                    // below the cursor forever. Failing leaves the
                    // cursor untouched; the next round retries.
                    return Err(SyncError::io(format!("reading log {name_str}: {err}")));
                }
            };
            out.push(LogFile {
                name: parsed,
                bytes,
            });
        }
        // Deterministic order — chronological by timestamp,
        // device-id tiebreak. Matches what the applier expects.
        out.sort_by(|a, b| {
            a.name
                .timestamp
                .cmp(&b.name.timestamp)
                .then_with(|| a.name.device_id.as_str().cmp(b.name.device_id.as_str()))
        });
        Ok(out)
    }

    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        self.ensure_dirs().await?;
        let path = self.log_dir().join(log.name.to_filename());
        // Log filenames embed (timestamp, device id) — collisions
        // between devices are impossible. We CAN race with our
        // own concurrent push of the same session file (the
        // currently-writing one), so we just overwrite — the
        // file's contents are append-only and longer overwrites
        // a shorter version. The applier's idempotency table
        // dedupes any envelopes the receiver already saw.
        let result: SyncResult<()> = async {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .await?;
            file.write_all(&log.bytes).await?;
            file.flush().await?;
            file.sync_data().await.ok();
            Ok(())
        }
        .await;
        self.invalidate_ensured_dirs_on_err(result)
    }

    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
        let path = self.snapshot_path();
        match fs::read(&path).await {
            Ok(bytes) => Ok(Some(Snapshot::from_bytes(&bytes)?)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(SyncError::io(err.to_string())),
        }
    }

    async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
        self.ensure_dirs().await?;
        let bytes = snapshot.to_bytes()?;
        self.invalidate_ensured_dirs_on_err(Self::atomic_write(&self.snapshot_path(), &bytes).await)
    }

    async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
        let path = self.log_dir().join(name.to_filename());
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(SyncError::io(err.to_string())),
        }
    }

    async fn push_sound_asset(&self, hash: &str, extension: &str, bytes: &[u8]) -> SyncResult<()> {
        self.ensure_dirs().await?;
        let path = self.sound_path(hash, extension);
        // Content-addressed paths are immutable once written. If
        // a file with the same hash already exists, skip the
        // write — the bytes must be identical by construction.
        if fs::metadata(&path).await.is_ok() {
            return Ok(());
        }
        self.invalidate_ensured_dirs_on_err(Self::atomic_write(&path, bytes).await)
    }

    async fn fetch_sound_asset(&self, hash: &str, extension: &str) -> SyncResult<Option<Vec<u8>>> {
        let path = self.sound_path(hash, extension);
        match fs::read(&path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(SyncError::io(err.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use sync_core::{DeviceId, EventEnvelope, IdPayload, MetaJson, Snapshot, SyncEvent};
    use tempfile::TempDir;

    fn fixture_envelope(device: DeviceId, ts_secs: i64, ev: SyncEvent) -> EventEnvelope {
        EventEnvelope {
            id: format!("evt_{:013x}", ts_secs),
            device_id: device,
            timestamp: Utc.timestamp_opt(ts_secs, 0).unwrap(),
            event: ev,
        }
    }

    fn fixture_log_file(device: DeviceId, ts_secs: i64, envelopes: Vec<EventEnvelope>) -> LogFile {
        LogFile::from_envelopes(device, Utc.timestamp_opt(ts_secs, 0).unwrap(), &envelopes).unwrap()
    }

    #[tokio::test]
    async fn test_connection_creates_directory_tree() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("sync");
        let adapter = LocalFsSyncAdapter::new(&root);
        // Pre-condition: directory doesn't exist yet.
        assert!(!root.exists());
        adapter.test_connection().await.unwrap();
        // The probe is gone; the layout dirs are present.
        assert!(root.exists());
        assert!(root.join("log").is_dir());
        assert!(root.join("assets").join("sounds").is_dir());
        assert!(!root.join(".aperio-write-probe").exists());
    }

    #[tokio::test]
    async fn fetch_new_logs_returns_empty_on_fresh_remote() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(tmp.path());
        let logs = adapter
            .fetch_new_logs(&DeviceCursor::epoch())
            .await
            .unwrap();
        assert!(logs.is_empty());
    }

    #[tokio::test]
    async fn push_log_then_fetch_round_trips() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(tmp.path());
        let device = DeviceId::from_string("dev-a".into());

        let log = fixture_log_file(
            device.clone(),
            1_000_000,
            vec![fixture_envelope(
                device.clone(),
                1_000_000,
                SyncEvent::EventDeleted(IdPayload { id: "x".into() }),
            )],
        );
        adapter.push_log(&log).await.unwrap();

        let fetched = adapter
            .fetch_new_logs(&DeviceCursor::epoch())
            .await
            .unwrap();
        assert_eq!(fetched.len(), 1);
        // Bytes round-trip exactly — same JSONL Aperio wrote.
        assert_eq!(fetched[0].bytes, log.bytes);
    }

    #[tokio::test]
    async fn grown_file_at_the_cursor_is_refetched() {
        // The append-miss fix: a peer's live session file gains events
        // AFTER we applied it; its timestamp sits at the cursor, but the
        // recorded applied length is smaller than the on-disk file, so it
        // must be fetched again.
        let tmp = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(tmp.path());
        let device = DeviceId::from_string("dev-a".into());
        let log = fixture_log_file(
            device.clone(),
            1_000,
            vec![fixture_envelope(
                device.clone(),
                1_000,
                SyncEvent::EventDeleted(IdPayload { id: "x".into() }),
            )],
        );
        adapter.push_log(&log).await.unwrap();

        let cursor_at = |known_len: u64| DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: None,
            known_lengths: vec![sync_core::KnownLogLength {
                name: log.name.to_filename(),
                len: known_len,
            }],
        };

        // Applied length smaller than the file → grown → refetched.
        let fetched = adapter
            .fetch_new_logs(&cursor_at(log.bytes.len() as u64 - 1))
            .await
            .unwrap();
        assert_eq!(fetched.len(), 1, "grown file re-fetched");

        // Applied length equals the file → unchanged → skipped.
        let fetched = adapter
            .fetch_new_logs(&cursor_at(log.bytes.len() as u64))
            .await
            .unwrap();
        assert!(fetched.is_empty(), "unchanged file skipped");
    }

    #[tokio::test]
    async fn unreadable_log_file_fails_the_whole_fetch() {
        // A per-file read error must NOT be silently skipped: the
        // orchestrator advances the cursor to the newest RETURNED
        // file, so a skipped older file would fall below the
        // cursor with no applied-length record and its events
        // would be lost on this device forever. The whole fetch
        // fails instead (cursor untouched, next round retries) —
        // the same contract as the WebDAV reference. A directory
        // squatting on the log's filename yields a deterministic
        // non-NotFound read error on both Unix (EISDIR) and
        // Windows (access denied).
        let tmp = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(tmp.path());
        let device = DeviceId::from_string("dev-a".into());

        let newer = fixture_log_file(
            device.clone(),
            2_000,
            vec![fixture_envelope(
                device.clone(),
                2_000,
                SyncEvent::EventDeleted(IdPayload { id: "x".into() }),
            )],
        );
        adapter.push_log(&newer).await.unwrap();

        // An older, validly-named entry that cannot be read.
        let broken = fixture_log_file(device.clone(), 1_000, vec![]);
        fs::create_dir_all(adapter.log_dir().join(broken.name.to_filename()))
            .await
            .unwrap();

        let result = adapter.fetch_new_logs(&DeviceCursor::epoch()).await;
        assert!(
            result.is_err(),
            "a failed log read must fail the whole fetch, not skip the file",
        );
    }

    #[tokio::test]
    async fn ensure_dirs_flag_survives_clones_and_clears_on_push_failure() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(tmp.path().join("sync"));
        let device = DeviceId::from_string("dev-a".into());
        let log = fixture_log_file(device.clone(), 1_000, vec![]);

        adapter.push_log(&log).await.unwrap();
        assert!(
            adapter.dirs_ensured.load(Ordering::Relaxed),
            "first push seeds the ensured-dirs cache",
        );
        // Clones share the cache (the orchestrator clones the
        // adapter into scheduler tasks).
        assert!(adapter.clone().dirs_ensured.load(Ordering::Relaxed));

        // The USB-remount case: the whole tree vanishes while the
        // flag still says "ensured" — the push fails (no silent
        // partial state) and drops the cache …
        fs::remove_dir_all(adapter.remote_root()).await.unwrap();
        assert!(adapter.push_log(&log).await.is_err());
        assert!(
            !adapter.dirs_ensured.load(Ordering::Relaxed),
            "push failure clears the ensured-dirs cache",
        );

        // … so the next push re-creates the layout and succeeds.
        adapter.push_log(&log).await.unwrap();
        assert!(adapter.log_dir().join(log.name.to_filename()).is_file());
    }

    #[tokio::test]
    async fn test_connection_reensures_even_when_the_cache_says_ensured() {
        // test_connection is the explicit health check — it must
        // bypass the ensured-dirs cache and rebuild a vanished
        // tree immediately, exactly like it does today.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("sync");
        let adapter = LocalFsSyncAdapter::new(&root);
        adapter.test_connection().await.unwrap();

        fs::remove_dir_all(&root).await.unwrap();
        adapter.test_connection().await.unwrap();
        assert!(root.join("log").is_dir());
        assert!(root.join("assets").join("sounds").is_dir());
    }

    #[tokio::test]
    async fn cursor_filters_already_seen_logs() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(tmp.path());
        let device = DeviceId::from_string("dev-a".into());

        // Push two log files at different timestamps.
        let early = fixture_log_file(device.clone(), 1_000, vec![]);
        let late = fixture_log_file(device.clone(), 2_000, vec![]);
        adapter.push_log(&early).await.unwrap();
        adapter.push_log(&late).await.unwrap();

        // Cursor at t=1500 should surface only the late one.
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_500, 0).unwrap(),
            exclude_device: None,
            known_lengths: Vec::new(),
        };
        let fetched = adapter.fetch_new_logs(&cursor).await.unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].name.timestamp.timestamp(), 2_000);
    }

    #[tokio::test]
    async fn meta_round_trips() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(tmp.path());

        assert!(adapter.fetch_meta().await.unwrap().is_none());

        let mut meta = MetaJson::fresh("1.0.0");
        meta.upsert_device(
            &DeviceId::from_string("dev-a".into()),
            sync_core::DeviceRecord {
                name: Some("Desktop".into()),
                last_seen_log: Utc::now(),
                last_seen: None,
                app_version: "1.0.0".into(),
                stale: false,
            },
        );
        adapter.push_meta(&meta).await.unwrap();

        let fetched = adapter.fetch_meta().await.unwrap().unwrap();
        assert_eq!(fetched.devices.len(), 1);
        assert!(fetched.devices.contains_key("dev-a"));
    }

    #[tokio::test]
    async fn snapshot_round_trips() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(tmp.path());

        assert!(adapter.fetch_snapshot().await.unwrap().is_none());

        let snap = Snapshot::new(Utc::now(), "1.0.0", serde_json::json!({ "events": [] }));
        adapter.push_snapshot(&snap).await.unwrap();

        let fetched = adapter.fetch_snapshot().await.unwrap().unwrap();
        assert_eq!(fetched.metadata.app_version, "1.0.0");
    }

    #[tokio::test]
    async fn sound_asset_dedupes_by_hash() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(tmp.path());

        let bytes = vec![1u8, 2, 3, 4, 5];
        adapter
            .push_sound_asset("hash123", "mp3", &bytes)
            .await
            .unwrap();
        // Second push with same hash is a no-op (content-
        // addressed; nothing to overwrite).
        adapter
            .push_sound_asset("hash123", "mp3", &bytes)
            .await
            .unwrap();

        let fetched = adapter.fetch_sound_asset("hash123", "mp3").await.unwrap();
        assert_eq!(fetched.as_deref(), Some(bytes.as_slice()));

        // Missing hash → None.
        let missing = adapter.fetch_sound_asset("nope", "mp3").await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn delete_log_removes_the_file() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(tmp.path());
        let device = DeviceId::from_string("dev-a".into());
        let log = fixture_log_file(device.clone(), 1_000, vec![]);
        adapter.push_log(&log).await.unwrap();
        assert_eq!(
            adapter
                .fetch_new_logs(&DeviceCursor::epoch())
                .await
                .unwrap()
                .len(),
            1
        );
        adapter.delete_log(&log.name).await.unwrap();
        assert!(adapter
            .fetch_new_logs(&DeviceCursor::epoch())
            .await
            .unwrap()
            .is_empty());
        // Deleting again is idempotent.
        adapter.delete_log(&log.name).await.unwrap();
    }

    #[tokio::test]
    async fn fetch_new_logs_skips_non_jsonl_entries() {
        let tmp = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(tmp.path());
        adapter.test_connection().await.unwrap();

        // Drop a stray non-log file into the log/ dir.
        fs::write(
            adapter.log_dir().join("README.md"),
            b"this is not a log file",
        )
        .await
        .unwrap();

        // Should return empty without erroring on the README.
        let logs = adapter
            .fetch_new_logs(&DeviceCursor::epoch())
            .await
            .unwrap();
        assert!(logs.is_empty());
    }

    #[tokio::test]
    async fn two_devices_against_same_root_see_each_others_logs() {
        // The integration scenario this whole adapter exists
        // for: two LocalFsSyncAdapter instances against the
        // same remote_root simulate two devices sharing a
        // network drive.
        let tmp = TempDir::new().unwrap();
        let dev_a = LocalFsSyncAdapter::new(tmp.path());
        let dev_b = LocalFsSyncAdapter::new(tmp.path());
        let id_a = DeviceId::from_string("dev-a".into());
        let id_b = DeviceId::from_string("dev-b".into());

        dev_a
            .push_log(&fixture_log_file(id_a.clone(), 1_000, vec![]))
            .await
            .unwrap();
        dev_b
            .push_log(&fixture_log_file(id_b.clone(), 2_000, vec![]))
            .await
            .unwrap();

        let from_a = dev_a.fetch_new_logs(&DeviceCursor::epoch()).await.unwrap();
        let from_b = dev_b.fetch_new_logs(&DeviceCursor::epoch()).await.unwrap();
        // Each device sees both — the orchestrator's job is to
        // filter out the originator's own logs.
        assert_eq!(from_a.len(), 2);
        assert_eq!(from_b.len(), 2);
        // Sorted chronologically.
        assert_eq!(from_a[0].name.timestamp.timestamp(), 1_000);
        assert_eq!(from_a[1].name.timestamp.timestamp(), 2_000);
    }
}
