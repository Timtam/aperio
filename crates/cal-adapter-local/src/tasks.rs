//! `TasksFeature` implementation plus local-only management methods.

use async_trait::async_trait;
use cal_core::{
    ColorLabelId, ContainerColor, DeadlineType, NewTask, Reminder, SoundConfig, Task, TaskList,
    TaskPriority, TaskRecurrence, TaskStatus, TasksFeature,
};
use chrono::Utc;
use rusqlite::params;
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
                    id, source, name, color_hex, color_source, default_sound,
                    embedded_in_calendar, read_only, created_at, updated_at
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
                params![
                    id,
                    SOURCE_ID,
                    name,
                    color_hex,
                    color_source,
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
            default_sound,
            embedded_in_calendar,
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
                        default_sound = ?, embedded_in_calendar = ?,
                        updated_at = ?
                  WHERE id = ?",
                params![
                    list.name,
                    color_hex,
                    color_source,
                    default_sound_json,
                    list.embedded_in_calendar,
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
}

#[async_trait]
impl TasksFeature for LocalAdapter {
    async fn list_task_lists(&self) -> cal_core::Result<Vec<TaskList>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, name, color_hex, color_source, default_sound,
                        embedded_in_calendar, read_only
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
                ))
            })
            .map_err(map_sql_err)?;

        let mut out = Vec::new();
        for r in rows {
            let (id, name, color, sound, embedded, read_only) = r.map_err(map_sql_err)?;
            out.push(TaskList {
                id: id?,
                name: name?,
                color: color?,
                default_sound: sound?,
                embedded_in_calendar: embedded?,
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
        let deadline_type = task.deadline_type.map(deadline_type_str);

        self.db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO tasks (
                    id, list_id, parent_id, title, description, status, priority,
                    scheduled_date, deadline_type, deadline_date, deadline_time,
                    recurrence, color_label_id, reminders, sound,
                    created_at, updated_at, completed_at, etag
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL)",
                params![
                    id,
                    list_id,
                    task.parent_id,
                    task.title,
                    task.description,
                    status,
                    priority,
                    task.scheduled_date.as_ref().map(fmt_date),
                    deadline_type,
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
            id,
            list_id: list_id.to_string(),
            title: task.title,
            description: task.description,
            status: task.status,
            priority: task.priority,
            scheduled_date: task.scheduled_date,
            deadline_type: task.deadline_type,
            deadline_date: task.deadline_date,
            deadline_time: task.deadline_time,
            recurrence: task.recurrence,
            parent_id: task.parent_id,
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
        let now = Utc::now();
        let mut task = task;
        task.updated_at = now;
        let reminders_json = encode_json(&task.reminders)?;
        let recurrence_json = task.recurrence.as_ref().map(encode_json).transpose()?;
        let sound_json = write_sound(&task.sound)?;
        let status = task_status_str(task.status);
        let priority = task_priority_str(task.priority);
        let deadline_type = task.deadline_type.map(deadline_type_str);

        let changed = self
            .db()
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE tasks
                    SET list_id = ?, parent_id = ?, title = ?, description = ?,
                        status = ?, priority = ?, scheduled_date = ?,
                        deadline_type = ?, deadline_date = ?, deadline_time = ?,
                        recurrence = ?, color_label_id = ?, reminders = ?, sound = ?,
                        updated_at = ?, completed_at = ?, etag = ?
                  WHERE id = ?",
                params![
                    task.list_id,
                    task.parent_id,
                    task.title,
                    task.description,
                    status,
                    priority,
                    task.scheduled_date.as_ref().map(fmt_date),
                    deadline_type,
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
}

const TASK_SELECT: &str = "SELECT id, list_id, parent_id, title, description, status,
            priority, scheduled_date, deadline_type, deadline_date,
            deadline_time, recurrence, color_label_id, reminders, sound,
            created_at, updated_at, completed_at, etag
       FROM tasks
      WHERE list_id = ?
      ORDER BY COALESCE(scheduled_date, deadline_date, ''), created_at";

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

fn deadline_type_str(t: DeadlineType) -> &'static str {
    match t {
        DeadlineType::On => "on",
        DeadlineType::By => "by",
    }
}

fn parse_deadline_type(s: &str) -> cal_core::Result<DeadlineType> {
    Ok(match s {
        "on" => DeadlineType::On,
        "by" => DeadlineType::By,
        other => return Err(unknown_enum("deadline type", other)),
    })
}

fn row_to_task(row: &rusqlite::Row<'_>) -> cal_core::Result<Task> {
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
    let deadline_type = match opt_text(row, 8)? {
        Some(s) => Some(parse_deadline_type(&s)?),
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

    Ok(Task {
        id,
        list_id,
        title,
        description,
        status,
        priority,
        scheduled_date,
        deadline_type,
        deadline_date,
        deadline_time,
        recurrence,
        parent_id,
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
        let list = a.create_task_list("Inbox", None, None, None).unwrap();
        (a, list)
    }

    fn mk_task(title: &str) -> NewTask {
        NewTask {
            title: title.into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            scheduled_date: None,
            deadline_type: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
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
        nt.deadline_type = Some(DeadlineType::By);
        nt.deadline_date = Some(NaiveDate::from_ymd_opt(2026, 7, 31).unwrap());
        a.create_task(&list.id, nt).await.unwrap();

        let tasks = a.get_tasks(&list.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].deadline_type, Some(DeadlineType::By));
        assert_eq!(
            tasks[0].deadline_date,
            Some(NaiveDate::from_ymd_opt(2026, 7, 31).unwrap())
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
}
