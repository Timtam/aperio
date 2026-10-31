//! Event-log writer for the cross-device sync layer (Phase Sb, DESIGN.md §19).
//!
//! Every local mutation that participates in sync funnels through this writer:
//!
//! ```text
//! command                  EventLogWriter            Disk
//!  ───────                   ────────────             ────
//!   update_event ── append(SyncEvent) ──►   send to channel
//!                                            │
//!                                            ▼
//!                                            drain_loop
//!                                            │
//!                                            ▼
//!                                    <data_dir>/sync/log/pending/
//!                                    <session-ts>_<device>.jsonl
//! ```
//!
//! - `append()` is **non-blocking** — it sends the envelope onto a tokio mpsc
//!   and returns. The mutation never waits on disk I/O.
//! - The **background drain task** owns the file handle. One file per app
//!   session, named with the session-start timestamp + device id (matches
//!   `sync_core::LogFileName`).
//! - Files land in `<data_dir>/sync/log/pending/`; a sync adapter moves them
//!   to `log/` after a successful push.
//!
//! Platform-agnostic: it uses `tokio::fs` against an injected `data_dir` and
//! takes the `device_id` + session timestamp as parameters, so the same writer
//! runs on the desktop (Tauri) and on mobile (UniFFI). The device id is loaded
//! from `user_prefs` by the host (a DB concern) and handed in.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use sync_core::{DeviceId, EventEnvelope, LogFileName, SyncEvent};
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, Notify};
use tracing::{debug, warn};

use crate::SyncStore;

/// A message on the drain queue. Either an event to append or a request to
/// roll over to a new session file. Routing the rotation through the same
/// ordered queue as the events guarantees a clean cut: every event enqueued
/// before a `Rotate` lands in the old file, everything after in the new one.
enum DrainMsg {
    /// Boxed because `EventEnvelope` dwarfs the `Rotate` variant; an unboxed
    /// enum would pad every queued event to the larger size.
    Event(Box<EventEnvelope>),
    Rotate {
        new_session_at: DateTime<Utc>,
        /// Replies with the path of the now-closed pre-rotation file so the
        /// caller can delete it once the snapshot covering it has been pushed.
        /// `None` when no rotation happened.
        ack: oneshot::Sender<Option<PathBuf>>,
    },
}

/// Handle to the per-process event-log writer.
#[derive(Clone)]
pub struct EventLogWriter {
    device_id: DeviceId,
    /// Sender half of the drain queue. `send` is allocation-only — appending a
    /// mutation event is constant time and never awaits.
    sender: mpsc::UnboundedSender<DrainMsg>,
    /// Optional kick handle the scheduler hands us so we can ping it after
    /// every append. `Option<_>` because tests + the "no scheduler yet"
    /// startup path skip it.
    ///
    /// `Notify::notify_one()` is fire-and-forget — if the scheduler is already
    /// inside a debounce window, the second ping is absorbed and the same
    /// round flushes both edits (the coalescing DESIGN.md §19.8 asks for).
    kick: Option<Arc<Notify>>,
    /// Where this device's own changes advance their row's clock
    /// (`sync_row_clock`, DESIGN-sync-snapshot-coverage.md), so an older
    /// change from another device, arriving late, does not win over them.
    /// Set once the store exists ([`Self::set_row_clock`]); shared by every
    /// clone of the handle.
    row_clock: Arc<OnceLock<Arc<dyn SyncStore>>>,
    /// The time the file the writer appends to is named by: this launch's
    /// boot, then each rotation's. Shared by every clone of the handle.
    session_at: Arc<std::sync::Mutex<DateTime<Utc>>>,
}

impl std::fmt::Debug for EventLogWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventLogWriter")
            .field("device_id", &self.device_id)
            .field("kick", &self.kick.is_some())
            .field("row_clock", &self.row_clock.get().is_some())
            .finish()
    }
}

impl EventLogWriter {
    /// Initialise the writer and spawn its background drain task. The drain
    /// task lives for the process lifetime; on `drop` of the last `Arc` the
    /// sender side closes and the loop exits cleanly.
    ///
    /// **Must be called from within a Tokio runtime context** (it starts the
    /// drain task with `tokio::spawn`). A caller with no ambient runtime — e.g.
    /// a desktop startup path that runs before the event loop — must enter one
    /// first (`Handle::enter`, or wrap the call in `block_on`), or it panics
    /// with "there is no reactor running".
    pub fn spawn(data_dir: PathBuf, device_id: DeviceId) -> Arc<Self> {
        Self::spawn_with_kick(data_dir, device_id, None, Utc::now())
    }

    /// Variant that wires the writer into a scheduler kick channel.
    ///
    /// `session_at` is THIS launch's boot timestamp and names the session's
    /// JSONL file. It MUST be the same instant the orchestrator stores as its
    /// `boot_at`: the orchestrator's empty-stub cleanup deletes pending files
    /// whose session timestamp is `< boot_at`, so a mismatched (earlier)
    /// instant here would let it delete the live session file out from under
    /// the open handle (on Windows `FILE_SHARE_DELETE` → silent event loss).
    /// Sharing one instant makes the comparison `session_at == boot_at` (never
    /// `<`), killing the race.
    ///
    /// Like [`Self::spawn`], **must be called from within a Tokio runtime
    /// context** — it `tokio::spawn`s the drain task.
    pub fn spawn_with_kick(
        data_dir: PathBuf,
        device_id: DeviceId,
        kick: Option<Arc<Notify>>,
        session_at: DateTime<Utc>,
    ) -> Arc<Self> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let writer = Arc::new(Self {
            device_id: device_id.clone(),
            sender,
            kick,
            row_clock: Arc::new(OnceLock::new()),
            session_at: Arc::new(std::sync::Mutex::new(session_at)),
        });
        let pending_dir = data_dir.join("sync").join("log").join("pending");
        tokio::spawn(drain_loop(pending_dir, device_id, session_at, receiver));
        writer
    }

    /// The time the file the writer appends to is named by. A pending file
    /// named before it is finished: nothing appends to it any more.
    pub fn session_at(&self) -> DateTime<Utc> {
        *self.session_at.lock().expect("session_at mutex poison")
    }

    /// Borrow the device id this writer is tagging events with.
    pub fn device_id(&self) -> &DeviceId {
        &self.device_id
    }

    /// Hand the writer the store whose row clocks this device's own changes
    /// advance. Once; a second call is ignored.
    pub fn set_row_clock(&self, store: Arc<dyn SyncStore>) {
        let _ = self.row_clock.set(store);
    }

    /// Queue one [`SyncEvent`] for writing. Returns immediately — the actual
    /// disk write happens in the drain task. A `send` error (only possible if
    /// the drain task has exited, which it doesn't in production) logs a
    /// warning; the mutation itself already succeeded against the store.
    pub fn append(&self, event: SyncEvent) {
        let mut envelope = EventEnvelope::new(self.device_id.clone(), event);
        // This change is the newest its row has seen: an older one from
        // another device, arriving later, must not win over it.
        if let (Some(store), Some(key)) = (self.row_clock.get(), envelope.event.row_key()) {
            // Newer than anything the row has seen, even where this device's
            // clock lags the one that wrote it: stamped older, every other
            // device would skip it for good (review round 2 of #130).
            match store.row_clock(&key) {
                Ok(Some(clock)) if clock.changed >= envelope.timestamp => {
                    envelope.timestamp = clock.changed + chrono::Duration::microseconds(1);
                }
                Ok(_) => {}
                Err(err) => warn!(?err, kind = key.kind, "couldn't read the row's clock"),
            }
            let write = crate::RowWrite::of(&envelope.event);
            if let Err(err) = store.advance_row_clock(&key, envelope.timestamp, write) {
                warn!(?err, kind = key.kind, "couldn't advance the row's clock");
            }
        }
        if let Err(err) = self.sender.send(DrainMsg::Event(Box::new(envelope))) {
            warn!(?err, "event-log writer channel closed; event lost");
        }
        // Tell the scheduler something happened. Notify is a one-shot latch —
        // extra `notify_one()` calls during a debounce window are absorbed, so
        // bulk operations don't fan out to N pushes.
        if let Some(kick) = &self.kick {
            kick.notify_one();
        }
    }

    /// Roll over to a fresh session file stamped `new_session_at`.
    ///
    /// Flushes + closes the current session file and opens a new one, then
    /// returns the path of the now-closed file so the caller can delete it.
    /// The compactor calls this so a compaction's snapshot makes the closed
    /// file redundant; post-compaction edits land in the newer file.
    ///
    /// Ordered through the same queue as [`append`](Self::append): every event
    /// enqueued before this call lands in the OLD file, everything after in the
    /// new one. Returns `None` when no rotation happened.
    pub async fn rotate(&self, new_session_at: DateTime<Utc>) -> Option<PathBuf> {
        let (ack, rx) = oneshot::channel();
        if self
            .sender
            .send(DrainMsg::Rotate {
                new_session_at,
                ack,
            })
            .is_err()
        {
            warn!("event-log writer channel closed; rotation skipped");
            return None;
        }
        // `Err` here means the drain task dropped the ack without replying (it
        // exited mid-rotation) — treat as "no rotation".
        let closed = rx.await.ok().flatten();
        if closed.is_some() {
            // Only after the swap: until then the old file is the live one.
            *self.session_at.lock().expect("session_at mutex poison") = new_session_at;
        }
        closed
    }
}

/// The drain task. Lives for the process lifetime — exits only when every
/// `EventLogWriter` clone has been dropped and the channel is empty.
async fn drain_loop(
    pending_dir: PathBuf,
    device_id: DeviceId,
    session_at: DateTime<Utc>,
    mut receiver: mpsc::UnboundedReceiver<DrainMsg>,
) {
    if let Err(err) = tokio::fs::create_dir_all(&pending_dir).await {
        warn!(
            path = %pending_dir.display(),
            ?err,
            "failed to create event-log staging directory; sync writes will be lost",
        );
        return;
    }

    // One open file at a time. Starts as this session's file; a `Rotate` swaps
    // it for a fresh one. Naming follows `sync_core::LogFileName` exactly so a
    // sync adapter can pick it up without reformatting.
    let mut path = session_file_path(&pending_dir, &device_id, session_at);
    debug!(path = %path.display(), "event-log writer opening session file");
    let Some(mut file) = open_append(&path).await else {
        return;
    };

    while let Some(msg) = receiver.recv().await {
        match msg {
            DrainMsg::Event(envelope) => {
                if let Err(err) = write_one(&mut file, &envelope).await {
                    warn!(
                        event_id = %envelope.id,
                        ?err,
                        "failed to write event to log; dropping it",
                    );
                }
            }
            DrainMsg::Rotate {
                new_session_at,
                ack,
            } => {
                // Flush the closing file so its bytes are durable before the
                // caller (the compactor) deletes it.
                if let Err(err) = file.flush().await {
                    warn!(?err, "flush before session rotation failed");
                }
                let new_path = session_file_path(&pending_dir, &device_id, new_session_at);
                match open_append(&new_path).await {
                    Some(new_file) => {
                        let old_path = std::mem::replace(&mut path, new_path);
                        file = new_file;
                        debug!(
                            old = %old_path.display(),
                            new = %path.display(),
                            "event-log writer rotated session file",
                        );
                        let _ = ack.send(Some(old_path));
                    }
                    None => {
                        // Couldn't open the new file — keep the current one so
                        // events aren't lost; signal "no rotation".
                        let _ = ack.send(None);
                    }
                }
            }
        }
    }

    // Channel closed (every writer Arc dropped). Make sure the last batch hit
    // disk before we exit.
    if let Err(err) = file.flush().await {
        warn!(?err, "flush on event-log shutdown failed");
    }
}

/// `<pending_dir>/<session-ts>_<device>.jsonl` — formatted exactly like
/// `sync_core::LogFileName` so a sync adapter can pick it up without
/// reformatting.
fn session_file_path(
    pending_dir: &Path,
    device_id: &DeviceId,
    session_at: DateTime<Utc>,
) -> PathBuf {
    let name = LogFileName::new(session_at, device_id.clone());
    pending_dir.join(name.to_filename())
}

/// Open a session file for append, creating it if absent. `None` on error
/// (logged) — the caller treats that as "writer can't persist".
async fn open_append(path: &Path) -> Option<File> {
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        Ok(f) => Some(f),
        Err(err) => {
            warn!(
                path = %path.display(),
                ?err,
                "failed to open event-log file; sync writes will be lost",
            );
            None
        }
    }
}

/// Serialise one envelope and append it as a JSONL line. Flushes after every
/// write — mutations happen at user speed (one at a time, not bursts), so the
/// per-event flush cost is invisible and gives durability across crashes.
async fn write_one(file: &mut File, envelope: &EventEnvelope) -> Result<(), std::io::Error> {
    let line = serde_json::to_vec(envelope)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
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
        writer.append(SyncEvent::EventDeleted(IdPayload { id: "ev-1".into() }));

        drop(writer);
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let pending = tmp.path().join("sync").join("log").join("pending");
            if let Ok(mut entries) = tokio::fs::read_dir(&pending).await {
                if let Ok(Some(entry)) = entries.next_entry().await {
                    let bytes = tokio::fs::read(entry.path()).await.unwrap();
                    let text = String::from_utf8_lossy(&bytes);
                    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
                    if lines.len() >= 2 {
                        assert!(
                            lines[0].contains(r#""type":"event.created""#),
                            "got: {}",
                            lines[0]
                        );
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
        writer.append(SyncEvent::EventDeleted(IdPayload { id: "x".into() }));
        drop(writer);

        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let pending = tmp.path().join("sync").join("log").join("pending");
            if let Ok(mut entries) = tokio::fs::read_dir(&pending).await {
                if let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    let parsed = LogFileName::from_filename(&name_str).expect("filename parseable");
                    assert_eq!(parsed.device_id.as_str(), "dev-format");
                    return;
                }
            }
        }
        panic!("no log file created");
    }

    #[tokio::test]
    async fn rotate_closes_old_file_opens_new_and_routes_events_cleanly() {
        use chrono::{TimeZone, Utc};
        let boot = Utc.with_ymd_and_hms(2026, 6, 1, 8, 0, 0).unwrap();
        let cut = Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap();
        let tmp = TempDir::new().unwrap();
        let device = DeviceId::from_string("dev-rot".into());
        let writer =
            EventLogWriter::spawn_with_kick(tmp.path().to_path_buf(), device.clone(), None, boot);

        writer.append(SyncEvent::EventDeleted(IdPayload {
            id: "evt-pre-rotation".into(),
        }));
        let old_path = writer
            .rotate(cut)
            .await
            .expect("rotation returns the closed file's path");
        writer.append(SyncEvent::EventDeleted(IdPayload {
            id: "evt-post-rotation".into(),
        }));
        drop(writer);

        let pending = tmp.path().join("sync").join("log").join("pending");
        let new_path = pending.join(LogFileName::new(cut, device.clone()).to_filename());

        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let (Ok(old_bytes), Ok(new_bytes)) = (
                tokio::fs::read(&old_path).await,
                tokio::fs::read(&new_path).await,
            ) else {
                continue;
            };
            let old = String::from_utf8_lossy(&old_bytes);
            let new = String::from_utf8_lossy(&new_bytes);
            if old.contains("evt-pre-rotation") && new.contains("evt-post-rotation") {
                assert!(
                    !old.contains("evt-post-rotation"),
                    "old file leaked the post-rotation event: {old}",
                );
                assert!(
                    !new.contains("evt-pre-rotation"),
                    "new file leaked the pre-rotation event: {new}",
                );
                assert_ne!(old_path, new_path);
                return;
            }
        }
        panic!("rotation didn't split the events across the two files within 1 s");
    }

    #[tokio::test]
    async fn own_changes_advance_their_rows_clock_as_they_are_written() {
        // DESIGN-sync-snapshot-coverage.md: this device's own change is the
        // newest its row has seen, so an older one from another device,
        // arriving later, does not win over it.
        let tmp = TempDir::new().unwrap();
        let writer =
            EventLogWriter::spawn(tmp.path().to_path_buf(), DeviceId::from_string("me".into()));
        let store = Arc::new(crate::test_support::FakeStore::default());
        writer.set_row_clock(store.clone());
        let before = Utc::now();
        writer.append(SyncEvent::SettingsUpdated(sync_core::SettingsPayload {
            key: "appearance.darkMode".into(),
            value: serde_json::json!(true),
        }));
        let key = sync_core::RowKey {
            kind: "setting",
            id: "appearance.darkMode".into(),
        };
        let clock = store.row_clocks.lock().unwrap().get(&key).copied();
        // A setting is written whole: both times move.
        assert!(
            clock.is_some_and(
                |clock| clock.changed >= before && clock.written == Some(clock.changed)
            ),
            "{clock:?}",
        );
    }

    #[tokio::test]
    async fn an_own_change_is_newer_than_its_row_even_when_this_clock_lags() {
        // The row's newest change came from a device whose clock runs ahead:
        // stamped with this device's time, this later change would count as
        // older everywhere else.
        let tmp = TempDir::new().unwrap();
        let writer =
            EventLogWriter::spawn(tmp.path().to_path_buf(), DeviceId::from_string("me".into()));
        let store = Arc::new(crate::test_support::FakeStore::default());
        writer.set_row_clock(store.clone());
        let key = sync_core::RowKey {
            kind: "setting",
            id: "appearance.darkMode".into(),
        };
        let ahead = Utc::now() + chrono::Duration::minutes(5);
        store
            .advance_row_clock(&key, ahead, crate::RowWrite::Whole)
            .unwrap();
        writer.append(SyncEvent::SettingsUpdated(sync_core::SettingsPayload {
            key: "appearance.darkMode".into(),
            value: serde_json::json!(false),
        }));
        let stamped = ahead + chrono::Duration::microseconds(1);
        let clock = store.row_clocks.lock().unwrap().get(&key).copied().unwrap();
        assert_eq!(clock.changed, stamped);
        // And that is the time the peers read in the file.
        let pending = tmp.path().join("sync").join("log").join("pending");
        let mut written = Vec::new();
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if let Some(file) = std::fs::read_dir(&pending)
                .ok()
                .and_then(|mut entries| entries.next())
                .and_then(|entry| entry.ok())
            {
                written = std::fs::read(file.path()).unwrap_or_default();
                if !written.is_empty() {
                    break;
                }
            }
        }
        let line = String::from_utf8(written).unwrap();
        let envelope: EventEnvelope = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(envelope.timestamp, stamped);
    }

    #[tokio::test]
    async fn session_file_is_named_with_the_injected_boot_instant() {
        use chrono::{TimeZone, Utc};
        let boot = Utc.with_ymd_and_hms(2026, 5, 31, 7, 0, 0).unwrap();
        let tmp = TempDir::new().unwrap();
        let writer = EventLogWriter::spawn_with_kick(
            tmp.path().to_path_buf(),
            DeviceId::from_string("dev-boot".into()),
            None,
            boot,
        );
        writer.append(SyncEvent::EventDeleted(IdPayload { id: "x".into() }));
        drop(writer);

        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let pending = tmp.path().join("sync").join("log").join("pending");
            if let Ok(mut entries) = tokio::fs::read_dir(&pending).await {
                if let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name();
                    let parsed = LogFileName::from_filename(&name.to_string_lossy())
                        .expect("filename parseable");
                    assert_eq!(parsed.timestamp, boot);
                    return;
                }
            }
        }
        panic!("no log file created");
    }
}
