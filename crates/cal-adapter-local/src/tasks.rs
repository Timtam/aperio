//! `TasksFeature` implementation plus local-only management methods.

use async_trait::async_trait;
use cal_core::{
    ColorLabelId, ContainerColor, NewTask, Reminder, Section, SoundConfig, Task, TaskList,
    TaskPriority, TaskRecurrence, TaskStatus, TasksFeature,
};
use chrono::Utc;
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

    /// On completion of a recurring task, create the next instance
    /// (DESIGN §9.12).
    ///
    /// The new row inherits everything from the template except the id and
    /// completion state (reset to open). Where it lands depends on
    /// `recurrence.placement`:
    ///
    /// - **`Schedule`** (default) — dated, advanced by `frequency × interval`
    ///   (or the next `fixed_dates` entry); the historical behavior.
    /// - **`Backlog`** — undated; its `resurface_date` gates when it
    ///   reappears in the active backlog (`FromCompletion` + interval, or
    ///   the next `fixed_dates` entry after completion; immediately when the
    ///   interval is 0/empty).
    ///
    /// Idempotency (DESIGN §9.12): a managed series spawns at most one open
    /// instance. If an open instance of this `series_id` already exists in
    /// the synced set (a second client got there first, or a re-trigger),
    /// this is a no-op — the other instance already covers the next turn.
    ///
    /// Returns `Ok(None)` when nothing should be spawned (rule ended, no
    /// anchor to advance from, or the series is already covered).
    fn spawn_next_recurring_task(&self, template: &Task) -> cal_core::Result<Option<Task>> {
        // Idempotency gate: never spawn a second open instance of a series.
        if let Some(sid) = template.series_id.as_deref() {
            if self.has_open_series_instance(sid)? {
                return Ok(None);
            }
        }

        // The placement-aware computation lives in `cal_core::spawn` so the
        // host can run the same logic for external lists (DESIGN §9.12).
        let Some(new) = cal_core::next_recurrence_instance(template) else {
            return Ok(None);
        };
        let task = self.create_task_sync(&template.list_id, new)?;
        Ok(Some(task))
    }

    /// True when an open (or in-progress) task already carries this
    /// `series_id`. Backs the idempotent spawner and is list-agnostic — a
    /// series lives in exactly one list, but we don't depend on that here.
    fn has_open_series_instance(&self, series_id: &str) -> cal_core::Result<bool> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks
                  WHERE series_id = ? AND status IN ('open', 'in_progress')",
                params![series_id],
                |row| row.get(0),
            )
            .map_err(map_sql_err)?;
        Ok(count > 0)
    }

    /// Synchronous task creation — the one implementation behind both the
    /// async [`TasksFeature::create_task`] (which simply forwards here) and
    /// the on-device FFI store. `TasksFeature` is async to match the trait
    /// shape shared with network-backed adapters, but the local adapter does
    /// no IO that benefits from async, so the real work lives here and no
    /// runtime handle is dragged through the call (the recurrence spawner
    /// relies on this too).
    pub fn create_task_sync(&self, list_id: &str, mut task: NewTask) -> cal_core::Result<Task> {
        ensure_series_id(&mut task);
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
                    recurrence, resurface_date, series_id, color_label_id, reminders, sound,
                    created_at, updated_at, completed_at, etag
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL)",
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
                    task.resurface_date.as_ref().map(fmt_date),
                    task.series_id,
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
            resurface_date: task.resurface_date,
            series_id: task.series_id,
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

    /// Synchronous task listing — the implementation behind the async
    /// [`TasksFeature::get_tasks`] and the FFI store. See
    /// [`LocalAdapter::create_task_sync`] for why the work is sync.
    pub fn get_tasks_sync(&self, list_id: &str) -> cal_core::Result<Vec<Task>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(TASK_SELECT).map_err(map_sql_err)?;
        let rows = stmt
            .query_map(params![list_id], |row| Ok(row_to_task(row)))
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)??);
        }
        Ok(dedupe_open_series(out))
    }

    /// Synchronous task update — the implementation behind the async
    /// [`TasksFeature::update_task`] and the FFI store. Carries the
    /// completion → next-instance recurrence spawn (DESIGN §9.12).
    pub fn update_task_sync(&self, task: Task) -> cal_core::Result<Task> {
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
                // `resurface_date` and `series_id` are written here too, in
                // step with the INSERT: a task edited into an on-demand
                // recurring one is assigned a series_id by the host command,
                // and that — plus any resurface trigger — must persist, or the
                // idempotent spawner (DESIGN §9.12) has nothing to dedup on.
                "UPDATE tasks
                    SET list_id = ?, parent_id = ?, section_id = ?, title = ?, description = ?,
                        status = ?, priority = ?, scheduled_date = ?, scheduled_time = ?,
                        deadline_date = ?, deadline_time = ?,
                        recurrence = ?, resurface_date = ?, series_id = ?, color_label_id = ?,
                        reminders = ?, sound = ?,
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
                    task.resurface_date.as_ref().map(fmt_date),
                    task.series_id,
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
        if task.status == TaskStatus::Completed
            && prev_status != Some(TaskStatus::Completed)
            && task.recurrence.is_some()
        {
            if let Some(next) = self.spawn_next_recurring_task(&task)? {
                // We don't return the next task — the caller wired
                // the existing one. The new row shows up on the
                // next list refresh, which views already trigger
                // on dialog close.
                let _ = next;
            }
        }

        Ok(task)
    }

    /// Synchronous task delete — the implementation behind the async
    /// [`TasksFeature::delete_task`] and the FFI store.
    pub fn delete_task_sync(&self, task_id: &str) -> cal_core::Result<()> {
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

    /// Synchronous task-list listing — the implementation behind the async
    /// [`TasksFeature::list_task_lists`] and the FFI store.
    pub fn list_task_lists_sync(&self) -> cal_core::Result<Vec<TaskList>> {
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

    /// Synchronous task-list rename — the implementation behind the async
    /// [`TasksFeature::rename_task_list`] and the FFI store. Rejects an
    /// empty (or whitespace-only) name.
    pub fn rename_task_list_sync(&self, list_id: &str, new_name: &str) -> cal_core::Result<()> {
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
                        created_at, updated_at, completed_at, etag, section_id,
                        resurface_date, series_id
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
        color_label: Option<ColorLabelId>,
    ) -> cal_core::Result<Section> {
        let id = Uuid::new_v4().to_string();
        let now_s = fmt_utc(&Utc::now());
        self.db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO sections (id, list_id, name, position, color_label_id, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    id,
                    list_id,
                    name,
                    position as i64,
                    color_label.as_ref().map(|c| c.as_str()),
                    now_s,
                    now_s
                ],
            )
            .map_err(map_sql_err)?;
        Ok(Section {
            id,
            list_id: list_id.to_string(),
            name: name.to_string(),
            color_label,
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
                "UPDATE sections SET name = ?, position = ?, color_label_id = ?, updated_at = ? WHERE id = ?",
                params![
                    section.name,
                    section.order as i64,
                    section.color_label.as_ref().map(|c| c.as_str()),
                    now_s,
                    section.id
                ],
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
            .prepare(
                "SELECT id, list_id, name, position, color_label_id FROM sections WHERE id = ?",
            )
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
        self.list_task_lists_sync()
    }

    async fn get_tasks(&self, list_id: &str) -> cal_core::Result<Vec<Task>> {
        self.get_tasks_sync(list_id)
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> cal_core::Result<Task> {
        self.create_task_sync(list_id, task)
    }

    async fn update_task(&self, task: Task) -> cal_core::Result<Task> {
        self.update_task_sync(task)
    }

    async fn delete_task(&self, task_id: &str) -> cal_core::Result<()> {
        self.delete_task_sync(task_id)
    }

    async fn rename_task_list(&self, list_id: &str, new_name: &str) -> cal_core::Result<()> {
        self.rename_task_list_sync(list_id, new_name)
    }

    async fn list_sections(&self, list_id: &str) -> cal_core::Result<Vec<Section>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, list_id, name, position, color_label_id
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
    let color_label = opt_text(row, 4)?.map(ColorLabelId);
    Ok(Section {
        id,
        list_id,
        name,
        color_label,
        // `position` is a non-negative INTEGER in the schema; clamp
        // defensively before the unsigned cast.
        order: position.max(0) as u32,
    })
}

const TASK_SELECT: &str = "SELECT id, list_id, parent_id, title, description, status,
            priority, scheduled_date, scheduled_time, deadline_date,
            deadline_time, recurrence, color_label_id, reminders, sound,
            created_at, updated_at, completed_at, etag, section_id,
            resurface_date, series_id
       FROM tasks
      WHERE list_id = ?
      ORDER BY COALESCE(scheduled_date, deadline_date, ''), created_at";

/// Assign a stable `series_id` to a recurring task that doesn't have one,
/// so the idempotent spawner (DESIGN §9.12) has a key to dedup on across
/// devices and shared lists. Non-recurring tasks, and tasks that already
/// carry a series id (e.g. a spawned instance inheriting its template's),
/// are left untouched.
fn ensure_series_id(task: &mut NewTask) {
    if task.series_id.is_none() && task.recurrence.is_some() {
        task.series_id = Some(Uuid::new_v4().to_string());
    }
}

/// True for statuses that count as a live (uncompleted) task.
fn is_open_status(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::Open | TaskStatus::InProgress)
}

/// Safety net for the idempotent spawner (DESIGN §9.12): should a sync race
/// produce two open instances of the same `series_id`, keep only the
/// canonical (oldest by `created_at`, then id) one in the view and drop the
/// rest. Completed instances are history and never collapsed; untagged tasks
/// pass through untouched.
fn dedupe_open_series(tasks: Vec<Task>) -> Vec<Task> {
    use std::collections::HashMap;
    // Pick the canonical open instance per series.
    let mut canonical: HashMap<&str, usize> = HashMap::new();
    for (i, t) in tasks.iter().enumerate() {
        if !is_open_status(t.status) {
            continue;
        }
        let Some(sid) = t.series_id.as_deref() else {
            continue;
        };
        match canonical.get(sid) {
            Some(&j) => {
                let cur = &tasks[j];
                if (t.created_at, &t.id) < (cur.created_at, &cur.id) {
                    canonical.insert(sid, i);
                }
            }
            None => {
                canonical.insert(sid, i);
            }
        }
    }
    let keep: std::collections::HashSet<usize> = canonical.into_values().collect();
    tasks
        .into_iter()
        .enumerate()
        .filter(|(i, t)| {
            // Drop an open, series-tagged task only when it isn't the
            // canonical instance for its series.
            !(is_open_status(t.status) && t.series_id.is_some() && !keep.contains(i))
        })
        .map(|(_, t)| t)
        .collect()
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
    let resurface_date = match opt_text(row, 20)? {
        Some(s) => Some(parse_date(&s)?),
        None => None,
    };
    let series_id = opt_text(row, 21)?;

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
        resurface_date,
        series_id,
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
    use cal_core::{
        MonthDay, RecurrenceAnchor, RecurrenceEnd, RecurrenceFrequency, RecurrencePlacement,
        TasksFeature, Weekday,
    };
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
            resurface_date: None,
            series_id: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: vec![],
            sound: None,
        }
    }

    /// Build a recurrence rule, defaulting the parts a test doesn't care
    /// about (no weekday/day-of-month filter, no end boundary).
    fn rec(
        frequency: RecurrenceFrequency,
        interval: u32,
        anchor: RecurrenceAnchor,
        placement: RecurrencePlacement,
        fixed_dates: Option<Vec<MonthDay>>,
    ) -> TaskRecurrence {
        TaskRecurrence {
            frequency,
            interval,
            day_of_week: None,
            day_of_month: None,
            end: None,
            anchor,
            placement,
            fixed_dates,
        }
    }

    /// Complete `task` on a specific calendar day and apply it, returning
    /// the persisted completed row (so its recurrence rule can be reused).
    async fn complete_on(a: &LocalAdapter, task: &Task, day: NaiveDate) -> Task {
        let mut done = task.clone();
        done.status = TaskStatus::Completed;
        done.completed_at = Some(day.and_hms_opt(9, 0, 0).unwrap().and_utc());
        a.update_task(done).await.unwrap()
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
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
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
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
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
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
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
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
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

    // ── DESIGN §9.12: backlog / on-demand recurrence ──────────────

    #[tokio::test]
    async fn create_assigns_series_id_to_recurring_task_only() {
        let (a, list) = adapter_with_list();
        let mut nt = mk_task("Daily");
        nt.scheduled_date = Some(NaiveDate::from_ymd_opt(2026, 5, 19).unwrap());
        nt.recurrence = Some(rec(
            RecurrenceFrequency::Daily,
            1,
            RecurrenceAnchor::FromDate,
            RecurrencePlacement::Schedule,
            None,
        ));
        let recurring = a.create_task(&list.id, nt).await.unwrap();
        assert!(
            recurring.series_id.is_some(),
            "a recurring task gets a stable series id"
        );

        let plain = a.create_task(&list.id, mk_task("One-off")).await.unwrap();
        assert!(
            plain.series_id.is_none(),
            "a non-recurring task stays unmanaged"
        );
    }

    #[tokio::test]
    async fn update_persists_series_id_and_resurface_date() {
        // Regression: editing a task into an on-demand recurring one — the
        // host assigns a series_id, and the rule may carry a resurface
        // trigger — must keep both across the write, or the idempotent
        // spawner has nothing to dedup on (DESIGN §9.12). The UPDATE used to
        // omit both columns and silently drop them.
        let (a, list) = adapter_with_list();
        let created = a.create_task(&list.id, mk_task("Edit me")).await.unwrap();
        assert!(created.series_id.is_none());
        assert!(created.resurface_date.is_none());

        let mut edited = created.clone();
        edited.recurrence = Some(rec(
            RecurrenceFrequency::Weekly,
            1,
            RecurrenceAnchor::FromCompletion,
            RecurrencePlacement::Backlog,
            None,
        ));
        edited.series_id = Some("series-xyz".to_string());
        edited.resurface_date = Some(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap());
        // Still Open, so this is a plain edit (no completion → no spawn).
        a.update_task(edited).await.unwrap();

        let reread = a.get_task_by_id(&created.id).unwrap().unwrap();
        assert_eq!(reread.series_id.as_deref(), Some("series-xyz"));
        assert_eq!(
            reread.resurface_date,
            Some(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap())
        );
    }

    #[tokio::test]
    async fn backlog_recurrence_resurfaces_immediately_without_interval() {
        // The dishwasher: empty it when full → straight back into the
        // backlog, no date attached.
        let (a, list) = adapter_with_list();
        let mut nt = mk_task("Empty dishwasher");
        nt.recurrence = Some(rec(
            RecurrenceFrequency::Daily,
            0,
            RecurrenceAnchor::FromCompletion,
            RecurrencePlacement::Backlog,
            None,
        ));
        let task = a.create_task(&list.id, nt).await.unwrap();
        complete_on(&a, &task, NaiveDate::from_ymd_opt(2026, 5, 10).unwrap()).await;

        let tasks = a.get_tasks(&list.id).await.unwrap();
        assert_eq!(tasks.len(), 2);
        let next = tasks.iter().find(|t| t.status == TaskStatus::Open).unwrap();
        assert_eq!(next.scheduled_date, None, "backlog instances are undated");
        assert_eq!(
            next.resurface_date, None,
            "interval 0 ⇒ visible in the backlog right away"
        );
        assert!(
            next.recurrence.is_some(),
            "the rule carries to the next turn"
        );
    }

    #[tokio::test]
    async fn backlog_recurrence_resurfaces_after_interval() {
        let (a, list) = adapter_with_list();
        let mut nt = mk_task("Water the plant");
        nt.recurrence = Some(rec(
            RecurrenceFrequency::Weekly,
            1,
            RecurrenceAnchor::FromCompletion,
            RecurrencePlacement::Backlog,
            None,
        ));
        let task = a.create_task(&list.id, nt).await.unwrap();
        complete_on(&a, &task, NaiveDate::from_ymd_opt(2026, 5, 10).unwrap()).await;

        let tasks = a.get_tasks(&list.id).await.unwrap();
        let next = tasks.iter().find(|t| t.status == TaskStatus::Open).unwrap();
        assert_eq!(next.scheduled_date, None);
        // Completed 10 May + 1 week → resurfaces 17 May.
        assert_eq!(
            next.resurface_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 17).unwrap()),
        );
    }

    #[tokio::test]
    async fn fixed_dates_backlog_resurfaces_on_next_seasonal_date() {
        // Swap summer↔winter shoes: surface around 1 Apr / 1 Oct.
        let (a, list) = adapter_with_list();
        let mut nt = mk_task("Swap shoes");
        nt.recurrence = Some(rec(
            RecurrenceFrequency::Yearly,
            1,
            RecurrenceAnchor::FromCompletion,
            RecurrencePlacement::Backlog,
            Some(vec![
                MonthDay { month: 4, day: 1 },
                MonthDay { month: 10, day: 1 },
            ]),
        ));
        let task = a.create_task(&list.id, nt).await.unwrap();
        // Completed in May → next seasonal trigger is 1 October.
        complete_on(&a, &task, NaiveDate::from_ymd_opt(2026, 5, 10).unwrap()).await;

        let tasks = a.get_tasks(&list.id).await.unwrap();
        let next = tasks.iter().find(|t| t.status == TaskStatus::Open).unwrap();
        assert_eq!(
            next.resurface_date,
            Some(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()),
        );
    }

    #[tokio::test]
    async fn fixed_dates_wrap_into_next_year() {
        let (a, list) = adapter_with_list();
        let mut nt = mk_task("Swap shoes");
        nt.recurrence = Some(rec(
            RecurrenceFrequency::Yearly,
            1,
            RecurrenceAnchor::FromCompletion,
            RecurrencePlacement::Backlog,
            Some(vec![
                MonthDay { month: 4, day: 1 },
                MonthDay { month: 10, day: 1 },
            ]),
        ));
        let task = a.create_task(&list.id, nt).await.unwrap();
        // Completed in November → both this year's dates are past;
        // the next trigger wraps to 1 April next year.
        complete_on(&a, &task, NaiveDate::from_ymd_opt(2026, 11, 15).unwrap()).await;

        let tasks = a.get_tasks(&list.id).await.unwrap();
        let next = tasks.iter().find(|t| t.status == TaskStatus::Open).unwrap();
        assert_eq!(
            next.resurface_date,
            Some(NaiveDate::from_ymd_opt(2027, 4, 1).unwrap()),
        );
    }

    #[tokio::test]
    async fn idempotent_spawner_skips_when_series_already_open() {
        let (a, list) = adapter_with_list();
        let mut nt = mk_task("Empty dishwasher");
        nt.recurrence = Some(rec(
            RecurrenceFrequency::Daily,
            0,
            RecurrenceAnchor::FromCompletion,
            RecurrencePlacement::Backlog,
            None,
        ));
        let task = a.create_task(&list.id, nt).await.unwrap();
        let completed = complete_on(&a, &task, NaiveDate::from_ymd_opt(2026, 5, 10).unwrap()).await;

        // Completing spawned one open follow-up. The gate detects it…
        let series = completed.series_id.clone().unwrap();
        assert!(a.has_open_series_instance(&series).unwrap());
        // …so a second spawn for the same template is a no-op (this is the
        // "other client already created it" path on a shared list).
        assert!(a.spawn_next_recurring_task(&completed).unwrap().is_none());

        let tasks = a.get_tasks(&list.id).await.unwrap();
        assert_eq!(tasks.len(), 2, "no third instance was created");
    }

    #[tokio::test]
    async fn dedupe_on_read_collapses_racing_open_instances() {
        let (a, list) = adapter_with_list();
        let rule = rec(
            RecurrenceFrequency::Daily,
            0,
            RecurrenceAnchor::FromCompletion,
            RecurrencePlacement::Backlog,
            None,
        );
        // Two open instances of the same series — what a sync race between
        // two clients could leave behind.
        let mut a1 = mk_task("Empty dishwasher");
        a1.series_id = Some("series-dish".into());
        a1.recurrence = Some(rule.clone());
        a.create_task(&list.id, a1).await.unwrap();

        let mut a2 = mk_task("Empty dishwasher");
        a2.series_id = Some("series-dish".into());
        a2.recurrence = Some(rule);
        a.create_task(&list.id, a2).await.unwrap();

        let tasks = a.get_tasks(&list.id).await.unwrap();
        assert_eq!(tasks.len(), 1, "the canonical instance wins the view");
        assert_eq!(tasks[0].series_id.as_deref(), Some("series-dish"));
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

        let s1 = a.create_section(&list.id, "Doing", 0, None).unwrap();
        let s2 = a.create_section(&list.id, "Done", 1, None).unwrap();
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
        let section = a.create_section(&list.id, "Sprint", 0, None).unwrap();

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
