//! `TasksFeature` implementation plus local-only management methods.

use async_trait::async_trait;
use cal_core::{
    ColorLabelId, ContainerColor, NewTask, RecurrenceEnd, RecurrenceFrequency, Reminder, Section,
    SoundConfig, Task, TaskList, TaskPriority, TaskRecurrence, TaskStatus, TasksFeature, Weekday,
};
use chrono::{Datelike, Days, Months, NaiveDate, Utc};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::mapping::{
    decode_json, encode_json, fmt_date, fmt_time, fmt_utc, opt_text, parse_date, parse_time,
    parse_utc, read_bool, read_container_color, read_sound, req_text, unknown_enum,
    write_container_color, write_sound,
};
use crate::{map_sql_err, LocalAdapter, SOURCE_ID};

impl LocalAdapter {
    pub fn create_task_list(
        &self,
        name: &str,
        color: Option<ContainerColor>,
        color_label: Option<ColorLabelId>,
        default_sound: Option<SoundConfig>,
        embedded_in_calendar: Option<String>,
    ) -> cal_core::Result<TaskList> {
        let id = Uuid::new_v4().to_string();
        let now_s = fmt_utc(&Utc::now());
        let (color_hex, color_source) = write_container_color(&color);
        let default_sound_json = write_sound(&default_sound)?;

        self.db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO task_lists (
                    id, source, name, color_hex, color_source, color_label_id,
                    default_sound, embedded_in_calendar, read_only, created_at,
                    updated_at
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
                params![
                    id,
                    SOURCE_ID,
                    name,
                    color_hex,
                    color_source,
                    color_label.as_ref().map(|c| c.as_str()),
                    default_sound_json,
                    embedded_in_calendar,
                    now_s,
                    now_s,
                ],
            )
            .map_err(map_sql_err)?;

        Ok(TaskList {
            id,
            name: name.to_string(),
            color,
            color_label,
            default_sound,
            embedded_in_calendar,
            parent_id: None,
            read_only: false,
        })
    }

    pub fn update_task_list(&self, list: TaskList) -> cal_core::Result<TaskList> {
        let (color_hex, color_source) = write_container_color(&list.color);
        let default_sound_json = write_sound(&list.default_sound)?;
        let now_s = fmt_utc(&Utc::now());

        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE task_lists
                    SET name = ?, color_hex = ?, color_source = ?,
                        color_label_id = ?, default_sound = ?,
                        embedded_in_calendar = ?, parent_id = ?, updated_at = ?
                  WHERE id = ?",
                params![
                    list.name,
                    color_hex,
                    color_source,
                    list.color_label.as_ref().map(|c| c.as_str()),
                    default_sound_json,
                    list.embedded_in_calendar,
                    list.parent_id,
                    now_s,
                    list.id,
                ],
            )
            .map_err(map_sql_err)?;

        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "task list '{}' not found",
                list.id
            )));
        }
        Ok(list)
    }

    /// Set (or clear) a task list's parent — the local-store backing for
    /// the sidebar's project-reparent gesture. `parent_id = None`
    /// promotes the list to top level. Focused `UPDATE` so it doesn't
    /// disturb the list's other fields. Cycle/self guards live in the
    /// host command; the FK only checks the parent exists.
    pub fn reparent_task_list(
        &self,
        id: &str,
        parent_id: Option<&str>,
    ) -> cal_core::Result<TaskList> {
        let now_s = fmt_utc(&Utc::now());
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE task_lists SET parent_id = ?, updated_at = ? WHERE id = ?",
                params![parent_id, now_s, id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "task list '{id}' not found"
            )));
        }
        self.get_task_list_by_id(id)?
            .ok_or_else(|| cal_core::Error::NotFound(format!("task list '{id}' not found")))
    }

    /// Read just the current status of a task by id. Used by
    /// `update_task` to detect the open→completed transition that
    /// triggers recurrence spawning.
    fn read_task_status(&self, id: &str) -> cal_core::Result<Option<TaskStatus>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let raw: Option<String> = conn
            .query_row(
                "SELECT status FROM tasks WHERE id = ?",
                params![id],
                |row| row.get(0),
            )
            .ok();
        match raw {
            None => Ok(None),
            Some(s) => Ok(Some(parse_task_status(&s)?)),
        }
    }

    /// On completion of a recurring task, create the next instance.
    ///
    /// The new row inherits everything from the template except the id,
    /// dates (advanced by `recurrence.frequency` × `interval`), and
    /// completion state (reset to open).
    ///
    /// Returns `Ok(None)` when:
    /// - the recurrence rule has hit its `end` boundary, or
    /// - the task has no date at all (nothing to advance — a recurring
    ///   backlog task does not make sense).
    fn spawn_next_recurring_task(
        &self,
        template: &Task,
        recurrence: &TaskRecurrence,
    ) -> cal_core::Result<Option<Task>> {
        // Anchor date: prefer scheduled_date, fall back to deadline_date.
        // Without either we can't compute a follow-up.
        let anchor = template.scheduled_date.or(template.deadline_date);
        let Some(anchor) = anchor else {
            return Ok(None);
        };

        let next_date = match advance(anchor, recurrence) {
            Some(d) => d,
            None => return Ok(None),
        };

        // End boundary check.
        if let Some(end) = &recurrence.end {
            match end {
                RecurrenceEnd::Never => {}
                RecurrenceEnd::OnDate { date } => {
                    if next_date > *date {
                        return Ok(None);
                    }
                }
                RecurrenceEnd::After { .. } => {
                    // Not tracked yet — needs an occurrence counter on
                    // the task row. Treat as Never for now; counted
                    // recurrence lands with the sync layer in Phase 7.
                }
            }
        }

        let new = NewTask {
            assignees: Vec::new(),
            title: template.title.clone(),
            description: template.description.clone(),
            status: TaskStatus::Open,
            priority: template.priority,
            scheduled_date: template.scheduled_date.map(|_| next_date),
            scheduled_time: template.scheduled_time,
            deadline_date: template.deadline_date.map(|_| next_date),
            deadline_time: template.deadline_time,
            recurrence: Some(recurrence.clone()),
            parent_id: None,
            // Keep the next occurrence in the same section as its template.
            section_id: template.section_id.clone(),
            color_label: template.color_label.clone(),
            reminders: template.reminders.clone(),
            sound: template.sound.clone(),
        };
        let task = self.create_task_sync(&template.list_id, new)?;
        Ok(Some(task))
    }

    /// Synchronous variant of `create_task` for the recurrence spawner.
    /// `TasksFeature::create_task` is async to match the trait shape
    /// shared with network-backed adapters, but the local adapter does
    /// no IO that benefits from async — duplicating the body here
    /// avoids dragging a runtime handle through the call.
    fn create_task_sync(&self, list_id: &str, task: NewTask) -> cal_core::Result<Task> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_s = fmt_utc(&now);
        let reminders_json = encode_json(&task.reminders)?;
        let recurrence_json = task.recurrence.as_ref().map(encode_json).transpose()?;
        let sound_json = write_sound(&task.sound)?;
        let status = task_status_str(task.status);
        let priority = task_priority_str(task.priority);

        self.db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO tasks (
                    id, list_id, parent_id, section_id, title, description, status, priority,
                    scheduled_date, scheduled_time, deadline_date, deadline_time,
                    recurrence, color_label_id, reminders, sound,
                    created_at, updated_at, completed_at, etag
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL)",
                params![
                    id,
                    list_id,
                    task.parent_id,
                    task.section_id,
                    task.title,
                    task.description,
                    status,
                    priority,
                    task.scheduled_date.as_ref().map(fmt_date),
                    task.scheduled_time.as_ref().map(fmt_time),
                    task.deadline_date.as_ref().map(fmt_date),
                    task.deadline_time.as_ref().map(fmt_time),
                    recurrence_json,
                    task.color_label.as_ref().map(|c| c.as_str()),
                    reminders_json,
                    sound_json,
                    now_s,
                    now_s,
                ],
            )
            .map_err(map_sql_err)?;

        Ok(Task {
            assignees: Vec::new(),
            id,
            list_id: list_id.to_string(),
            title: task.title,
            description: task.description,
            status: task.status,
            priority: task.priority,
            scheduled_date: task.scheduled_date,
            scheduled_time: task.scheduled_time,
            deadline_date: task.deadline_date,
            deadline_time: task.deadline_time,
            recurrence: task.recurrence,
            parent_id: task.parent_id,
            section_id: task.section_id,
            color_label: task.color_label,
            reminders: task.reminders,
            sound: task.sound,
            created_at: now,
            updated_at: now,
            completed_at: None,
            etag: None,
        })
    }

    pub fn delete_task_list(&self, id: &str) -> cal_core::Result<()> {
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute("DELETE FROM task_lists WHERE id = ?", params![id])
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "task list '{id}' not found"
            )));
        }
        Ok(())
    }

    /// Fetch a single task list by id, returning `None` if missing.
    /// Used by the conflict-detection path to compare the live row
    /// against an incoming patch.
    pub fn get_task_list_by_id(&self, id: &str) -> cal_core::Result<Option<TaskList>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, name, color_hex, color_source, default_sound,
                        embedded_in_calendar, read_only, parent_id, color_label_id
                   FROM task_lists WHERE id = ?",
            )
            .map_err(map_sql_err)?;
        let row = stmt
            .query_row(params![id], |r| {
                Ok((
                    req_text(r, 0),
                    req_text(r, 1),
                    read_container_color(r, 2, 3),
                    read_sound(r, 4),
                    opt_text(r, 5),
                    read_bool(r, 6),
                    opt_text(r, 7),
                    opt_text(r, 8),
                ))
            })
            .optional()
            .map_err(map_sql_err)?;
        let Some(parts) = row else {
            return Ok(None);
        };
        let (id, name, color, sound, embedded, read_only, parent_id, color_label) = parts;
        Ok(Some(TaskList {
            color_label: color_label?.map(ColorLabelId),
            id: id?,
            name: name?,
            color: color?,
            default_sound: sound?,
            embedded_in_calendar: embedded?,
            parent_id: parent_id?,
            read_only: read_only?,
        }))
    }

    /// Fetch a single task by id. Returns `Ok(None)` when missing.
    /// Counterpart to `get_event_by_id` for the reminders overview.
    pub fn get_task_by_id(&self, id: &str) -> cal_core::Result<Option<Task>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, list_id, parent_id, title, description, status,
                        priority, scheduled_date, scheduled_time, deadline_date,
                        deadline_time, recurrence, color_label_id, reminders, sound,
                        created_at, updated_at, completed_at, etag, section_id
                   FROM tasks WHERE id = ?",
            )
            .map_err(map_sql_err)?;
        let row = stmt
            .query_row(params![id], |r| Ok(row_to_task(r)))
            .optional()
            .map_err(map_sql_err)?;
        match row {
            None => Ok(None),
            Some(res) => res.map(Some),
        }
    }

    // ── Sections (Vikunja-bucket / Todoist-section equivalent) ──────────
    //
    // The local backend is flat at the project level (no nested_projects)
    // but DOES let the user group a list's tasks into ordered sections.
    // These inherent methods back the host's section commands; sync
    // emission + the applier ride the `section.*` SyncEvent variants.

    /// Create a section in a list. `position` drives display order.
    pub fn create_section(
        &self,
        list_id: &str,
        name: &str,
        position: u32,
    ) -> cal_core::Result<Section> {
        let id = Uuid::new_v4().to_string();
        let now_s = fmt_utc(&Utc::now());
        self.db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO sections (id, list_id, name, position, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![id, list_id, name, position as i64, now_s, now_s],
            )
            .map_err(map_sql_err)?;
        Ok(Section {
            id,
            list_id: list_id.to_string(),
            name: name.to_string(),
            order: position,
        })
    }

    /// Rename / reorder a section. Returns the same row echoed back,
    /// matching the `update_task_list` shape.
    pub fn update_section(&self, section: Section) -> cal_core::Result<Section> {
        let now_s = fmt_utc(&Utc::now());
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE sections SET name = ?, position = ?, updated_at = ? WHERE id = ?",
                params![section.name, section.order as i64, now_s, section.id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "section '{}' not found",
                section.id
            )));
        }
        Ok(section)
    }

    /// Delete a section. Its tasks survive — `tasks.section_id` is
    /// `ON DELETE SET NULL`, so they fall back to the ungrouped bucket.
    pub fn delete_section(&self, id: &str) -> cal_core::Result<()> {
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute("DELETE FROM sections WHERE id = ?", params![id])
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "section '{id}' not found"
            )));
        }
        Ok(())
    }

    /// Fetch one section by id. `None` when missing. Used by the
    /// snapshot/merge paths and the host's section commands.
    pub fn get_section_by_id(&self, id: &str) -> cal_core::Result<Option<Section>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare("SELECT id, list_id, name, position FROM sections WHERE id = ?")
            .map_err(map_sql_err)?;
        let row = stmt
            .query_row(params![id], |r| Ok(row_to_section(r)))
            .optional()
            .map_err(map_sql_err)?;
        match row {
            None => Ok(None),
            Some(res) => res.map(Some),
        }
    }
}

#[async_trait]
impl TasksFeature for LocalAdapter {
    async fn list_task_lists(&self) -> cal_core::Result<Vec<TaskList>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, name, color_hex, color_source, default_sound,
                        embedded_in_calendar, read_only, parent_id, color_label_id
                   FROM task_lists
                  ORDER BY name COLLATE NOCASE",
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

    async fn get_tasks(&self, list_id: &str) -> cal_core::Result<Vec<Task>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(TASK_SELECT).map_err(map_sql_err)?;
        let rows = stmt
            .query_map(params![list_id], |row| Ok(row_to_task(row)))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)??);
        }
        Ok(out)
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> cal_core::Result<Task> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_s = fmt_utc(&now);
        let reminders_json = encode_json(&task.reminders)?;
        let recurrence_json = task.recurrence.as_ref().map(encode_json).transpose()?;
        let sound_json = write_sound(&task.sound)?;
        let status = task_status_str(task.status);
        let priority = task_priority_str(task.priority);

        self.db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO tasks (
                    id, list_id, parent_id, section_id, title, description, status, priority,
                    scheduled_date, scheduled_time, deadline_date, deadline_time,
                    recurrence, color_label_id, reminders, sound,
                    created_at, updated_at, completed_at, etag
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL)",
                params![
                    id,
                    list_id,
                    task.parent_id,
                    task.section_id,
                    task.title,
                    task.description,
                    status,
                    priority,
                    task.scheduled_date.as_ref().map(fmt_date),
                    task.scheduled_time.as_ref().map(fmt_time),
                    task.deadline_date.as_ref().map(fmt_date),
                    task.deadline_time.as_ref().map(fmt_time),
                    recurrence_json,
                    task.color_label.as_ref().map(|c| c.as_str()),
                    reminders_json,
                    sound_json,
                    now_s,
                    now_s,
                ],
            )
            .map_err(map_sql_err)?;

        Ok(Task {
            assignees: Vec::new(),
            id,
            list_id: list_id.to_string(),
            title: task.title,
            description: task.description,
            status: task.status,
            priority: task.priority,
            scheduled_date: task.scheduled_date,
            scheduled_time: task.scheduled_time,
            deadline_date: task.deadline_date,
            deadline_time: task.deadline_time,
            recurrence: task.recurrence,
            parent_id: task.parent_id,
            section_id: task.section_id,
            color_label: task.color_label,
            reminders: task.reminders,
            sound: task.sound,
            created_at: now,
            updated_at: now,
            completed_at: None,
            etag: None,
        })
    }

    async fn update_task(&self, task: Task) -> cal_core::Result<Task> {
        // If this update completes a recurring task, generate the next
        // instance afterwards. We snapshot the previous status before
        // the write so we can detect the transition reliably.
        let prev_status = self.read_task_status(&task.id)?;

        let now = Utc::now();
        let mut task = task;
        task.updated_at = now;
        let reminders_json = encode_json(&task.reminders)?;
        let recurrence_json = task.recurrence.as_ref().map(encode_json).transpose()?;
        let sound_json = write_sound(&task.sound)?;
        let status = task_status_str(task.status);
        let priority = task_priority_str(task.priority);

        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE tasks
                    SET list_id = ?, parent_id = ?, section_id = ?, title = ?, description = ?,
                        status = ?, priority = ?, scheduled_date = ?, scheduled_time = ?,
                        deadline_date = ?, deadline_time = ?,
                        recurrence = ?, color_label_id = ?, reminders = ?, sound = ?,
                        updated_at = ?, completed_at = ?, etag = ?
                  WHERE id = ?",
                params![
                    task.list_id,
                    task.parent_id,
                    task.section_id,
                    task.title,
                    task.description,
                    status,
                    priority,
                    task.scheduled_date.as_ref().map(fmt_date),
                    task.scheduled_time.as_ref().map(fmt_time),
                    task.deadline_date.as_ref().map(fmt_date),
                    task.deadline_time.as_ref().map(fmt_time),
                    recurrence_json,
                    task.color_label.as_ref().map(|c| c.as_str()),
                    reminders_json,
                    sound_json,
                    fmt_utc(&task.updated_at),
                    task.completed_at.as_ref().map(fmt_utc),
                    task.etag,
                    task.id,
                ],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "task '{}' not found",
                task.id
            )));
        }

        // Recurrence: when a recurring task transitions into Completed
        // (and was not already there) the template generates its next
        // instance. The "post-completion, not pre" semantics matches
        // DESIGN.md section 9.6.
        if task.status == TaskStatus::Completed && prev_status != Some(TaskStatus::Completed) {
            if let Some(recurrence) = task.recurrence.clone() {
                if let Some(next) = self.spawn_next_recurring_task(&task, &recurrence)? {
                    // We don't return the next task — the caller wired
                    // the existing one. The new row shows up on the
                    // next list refresh, which views already trigger
                    // on dialog close.
                    let _ = next;
                }
            }
        }

        Ok(task)
    }

    async fn delete_task(&self, task_id: &str) -> cal_core::Result<()> {
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute("DELETE FROM tasks WHERE id = ?", params![task_id])
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "task '{task_id}' not found"
            )));
        }
        Ok(())
    }

    async fn rename_task_list(&self, list_id: &str, new_name: &str) -> cal_core::Result<()> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(cal_core::Error::InvalidInput(
                "task list name must not be empty".into(),
            ));
        }
        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE task_lists SET name = ? WHERE id = ?",
                params![trimmed, list_id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "task list '{list_id}' not found"
            )));
        }
        Ok(())
    }

    async fn list_sections(&self, list_id: &str) -> cal_core::Result<Vec<Section>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, list_id, name, position
                   FROM sections
                  WHERE list_id = ?
                  ORDER BY position, name COLLATE NOCASE",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map(params![list_id], |row| Ok(row_to_section(row)))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)??);
        }
        Ok(out)
    }
}

pub(crate) fn row_to_section(row: &rusqlite::Row<'_>) -> cal_core::Result<Section> {
    let id = req_text(row, 0)?;
    let list_id = req_text(row, 1)?;
    let name = req_text(row, 2)?;
    let position: i64 = row.get(3).map_err(map_sql_err)?;
    Ok(Section {
        id,
        list_id,
        name,
        // `position` is a non-negative INTEGER in the schema; clamp
        // defensively before the unsigned cast.
        order: position.max(0) as u32,
    })
}

const TASK_SELECT: &str = "SELECT id, list_id, parent_id, title, description, status,
            priority, scheduled_date, scheduled_time, deadline_date,
            deadline_time, recurrence, color_label_id, reminders, sound,
            created_at, updated_at, completed_at, etag, section_id
       FROM tasks
      WHERE list_id = ?
      ORDER BY COALESCE(scheduled_date, deadline_date, ''), created_at";

/// Compute the next occurrence date for a recurring task.
///
/// Honours `interval` (every N days/weeks/months/years) and, for
/// weekly rules with `day_of_week` set, snaps forward to the next
/// listed weekday relative to the anchor. `day_of_month` for monthly
/// rules is respected verbatim, clamped to the target month's length
/// (e.g. the 31st in February becomes the last day of February).
pub(crate) fn advance(anchor: NaiveDate, rule: &TaskRecurrence) -> Option<NaiveDate> {
    let interval = rule.interval.max(1) as i64;
    match rule.frequency {
        RecurrenceFrequency::Daily => anchor.checked_add_days(Days::new(interval as u64)),
        RecurrenceFrequency::Weekly => {
            if let Some(days) = rule.day_of_week.as_ref().filter(|d| !d.is_empty()) {
                next_weekday_after(anchor, days, interval as u64)
            } else {
                anchor.checked_add_days(Days::new(7 * interval as u64))
            }
        }
        RecurrenceFrequency::Monthly => {
            let next = anchor.checked_add_months(Months::new(interval as u32))?;
            if let Some(d) = rule.day_of_month {
                clamp_to_month(next.year(), next.month(), d.into())
            } else {
                Some(next)
            }
        }
        RecurrenceFrequency::Yearly => anchor.checked_add_months(Months::new(12 * interval as u32)),
    }
}

/// Within the same week (or the next interval-week block), find the
/// first weekday listed in `days` after the anchor.
fn next_weekday_after(
    anchor: NaiveDate,
    days: &[Weekday],
    interval_weeks: u64,
) -> Option<NaiveDate> {
    let allowed: Vec<u32> = days.iter().map(|w| weekday_to_iso(*w)).collect();
    if allowed.is_empty() {
        return None;
    }
    // Step day by day up to 7 days; if none of the next 7 days match,
    // jump to the start of the interval-week block after.
    for offset in 1..=7 {
        let candidate = anchor.checked_add_days(Days::new(offset))?;
        let iso = candidate.weekday().number_from_monday();
        if allowed.contains(&iso) {
            return Some(candidate);
        }
    }
    // Fallback for interval > 1: skip the whole gap.
    anchor.checked_add_days(Days::new(7 * interval_weeks.max(1)))
}

fn weekday_to_iso(w: Weekday) -> u32 {
    match w {
        Weekday::Monday => 1,
        Weekday::Tuesday => 2,
        Weekday::Wednesday => 3,
        Weekday::Thursday => 4,
        Weekday::Friday => 5,
        Weekday::Saturday => 6,
        Weekday::Sunday => 7,
    }
}

fn clamp_to_month(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    let last = last_day_of_month(year, month);
    NaiveDate::from_ymd_opt(year, month, day.min(last))
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    // First day of the next month minus one.
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = NaiveDate::from_ymd_opt(ny, nm, 1).unwrap();
    first_next.pred_opt().unwrap().day()
}

fn task_status_str(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Open => "open",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Completed => "completed",
        TaskStatus::Cancelled => "cancelled",
    }
}

fn parse_task_status(s: &str) -> cal_core::Result<TaskStatus> {
    Ok(match s {
        "open" => TaskStatus::Open,
        "in_progress" => TaskStatus::InProgress,
        "completed" => TaskStatus::Completed,
        "cancelled" => TaskStatus::Cancelled,
        other => return Err(unknown_enum("task status", other)),
    })
}

fn task_priority_str(p: TaskPriority) -> &'static str {
    match p {
        TaskPriority::Low => "low",
        TaskPriority::Medium => "medium",
        TaskPriority::High => "high",
    }
}

fn parse_task_priority(s: &str) -> cal_core::Result<TaskPriority> {
    Ok(match s {
        "low" => TaskPriority::Low,
        "medium" => TaskPriority::Medium,
        "high" => TaskPriority::High,
        other => return Err(unknown_enum("task priority", other)),
    })
}

pub(crate) fn row_to_task(row: &rusqlite::Row<'_>) -> cal_core::Result<Task> {
    let id = req_text(row, 0)?;
    let list_id = req_text(row, 1)?;
    let parent_id = opt_text(row, 2)?;
    let title = req_text(row, 3)?;
    let description = opt_text(row, 4)?;
    let status = parse_task_status(&req_text(row, 5)?)?;
    let priority = parse_task_priority(&req_text(row, 6)?)?;
    let scheduled_date = match opt_text(row, 7)? {
        Some(s) => Some(parse_date(&s)?),
        None => None,
    };
    let scheduled_time = match opt_text(row, 8)? {
        Some(s) => Some(parse_time(&s)?),
        None => None,
    };
    let deadline_date = match opt_text(row, 9)? {
        Some(s) => Some(parse_date(&s)?),
        None => None,
    };
    let deadline_time = match opt_text(row, 10)? {
        Some(s) => Some(parse_time(&s)?),
        None => None,
    };
    let recurrence: Option<TaskRecurrence> = match opt_text(row, 11)? {
        Some(s) => Some(decode_json(&s)?),
        None => None,
    };
    let color_label = opt_text(row, 12)?.map(ColorLabelId);
    let reminders: Vec<Reminder> = decode_json(&req_text(row, 13)?)?;
    let sound = read_sound(row, 14)?;
    let created_at = parse_utc(&req_text(row, 15)?)?;
    let updated_at = parse_utc(&req_text(row, 16)?)?;
    let completed_at = match opt_text(row, 17)? {
        Some(s) => Some(parse_utc(&s)?),
        None => None,
    };
    let etag = opt_text(row, 18)?;
    let section_id = opt_text(row, 19)?;

    Ok(Task {
        assignees: Vec::new(),
        id,
        list_id,
        title,
        description,
        status,
        priority,
        scheduled_date,
        scheduled_time,
        deadline_date,
        deadline_time,
        recurrence,
        parent_id,
        section_id,
        color_label,
        reminders,
        sound,
        created_at,
        updated_at,
        completed_at,
        etag,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_test_db;
    use cal_core::TasksFeature;
    use chrono::NaiveDate;

    fn adapter_with_list() -> (LocalAdapter, TaskList) {
        let a = LocalAdapter::new(open_test_db());
        let list = a.create_task_list("Inbox", None, None, None, None).unwrap();
        (a, list)
    }

    fn mk_task(title: &str) -> NewTask {
        NewTask {
            assignees: Vec::new(),
            title: title.into(),
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
        }
    }

    #[tokio::test]
    async fn list_tasks_empty_initially() {
        let (a, list) = adapter_with_list();
        let tasks = a.get_tasks(&list.id).await.unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn create_and_complete_task() {
        let (a, list) = adapter_with_list();
        let mut t = a.create_task(&list.id, mk_task("Buy milk")).await.unwrap();
        t.status = TaskStatus::Completed;
        t.completed_at = Some(Utc::now());
        a.update_task(t).await.unwrap();

        let tasks = a.get_tasks(&list.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert!(tasks[0].completed_at.is_some());
    }

    #[tokio::test]
    async fn deadline_fields_roundtrip() {
        let (a, list) = adapter_with_list();
        let mut nt = mk_task("File taxes");
        nt.deadline_date = Some(NaiveDate::from_ymd_opt(2026, 7, 31).unwrap());
        a.create_task(&list.id, nt).await.unwrap();

        let tasks = a.get_tasks(&list.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].deadline_date,
            Some(NaiveDate::from_ymd_opt(2026, 7, 31).unwrap())
        );
    }

    #[tokio::test]
    async fn scheduled_time_roundtrips() {
        // Times pair with their date; the DB CHECK constraint blocks
        // a time without a date, and round-tripping both ways is the
        // happy path. New field in migration 0006.
        let (a, list) = adapter_with_list();
        let mut nt = mk_task("Standup");
        nt.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 5, 21).unwrap());
        nt.scheduled_time = Some(chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap());
        a.create_task(&list.id, nt).await.unwrap();

        let tasks = a.get_tasks(&list.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 21).unwrap())
        );
        assert_eq!(
            tasks[0].scheduled_time,
            Some(chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap())
        );
    }

    #[tokio::test]
    async fn delete_task_list_cascades_tasks() {
        let (a, list) = adapter_with_list();
        a.create_task(&list.id, mk_task("Foo")).await.unwrap();
        a.delete_task_list(&list.id).unwrap();

        let conn = a.db().lock().unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn completing_recurring_task_spawns_next() {
        let (a, list) = adapter_with_list();
        let anchor = NaiveDate::from_ymd_opt(2026, 5, 19).unwrap();
        let mut nt = mk_task("Water plants");
        nt.scheduled_date = Some(anchor);
        nt.recurrence = Some(TaskRecurrence {
            frequency: RecurrenceFrequency::Daily,
            interval: 3,
            day_of_week: None,
            day_of_month: None,
            end: None,
        });
        let original = a.create_task(&list.id, nt).await.unwrap();

        // Complete the task.
        let mut completed = original.clone();
        completed.status = TaskStatus::Completed;
        completed.completed_at = Some(Utc::now());
        a.update_task(completed).await.unwrap();

        // Now there should be two tasks: the original (completed) plus
        // a new open one three days later.
        let tasks = a.get_tasks(&list.id).await.unwrap();
        assert_eq!(tasks.len(), 2);
        let next = tasks
            .iter()
            .find(|t| t.status == TaskStatus::Open)
            .expect("should have a fresh open task");
        assert_eq!(
            next.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
        );
    }

    #[tokio::test]
    async fn weekly_recurrence_picks_next_listed_weekday() {
        let (a, list) = adapter_with_list();
        // 2026-05-19 is a Tuesday.
        let anchor = NaiveDate::from_ymd_opt(2026, 5, 19).unwrap();
        let mut nt = mk_task("Standup");
        nt.scheduled_date = Some(anchor);
        nt.recurrence = Some(TaskRecurrence {
            frequency: RecurrenceFrequency::Weekly,
            interval: 1,
            day_of_week: Some(vec![Weekday::Thursday]),
            day_of_month: None,
            end: None,
        });
        let original = a.create_task(&list.id, nt).await.unwrap();

        let mut completed = original.clone();
        completed.status = TaskStatus::Completed;
        a.update_task(completed).await.unwrap();

        let tasks = a.get_tasks(&list.id).await.unwrap();
        let next = tasks.iter().find(|t| t.status == TaskStatus::Open).unwrap();
        // Tuesday → next Thursday = 21 May.
        assert_eq!(
            next.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 21).unwrap()),
        );
    }

    #[tokio::test]
    async fn recurrence_end_on_date_stops_spawning() {
        let (a, list) = adapter_with_list();
        let anchor = NaiveDate::from_ymd_opt(2026, 5, 19).unwrap();
        let mut nt = mk_task("One last time");
        nt.scheduled_date = Some(anchor);
        nt.recurrence = Some(TaskRecurrence {
            frequency: RecurrenceFrequency::Daily,
            interval: 1,
            day_of_week: None,
            day_of_month: None,
            end: Some(RecurrenceEnd::OnDate {
                date: NaiveDate::from_ymd_opt(2026, 5, 19).unwrap(),
            }),
        });
        let original = a.create_task(&list.id, nt).await.unwrap();

        let mut completed = original;
        completed.status = TaskStatus::Completed;
        a.update_task(completed).await.unwrap();

        // The next would be 20 May, past the end date — nothing spawned.
        let tasks = a.get_tasks(&list.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
    }

    #[tokio::test]
    async fn re_completing_completed_task_does_not_double_spawn() {
        let (a, list) = adapter_with_list();
        let anchor = NaiveDate::from_ymd_opt(2026, 5, 19).unwrap();
        let mut nt = mk_task("Daily");
        nt.scheduled_date = Some(anchor);
        nt.recurrence = Some(TaskRecurrence {
            frequency: RecurrenceFrequency::Daily,
            interval: 1,
            day_of_week: None,
            day_of_month: None,
            end: None,
        });
        let original = a.create_task(&list.id, nt).await.unwrap();

        let mut completed = original.clone();
        completed.status = TaskStatus::Completed;
        let after_first = a.update_task(completed).await.unwrap();
        // Save again — status hasn't changed, no second spawn.
        a.update_task(after_first).await.unwrap();

        let tasks = a.get_tasks(&list.id).await.unwrap();
        assert_eq!(tasks.len(), 2);
    }

    #[tokio::test]
    async fn subtask_parent_chain() {
        let (a, list) = adapter_with_list();
        let parent = a.create_task(&list.id, mk_task("Parent")).await.unwrap();
        let mut sub_payload = mk_task("Sub");
        sub_payload.parent_id = Some(parent.id.clone());
        a.create_task(&list.id, sub_payload).await.unwrap();

        let tasks = a.get_tasks(&list.id).await.unwrap();
        let sub = tasks.iter().find(|t| t.title == "Sub").unwrap();
        assert_eq!(sub.parent_id.as_deref(), Some(parent.id.as_str()));
    }

    #[tokio::test]
    async fn rename_task_list_updates_the_row() {
        let (a, list) = adapter_with_list();
        a.rename_task_list(&list.id, "Renamed").await.unwrap();
        let lists = a.list_task_lists().await.unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].name, "Renamed");
    }

    #[tokio::test]
    async fn rename_task_list_rejects_empty_name() {
        let (a, list) = adapter_with_list();
        let err = a.rename_task_list(&list.id, "  ").await.unwrap_err();
        assert!(matches!(err, cal_core::Error::InvalidInput(_)));
        assert_eq!(a.list_task_lists().await.unwrap()[0].name, "Inbox");
    }

    #[tokio::test]
    async fn rename_task_list_returns_not_found_for_unknown_id() {
        let (a, _list) = adapter_with_list();
        let err = a.rename_task_list("does-not-exist", "X").await.unwrap_err();
        assert!(matches!(err, cal_core::Error::NotFound(_)));
    }

    #[tokio::test]
    async fn section_crud_and_listing() {
        let (a, list) = adapter_with_list();
        assert!(a.list_sections(&list.id).await.unwrap().is_empty());

        let s1 = a.create_section(&list.id, "Doing", 0).unwrap();
        let s2 = a.create_section(&list.id, "Done", 1).unwrap();
        assert_eq!(s1.list_id, list.id);

        let listed = a.list_sections(&list.id).await.unwrap();
        assert_eq!(listed.len(), 2);
        // Ordered by `position`.
        assert_eq!(listed[0].name, "Doing");
        assert_eq!(listed[1].name, "Done");

        // Rename + reorder.
        let renamed = a
            .update_section(Section {
                name: "Shipped".into(),
                order: 5,
                ..s2.clone()
            })
            .unwrap();
        assert_eq!(renamed.name, "Shipped");
        assert_eq!(
            a.get_section_by_id(&s2.id).unwrap().unwrap().name,
            "Shipped"
        );

        // Delete ungroups but keeps the list intact.
        a.delete_section(&s1.id).unwrap();
        assert_eq!(a.list_sections(&list.id).await.unwrap().len(), 1);
        assert!(a.get_section_by_id(&s1.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn task_section_id_roundtrips_and_survives_delete() {
        let (a, list) = adapter_with_list();
        let section = a.create_section(&list.id, "Sprint", 0).unwrap();

        let mut new = mk_task("Ship it");
        new.section_id = Some(section.id.clone());
        let created = a.create_task(&list.id, new).await.unwrap();
        assert_eq!(created.section_id.as_deref(), Some(section.id.as_str()));

        // Read-back path carries it too.
        let fetched = a.get_task_by_id(&created.id).unwrap().unwrap();
        assert_eq!(fetched.section_id.as_deref(), Some(section.id.as_str()));

        // Deleting the section ungroups the task (ON DELETE SET NULL),
        // it does not delete the task.
        a.delete_section(&section.id).unwrap();
        let after = a.get_task_by_id(&created.id).unwrap().unwrap();
        assert!(after.section_id.is_none());
    }

    #[tokio::test]
    async fn task_list_parent_id_roundtrips() {
        let a = LocalAdapter::new(open_test_db());
        let parent = a
            .create_task_list("Parent", None, None, None, None)
            .unwrap();
        let mut child = a.create_task_list("Child", None, None, None, None).unwrap();
        assert!(child.parent_id.is_none());

        child.parent_id = Some(parent.id.clone());
        a.update_task_list(child.clone()).unwrap();

        let reloaded = a.get_task_list_by_id(&child.id).unwrap().unwrap();
        assert_eq!(reloaded.parent_id.as_deref(), Some(parent.id.as_str()));

        // Deleting the parent promotes the child to top-level
        // (ON DELETE SET NULL) rather than cascading it away.
        a.delete_task_list(&parent.id).unwrap();
        let orphan = a.get_task_list_by_id(&child.id).unwrap().unwrap();
        assert!(orphan.parent_id.is_none());
    }

    #[tokio::test]
    async fn reparent_task_list_sets_and_clears_parent() {
        let a = LocalAdapter::new(open_test_db());
        let parent = a
            .create_task_list("Parent", None, None, None, None)
            .unwrap();
        let child = a.create_task_list("Child", None, None, None, None).unwrap();

        let moved = a.reparent_task_list(&child.id, Some(&parent.id)).unwrap();
        assert_eq!(moved.parent_id.as_deref(), Some(parent.id.as_str()));
        assert_eq!(
            a.get_task_list_by_id(&child.id)
                .unwrap()
                .unwrap()
                .parent_id
                .as_deref(),
            Some(parent.id.as_str()),
        );

        // Clearing the parent promotes the list back to top level.
        let promoted = a.reparent_task_list(&child.id, None).unwrap();
        assert!(promoted.parent_id.is_none());

        // Unknown id surfaces NotFound.
        assert!(matches!(
            a.reparent_task_list("nope", None).unwrap_err(),
            cal_core::Error::NotFound(_),
        ));
    }
}
