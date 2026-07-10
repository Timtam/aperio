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
use tracing::warn;

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
                supports_scheduling: false,
                supports_event_color: true,
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
                        created_at, updated_at, etag, rrule_tzid
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
            .prepare("SELECT id, list_id, name, position, color_label_id FROM sections")
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
                        created_at, updated_at, completed_at, etag, section_id,
                        resurface_date, series_id, effort, deadline_reminder_days
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
            .prepare("SELECT id, name, hex, ad_hoc FROM color_labels")
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    req_text(row, 0),
                    req_text(row, 1),
                    req_text(row, 2),
                    row.get::<_, i64>(3),
                ))
            })
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, hex, ad_hoc) = r.map_err(map_sql_err)?;
            out.push(ColorLabel {
                id: ColorLabelId::new(id?),
                name: name?,
                hex: hex?,
                ad_hoc: ad_hoc.map_err(map_sql_err)? != 0,
            });
        }
        Ok(out)
    }

    /// Apply a `SnapshotDump` onto local SQLite, upserting every row.
    ///
    /// Order matters: parent rows go in before children so the FK
    /// constraints are satisfied on insert. `calendars` and
    /// `color_labels` go in FIRST: calendars, task_lists, sections, events and
    /// tasks all carry a `color_label_id` that `REFERENCES color_labels(id)`, and
    /// with `foreign_keys=ON` inserting a referencing row before its label fails
    /// the FK and silently drops the row. That was the cause of "several local
    /// lists, but only one shows on the second device" after a snapshot pull —
    /// every colour-labelled list whose label wasn't already present on the
    /// receiving device vanished. Then calendars/task_lists/sections, then
    /// events/tasks reference them.
    ///
    /// Per-row failures are logged and skipped — a malformed row
    /// from a future Aperio version mustn't sink the rest of the
    /// import. The caller gets back a `(applied, failed)` count.
    pub fn apply_snapshot_dump(
        &self,
        dump: &SnapshotDump,
    ) -> cal_core::Result<SnapshotApplyReport> {
        let mut report = SnapshotApplyReport::default();
        // Colour labels first — they're the FK target for calendars / task_lists
        // / sections / events / tasks (`foreign_keys=ON`), so any referencing row
        // inserted before its label would FK-fail and be dropped.
        for label in &dump.color_labels {
            match self.upsert_color_label_from_sync(label) {
                Ok(()) => report.applied += 1,
                Err(_) => report.failed += 1,
            }
        }
        for cal in &dump.calendars {
            match self.upsert_calendar_from_sync(cal) {
                Ok(()) => report.applied += 1,
                Err(_) => report.failed += 1,
            }
        }
        // Parent-before-child order: `task_lists.parent_id` is a
        // self-referential FK (migration 0018, ON DELETE SET NULL) and
        // foreign_keys is ON, so inserting a child before its parent fails
        // the FK check. The dump comes out in rowid order, which is NOT
        // hierarchy order once a list has been reparented under a
        // later-created one — the child then has the lower rowid and lands
        // first. That insert used to fail silently (Err(_) dropped), so a
        // user who nested several lists under one parent saw only the
        // parent survive on the second device. Re-order here so every
        // parent is inserted first; this also repairs snapshots already
        // sitting on the remote (the fix is on the apply side).
        for list in order_parent_first(
            &dump.task_lists,
            |l| l.id.as_str(),
            |l| l.parent_id.as_deref(),
        ) {
            match self.upsert_task_list_from_sync(list) {
                Ok(()) => report.applied += 1,
                Err(err) => {
                    warn!(
                        list_id = %list.id,
                        ?err,
                        "snapshot apply: task_list upsert failed",
                    );
                    report.failed += 1;
                }
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
        for ev in &dump.events {
            match self.upsert_event_from_sync(ev) {
                Ok(()) => report.applied += 1,
                Err(_) => report.failed += 1,
            }
        }
        // Subtasks have the same self-referential FK hazard as nested
        // task lists (tasks.parent_id REFERENCES tasks(id)), so order
        // parents before children here too — otherwise a reparented
        // subtask dumped ahead of its parent would FK-fail and vanish.
        for task in order_parent_first(&dump.tasks, |t| t.id.as_str(), |t| t.parent_id.as_deref()) {
            match self.upsert_task_from_sync(task) {
                Ok(()) => report.applied += 1,
                Err(err) => {
                    warn!(
                        task_id = %task.id,
                        ?err,
                        "snapshot apply: task upsert failed",
                    );
                    report.failed += 1;
                }
            }
        }
        Ok(report)
    }
}

/// Order self-referential rows so a parent always precedes its children,
/// making the snapshot insert FK-safe no matter what order rows came out of
/// the source DB.
///
/// Both `task_lists.parent_id` and `tasks.parent_id` are self-referential
/// FKs; with `foreign_keys=ON` inserting a child before its parent fails.
/// The dump comes out in rowid order, which is NOT hierarchy order once a
/// row has been reparented under a later-created one (the child then has the
/// lower rowid and lands first). Such inserts used to fail silently and the
/// row vanished — that's how "several local lists, only one on the second
/// device" happened, and the same hazard applies to subtasks.
///
/// A row whose parent isn't in `items` is treated as a root (defensive — the
/// `ON DELETE SET NULL` / `ON DELETE CASCADE` constraints mean a parent
/// can't actually dangle). A cycle (which the create/reparent paths can't
/// produce) can't stall the loop: once a pass makes no progress the
/// remaining rows are emitted in their original order.
fn order_parent_first<T>(
    items: &[T],
    id_of: impl Fn(&T) -> &str,
    parent_of: impl Fn(&T) -> Option<&str>,
) -> Vec<&T> {
    use std::collections::HashSet;
    let ids: HashSet<&str> = items.iter().map(&id_of).collect();
    let mut emitted: HashSet<&str> = HashSet::with_capacity(items.len());
    let mut out: Vec<&T> = Vec::with_capacity(items.len());
    loop {
        let before = out.len();
        for it in items {
            let id = id_of(it);
            if emitted.contains(id) {
                continue;
            }
            let ready = match parent_of(it) {
                None => true,
                Some(p) => !ids.contains(p) || emitted.contains(p),
            };
            if ready {
                out.push(it);
                emitted.insert(id);
            }
        }
        // Whole set placed, or a pass made no progress (cycle) — stop.
        if out.len() == items.len() || out.len() == before {
            break;
        }
    }
    // Cycle fallback: append anything still unplaced, original order.
    for it in items {
        if !emitted.contains(id_of(it)) {
            out.push(it);
        }
    }
    out
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
        Calendar, CalendarFeature, ColorLabel, ColorLabelId, ContainerColor, Event, Task,
        TaskEffort, TaskList, TaskPriority, TaskStatus,
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
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            send_invitations: false,
            created_at: now,
            updated_at: now,
            etag: None,
            organizer: None,
            attendee_responses: vec![],
            cancelled: false,
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
            effort: TaskEffort::Medium,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            deadline_reminder_days: None,
            recurrence: None,
            resurface_date: None,
            series_id: None,
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
            supports_scheduling: false,
            supports_event_color: true,
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

    /// Regression: nested task lists must all survive a snapshot apply
    /// regardless of the order they appear in the dump. Before the fix the
    /// dump came out in rowid order, so a list reparented under a
    /// later-created one (child rowid < parent rowid) was inserted before
    /// its parent existed, hit the self-referential FK, and was silently
    /// dropped — leaving "several local lists" looking like only one on the
    /// second device. Feed the apply a fully parent-after-child order and
    /// assert the whole 3-level chain lands with its links intact.
    #[test]
    fn apply_orders_nested_task_lists_so_none_are_dropped() {
        let grandparent = fake_task_list("L-gp", "Grandparent");
        let mut parent = fake_task_list("L-p", "Parent");
        parent.parent_id = Some("L-gp".into());
        let mut child = fake_task_list("L-c", "Child");
        child.parent_id = Some("L-p".into());

        // Fully reversed: every list precedes its parent — the FK-hostile
        // order the unsorted dump could produce.
        let dump = SnapshotDump {
            task_lists: vec![child, parent, grandparent],
            ..SnapshotDump::default()
        };

        let dst = make_adapter();
        let report = dst.apply_snapshot_dump(&dump).unwrap();
        assert_eq!(report.failed, 0, "no FK failures expected after reordering");
        assert_eq!(report.applied, 3, "all three nested lists should land");

        let got = dst.dump_for_snapshot().unwrap().task_lists;
        assert_eq!(got.len(), 3, "every nested list survives the round-trip");
        let by_id = |id: &str| got.iter().find(|l| l.id == id).cloned().unwrap();
        assert_eq!(by_id("L-p").parent_id.as_deref(), Some("L-gp"));
        assert_eq!(by_id("L-c").parent_id.as_deref(), Some("L-p"));
    }

    /// Regression: a colour-labelled list must survive a snapshot apply even
    /// though its `color_label` is itself in the same dump.
    /// `task_lists.color_label_id REFERENCES color_labels(id)` with
    /// `foreign_keys=ON`, so applying the list BEFORE its label fails the FK and
    /// used to silently drop the list. That was the cause of "several local
    /// lists, but only the colour-unlabelled / already-known one shows on the
    /// second device" after a snapshot pull — the remote snapshot was correct;
    /// the bug was purely the apply order. Colour labels must go in first.
    #[test]
    fn apply_inserts_color_labels_before_referencing_lists() {
        let label = ColorLabel {
            id: ColorLabelId::new("lbl-work".to_string()),
            name: "Work".into(),
            hex: "#3366cc".into(),
            ad_hoc: false,
        };
        let mut list = fake_task_list("L-labelled", "Freelance");
        list.color_label = Some(ColorLabelId::new("lbl-work".to_string()));

        let dump = SnapshotDump {
            task_lists: vec![list],
            color_labels: vec![label],
            ..SnapshotDump::default()
        };

        let dst = make_adapter();
        let report = dst.apply_snapshot_dump(&dump).unwrap();
        assert_eq!(
            report.failed, 0,
            "the label must be inserted before the list that references it",
        );

        let got = dst.dump_for_snapshot().unwrap().task_lists;
        assert_eq!(
            got.len(),
            1,
            "the colour-labelled list must survive the apply"
        );
        assert_eq!(
            got[0].color_label.as_ref().map(|c| c.0.as_str()),
            Some("lbl-work"),
            "the colour-label reference round-trips",
        );
    }

    /// Regression for the LOG-apply path: a colour-labelled row that arrives
    /// BEFORE its `color_label.created` event (cross-device / cross-log
    /// wall-clock ordering — the snapshot reorder can't help here) must NOT be
    /// dropped. `upsert_*_from_sync` now inserts a self-healing placeholder for an
    /// unknown label id so the FK holds; the real label fills it in on arrival.
    #[test]
    fn log_apply_keeps_a_list_whose_color_label_arrives_later() {
        let dst = make_adapter();
        // Apply the list FIRST, referencing a label that doesn't exist yet.
        let mut list = fake_task_list("L1", "Work");
        list.color_label = Some(ColorLabelId::new("lbl-late".to_string()));
        dst.upsert_task_list_from_sync(&list)
            .expect("the list must survive a not-yet-present label");

        let got = dst.dump_for_snapshot().unwrap().task_lists;
        assert_eq!(got.len(), 1, "the list survives despite the missing label");
        assert_eq!(
            got[0].color_label.as_ref().map(|c| c.0.as_str()),
            Some("lbl-late"),
        );

        // The real label arriving later heals the placeholder in place.
        dst.upsert_color_label_from_sync(&ColorLabel {
            id: ColorLabelId::new("lbl-late".to_string()),
            name: "Work".into(),
            hex: "#ff8800".into(),
            ad_hoc: false,
        })
        .unwrap();
        let labels = dst.dump_for_snapshot().unwrap().color_labels;
        let lbl = labels
            .iter()
            .find(|l| l.id.0 == "lbl-late")
            .expect("label present");
        assert_eq!(lbl.name, "Work", "placeholder healed into the real label");
        assert_eq!(lbl.hex, "#ff8800");
    }

    /// Same FK hazard, sibling table: `tasks.parent_id` is self-referential
    /// too, so a subtask dumped before its parent task must still survive.
    #[test]
    fn apply_orders_subtasks_so_none_are_dropped() {
        let list = fake_task_list("L1", "List");
        let parent = fake_task("T-parent", "L1");
        let mut child = fake_task("T-child", "L1");
        child.parent_id = Some("T-parent".into());

        let dump = SnapshotDump {
            task_lists: vec![list],
            // Child BEFORE parent — the FK-hostile order.
            tasks: vec![child, parent],
            ..SnapshotDump::default()
        };

        let dst = make_adapter();
        let report = dst.apply_snapshot_dump(&dump).unwrap();
        assert_eq!(report.failed, 0, "no FK failures expected after reordering");
        assert_eq!(report.applied, 3, "1 list + parent task + subtask");

        let tasks = dst.dump_for_snapshot().unwrap().tasks;
        assert_eq!(
            tasks.len(),
            2,
            "both the parent task and its subtask survive"
        );
        let child = tasks.iter().find(|t| t.id == "T-child").unwrap();
        assert_eq!(child.parent_id.as_deref(), Some("T-parent"));
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
            ad_hoc: false,
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
