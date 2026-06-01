//! Snapshot dump + restore helpers for the cross-device sync layer
//! (DESIGN.md §19.10, Phase Sg).
//!
//! The snapshot is a JSON document carrying every row from the
//! local SQLite tables that participate in sync. Its purpose is two-
//! fold:
//!
//! 1. **Compaction** — once a snapshot exists at the remote, log
//!    files older than `snapshot_timestamp` can be GC'd. The
//!    snapshot replaces them as the "starting point" new devices
//!    pull when they onboard.
//! 2. **Faster onboarding** — instead of replaying years of logs,
//!    a fresh device downloads one snapshot + the small backlog of
//!    logs newer than it.
//!
//! ## Dump side
//!
//! Each `dump_*_for_snapshot` walks the corresponding table and
//! returns a `Vec` of the typed cal-core struct. The orchestrator
//! serialises these via `serde_json::Value` into the
//! `Snapshot.body` JSON object.
//!
//! We deliberately reuse the same `row_to_*` helpers the
//! `list_calendars` / `get_events` / `get_tasks` paths use, so the
//! "what does Aperio mean by an Event row?" definition lives in a
//! single place. A schema change is a single-file diff in
//! `calendars.rs` or `tasks.rs`.
//!
//! ## Restore side
//!
//! `apply_snapshot_body` deserialises a body JSON value, walks each
//! section, and calls the matching `upsert_*_from_sync` helper —
//! the same one the event-log applier already uses. That means a
//! snapshot apply and a log apply produce identical row contents,
//! and the `ON CONFLICT … DO UPDATE` shape protects cascading FKs
//! exactly as documented in `sync_apply.rs`.
//!
//! ## What's NOT in here
//!
//! Snapshots intentionally exclude:
//!
//! - External-adapter data (Google / iCloud / Graph / EWS / CardDAV).
//!   Those sync via their own provider APIs and aren't backed by the
//!   event log.
//! - Contacts (local CardDAV). Contacts are §10.5 territory and
//!   ride a separate sync path; including them would require
//!   double-encoding photos as base64 + ballooning the snapshot
//!   size. Phase Sj adds a `contacts` section if the design ever
//!   pulls them in.
//! - Account configs / credentials. §19.2.1 always-local.

use cal_core::{Calendar, ColorLabel, ColorLabelId, Event, Section, Task, TaskList};
use serde::{Deserialize, Serialize};

use crate::calendars::row_to_event;
use crate::mapping::{opt_text, read_bool, read_container_color, read_sound, req_text};
use crate::tasks::{row_to_section, row_to_task};
use crate::{map_sql_err, LocalAdapter};

/// Aggregated dump returned by [`LocalAdapter::dump_for_snapshot`].
/// Lays out the snapshot body shape so the orchestrator doesn't
/// have to hand-stitch a JSON object.
///
/// `Serialize` so `serde_json::to_value(&dump)` produces the
/// canonical snapshot body. `Deserialize` so the restore side can
/// parse the same shape back out for `apply_snapshot_body`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SnapshotDump {
    #[serde(default)]
    pub calendars: Vec<Calendar>,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub task_lists: Vec<TaskList>,
    /// Sections (Vikunja buckets / Todoist sections) of the local
    /// lists. `default` so snapshots written before sections existed
    /// deserialise into an empty list. Restored after `task_lists`
    /// (FK target) and before `tasks` (which reference them).
    #[serde(default)]
    pub sections: Vec<Section>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub color_labels: Vec<ColorLabel>,
}

impl LocalAdapter {
    /// Build the snapshot dump in one shot. Cheaper than calling
    /// each `dump_*` separately because the SQLite mutex is taken
    /// once per section rather than once total.
    ///
    /// Returned values preserve the wire ids — exactly what
    /// `upsert_*_from_sync` expects on the apply side.
    pub fn dump_for_snapshot(&self) -> cal_core::Result<SnapshotDump> {
        Ok(SnapshotDump {
            calendars: self.dump_calendars_for_snapshot()?,
            events: self.dump_events_for_snapshot()?,
            task_lists: self.dump_task_lists_for_snapshot()?,
            sections: self.dump_sections_for_snapshot()?,
            tasks: self.dump_tasks_for_snapshot()?,
            color_labels: self.dump_color_labels_for_snapshot()?,
        })
    }

    /// Read every calendar row.
    pub fn dump_calendars_for_snapshot(&self) -> cal_core::Result<Vec<Calendar>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, name, color_hex, color_source, read_only, default_sound,
                        color_label_id
                   FROM calendars",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    req_text(row, 0),
                    req_text(row, 1),
                    read_container_color(row, 2, 3),
                    read_bool(row, 4),
                    read_sound(row, 5),
                    opt_text(row, 6),
                ))
            })
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, color, read_only, sound, color_label) = r.map_err(map_sql_err)?;
            out.push(Calendar {
                color_label: color_label?.map(ColorLabelId),
                id: id?,
                name: name?,
                color: color?,
                read_only: read_only?,
                default_sound: sound?,
            });
        }
        Ok(out)
    }

    /// Read every event row across every calendar. Recurrence
    /// expansion is NOT applied — we want the raw rows so the
    /// applied state on another device matches this device's
    /// stored state exactly.
    pub fn dump_events_for_snapshot(&self) -> cal_core::Result<Vec<Event>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, calendar_id, title, description, location,
                        start_utc, end_utc, all_day, rrule, rrule_exceptions,
                        color_label_id, reminders, sound, attendees,
                        created_at, updated_at, etag
                   FROM events",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| Ok(row_to_event(row)))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)??);
        }
        Ok(out)
    }

    /// Read every task list row.
    pub fn dump_task_lists_for_snapshot(&self) -> cal_core::Result<Vec<TaskList>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, name, color_hex, color_source, default_sound,
                        embedded_in_calendar, read_only, parent_id, color_label_id
                   FROM task_lists",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    req_text(row, 0),
                    req_text(row, 1),
                    read_container_color(row, 2, 3),
                    read_sound(row, 4),
                    opt_text(row, 5),
                    read_bool(row, 6),
                    opt_text(row, 7),
                    opt_text(row, 8),
                ))
            })
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, color, sound, embedded, read_only, parent_id, color_label) =
                r.map_err(map_sql_err)?;
            out.push(TaskList {
                color_label: color_label?.map(ColorLabelId),
                id: id?,
                name: name?,
                color: color?,
                default_sound: sound?,
                embedded_in_calendar: embedded?,
                parent_id: parent_id?,
                read_only: read_only?,
            });
        }
        Ok(out)
    }

    /// Read every section row across every list.
    pub fn dump_sections_for_snapshot(&self) -> cal_core::Result<Vec<Section>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT id, list_id, name, position FROM sections")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| Ok(row_to_section(row)))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)??);
        }
        Ok(out)
    }

    /// Read every task row across every task list.
    pub fn dump_tasks_for_snapshot(&self) -> cal_core::Result<Vec<Task>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, list_id, parent_id, title, description, status,
                        priority, scheduled_date, scheduled_time, deadline_date,
                        deadline_time, recurrence, color_label_id, reminders, sound,
                        created_at, updated_at, completed_at, etag, section_id
                   FROM tasks",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| Ok(row_to_task(row)))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)??);
        }
        Ok(out)
    }

    /// Read every color-label row. Mirrors the `list_color_labels`
    /// SQL shape but drops the ORDER BY — order doesn't matter for
    /// a snapshot serialisation.
    pub fn dump_color_labels_for_snapshot(&self) -> cal_core::Result<Vec<ColorLabel>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT id, name, hex FROM color_labels")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((req_text(row, 0), req_text(row, 1), req_text(row, 2)))
            })
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, hex) = r.map_err(map_sql_err)?;
            out.push(ColorLabel {
                id: ColorLabelId::new(id?),
                name: name?,
                hex: hex?,
            });
        }
        Ok(out)
    }

    /// Apply a `SnapshotDump` onto local SQLite, upserting every row.
    ///
    /// Order matters: parent rows go in before children so the FK
    /// constraints are satisfied on insert. `calendars` and
    /// `task_lists` and `color_labels` go in first, then `events`
    /// and `tasks` reference them.
    ///
    /// Per-row failures are logged and skipped — a malformed row
    /// from a future Aperio version mustn't sink the rest of the
    /// import. The caller gets back a `(applied, failed)` count.
    pub fn apply_snapshot_dump(
        &self,
        dump: &SnapshotDump,
    ) -> cal_core::Result<SnapshotApplyReport> {
        let mut report = SnapshotApplyReport::default();
        for cal in &dump.calendars {
            match self.upsert_calendar_from_sync(cal) {
                Ok(()) => report.applied += 1,
                Err(_) => report.failed += 1,
            }
        }
        for list in &dump.task_lists {
            match self.upsert_task_list_from_sync(list) {
                Ok(()) => report.applied += 1,
                Err(_) => report.failed += 1,
            }
        }
        // Sections reference task_lists and are referenced by tasks, so
        // they slot between the two in the FK-safe insertion order.
        for section in &dump.sections {
            match self.upsert_section_from_sync(section) {
                Ok(()) => report.applied += 1,
                Err(_) => report.failed += 1,
            }
        }
        for label in &dump.color_labels {
            match self.upsert_color_label_from_sync(label) {
                Ok(()) => report.applied += 1,
                Err(_) => report.failed += 1,
            }
        }
        for ev in &dump.events {
            match self.upsert_event_from_sync(ev) {
                Ok(()) => report.applied += 1,
                Err(_) => report.failed += 1,
            }
        }
        for task in &dump.tasks {
            match self.upsert_task_from_sync(task) {
                Ok(()) => report.applied += 1,
                Err(_) => report.failed += 1,
            }
        }
        Ok(report)
    }
}

/// Counter pair returned by [`LocalAdapter::apply_snapshot_dump`]. The
/// orchestrator merges this into its `SyncRoundReport` so the
/// frontend can show "snapshot applied: 1240 rows" without needing
/// to know which sections contributed.
#[derive(Debug, Default, Clone, Copy)]
pub struct SnapshotApplyReport {
    pub applied: usize,
    pub failed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_test_db;
    use cal_core::{
        Calendar, CalendarFeature, ColorLabel, ColorLabelId, ContainerColor, Event, Task, TaskList,
        TaskPriority, TaskStatus,
    };
    use chrono::{TimeZone, Utc};

    // Re-bind the colour helper into the test module so the test
    // body can call it without fully-qualifying ContainerColor.
    fn container(hex: &str) -> ContainerColor {
        ContainerColor::custom(hex)
    }

    fn make_adapter() -> LocalAdapter {
        LocalAdapter::new(open_test_db())
    }

    fn fake_event(id: &str, calendar_id: &str) -> Event {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        Event {
            id: id.into(),
            calendar_id: calendar_id.into(),
            title: "snapshot me".into(),
            description: None,
            location: None,
            start: now,
            end: now + chrono::Duration::hours(1),
            all_day: false,
            recurrence: None,
            color_label: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            created_at: now,
            updated_at: now,
            etag: None,
        }
    }

    fn fake_task(id: &str, list_id: &str) -> Task {
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        Task {
            assignees: Vec::new(),
            id: id.into(),
            list_id: list_id.into(),
            title: "test task".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: vec![],
            sound: None,
            created_at: now,
            updated_at: now,
            completed_at: None,
            etag: None,
        }
    }

    fn fake_calendar(id: &str, name: &str) -> Calendar {
        Calendar {
            color_label: None,
            id: id.into(),
            name: name.into(),
            color: Some(container("#112233")),
            read_only: false,
            default_sound: None,
        }
    }

    fn fake_task_list(id: &str, name: &str) -> TaskList {
        TaskList {
            color_label: None,
            id: id.into(),
            name: name.into(),
            color: Some(container("#445566")),
            default_sound: None,
            embedded_in_calendar: None,
            parent_id: None,
            read_only: false,
        }
    }

    #[tokio::test]
    async fn empty_db_dumps_an_empty_snapshot() {
        let a = make_adapter();
        let dump = a.dump_for_snapshot().unwrap();
        assert!(dump.calendars.is_empty());
        assert!(dump.events.is_empty());
        assert!(dump.tasks.is_empty());
        assert!(dump.task_lists.is_empty());
        assert!(dump.color_labels.is_empty());
    }

    #[tokio::test]
    async fn round_trip_through_dump_and_apply_preserves_rows() {
        // Seed the source device via the upsert helpers — bypasses
        // the higher-level create paths so the test stays focused
        // on dump/apply rather than CalendarFeature ergonomics.
        let src = make_adapter();
        src.upsert_calendar_from_sync(&fake_calendar("cal-1", "Work"))
            .unwrap();
        src.upsert_event_from_sync(&fake_event("ev-1", "cal-1"))
            .unwrap();
        src.upsert_task_list_from_sync(&fake_task_list("list-1", "Personal"))
            .unwrap();
        src.upsert_task_from_sync(&fake_task("task-1", "list-1"))
            .unwrap();
        src.upsert_color_label_from_sync(&ColorLabel {
            id: ColorLabelId::new("lbl-1".to_string()),
            name: "Red".into(),
            hex: "#ff0000".into(),
        })
        .unwrap();

        let dump = src.dump_for_snapshot().unwrap();
        assert_eq!(dump.calendars.len(), 1);
        assert_eq!(dump.events.len(), 1);
        assert_eq!(dump.task_lists.len(), 1);
        assert_eq!(dump.tasks.len(), 1);
        assert_eq!(dump.color_labels.len(), 1);

        // Round-trip through JSON the way the real snapshot
        // pipeline does.
        let body_json = serde_json::to_value(&dump).unwrap();
        let parsed: SnapshotDump = serde_json::from_value(body_json).unwrap();

        // Apply to a fresh device.
        let dst = make_adapter();
        let report = dst.apply_snapshot_dump(&parsed).unwrap();
        // 1 calendar + 1 list + 1 label + 1 event + 1 task = 5
        // successful upserts.
        assert_eq!(report.applied, 5);
        assert_eq!(report.failed, 0);

        // The new device sees the same wire ids we started with.
        let dst_cals = dst.list_calendars().await.unwrap();
        assert_eq!(dst_cals[0].id, "cal-1");
        assert_eq!(dst_cals[0].name, "Work");

        // Event round-tripped with title preserved.
        let dst_event = dst.get_event_by_id("ev-1").unwrap().unwrap();
        assert_eq!(dst_event.title, "snapshot me");
    }

    #[tokio::test]
    async fn apply_is_idempotent_on_reapply() {
        let dump = SnapshotDump {
            calendars: vec![fake_calendar("cal-1", "Once")],
            events: vec![fake_event("ev-1", "cal-1")],
            task_lists: vec![],
            sections: vec![],
            tasks: vec![],
            color_labels: vec![],
        };
        let dst = make_adapter();
        let first = dst.apply_snapshot_dump(&dump).unwrap();
        let second = dst.apply_snapshot_dump(&dump).unwrap();
        // Both runs report the same applied count — `ON CONFLICT
        // DO UPDATE` patches the existing rows in place.
        assert_eq!(first.applied, second.applied);
        assert_eq!(first.failed, 0);
        assert_eq!(second.failed, 0);
        // Still exactly one calendar + one event after the second run.
        let dump_again = dst.dump_for_snapshot().unwrap();
        assert_eq!(dump_again.calendars.len(), 1);
        assert_eq!(dump_again.events.len(), 1);
    }
}
