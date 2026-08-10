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
//! ## Why these live in adapter-local
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
use rusqlite::{params, Connection, OptionalExtension};

use crate::calendars::split_recurrence;
use crate::mapping::{encode_json, fmt_date, fmt_time, fmt_utc, write_sound};
use crate::{map_sql_err, LocalAdapter};

/// Remember that a group was dissolved, and when.
///
/// Kept monotonic: a later dissolve wins, an earlier one leaves the existing
/// mark alone, so replaying the same log twice cannot move the line backwards.
fn tombstone(
    conn: &rusqlite::Connection,
    group_id: &str,
    dissolved_at: &str,
) -> rusqlite::Result<()> {
    let current: Option<String> = conn
        .query_row(
            "SELECT dissolved_at FROM event_group_tombstones WHERE group_id = ?",
            params![group_id],
            |row| row.get(0),
        )
        .optional()?;
    let keep = match &current {
        Some(existing) => is_newer_claim((dissolved_at, group_id), (existing, group_id)),
        None => true,
    };
    if keep {
        conn.execute(
            "INSERT INTO event_group_tombstones (group_id, dissolved_at)
             VALUES (?, ?)
             ON CONFLICT(group_id) DO UPDATE SET dissolved_at = excluded.dissolved_at",
            params![group_id, dissolved_at],
        )?;
    }
    Ok(())
}

/// Which of two claims about a group is the later one.
///
/// The timestamp decides; the group id breaks a tie, so two devices that
/// stamped the same second still reach the same answer. A timestamp that will
/// not parse loses to one that will — a claim we cannot date cannot be shown
/// to be the newer one — and when neither parses the strings decide, which is
/// at least the same decision everywhere.
fn is_newer_claim(candidate: (&str, &str), incumbent: (&str, &str)) -> bool {
    let parsed = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc))
    };
    match (parsed(candidate.0), parsed(incumbent.0)) {
        (Some(a), Some(b)) if a != b => a > b,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (Some(_), Some(_)) => candidate.1 > incumbent.1,
        (None, None) => (candidate.0, candidate.1) > (incumbent.0, incumbent.1),
    }
}

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
        let (rrule, exceptions, rrule_tzid) = split_recurrence(&event.recurrence)?;

        let conn = self.db().lock().expect("db mutex poisoned");
        ensure_color_label_ref(&conn, event.color_label.as_ref().map(|c| c.as_str()))?;
        conn.execute(
            "INSERT INTO events (
                id, calendar_id, title, description, location,
                start_utc, end_utc, all_day, rrule, rrule_exceptions, rrule_tzid,
                color_label_id, reminders, sound, attendees,
                created_at, updated_at, etag
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                 rrule_tzid       = excluded.rrule_tzid,
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
                rrule_tzid,
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
        ensure_color_label_ref(&conn, cal.color_label.as_ref().map(|c| c.as_str()))?;
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
        ensure_color_label_ref(&conn, task.color_label.as_ref().map(|c| c.as_str()))?;
        conn.execute(
            "INSERT INTO tasks (
                id, list_id, parent_id, section_id, title, description, status, priority,
                effort, scheduled_date, scheduled_time, deadline_date, deadline_time,
                deadline_reminder_days,
                recurrence, color_label_id, reminders, sound,
                created_at, updated_at, completed_at, etag,
                resurface_date, series_id
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 list_id        = excluded.list_id,
                 parent_id      = excluded.parent_id,
                 section_id     = excluded.section_id,
                 title          = excluded.title,
                 description    = excluded.description,
                 status         = excluded.status,
                 priority       = excluded.priority,
                 effort         = excluded.effort,
                 scheduled_date = excluded.scheduled_date,
                 scheduled_time = excluded.scheduled_time,
                 deadline_date  = excluded.deadline_date,
                 deadline_time  = excluded.deadline_time,
                 deadline_reminder_days = excluded.deadline_reminder_days,
                 recurrence     = excluded.recurrence,
                 color_label_id = excluded.color_label_id,
                 reminders      = excluded.reminders,
                 sound          = excluded.sound,
                 updated_at     = excluded.updated_at,
                 completed_at   = excluded.completed_at,
                 etag           = excluded.etag,
                 resurface_date = excluded.resurface_date,
                 series_id      = excluded.series_id",
            params![
                task.id,
                task.list_id,
                task.parent_id,
                task.section_id,
                task.title,
                task.description,
                task_status_to_text(task.status),
                task_priority_to_text(task.priority),
                task_effort_to_text(task.effort),
                task.scheduled_date.as_ref().map(fmt_date),
                task.scheduled_time.as_ref().map(fmt_time),
                task.deadline_date.as_ref().map(fmt_date),
                task.deadline_time.as_ref().map(fmt_time),
                task.deadline_reminder_days,
                recurrence_json,
                task.color_label.as_ref().map(|c| c.as_str()),
                reminders_json,
                sound_json,
                fmt_utc(&task.created_at),
                fmt_utc(&task.updated_at),
                task.completed_at.as_ref().map(fmt_utc),
                task.etag,
                task.resurface_date.as_ref().map(fmt_date),
                task.series_id,
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
        ensure_color_label_ref(&conn, list.color_label.as_ref().map(|c| c.as_str()))?;
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
        ensure_color_label_ref(&conn, section.color_label.as_ref().map(|c| c.as_str()))?;
        conn.execute(
            "INSERT INTO sections (id, list_id, name, position, color_label_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 list_id        = excluded.list_id,
                 name           = excluded.name,
                 position       = excluded.position,
                 color_label_id = excluded.color_label_id,
                 updated_at     = excluded.updated_at",
            params![
                section.id,
                section.list_id,
                section.name,
                section.order as i64,
                section.color_label.as_ref().map(|c| c.as_str()),
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

    /// Apply an arriving group, deciding every contested member the same way on
    /// every device.
    ///
    /// Wholesale, not merged, because the event carries the whole set: the
    /// arriving value IS the group, and reconciling it field by field would
    /// invent a state neither device holds. Members are deleted and re-inserted
    /// inside one transaction so no reader ever sees a half-membership.
    ///
    /// ## Why this is not a plain upsert
    ///
    /// The applier never re-applies a device's OWN envelopes, so "store
    /// whatever arrives" is last-APPLIED-wins, not last-writer-wins — and two
    /// devices apply in opposite orders. A groups a+b on Wednesday while B,
    /// offline since Monday, groups a+c: A would end on B's Monday claim and B
    /// on A's Wednesday one, each holding the other's, permanently, with
    /// nothing left in the log to reconcile them.
    ///
    /// So the decision is taken from the DATA rather than from arrival order,
    /// and every device therefore reaches the same answer without another
    /// round:
    ///
    /// - An arrival that is not newer than the group we already hold under that
    ///   id is dropped. Replay in any order lands on the same membership.
    /// - A member currently held by a DIFFERENT group is taken only when the
    ///   arriving group is the newer claim; otherwise it stays where it is and
    ///   the arriving group goes without it. `updated_at` decides and the group
    ///   id breaks a tie — arbitrary, but identically arbitrary everywhere.
    /// - A group left under two members is KEPT, holding whatever it still
    ///   has, and simply stops being shown (the read paths in host-core skip a
    ///   group with fewer than two members). Deleting it was the last piece of
    ///   order-dependence: a starved group forgets the members it had won, so
    ///   the answer depended on whether it happened to die before or after the
    ///   next claim arrived. With three overlapping claims — a+b Monday, b+c
    ///   Tuesday, c+d Wednesday — one order ended with a+b intact and another
    ///   with a and b loose, and both stuck. Nothing is deleted here now, so
    ///   each member simply ends up with the newest group that ever claimed
    ///   it, which is a running maximum and therefore the same everywhere.
    pub fn upsert_event_group_from_sync(
        &self,
        group: &cal_core::EventGroup,
    ) -> cal_core::Result<()> {
        let mut conn = self.db().lock().expect("db mutex poisoned");
        let tx = conn.transaction().map_err(map_sql_err)?;

        // An older claim about a group we already hold changes nothing — and
        // neither does one older than the group's own DISSOLVE. Without the
        // second half a dissolve is silently undone: the device that dissolved
        // never re-applies its own event, so an update another device wrote
        // before it heard the news arrives to an empty table and re-creates the
        // group there, and only there.
        let local: Option<String> = tx
            .query_row(
                "SELECT updated_at FROM event_groups WHERE id = ?",
                params![group.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql_err)?;
        let dissolved: Option<String> = tx
            .query_row(
                "SELECT dissolved_at FROM event_group_tombstones WHERE group_id = ?",
                params![group.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql_err)?;
        for incumbent in [local, dissolved].into_iter().flatten() {
            if !is_newer_claim((&group.updated_at, &group.id), (&incumbent, &group.id)) {
                return Ok(());
            }
        }

        // Who may join: everyone free, plus everyone whose current group is the
        // older claim.
        let mut members: Vec<&cal_core::EventGroupMember> = Vec::new();
        for m in &group.members {
            let holder: Option<(String, String)> = tx
                .query_row(
                    "SELECT g.id, g.updated_at
                       FROM event_group_members m
                       JOIN event_groups g ON g.id = m.group_id
                      WHERE m.calendar_id = ? AND m.event_id = ?",
                    params![m.calendar_id, m.event_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(map_sql_err)?;
            match holder {
                Some((held_by, held_updated)) if held_by != group.id => {
                    if is_newer_claim((&group.updated_at, &group.id), (&held_updated, &held_by)) {
                        members.push(m);
                    }
                    // Else: the group holding it made the newer claim, so the
                    // arriving group simply does not get this member. The other
                    // device runs the same comparison and keeps it there, which
                    // is how both end up agreeing.
                }
                _ => members.push(m),
            }
        }

        tx.execute(
            "INSERT INTO event_groups (id, created_at, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at",
            params![group.id, group.created_at, group.updated_at],
        )
        .map_err(map_sql_err)?;
        tx.execute(
            "DELETE FROM event_group_members WHERE group_id = ?",
            params![group.id],
        )
        .map_err(map_sql_err)?;
        for m in members {
            tx.execute(
                "INSERT INTO event_group_members
                     (group_id, calendar_id, event_id, title, starts_at, added_at)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT(calendar_id, event_id) DO UPDATE SET
                     group_id  = excluded.group_id,
                     title     = excluded.title,
                     starts_at = excluded.starts_at,
                     added_at  = excluded.added_at",
                params![
                    group.id,
                    m.calendar_id,
                    m.event_id,
                    m.title,
                    m.starts_at,
                    // Carried, not kept: `added_at` orders the membership when
                    // it is read back, so leaving the old group's timestamp in
                    // place would list one group in a different order per
                    // device.
                    m.added_at
                ],
            )
            .map_err(map_sql_err)?;
        }
        tx.commit().map_err(map_sql_err)?;
        Ok(())
    }

    /// Remember a decline another device made.
    ///
    /// Insert-only, and that is the whole story: the declines are a set that
    /// only grows, so applying the same one twice or in any order lands in the
    /// same place.
    pub fn upsert_suggestion_decline_from_sync(
        &self,
        decline: &cal_core::SuggestionDecline,
    ) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO event_group_suggestion_declines
                 (calendar_a, event_a, calendar_b, event_b, declined_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(calendar_a, event_a, calendar_b, event_b) DO NOTHING",
            params![
                decline.calendar_a,
                decline.event_a,
                decline.calendar_b,
                decline.event_b,
                decline.declined_at
            ],
        )
        .map_err(map_sql_err)?;
        Ok(())
    }

    /// Record a dissolve mark that arrived in a snapshot.
    ///
    /// Separate from `delete_event_group_from_sync` because a snapshot's marks
    /// are applied BEFORE its groups: deleting here would delete a group the
    /// same snapshot is about to insert.
    pub fn mark_event_group_dissolved_from_sync(
        &self,
        group_id: &str,
        dissolved_at: &str,
    ) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        tombstone(&conn, group_id, dissolved_at).map_err(map_sql_err)?;
        Ok(())
    }

    /// Apply a dissolve, and leave a mark that outlives the row.
    ///
    /// `dissolved_at` is the arriving envelope's timestamp — WHEN the other
    /// device decided, not when we heard about it. Stamping "now" here would
    /// make the tombstone newer than a re-grouping the user did in between,
    /// and swallow it.
    pub fn delete_event_group_from_sync(
        &self,
        group_id: &str,
        dissolved_at: &str,
    ) -> cal_core::Result<()> {
        let mut conn = self.db().lock().expect("db mutex poisoned");
        let tx = conn.transaction().map_err(map_sql_err)?;
        tx.execute("DELETE FROM event_groups WHERE id = ?", params![group_id])
            .map_err(map_sql_err)?;
        tombstone(&tx, group_id, dissolved_at).map_err(map_sql_err)?;
        tx.commit().map_err(map_sql_err)?;
        Ok(())
    }

    pub fn delete_color_label_from_sync(&self, label_id: &str) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM color_labels WHERE id = ?", params![label_id])
            .map_err(map_sql_err)?;
        Ok(())
    }
}

/// Ensure a referenced colour label exists before inserting a row that
/// FK-references it (`color_label_id REFERENCES color_labels(id)` with
/// `foreign_keys=ON`).
///
/// On the event-log apply path events arrive in wall-clock order across devices
/// and log files, so a row's `color_label.created` can land AFTER a row that
/// references it — which would FK-fail and silently drop the row (the same
/// hazard the snapshot apply hit, fixed there by ordering color_labels first).
/// Insert a minimal placeholder for an unknown id; the real `color_label.created`
/// fills it in via its `ON CONFLICT DO UPDATE`, so the row survives AND keeps its
/// colour link. No-op when the id is `None` or the label already exists.
fn ensure_color_label_ref(conn: &Connection, color_label_id: Option<&str>) -> cal_core::Result<()> {
    if let Some(id) = color_label_id {
        conn.execute(
            "INSERT OR IGNORE INTO color_labels (id, name, hex, ad_hoc) VALUES (?, '', '', 0)",
            params![id],
        )
        .map_err(map_sql_err)?;
    }
    Ok(())
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

fn task_effort_to_text(e: cal_core::TaskEffort) -> &'static str {
    use cal_core::TaskEffort::*;
    match e {
        Small => "small",
        Medium => "medium",
        Large => "large",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_test_db;
    use cal_core::{EventGroup, EventGroupMember};

    fn adapter() -> LocalAdapter {
        LocalAdapter::new(open_test_db())
    }

    fn member(calendar: &str, event: &str) -> EventGroupMember {
        EventGroupMember {
            calendar_id: calendar.into(),
            event_id: event.into(),
            title: "Wochenplanung".into(),
            starts_at: "2026-08-10T08:00:00Z".into(),
            added_at: "2026-08-09T12:00:00Z".into(),
        }
    }

    fn group(id: &str, members: Vec<EventGroupMember>) -> EventGroup {
        stamped(id, "2026-08-09T12:00:00Z", members)
    }

    /// A group as a device would have written it at a particular moment.
    fn stamped(id: &str, updated_at: &str, members: Vec<EventGroupMember>) -> EventGroup {
        EventGroup {
            id: id.into(),
            created_at: "2026-08-09T12:00:00Z".into(),
            updated_at: updated_at.into(),
            members,
        }
    }

    /// What a device holds afterwards, as (group, its members) pairs — the
    /// shape a convergence test compares between two devices.
    fn state(a: &LocalAdapter) -> Vec<(String, Vec<String>)> {
        group_ids(a)
            .into_iter()
            .map(|id| {
                let members = members_of(a, &id);
                (id, members)
            })
            .collect()
    }

    fn members_of(a: &LocalAdapter, group_id: &str) -> Vec<String> {
        let conn = a.db().lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT event_id FROM event_group_members WHERE group_id = ? ORDER BY event_id",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![group_id], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    fn group_ids(a: &LocalAdapter) -> Vec<String> {
        let conn = a.db().lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM event_groups ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    #[test]
    fn an_arriving_group_replaces_its_membership_rather_than_adding_to_it() {
        let a = adapter();
        a.upsert_event_group_from_sync(&group(
            "g1",
            vec![member("work", "ev-a"), member("private", "ev-b")],
        ))
        .unwrap();

        // The other device took one out again and put another in. The event
        // carries the whole membership, so the removal has to land as a
        // removal — a merge would keep ev-b forever and the two devices would
        // never agree. Stamped later, as a real edit would be: an arrival that
        // is not newer than what we hold is deliberately ignored.
        a.upsert_event_group_from_sync(&stamped(
            "g1",
            "2026-08-12T09:00:00Z",
            vec![member("work", "ev-a"), member("colleague", "ev-c")],
        ))
        .unwrap();

        assert_eq!(members_of(&a, "g1"), vec!["ev-a", "ev-c"]);
    }

    #[test]
    fn a_member_claimed_by_an_arriving_group_leaves_the_one_it_was_in() {
        let a = adapter();
        // Two devices grouped overlapping sets without knowing of each other.
        a.upsert_event_group_from_sync(&group(
            "local",
            vec![
                member("work", "ev-a"),
                member("private", "ev-b"),
                member("colleague", "ev-c"),
            ],
        ))
        .unwrap();
        a.upsert_event_group_from_sync(&stamped(
            "remote",
            "2026-08-12T09:00:00Z",
            vec![member("work", "ev-a"), member("other", "ev-d")],
        ))
        .unwrap();

        // The NEWER claim wins the disputed event...
        assert_eq!(members_of(&a, "remote"), vec!["ev-a", "ev-d"]);
        // ...and the group it left keeps the members nobody claimed.
        assert_eq!(members_of(&a, "local"), vec!["ev-b", "ev-c"]);
    }

    #[test]
    fn a_group_robbed_down_to_one_member_is_dropped() {
        let a = adapter();
        a.upsert_event_group_from_sync(&group(
            "local",
            vec![member("work", "ev-a"), member("private", "ev-b")],
        ))
        .unwrap();
        a.upsert_event_group_from_sync(&stamped(
            "remote",
            "2026-08-12T09:00:00Z",
            vec![member("work", "ev-a"), member("other", "ev-d")],
        ))
        .unwrap();

        // "local" is left holding ev-b alone. The ROW stays — deleting it is
        // what used to make the outcome depend on arrival order — but nobody
        // is told about it: the host-core read paths skip a group of one.
        assert_eq!(group_ids(&a), vec!["local", "remote"]);
        assert_eq!(members_of(&a, "local"), vec!["ev-b"]);
    }

    /// Three claims chained by a shared event, in every order they can arrive.
    ///
    /// The case an adversarial review reproduced: a+b Monday, b+c Tuesday, c+d
    /// Wednesday. Deleting a group starved below two members made two of the
    /// six orders end with a+b intact and four with a and b loose — and both
    /// stuck, because the applier never re-applies a device's own envelopes
    /// and cannot emit. Devices really do reach different orders: one that was
    /// offline delivers its old envelope in a later round, so the sort inside
    /// a batch does not put them all in one line.
    #[test]
    fn three_chained_claims_land_the_same_way_in_every_order() {
        let claims = [
            (
                "g1",
                "2026-08-10T09:00:00Z",
                ("work", "ev-a"),
                ("private", "ev-b"),
            ),
            (
                "g2",
                "2026-08-11T09:00:00Z",
                ("private", "ev-b"),
                ("colleague", "ev-c"),
            ),
            (
                "g3",
                "2026-08-12T09:00:00Z",
                ("colleague", "ev-c"),
                ("other", "ev-d"),
            ),
        ];
        let orders = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];

        let mut seen: Vec<Vec<(String, Vec<String>)>> = Vec::new();
        for order in orders {
            let a = adapter();
            for i in order {
                let (id, at, first, second) = claims[i];
                a.upsert_event_group_from_sync(&stamped(
                    id,
                    at,
                    vec![member(first.0, first.1), member(second.0, second.1)],
                ))
                .unwrap();
            }
            // What a user is actually shown: groups of one are records of a
            // claim that lost, not groups.
            let visible: Vec<(String, Vec<String>)> = state(&a)
                .into_iter()
                .filter(|(_, members)| members.len() >= 2)
                .collect();
            seen.push(visible);
        }

        for (i, outcome) in seen.iter().enumerate() {
            assert_eq!(outcome, &seen[0], "order {i} disagrees with the first one",);
        }
        // And the answer is the one every device can justify: each event ended
        // up with the newest group that ever claimed it.
        assert_eq!(
            seen[0],
            vec![(
                "g3".to_string(),
                vec!["ev-c".to_string(), "ev-d".to_string()]
            )],
        );
    }

    #[test]
    fn an_older_claim_does_not_undo_a_newer_one() {
        let a = adapter();
        // Wednesday: this device added ev-d.
        a.upsert_event_group_from_sync(&stamped(
            "g1",
            "2026-08-12T09:00:00Z",
            vec![member("work", "ev-a"), member("other", "ev-d")],
        ))
        .unwrap();
        // Monday, from a device that was offline until now.
        a.upsert_event_group_from_sync(&stamped(
            "g1",
            "2026-08-10T09:00:00Z",
            vec![member("work", "ev-a"), member("colleague", "ev-c")],
        ))
        .unwrap();

        assert_eq!(
            members_of(&a, "g1"),
            vec!["ev-a", "ev-d"],
            "arrival order must not decide; the later claim stands",
        );
    }

    #[test]
    fn two_devices_that_grouped_the_same_event_end_up_agreeing() {
        // The case that used to split permanently: each device holds its own
        // claim and applies the other's, in opposite orders.
        let earlier = || {
            stamped(
                "g-early",
                "2026-08-10T09:00:00Z",
                vec![member("work", "ev-a"), member("private", "ev-b")],
            )
        };
        let later = || {
            stamped(
                "g-late",
                "2026-08-12T09:00:00Z",
                vec![member("work", "ev-a"), member("other", "ev-d")],
            )
        };

        let a = adapter();
        a.upsert_event_group_from_sync(&earlier()).unwrap();
        a.upsert_event_group_from_sync(&later()).unwrap();

        let b = adapter();
        b.upsert_event_group_from_sync(&later()).unwrap();
        b.upsert_event_group_from_sync(&earlier()).unwrap();

        assert_eq!(state(&a), state(&b), "both devices must land on one answer");
        // And that answer is the later claim: ev-a means the appointment the
        // newer group says it does. The older group keeps its row holding
        // ev-b alone — the record of a claim that lost, kept so the outcome
        // does not depend on arrival order — and the read paths never surface
        // a group of one.
        assert_eq!(
            state(&a),
            vec![
                ("g-early".to_string(), vec!["ev-b".to_string()]),
                (
                    "g-late".to_string(),
                    vec!["ev-a".to_string(), "ev-d".to_string()]
                ),
            ],
        );
    }

    #[test]
    fn a_bystander_group_survives_a_claim_that_never_named_it() {
        let a = adapter();
        // Two unrelated groups, neither touching the other.
        a.upsert_event_group_from_sync(&group(
            "bystander",
            vec![member("work", "ev-a"), member("private", "ev-b")],
        ))
        .unwrap();
        a.upsert_event_group_from_sync(&stamped(
            "arriving",
            "2026-08-12T09:00:00Z",
            vec![member("colleague", "ev-c"), member("other", "ev-d")],
        ))
        .unwrap();

        // The sweep used to be global: any group under two members went, and
        // since the applier cannot emit, no other device would ever hear that
        // this one had decided to dissolve it.
        assert_eq!(members_of(&a, "bystander"), vec!["ev-a", "ev-b"]);
    }

    #[test]
    fn an_arriving_group_that_cannot_gather_two_members_is_not_stored() {
        let a = adapter();
        a.upsert_event_group_from_sync(&stamped(
            "held",
            "2026-08-12T09:00:00Z",
            vec![member("work", "ev-a"), member("private", "ev-b")],
        ))
        .unwrap();
        // An older claim over the same two events: it may take neither, so it
        // is stored empty. The row is the record of a claim that lost; the
        // read paths never surface it.
        a.upsert_event_group_from_sync(&stamped(
            "older",
            "2026-08-10T09:00:00Z",
            vec![member("work", "ev-a"), member("private", "ev-b")],
        ))
        .unwrap();

        assert_eq!(members_of(&a, "held"), vec!["ev-a", "ev-b"]);
        assert!(members_of(&a, "older").is_empty());
    }

    #[test]
    fn a_stolen_member_carries_its_new_joining_time() {
        let a = adapter();
        a.upsert_event_group_from_sync(&group(
            "first",
            vec![member("work", "ev-a"), member("private", "ev-b")],
        ))
        .unwrap();
        let mut taken = member("work", "ev-a");
        taken.added_at = "2026-08-12T09:00:00Z".into();
        a.upsert_event_group_from_sync(&stamped(
            "second",
            "2026-08-12T09:00:00Z",
            vec![taken, member("other", "ev-d")],
        ))
        .unwrap();

        let conn = a.db().lock().unwrap();
        let added: String = conn
            .query_row(
                "SELECT added_at FROM event_group_members
                  WHERE calendar_id = 'work' AND event_id = 'ev-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // The membership is read back ORDER BY added_at, so keeping the old
        // group's timestamp would order one group differently per device.
        assert_eq!(added, "2026-08-12T09:00:00Z");
    }

    #[test]
    fn a_dissolved_group_is_not_brought_back_by_an_older_update() {
        let a = adapter();
        a.upsert_event_group_from_sync(&stamped(
            "g1",
            "2026-08-10T09:00:00Z",
            vec![member("work", "ev-a"), member("private", "ev-b")],
        ))
        .unwrap();
        a.delete_event_group_from_sync("g1", "2026-08-12T09:00:00Z")
            .unwrap();
        // An update written by a device that had not yet heard of the
        // dissolve. With the row gone there is nothing left to compare it
        // against, so without a tombstone it simply re-creates the group —
        // here, and nowhere else.
        a.upsert_event_group_from_sync(&stamped(
            "g1",
            "2026-08-11T09:00:00Z",
            vec![member("work", "ev-a"), member("private", "ev-b")],
        ))
        .unwrap();

        assert!(group_ids(&a).is_empty(), "a dissolved group came back");
    }

    #[test]
    fn a_dissolve_does_not_bury_a_later_regrouping() {
        let a = adapter();
        a.delete_event_group_from_sync("g1", "2026-08-10T09:00:00Z")
            .unwrap();
        // The user grouped those events again afterwards. The tombstone is
        // about a moment, not about the id forever.
        a.upsert_event_group_from_sync(&stamped(
            "g1",
            "2026-08-12T09:00:00Z",
            vec![member("work", "ev-a"), member("private", "ev-b")],
        ))
        .unwrap();

        assert_eq!(members_of(&a, "g1"), vec!["ev-a", "ev-b"]);
    }

    #[test]
    fn dissolving_takes_the_members_with_it_and_leaves_the_events_alone() {
        let a = adapter();
        a.upsert_event_group_from_sync(&group(
            "g1",
            vec![member("work", "ev-a"), member("private", "ev-b")],
        ))
        .unwrap();
        a.delete_event_group_from_sync("g1", "2026-08-12T09:00:00Z")
            .unwrap();

        assert!(group_ids(&a).is_empty());
        assert!(
            members_of(&a, "g1").is_empty(),
            "cascade must clear members"
        );
        // Dissolving is idempotent: a second device dissolving the same group
        // must not fail the round.
        a.delete_event_group_from_sync("g1", "2026-08-12T09:00:00Z")
            .unwrap();
    }
}
