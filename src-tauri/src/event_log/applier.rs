//! Event-log applier — the consumer side of Phase Sc.
//!
//! Reads [`sync_core::EventEnvelope`]s out of a [`LogFile`]
//! (typically one fetched from a remote sync adapter), and
//! integrates them into the local SQLite cache. The pipeline is:
//!
//! ```text
//!   LogFile bytes ──► EventEnvelope[]
//!                     │
//!                     ▼  sort chronologically
//!                     │
//!                     ├─ skip envelopes from this device's id (we
//!                     │  already applied them when we minted them)
//!                     ├─ skip envelopes whose `id` is already in
//!                     │  sync_applied_events (idempotency)
//!                     │
//!                     ▼  dispatch on SyncEvent variant
//!                     │
//!         ┌───────────┼───────────────┐
//!         ▼           ▼               ▼
//!     events/      tasks/         color_labels/
//!     calendars    task_lists     settings (user_prefs)
//!         │           │               │
//!         └─ upsert helpers on LocalAdapter (`*_from_sync`)
//!         └─ idempotent INSERT OR DO UPDATE
//!         └─ record event_id in sync_applied_events
//! ```
//!
//! ## What the applier deliberately does NOT do (yet)
//!
//! - **Field-level merge with conflict detection.** Phase Sb's
//!   writer emits the full row on update, so the applier
//!   currently does last-write-wins by event timestamp. Real
//!   diff-based merge + conflict surfacing lands with the
//!   conflict-UI work in Phase Sh.
//! - **Cross-device cursor management.** That's the sync
//!   scheduler's job (Phase Se / Sd) — knowing which files we
//!   already fetched. The applier just acts on whatever envelope
//!   list it's handed.
//! - **Snapshot application.** Snapshots are a Phase-Sg artefact;
//!   the applier handles the per-event path only.
//! - **plugin.* / shortcut.*** event variants. Phase Sb defines
//!   them in the SyncEvent enum but doesn't yet emit them
//!   (plugin manager + shortcut overrides aren't wired). The
//!   applier mirrors that — those variants log a debug line and
//!   no-op rather than failing, so a forward-compat dataset
//!   from a future Aperio doesn't break us.

use std::sync::Arc;

use cal_adapter_local::LocalAdapter;
use chrono::Utc;
use rusqlite::params;
use sync_core::{
    DeviceId, EventEnvelope, EventPayload, IdPayload, LogFile, SettingsPayload,
    SyncError, SyncEvent, SyncResult,
};
use tracing::{debug, warn};

use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

/// Per-call summary the applier hands back so callers (the sync
/// scheduler, settings dialog "Reapply log" actions, tests) can
/// surface what happened without grovelling through tracing
/// output.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    /// Envelopes whose `event` actually wrote to SQLite.
    pub applied: usize,
    /// Envelopes whose `device_id` matched the local one — we
    /// minted them and applied them at write time, so the
    /// loopback pass through the applier is a no-op.
    pub skipped_own: usize,
    /// Envelopes whose `id` was already in
    /// `sync_applied_events`. Re-fetches and overlapping log
    /// files both end up here.
    pub skipped_already_applied: usize,
    /// Variants we don't have a handler for yet (plugin.*,
    /// shortcut.*). Logged at debug.
    pub skipped_unsupported: usize,
    /// Per-envelope failures. Doesn't sink the run — we keep
    /// applying the remaining envelopes; a single bad row
    /// shouldn't strand a 1000-row log file. The number here
    /// signals whether we should warn the user that some data
    /// didn't make it.
    pub failed: usize,
}

/// The applier itself.
pub struct EventLogApplier {
    db: SharedConn,
    /// Reference to the local adapter so we can call its
    /// `*_from_sync` upsert helpers. Wrapped in `Arc` because the
    /// applier is constructed once and held by the sync scheduler.
    adapter: Arc<LocalAdapter>,
    /// This device's id. Envelopes carrying this id originated
    /// here and have already been applied locally during their
    /// emit — skipping them in the applier prevents re-running
    /// the same insert.
    local_device_id: DeviceId,
}

impl EventLogApplier {
    pub fn new(
        db: SharedConn,
        adapter: Arc<LocalAdapter>,
        local_device_id: DeviceId,
    ) -> Self {
        Self {
            db,
            adapter,
            local_device_id,
        }
    }

    /// Apply every envelope in one log file. Convenience wrapper
    /// around `apply_envelopes` that handles the JSONL → envelope
    /// decoding via `LogFile::into_envelopes`.
    pub fn apply_log_file(&self, log: &LogFile) -> SyncResult<ApplyReport> {
        let envelopes = log.into_envelopes()?;
        self.apply_envelopes(envelopes)
    }

    /// Apply a batch of envelopes. The caller can hand in
    /// envelopes from any source (one log file, several, or a
    /// snapshot's appended tail) — the applier sorts them by
    /// (timestamp, id) before dispatching, so the same input set
    /// produces the same final state regardless of source order.
    pub fn apply_envelopes(
        &self,
        mut envelopes: Vec<EventEnvelope>,
    ) -> SyncResult<ApplyReport> {
        // Chronological order. ULID-prefixed ids sort
        // lexicographically by timestamp too — so the secondary
        // key is just for ties at the same wall-clock moment.
        envelopes.sort_by(|a, b| {
            a.timestamp
                .cmp(&b.timestamp)
                .then_with(|| a.id.cmp(&b.id))
        });

        let mut report = ApplyReport::default();
        for env in envelopes {
            if env.device_id == self.local_device_id {
                report.skipped_own += 1;
                continue;
            }
            match self.is_already_applied(&env.id) {
                Ok(true) => {
                    report.skipped_already_applied += 1;
                    continue;
                }
                Ok(false) => {}
                Err(err) => {
                    warn!(
                        event_id = %env.id,
                        ?err,
                        "could not check sync_applied_events; skipping envelope",
                    );
                    report.failed += 1;
                    continue;
                }
            }

            match self.apply_one(&env) {
                Ok(true) => {
                    // Mark as applied. If marking itself fails we
                    // log + count as `failed` so callers know to
                    // raise a "sync state inconsistent" alert —
                    // re-running the applier would re-apply the
                    // event, which our handlers tolerate (idempotent
                    // upserts) but it's still worth visibility.
                    if let Err(err) = self.mark_applied(&env.id) {
                        warn!(
                            event_id = %env.id,
                            ?err,
                            "applied envelope but couldn't write idempotency row",
                        );
                        report.failed += 1;
                    } else {
                        report.applied += 1;
                    }
                }
                Ok(false) => {
                    report.skipped_unsupported += 1;
                }
                Err(err) => {
                    warn!(
                        event_id = %env.id,
                        device_id = %env.device_id,
                        ?err,
                        "failed to apply envelope; skipping",
                    );
                    report.failed += 1;
                }
            }
        }
        Ok(report)
    }

    /// Dispatch one envelope to its variant handler. Returns
    /// `Ok(true)` when something was actually applied,
    /// `Ok(false)` when the variant is one we don't handle yet
    /// (plugin.*, shortcut.*), `Err` on a real failure.
    fn apply_one(&self, env: &EventEnvelope) -> SyncResult<bool> {
        match &env.event {
            SyncEvent::EventCreated(payload)
            | SyncEvent::EventUpdated(payload) => {
                self.apply_event_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::EventDeleted(payload) => {
                self.apply_event_delete(payload)?;
                Ok(true)
            }
            SyncEvent::TaskCreated(payload)
            | SyncEvent::TaskUpdated(payload) => {
                self.apply_task_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::TaskDeleted(payload) => {
                self.apply_task_delete(payload)?;
                Ok(true)
            }
            SyncEvent::TaskListCreated(payload)
            | SyncEvent::TaskListUpdated(payload) => {
                self.apply_task_list_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::TaskListDeleted(payload) => {
                self.apply_task_list_delete(payload)?;
                Ok(true)
            }
            SyncEvent::CalendarCreated(payload)
            | SyncEvent::CalendarUpdated(payload) => {
                self.apply_calendar_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::CalendarDeleted(payload) => {
                self.apply_calendar_delete(payload)?;
                Ok(true)
            }
            SyncEvent::ColorLabelCreated(payload)
            | SyncEvent::ColorLabelUpdated(payload) => {
                self.apply_color_label_upsert(payload)?;
                Ok(true)
            }
            SyncEvent::ColorLabelDeleted(payload) => {
                self.apply_color_label_delete(payload)?;
                Ok(true)
            }
            SyncEvent::SettingsUpdated(payload) => {
                self.apply_settings_updated(payload)?;
                Ok(true)
            }
            SyncEvent::PluginInstalled(_)
            | SyncEvent::PluginUpdated(_)
            | SyncEvent::PluginUninstalled(_)
            | SyncEvent::ShortcutSet(_)
            | SyncEvent::ShortcutReset(_)
            | SyncEvent::ShortcutCleared(_) => {
                // Forward-compat: variants without local handlers
                // log + skip. Once the plugin manager / shortcut
                // overrides land they'll grow handlers here.
                debug!(
                    event_id = %env.id,
                    "skipping envelope: variant not handled yet",
                );
                Ok(false)
            }
        }
    }

    fn apply_event_upsert(&self, payload: &EventPayload) -> SyncResult<()> {
        let event: cal_core::Event =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!(
                    "event upsert payload not a valid Event: {err}",
                ))
            })?;
        // Pin the wire id even if the deserialised payload's
        // `id` differs (defensive: shouldn't happen, but the
        // envelope's `id` is the canonical one for sync).
        let mut event = event;
        event.id = payload.id.clone();
        self.adapter
            .upsert_event_from_sync(&event)
            .map_err(core_to_sync)
    }

    fn apply_event_delete(&self, payload: &IdPayload) -> SyncResult<()> {
        self.adapter
            .delete_event_from_sync(&payload.id)
            .map_err(core_to_sync)
    }

    fn apply_task_upsert(&self, payload: &EventPayload) -> SyncResult<()> {
        let task: cal_core::Task =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!(
                    "task upsert payload not a valid Task: {err}",
                ))
            })?;
        let mut task = task;
        task.id = payload.id.clone();
        self.adapter
            .upsert_task_from_sync(&task)
            .map_err(core_to_sync)
    }

    fn apply_task_delete(&self, payload: &IdPayload) -> SyncResult<()> {
        self.adapter
            .delete_task_from_sync(&payload.id)
            .map_err(core_to_sync)
    }

    fn apply_task_list_upsert(
        &self,
        payload: &EventPayload,
    ) -> SyncResult<()> {
        let list: cal_core::TaskList =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!(
                    "task_list upsert payload not a valid TaskList: {err}",
                ))
            })?;
        let mut list = list;
        list.id = payload.id.clone();
        self.adapter
            .upsert_task_list_from_sync(&list)
            .map_err(core_to_sync)
    }

    fn apply_task_list_delete(&self, payload: &IdPayload) -> SyncResult<()> {
        self.adapter
            .delete_task_list_from_sync(&payload.id)
            .map_err(core_to_sync)
    }

    fn apply_calendar_upsert(
        &self,
        payload: &EventPayload,
    ) -> SyncResult<()> {
        let cal: cal_core::Calendar =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!(
                    "calendar upsert payload not a valid Calendar: {err}",
                ))
            })?;
        let mut cal = cal;
        cal.id = payload.id.clone();
        self.adapter
            .upsert_calendar_from_sync(&cal)
            .map_err(core_to_sync)
    }

    fn apply_calendar_delete(&self, payload: &IdPayload) -> SyncResult<()> {
        self.adapter
            .delete_calendar_from_sync(&payload.id)
            .map_err(core_to_sync)
    }

    fn apply_color_label_upsert(
        &self,
        payload: &EventPayload,
    ) -> SyncResult<()> {
        let label: cal_core::ColorLabel =
            serde_json::from_value(payload.fields.clone()).map_err(|err| {
                SyncError::protocol(format!(
                    "color_label upsert payload not a valid ColorLabel: {err}",
                ))
            })?;
        self.adapter
            .upsert_color_label_from_sync(&label)
            .map_err(core_to_sync)
    }

    fn apply_color_label_delete(
        &self,
        payload: &IdPayload,
    ) -> SyncResult<()> {
        self.adapter
            .delete_color_label_from_sync(&payload.id)
            .map_err(core_to_sync)
    }

    /// Settings live in `user_prefs`. Phase Sb's whitelist
    /// already gates which keys propagate; the applier writes
    /// whatever it receives (the whitelist on the writer side is
    /// the producer's responsibility, not the consumer's). Value
    /// = JSON null encodes "delete the row" — see the
    /// `delete_user_pref` hook for the symmetric write side.
    fn apply_settings_updated(
        &self,
        payload: &SettingsPayload,
    ) -> SyncResult<()> {
        let repo = UserPrefsRepo::new(&self.db);
        if payload.value.is_null() {
            repo.delete(&payload.key).map_err(|err| {
                SyncError::internal(format!(
                    "user_prefs delete failed for {}: {err}",
                    payload.key,
                ))
            })?;
        } else {
            // Encode the JSON value back to a string. The
            // user_prefs table holds opaque strings; the
            // frontend re-parses on read. A bare string value
            // round-trips as a JSON-quoted string ("foo") to
            // match the writer's emission semantics — the writer
            // tries `from_str` first so this re-quoting stays
            // symmetric.
            let stored = match &payload.value {
                serde_json::Value::String(s) => s.clone(),
                other => serde_json::to_string(other)?,
            };
            repo.set(&payload.key, &stored).map_err(|err| {
                SyncError::internal(format!(
                    "user_prefs set failed for {}: {err}",
                    payload.key,
                ))
            })?;
        }
        Ok(())
    }

    fn is_already_applied(&self, event_id: &str) -> SyncResult<bool> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT 1 FROM sync_applied_events WHERE event_id = ?")
            .map_err(|err| SyncError::internal(err.to_string()))?;
        let exists = stmt
            .query_row(params![event_id], |_| Ok(()))
            .is_ok();
        Ok(exists)
    }

    fn mark_applied(&self, event_id: &str) -> SyncResult<()> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO sync_applied_events
                (event_id, applied_at) VALUES (?, ?)",
            params![event_id, now],
        )
        .map_err(|err| SyncError::internal(err.to_string()))?;
        Ok(())
    }
}

/// Convert a cal-core error from an adapter call into a SyncError
/// flavour appropriate for the apply path. Most cal-core variants
/// map to `Internal` because they signal "the row payload doesn't
/// match local invariants" — exactly the case where the user
/// needs to know "the sync hit an unexpected state" without us
/// claiming a particular root cause.
fn core_to_sync(err: cal_core::Error) -> SyncError {
    SyncError::Internal(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cal_core::{Calendar, ColorLabel, Event, Reminder};
    use chrono::TimeZone;
    use sync_core::{EventEnvelope, EventPayload, IdPayload, SyncEvent};

    /// Set up an in-memory DB + adapter for the test. The
    /// LocalAdapter's `open_test_db` already runs every migration
    /// including 0012 (sync_applied_events), so the applier can
    /// write its idempotency rows.
    fn fixture() -> (Arc<LocalAdapter>, SharedConn) {
        let shared = cal_adapter_local::test_support::open_test_db();
        let adapter = Arc::new(LocalAdapter::new(shared.clone()));
        (adapter, shared)
    }

    fn fixture_event(id: &str, calendar_id: &str) -> Event {
        Event {
            id: id.into(),
            calendar_id: calendar_id.into(),
            title: "Synced from elsewhere".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 12, 10, 0, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            reminders: Vec::<Reminder>::new(),
            sound: None,
            attendees: Vec::new(),
            created_at: Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap(),
            etag: None,
        }
    }

    fn fixture_envelope(
        device_id: DeviceId,
        event: SyncEvent,
        timestamp_secs: i64,
    ) -> EventEnvelope {
        EventEnvelope {
            id: format!("evt_{:013x}", timestamp_secs),
            device_id,
            timestamp: Utc.timestamp_opt(timestamp_secs, 0).unwrap(),
            event,
        }
    }

    fn fixture_calendar(id: &str) -> Calendar {
        Calendar {
            id: id.into(),
            name: "From remote".into(),
            color: None,
            read_only: false,
            default_sound: None,
        }
    }

    #[test]
    fn apply_event_created_inserts_row() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        // Need a calendar locally for the FK to succeed —
        // apply CalendarCreated first.
        let cal = fixture_calendar("cal-x");
        let env_cal = fixture_envelope(
            other.clone(),
            SyncEvent::CalendarCreated(EventPayload {
                id: cal.id.clone(),
                fields: serde_json::to_value(&cal).unwrap(),
            }),
            1000,
        );
        let env_ev = fixture_envelope(
            other,
            SyncEvent::EventCreated(EventPayload {
                id: "ev-1".into(),
                fields: serde_json::to_value(&fixture_event("ev-1", "cal-x"))
                    .unwrap(),
            }),
            2000,
        );

        let report = applier
            .apply_envelopes(vec![env_ev, env_cal]) // out of order on purpose
            .unwrap();
        assert_eq!(report.applied, 2);
        assert_eq!(report.skipped_already_applied, 0);
        assert_eq!(report.failed, 0);

        // Row should be queryable from SQLite.
        let conn = db.lock().unwrap();
        let title: String = conn
            .query_row(
                "SELECT title FROM events WHERE id = ?",
                params!["ev-1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "Synced from elsewhere");
    }

    #[test]
    fn applying_same_envelope_twice_is_idempotent() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        let cal = fixture_calendar("cal-x");
        let envelopes = vec![
            fixture_envelope(
                other.clone(),
                SyncEvent::CalendarCreated(EventPayload {
                    id: cal.id.clone(),
                    fields: serde_json::to_value(&cal).unwrap(),
                }),
                1000,
            ),
            fixture_envelope(
                other,
                SyncEvent::EventCreated(EventPayload {
                    id: "ev-1".into(),
                    fields: serde_json::to_value(&fixture_event(
                        "ev-1", "cal-x",
                    ))
                    .unwrap(),
                }),
                2000,
            ),
        ];

        let first = applier.apply_envelopes(envelopes.clone()).unwrap();
        assert_eq!(first.applied, 2);

        let second = applier.apply_envelopes(envelopes).unwrap();
        // Second pass: both rows hit sync_applied_events.
        assert_eq!(second.applied, 0);
        assert_eq!(second.skipped_already_applied, 2);
    }

    #[test]
    fn applying_own_device_envelopes_skips_them() {
        let (adapter, db) = fixture();
        let me = DeviceId::from_string("dev-me".into());
        let applier =
            EventLogApplier::new(db.clone(), adapter.clone(), me.clone());

        let cal = fixture_calendar("cal-x");
        // Both envelopes from this device — the applier should
        // count them as `skipped_own` and not touch the DB.
        let envelopes = vec![
            fixture_envelope(
                me.clone(),
                SyncEvent::CalendarCreated(EventPayload {
                    id: cal.id.clone(),
                    fields: serde_json::to_value(&cal).unwrap(),
                }),
                1000,
            ),
            fixture_envelope(
                me,
                SyncEvent::EventCreated(EventPayload {
                    id: "ev-1".into(),
                    fields: serde_json::to_value(&fixture_event(
                        "ev-1", "cal-x",
                    ))
                    .unwrap(),
                }),
                2000,
            ),
        ];
        let report = applier.apply_envelopes(envelopes).unwrap();
        assert_eq!(report.applied, 0);
        assert_eq!(report.skipped_own, 2);
        // Calendar table is empty — own-device envelopes never
        // touched it.
        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM calendars WHERE id = 'cal-x'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn apply_event_deleted_removes_row() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        let cal = fixture_calendar("cal-x");
        // Apply create + delete.
        applier
            .apply_envelopes(vec![
                fixture_envelope(
                    other.clone(),
                    SyncEvent::CalendarCreated(EventPayload {
                        id: cal.id.clone(),
                        fields: serde_json::to_value(&cal).unwrap(),
                    }),
                    1000,
                ),
                fixture_envelope(
                    other.clone(),
                    SyncEvent::EventCreated(EventPayload {
                        id: "ev-1".into(),
                        fields: serde_json::to_value(&fixture_event(
                            "ev-1", "cal-x",
                        ))
                        .unwrap(),
                    }),
                    2000,
                ),
                fixture_envelope(
                    other,
                    SyncEvent::EventDeleted(IdPayload {
                        id: "ev-1".into(),
                    }),
                    3000,
                ),
            ])
            .unwrap();

        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE id = 'ev-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn apply_color_label_created_inserts_row() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db.clone(), adapter.clone(), me);

        let label = ColorLabel {
            id: cal_core::ColorLabelId::new("lbl-a"),
            name: "Work".into(),
            hex: "#ff0000".into(),
        };
        let env = fixture_envelope(
            other,
            SyncEvent::ColorLabelCreated(EventPayload {
                id: "lbl-a".into(),
                fields: serde_json::to_value(&label).unwrap(),
            }),
            1000,
        );
        applier.apply_envelopes(vec![env]).unwrap();

        let conn = db.lock().unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM color_labels WHERE id = ?",
                params!["lbl-a"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Work");
    }

    #[test]
    fn settings_updated_writes_user_prefs() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier =
            EventLogApplier::new(db.clone(), adapter.clone(), me);

        let env = fixture_envelope(
            other.clone(),
            SyncEvent::SettingsUpdated(SettingsPayload {
                key: "appearance.darkMode".into(),
                value: serde_json::json!(true),
            }),
            1000,
        );
        let env_delete = fixture_envelope(
            other,
            SyncEvent::SettingsUpdated(SettingsPayload {
                key: "appearance.colorScheme".into(),
                value: serde_json::Value::Null,
            }),
            2000,
        );
        // Pre-seed a value the delete event will remove.
        {
            let shared = db.clone();
            let repo = UserPrefsRepo::new(&shared);
            repo.set("appearance.colorScheme", "blue").unwrap();
        }

        applier.apply_envelopes(vec![env, env_delete]).unwrap();

        let shared = db.clone();
        let repo = UserPrefsRepo::new(&shared);
        // The set event arrived as a bool — encoded as "true".
        let dark = repo.get("appearance.darkMode").unwrap();
        assert_eq!(dark.as_deref(), Some("true"));
        // The delete event wiped the seeded row.
        assert!(repo.get("appearance.colorScheme").unwrap().is_none());
    }

    #[test]
    fn unsupported_variants_count_as_skipped_not_failed() {
        let (adapter, db) = fixture();
        let other = DeviceId::from_string("dev-other".into());
        let me = DeviceId::from_string("dev-me".into());
        let applier = EventLogApplier::new(db, adapter, me);

        let env = fixture_envelope(
            other,
            SyncEvent::ShortcutSet(sync_core::ShortcutPayload {
                action: "event.save".into(),
                binding: "Mod+S".into(),
            }),
            1000,
        );
        let report = applier.apply_envelopes(vec![env]).unwrap();
        // We don't have a shortcut store yet — counted as
        // skipped_unsupported, not failed.
        assert_eq!(report.applied, 0);
        assert_eq!(report.skipped_unsupported, 1);
        assert_eq!(report.failed, 0);
    }
}
