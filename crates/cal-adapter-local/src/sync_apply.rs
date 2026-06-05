//! Upsert helpers for the cross-device event-log applier
//! (DESIGN.md §19, Phase Sc).
//!
//! Each method takes a fully-typed cal-core row (an `Event`,
//! `Task`, …) and writes it to SQLite preserving the **wire id**
//! — i.e. the id the other device emitted, not a freshly-minted
//! one. The applier doesn't go through the regular `create_event`
//! / `update_event` paths because those mint UUIDs and would
//! lose the cross-device identity.
//!
//! ## INSERT vs UPDATE: `ON CONFLICT … DO UPDATE`
//!
//! We use SQLite UPSERT syntax (`INSERT … ON CONFLICT(id) DO
//! UPDATE SET …`) rather than `INSERT OR REPLACE`. The
//! difference matters for tables with cascading foreign keys —
//! a calendar has `events.calendar_id REFERENCES calendars(id)
//! ON DELETE CASCADE`, so `INSERT OR REPLACE` on a calendar
//! would wipe all its events when re-applied. `ON CONFLICT DO
//! UPDATE` patches the existing row in place without firing
//! the delete-cascade.
//!
//! ## Why these live in cal-adapter-local
//!
//! The encoding logic (sound config JSON, recurrence split,
//! datetime formatting) already exists here as `pub(crate)`
//! helpers. Duplicating that in a sync-side module would mean
//! two copies of "how Aperio represents an Event on disk"
//! drifting apart. Keeping the apply path next to the
//! create / update paths makes a schema change a single-file
//! diff.

use cal_core::{Calendar, ColorLabel, Event, Section, Task, TaskList};
use chrono::Utc;
use rusqlite::params;

use crate::calendars::split_recurrence;
use crate::mapping::{encode_json, fmt_date, fmt_time, fmt_utc, write_sound};
use crate::{map_sql_err, LocalAdapter};

impl LocalAdapter {
    /// Insert or update an event row exactly as another device
    /// emitted it. Preserves the wire id; non-destructive for
    /// cascading FKs (events don't have downstream dependents,
    /// but the same UPSERT shape applies across every helper
    /// in this module for consistency).
    pub fn upsert_event_from_sync(&self, event: &Event) -> cal_core::Result<()> {
        let reminders_json = encode_json(&event.reminders)?;
        let attendees_json = encode_json(&event.attendees)?;
        let sound_json = write_sound(&event.sound)?;
        let (rrule, exceptions) = split_recurrence(&event.recurrence)?;

        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO events (
                id, calendar_id, title, description, location,
                start_utc, end_utc, all_day, rrule, rrule_exceptions,
                color_label_id, reminders, sound, attendees,
                created_at, updated_at, etag
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 calendar_id      = excluded.calendar_id,
                 title            = excluded.title,
                 description      = excluded.description,
                 location         = excluded.location,
                 start_utc        = excluded.start_utc,
                 end_utc          = excluded.end_utc,
                 all_day          = excluded.all_day,
                 rrule            = excluded.rrule,
                 rrule_exceptions = excluded.rrule_exceptions,
                 color_label_id   = excluded.color_label_id,
                 reminders        = excluded.reminders,
                 sound            = excluded.sound,
                 attendees        = excluded.attendees,
                 updated_at       = excluded.updated_at,
                 etag             = excluded.etag",
            params![
                event.id,
                event.calendar_id,
                event.title,
                event.description,
                event.location,
                fmt_utc(&event.start),
                fmt_utc(&event.end),
                event.all_day as i64,
                rrule,
                exceptions,
                event.color_label.as_ref().map(|c| c.as_str()),
                reminders_json,
                sound_json,
                attendees_json,
                fmt_utc(&event.created_at),
                fmt_utc(&event.updated_at),
                event.etag,
            ],
        )
        .map_err(map_sql_err)?;
        Ok(())
    }

    /// Delete an event row by id, idempotent. Returns `Ok(())`
    /// whether the row existed or not — the applier treats
    /// "row missing locally" as a no-op success because re-
    /// applying a delete on a row that was never present is
    /// the desired outcome.
    pub fn delete_event_from_sync(&self, event_id: &str) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM events WHERE id = ?", params![event_id])
            .map_err(map_sql_err)?;
        Ok(())
    }

    /// Upsert a calendar — same `ON CONFLICT` pattern as
    /// `upsert_event_from_sync`. Crucially **not**
    /// `INSERT OR REPLACE` because that would cascade-delete
    /// every event whose `calendar_id` matches.
    pub fn upsert_calendar_from_sync(&self, cal: &Calendar) -> cal_core::Result<()> {
        let (hex, source) = cal
            .color
            .as_ref()
            .map(|c| {
                (
                    Some(c.hex.clone()),
                    Some(color_source_to_text(c.source).to_string()),
                )
            })
            .unwrap_or((None, None));
        let sound_json = write_sound(&cal.default_sound)?;
        let now_s = fmt_utc(&Utc::now());
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO calendars (
                id, source, name, color_hex, color_source, color_label_id,
                default_sound, read_only, created_at, updated_at
             ) VALUES (?, 'local', ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 name           = excluded.name,
                 color_hex      = excluded.color_hex,
                 color_source   = excluded.color_source,
                 color_label_id = excluded.color_label_id,
                 default_sound  = excluded.default_sound,
                 read_only      = excluded.read_only,
                 updated_at     = excluded.updated_at",
            params![
                cal.id,
                cal.name,
                hex,
                source,
                cal.color_label.as_ref().map(|c| c.as_str()),
                sound_json,
                cal.read_only as i64,
                now_s,
                now_s,
            ],
        )
        .map_err(map_sql_err)?;
        Ok(())
    }

    pub fn delete_calendar_from_sync(&self, calendar_id: &str) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM calendars WHERE id = ?", params![calendar_id])
            .map_err(map_sql_err)?;
        Ok(())
    }

    /// Upsert a task. Same shape as events. Cascading FKs from
    /// `tasks.parent_id` are nullable + `ON DELETE SET NULL`,
    /// so the UPSERT vs REPLACE distinction matters less here —
    /// but we keep the pattern uniform across all helpers.
    pub fn upsert_task_from_sync(&self, task: &Task) -> cal_core::Result<()> {
        let reminders_json = encode_json(&task.reminders)?;
        let sound_json = write_sound(&task.sound)?;
        let recurrence_json = match &task.recurrence {
            Some(r) => Some(encode_json(r)?),
            None => None,
        };
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO tasks (
                id, list_id, parent_id, section_id, title, description, status, priority,
                scheduled_date, scheduled_time, deadline_date, deadline_time,
                recurrence, color_label_id, reminders, sound,
                created_at, updated_at, completed_at, etag
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 list_id        = excluded.list_id,
                 parent_id      = excluded.parent_id,
                 section_id     = excluded.section_id,
                 title          = excluded.title,
                 description    = excluded.description,
                 status         = excluded.status,
                 priority       = excluded.priority,
                 scheduled_date = excluded.scheduled_date,
                 scheduled_time = excluded.scheduled_time,
                 deadline_date  = excluded.deadline_date,
                 deadline_time  = excluded.deadline_time,
                 recurrence     = excluded.recurrence,
                 color_label_id = excluded.color_label_id,
                 reminders      = excluded.reminders,
                 sound          = excluded.sound,
                 updated_at     = excluded.updated_at,
                 completed_at   = excluded.completed_at,
                 etag           = excluded.etag",
            params![
                task.id,
                task.list_id,
                task.parent_id,
                task.section_id,
                task.title,
                task.description,
                task_status_to_text(task.status),
                task_priority_to_text(task.priority),
                task.scheduled_date.as_ref().map(fmt_date),
                task.scheduled_time.as_ref().map(fmt_time),
                task.deadline_date.as_ref().map(fmt_date),
                task.deadline_time.as_ref().map(fmt_time),
                recurrence_json,
                task.color_label.as_ref().map(|c| c.as_str()),
                reminders_json,
                sound_json,
                fmt_utc(&task.created_at),
                fmt_utc(&task.updated_at),
                task.completed_at.as_ref().map(fmt_utc),
                task.etag,
            ],
        )
        .map_err(map_sql_err)?;
        Ok(())
    }

    pub fn delete_task_from_sync(&self, task_id: &str) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM tasks WHERE id = ?", params![task_id])
            .map_err(map_sql_err)?;
        Ok(())
    }

    pub fn upsert_task_list_from_sync(&self, list: &TaskList) -> cal_core::Result<()> {
        let (hex, source) = list
            .color
            .as_ref()
            .map(|c| {
                (
                    Some(c.hex.clone()),
                    Some(color_source_to_text(c.source).to_string()),
                )
            })
            .unwrap_or((None, None));
        let sound_json = write_sound(&list.default_sound)?;
        let now_s = fmt_utc(&Utc::now());
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO task_lists (
                id, source, name, color_hex, color_source, color_label_id,
                default_sound, embedded_in_calendar, read_only, parent_id,
                created_at, updated_at
             ) VALUES (?, 'local', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 name                 = excluded.name,
                 color_hex            = excluded.color_hex,
                 color_source         = excluded.color_source,
                 color_label_id       = excluded.color_label_id,
                 default_sound        = excluded.default_sound,
                 embedded_in_calendar = excluded.embedded_in_calendar,
                 read_only            = excluded.read_only,
                 parent_id            = excluded.parent_id,
                 updated_at           = excluded.updated_at",
            params![
                list.id,
                list.name,
                hex,
                source,
                list.color_label.as_ref().map(|c| c.as_str()),
                sound_json,
                list.embedded_in_calendar,
                list.read_only as i64,
                list.parent_id,
                now_s,
                now_s,
            ],
        )
        .map_err(map_sql_err)?;
        Ok(())
    }

    pub fn delete_task_list_from_sync(&self, list_id: &str) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM task_lists WHERE id = ?", params![list_id])
            .map_err(map_sql_err)?;
        Ok(())
    }

    /// Insert or update a section row from another device. Sections
    /// carry no timestamps in the cal-core model, so — like the task
    /// list helper — we stamp `created_at`/`updated_at` with the local
    /// apply time. The id is the wire id the originator emitted.
    pub fn upsert_section_from_sync(&self, section: &Section) -> cal_core::Result<()> {
        let now_s = fmt_utc(&Utc::now());
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO sections (id, list_id, name, position, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 list_id    = excluded.list_id,
                 name       = excluded.name,
                 position   = excluded.position,
                 updated_at = excluded.updated_at",
            params![
                section.id,
                section.list_id,
                section.name,
                section.order as i64,
                now_s,
                now_s,
            ],
        )
        .map_err(map_sql_err)?;
        Ok(())
    }

    pub fn delete_section_from_sync(&self, section_id: &str) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM sections WHERE id = ?", params![section_id])
            .map_err(map_sql_err)?;
        Ok(())
    }

    pub fn upsert_color_label_from_sync(&self, label: &ColorLabel) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO color_labels (id, name, hex, ad_hoc)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 name   = excluded.name,
                 hex    = excluded.hex,
                 ad_hoc = excluded.ad_hoc",
            params![
                label.id.as_str(),
                label.name,
                label.hex,
                label.ad_hoc as i64
            ],
        )
        .map_err(map_sql_err)?;
        Ok(())
    }

    pub fn delete_color_label_from_sync(&self, label_id: &str) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM color_labels WHERE id = ?", params![label_id])
            .map_err(map_sql_err)?;
        Ok(())
    }
}

fn color_source_to_text(source: cal_core::ColorSource) -> &'static str {
    use cal_core::ColorSource::*;
    match source {
        Native => "native",
        Custom => "custom",
    }
}

fn task_status_to_text(status: cal_core::TaskStatus) -> &'static str {
    use cal_core::TaskStatus::*;
    match status {
        Open => "open",
        InProgress => "in_progress",
        Completed => "completed",
        Cancelled => "cancelled",
    }
}

fn task_priority_to_text(p: cal_core::TaskPriority) -> &'static str {
    use cal_core::TaskPriority::*;
    match p {
        Low => "low",
        Medium => "medium",
        High => "high",
    }
}
