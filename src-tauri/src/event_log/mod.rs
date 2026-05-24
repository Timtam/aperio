//! Event-log writer for the cross-device sync layer (Phase Sb,
//! DESIGN.md §19).
//!
//! Every local mutation that participates in sync funnels through
//! this writer:
//!
//! ```text
//! Tauri command            EventLogWriter            Disk
//!  ─────────────             ────────────             ────
//!   commands::update_event ── append(SyncEvent) ──►   send to channel
//!                                                      │
//!                                                      ▼
//!                                                      drain_loop
//!                                                      │
//!                                                      ▼
//!                                              <data_dir>/sync/log/pending/
//!                                              <session-ts>_<device>.jsonl
//! ```
//!
//! - `append()` is **non-blocking** — it sends the envelope onto a
//!   tokio mpsc and returns. The mutation command never waits on
//!   disk I/O.
//! - The **background drain task** owns the file handle. One file
//!   per app session, named with the session-start timestamp +
//!   device id (matches `sync_core::LogFileName`).
//! - Files land in `<data_dir>/sync/log/pending/` rather than
//!   `<data_dir>/sync/log/` proper. "Pending" means "not yet
//!   uploaded by a sync adapter" — Phase Sd's adapter moves them
//!   to `log/` after a successful push.
//!
//! The device id is loaded from `user_prefs.sync.deviceId` on
//! first call to [`EventLogWriter::load_or_mint_device_id`]; if
//! unset, a fresh UUID v4 is minted and persisted. The writer
//! then carries that id for the rest of the process lifetime —
//! re-installing Aperio mints a new id, which is the intended
//! "different device" semantics per §19.

pub mod applier;
pub mod orchestrator;
pub mod scheduler;

pub use applier::{ApplyReport, EventLogApplier};
pub use orchestrator::{
    SyncOrchestrator, SyncRoundReport, SyncStatus, SYNC_CURSOR_PREF_KEY,
};
pub use scheduler::{
    read_interval_minutes, SyncScheduler, SyncStatusPayload,
    DEFAULT_SYNC_INTERVAL_MINUTES, PREF_SYNC_INTERVAL_MINUTES,
};

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use sync_core::{DeviceId, EventEnvelope, LogFileName, SyncEvent, DEVICE_ID_PREF_KEY};
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Notify};
use tracing::{debug, warn};

use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

/// Handle to the per-process event-log writer.
///
/// Stored as Tauri-managed state so every `#[tauri::command]` can
/// borrow it via `State<'_, Arc<EventLogWriter>>` and emit
/// mutations without thinking about file I/O.
#[derive(Debug, Clone)]
pub struct EventLogWriter {
    device_id: DeviceId,
    /// Sender half of the drain queue. `UnboundedSender::send` is
    /// effectively allocation-only — appending a mutation event
    /// is constant time and never awaits.
    sender: mpsc::UnboundedSender<EventEnvelope>,
    /// Optional kick handle the [`SyncScheduler`] hands us so we can
    /// ping it after every append. `Option<_>` because tests + the
    /// fallback "no scheduler yet" startup path skip it; in
    /// production `lib.rs` always wires one through.
    ///
    /// `Notify::notify_one()` is fire-and-forget — if the scheduler
    /// is already inside a debounce window, the second ping is
    /// absorbed and the same round flushes both edits. That's the
    /// coalescing behaviour DESIGN.md §19.8 asks for.
    kick: Option<Arc<Notify>>,
}

impl EventLogWriter {
    /// Initialise the writer and spawn its background drain task.
    ///
    /// Idempotent over `data_dir` — calling twice with the same
    /// directory just creates the staging tree twice (harmless,
    /// `create_dir_all` is forgiving). The drain task lives for
    /// the process lifetime; on `drop` of the last `Arc` the
    /// sender side closes and the loop exits cleanly.
    pub fn spawn(data_dir: PathBuf, device_id: DeviceId) -> Arc<Self> {
        Self::spawn_with_kick(data_dir, device_id, None)
    }

    /// Variant that wires the writer into a [`SyncScheduler`] kick
    /// channel. `lib.rs` uses this so every local mutation triggers
    /// a debounced sync push; tests use [`Self::spawn`] without one.
    pub fn spawn_with_kick(
        data_dir: PathBuf,
        device_id: DeviceId,
        kick: Option<Arc<Notify>>,
    ) -> Arc<Self> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let writer = Arc::new(Self {
            device_id: device_id.clone(),
            sender,
            kick,
        });
        let pending_dir = data_dir.join("sync").join("log").join("pending");
        tauri::async_runtime::spawn(drain_loop(
            pending_dir,
            device_id,
            receiver,
        ));
        writer
    }

    /// Load the persisted device id from `user_prefs`, or mint a
    /// fresh one and persist it. Called once during `setup()`
    /// before the writer is spawned.
    ///
    /// We don't tie this to the writer's construction because
    /// the id is needed elsewhere too (meta.json registrar, the
    /// snapshot generator, the sync scheduler) — keeping the
    /// load helper free-standing means every consumer reads the
    /// same single source of truth.
    pub fn load_or_mint_device_id(db: &SharedConn) -> DeviceId {
        let repo = UserPrefsRepo::new(db);
        match repo.get(DEVICE_ID_PREF_KEY) {
            Ok(Some(stored)) if !stored.is_empty() => {
                DeviceId::from_string(stored)
            }
            _ => {
                let fresh = DeviceId::new();
                // Persist immediately. If the write fails (unlikely
                // — only a SQLite I/O hiccup), we warn and continue
                // with the in-memory id; the next app start will
                // re-mint, which is the right "device looked like
                // a new install" behaviour for a corrupted DB.
                if let Err(err) = repo.set(DEVICE_ID_PREF_KEY, fresh.as_str()) {
                    warn!(
                        ?err,
                        "couldn't persist {DEVICE_ID_PREF_KEY}; falling back to a transient id",
                    );
                }
                fresh
            }
        }
    }

    /// Borrow the device id this writer is tagging events with.
    /// Used by the (forthcoming) meta.json registrar + scheduler.
    pub fn device_id(&self) -> &DeviceId {
        &self.device_id
    }

    /// Queue one [`SyncEvent`] for writing. Returns immediately —
    /// the actual disk write happens in the drain task. Errors
    /// from the channel `send` (only possible if the drain task
    /// has exited, which it doesn't in production) log a
    /// warning; the mutation itself succeeded against SQLite so
    /// we don't surface the error to the user.
    pub fn append(&self, event: SyncEvent) {
        let envelope = EventEnvelope::new(self.device_id.clone(), event);
        if let Err(err) = self.sender.send(envelope) {
            warn!(?err, "event-log writer channel closed; event lost");
        }
        // Tell the scheduler something happened. Notify is a one-shot
        // latch — extra `notify_one()` calls during a debounce window
        // are absorbed, so bulk operations don't fan out to N pushes.
        if let Some(kick) = &self.kick {
            kick.notify_one();
        }
    }
}

/// The drain task. Lives for the process lifetime — exits only
/// when every `EventLogWriter` clone has been dropped and the
/// channel is empty.
async fn drain_loop(
    pending_dir: PathBuf,
    device_id: DeviceId,
    mut receiver: mpsc::UnboundedReceiver<EventEnvelope>,
) {
    if let Err(err) = tokio::fs::create_dir_all(&pending_dir).await {
        warn!(
            path = %pending_dir.display(),
            ?err,
            "failed to create event-log staging directory; sync writes will be lost",
        );
        return;
    }

    // One file per app session. Naming follows
    // `sync_core::LogFileName` exactly so a sync adapter can pick
    // it up later without reformatting.
    let session_name = LogFileName::new(Utc::now(), device_id.clone());
    let path = pending_dir.join(session_name.to_filename());
    debug!(
        path = %path.display(),
        "event-log writer opening session file",
    );

    let mut file = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    {
        Ok(f) => f,
        Err(err) => {
            warn!(
                path = %path.display(),
                ?err,
                "failed to open event-log file; sync writes will be lost",
            );
            return;
        }
    };

    while let Some(envelope) = receiver.recv().await {
        if let Err(err) = write_one(&mut file, &envelope).await {
            warn!(
                event_id = %envelope.id,
                ?err,
                "failed to write event to log; dropping it",
            );
        }
    }

    // Channel closed (every writer Arc has been dropped). Make
    // sure the last batch hit disk before we exit.
    if let Err(err) = file.flush().await {
        warn!(?err, "flush on event-log shutdown failed");
    }
}

/// Serialise one envelope and append it as a JSONL line. Flushes
/// after every write — mutations happen at user speed (one at a
/// time, not bursts), so the per-event flush cost is invisible
/// and gives us durability across crashes.
async fn write_one(
    file: &mut File,
    envelope: &EventEnvelope,
) -> Result<(), std::io::Error> {
    let line = serde_json::to_vec(envelope).map_err(|err| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, err)
    })?;
    file.write_all(&line).await?;
    file.write_all(b"\n").await?;
    file.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sync_core::{EventPayload, IdPayload};
    use tempfile::TempDir;

    #[tokio::test]
    async fn append_writes_one_jsonl_line_per_event() {
        let tmp = TempDir::new().unwrap();
        let writer = EventLogWriter::spawn(
            tmp.path().to_path_buf(),
            DeviceId::from_string("dev-test".into()),
        );

        writer.append(SyncEvent::EventCreated(EventPayload {
            id: "ev-1".into(),
            fields: serde_json::json!({ "title": "hello" }),
        }));
        writer.append(SyncEvent::EventDeleted(IdPayload {
            id: "ev-1".into(),
        }));

        // Drop our handle so the drain loop exits and the file is
        // flushed/closed. The test then reads it back.
        drop(writer);
        // Give the runtime a tick to actually run the drain loop's
        // final flush.
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let pending = tmp.path().join("sync").join("log").join("pending");
            if let Ok(mut entries) = tokio::fs::read_dir(&pending).await {
                if let Ok(Some(entry)) = entries.next_entry().await {
                    let bytes = tokio::fs::read(entry.path()).await.unwrap();
                    let text = String::from_utf8_lossy(&bytes);
                    let lines: Vec<&str> =
                        text.lines().filter(|l| !l.is_empty()).collect();
                    if lines.len() >= 2 {
                        // First line should be the created event.
                        assert!(
                            lines[0].contains(r#""type":"event.created""#),
                            "got: {}",
                            lines[0]
                        );
                        // Second line the delete.
                        assert!(
                            lines[1].contains(r#""type":"event.deleted""#),
                            "got: {}",
                            lines[1]
                        );
                        return;
                    }
                }
            }
        }
        panic!("drain loop didn't write the expected lines within 1 s");
    }

    #[tokio::test]
    async fn filename_matches_sync_core_log_file_name_format() {
        let tmp = TempDir::new().unwrap();
        let writer = EventLogWriter::spawn(
            tmp.path().to_path_buf(),
            DeviceId::from_string("dev-format".into()),
        );
        writer.append(SyncEvent::EventDeleted(IdPayload {
            id: "x".into(),
        }));
        drop(writer);

        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let pending = tmp.path().join("sync").join("log").join("pending");
            if let Ok(mut entries) = tokio::fs::read_dir(&pending).await {
                if let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    // Parse via the sync-core helper — round-trip
                    // through the same code path the sync adapter
                    // will use when it pushes the file. The id
                    // we round-trip is the bare "dev-format"
                    // string; the timestamp restoration check is
                    // implicit because the parser is the contract.
                    let parsed = LogFileName::from_filename(&name_str)
                        .expect("filename parseable");
                    assert_eq!(parsed.device_id.as_str(), "dev-format");
                    return;
                }
            }
        }
        panic!("no log file created");
    }
}
