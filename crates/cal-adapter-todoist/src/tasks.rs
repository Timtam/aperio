//! Todoist Tasks API mapping (REST v2).
//!
//! Todoist's data model:
//!
//!   - **Projects** play the role of Aperio's `TaskList`. Projects
//!     nest via `parent_id`, which we surface as `TaskList.parent_id`
//!     so the sidebar renders the tree.
//!   - **Sections** group a project's tasks. They're a first-class
//!     resource (`GET /sections?project_id=…`); we map them to Aperio
//!     `Section`s (`list_sections`) and a task's `section_id` to
//!     `Task.section_id`. A task can be filed into a section on create;
//!     moving it between sections later needs the Sync API (same
//!     limitation as cross-project moves), so the update body omits it.
//!   - **Tasks** carry `content` (title), `description`, `due`,
//!     `deadline` (added to the API in late 2024), `priority` (1-4,
//!     INVERTED from the UI labelling — see priority mapper),
//!     `labels`, `parent_id` (for subtasks), `section_id`.
//!   - **Status** is the boolean `is_completed`. To flip it via the
//!     REST v2 API you MUST use the dedicated `/close` and
//!     `/reopen` endpoints — the regular `POST /tasks/{id}` update
//!     does not accept `is_completed`.
//!
//! Status mapping (DESIGN.md §9.7):
//!
//!   - Aperio Open / InProgress  → Todoist `is_completed = false`
//!   - Aperio Completed          → Todoist `is_completed = true`
//!     (driven via `/close`, not the update body)
//!   - Aperio Cancelled          → Todoist `is_completed = false`
//!     (no equivalent — cancelled marker stays local)
//!
//! Date semantics:
//!
//!   - Todoist's `due` is a structured object with `date` (date-only)
//!     and optional `datetime` (RFC 3339). We pick the more specific
//!     of the two on read and send `due_date` vs `due_datetime`
//!     accordingly on write.
//!   - Todoist added `deadline` (date-only) to the API in late 2024.
//!     We map it to `deadline_date` and accept that `deadline_time`
//!     drops on write (Todoist has no time slot for deadlines).
//!
//! Out of scope for Phase 6h.1 (logged with `tracing::warn` on
//! write so we know the field is being dropped):
//!
//!   - Recurrence (Todoist uses natural-language `due_string` like
//!     "every monday"; mapping to/from Aperio's RRULE enum would
//!     need a parser pair)
//!   - Reminders (Todoist has a separate `/reminders` endpoint that
//!     Aperio's per-task editor doesn't surface yet)
//!   - Labels (Aperio's color labels are local-only; Todoist labels
//!     are workspace-wide strings — different mental model)
//!   - Moving a task between projects (Todoist REST v2's PUT doesn't
//!     accept `project_id` either — would need the Sync API)

use cal_core::{NewTask, Section, Task, TaskList, TaskPriority, TaskStatus};
use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::api::TodoistClient;
use crate::error::TodoistResult;

// ── Public adapter-side surface ────────────────────────────────────────

/// `GET /projects`. Returns every project the authenticated user
/// owns or has joined. Todoist returns the whole list in one
/// response — no pagination.
pub async fn list_task_lists(client: &TodoistClient) -> TodoistResult<Vec<TaskList>> {
    let entries: Vec<ProjectEntry> = client.get_json("/projects").await?;
    Ok(entries.into_iter().map(map_project).collect())
}

/// `GET /tasks?project_id={id}`. Returns active (non-completed)
/// tasks. Todoist hides completed tasks behind a separate
/// `/tasks/completed/by_due_date` Sync-API surface; surfacing them
/// here is a follow-up if anyone misses them in the Aperio
/// "completed today" view.
pub async fn get_tasks(client: &TodoistClient, list_id: &str) -> TodoistResult<Vec<Task>> {
    let encoded = urlencoding(list_id);
    let path = format!("/tasks?project_id={encoded}");
    let entries: Vec<TaskEntry> = client.get_json(&path).await?;
    Ok(entries.into_iter().map(|e| map_task(e, list_id)).collect())
}

/// `GET /sections?project_id={id}`. Todoist sections are a first-class
/// resource, so unlike Vikunja there's no view indirection — one call
/// returns every section of the project in display order.
pub async fn list_sections(client: &TodoistClient, list_id: &str) -> TodoistResult<Vec<Section>> {
    let encoded = urlencoding(list_id);
    let path = format!("/sections?project_id={encoded}");
    let entries: Vec<SectionEntry> = client.get_json(&path).await?;
    Ok(entries
        .into_iter()
        .map(|e| map_section(e, list_id))
        .collect())
}

/// `POST /tasks`. Returns the freshly-created task. When the input
/// `NewTask` is already `Completed` we follow up with `/close` so
/// the round-trip preserves the status — the create endpoint
/// doesn't accept `is_completed`.
pub async fn create_task(
    client: &TodoistClient,
    list_id: &str,
    task: NewTask,
) -> TodoistResult<Task> {
    let body = new_task_to_create_body(list_id, &task);
    let entry: TaskEntry = client.post_json("/tasks", &body).await?;
    let needs_close = matches!(task.status, TaskStatus::Completed);
    let mut result = map_task(entry, list_id);
    if needs_close {
        let path = format!("/tasks/{}/close", urlencoding(&result.id));
        client.post_empty(&path).await?;
        // Mirror the close in the returned struct so the caller
        // sees a consistent picture — saves a follow-up GET.
        result.status = TaskStatus::Completed;
        result.completed_at = Some(Utc::now());
    }
    Ok(result)
}

/// `POST /tasks/{id}` with the field-update body, plus a separate
/// `/close` or `/reopen` call to bring the server's
/// `is_completed` flag in line with the desired status. Both API
/// calls are idempotent — a `/close` against an already-completed
/// task is a 204 no-op.
pub async fn update_task(client: &TodoistClient, task: &Task) -> TodoistResult<Task> {
    let encoded = urlencoding(&task.id);
    let body = task_to_update_body(task);
    let entry: TaskEntry = client
        .post_json(&format!("/tasks/{encoded}"), &body)
        .await?;
    // The update body never carries status — drive it via the
    // dedicated endpoints. We unconditionally fire either close or
    // reopen so the wire state matches `task.status` even if the
    // caller is "fixing up" a desynced row.
    let status_path = match task.status {
        TaskStatus::Completed => format!("/tasks/{encoded}/close"),
        _ => format!("/tasks/{encoded}/reopen"),
    };
    client.post_empty(&status_path).await?;
    let mut result = map_task(entry, &task.list_id);
    // The update response shows the pre-close `is_completed`
    // value; patch it locally so the caller's `Task` matches what
    // the next `get_tasks` will return.
    result.status = task.status;
    if matches!(task.status, TaskStatus::Completed) && result.completed_at.is_none() {
        result.completed_at = Some(Utc::now());
    }
    Ok(result)
}

/// `DELETE /tasks/{id}`. Todoist returns 204 on success.
pub async fn delete_task(client: &TodoistClient, task_id: &str) -> TodoistResult<()> {
    let encoded = urlencoding(task_id);
    client.delete(&format!("/tasks/{encoded}")).await
}

/// `POST /projects/{id}` with `{"name": "..."}`.
pub async fn rename_task_list(
    client: &TodoistClient,
    list_id: &str,
    new_name: &str,
) -> TodoistResult<()> {
    let encoded = urlencoding(list_id);
    let body = serde_json::json!({ "name": new_name });
    let _: serde_json::Value = client
        .post_json(&format!("/projects/{encoded}"), &body)
        .await?;
    Ok(())
}

/// `POST /projects` — create a project. `parent_id` nests it under
/// another project (Todoist's `parent_id`); `None` ⇒ top level.
/// Returns the created project mapped to a `TaskList`.
pub async fn create_task_list(
    client: &TodoistClient,
    name: &str,
    parent_id: Option<&str>,
) -> TodoistResult<TaskList> {
    let mut body = serde_json::json!({ "name": name });
    if let Some(parent) = parent_id.filter(|s| !s.is_empty()) {
        body["parent_id"] = serde_json::json!(parent);
    }
    let created: ProjectEntry = client.post_json("/projects", &body).await?;
    Ok(map_project(created))
}

/// `DELETE /projects/{id}` — remove a project (with its tasks) at the
/// source.
pub async fn delete_task_list(client: &TodoistClient, list_id: &str) -> TodoistResult<()> {
    let encoded = urlencoding(list_id);
    client.delete(&format!("/projects/{encoded}")).await
}

// ── JSON wire shapes ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ProjectEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    color: Option<String>,
    /// Parent project id for Todoist's nested-project tree. Absent /
    /// empty ⇒ a top-level project.
    #[serde(default)]
    parent_id: Option<String>,
}

/// `GET /sections?project_id={id}`. A Todoist section groups tasks
/// inside one project.
#[derive(Debug, Deserialize)]
struct SectionEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    /// Todoist's display order within the project (1-based).
    #[serde(default)]
    order: i64,
}

#[derive(Debug, Default, Deserialize)]
struct TaskEntry {
    #[serde(default)]
    id: String,
    /// Echoed back by Todoist on every task response. We pass the
    /// list id alongside through the caller's `list_id` parameter
    /// rather than trusting this field, so the value is unread —
    /// but it stays in the struct so the wire shape stays
    /// self-documenting.
    #[allow(dead_code)]
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    is_completed: bool,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    due: Option<DueEntry>,
    /// Added to the Todoist REST API in late 2024. Missing on
    /// older tasks; we tolerate either shape.
    #[serde(default)]
    deadline: Option<DeadlineEntry>,
    #[serde(default)]
    parent_id: Option<String>,
    /// Section the task is filed under, mapped to Aperio's section.
    /// Absent / empty ⇒ ungrouped.
    #[serde(default)]
    section_id: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    /// Set when the task was completed via `/close`. The REST API
    /// emits an ISO 8601 timestamp; absent for active tasks.
    #[serde(default)]
    completed_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DueEntry {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    datetime: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeadlineEntry {
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct CreateTaskBody {
    project_id: Option<String>,
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
    /// Section to file the new task under. Todoist accepts this on
    /// create; moving a task between sections afterwards needs the
    /// Sync API (same limitation as cross-project moves), so the
    /// update body omits it.
    #[serde(skip_serializing_if = "Option::is_none")]
    section_id: Option<String>,
    /// `due_date` and `due_datetime` are mutually exclusive on the
    /// Todoist side. We send at most one.
    #[serde(skip_serializing_if = "Option::is_none")]
    due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_datetime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deadline_date: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct UpdateTaskBody {
    /// `project_id` cannot be changed via this endpoint — the
    /// field is intentionally absent. Moving a task between
    /// projects requires the Sync API (out of scope for 6h.1).
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_datetime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deadline_date: Option<String>,
}

// ── Mappers ────────────────────────────────────────────────────────────

fn map_project(entry: ProjectEntry) -> TaskList {
    TaskList {
        id: entry.id,
        name: entry.name.unwrap_or_else(|| "Inbox".into()),
        // Todoist's API surfaces project colour as a named enum
        // (`berry_red`, `sky_blue`, …) — not hex. We translate to
        // Aperio's hex form via a lookup table; unknown names fall
        // through to `None` so the colour-label / container-default
        // chain can take over.
        color: entry
            .color
            .as_deref()
            .and_then(todoist_color_to_hex)
            .map(|hex| cal_core::ContainerColor::native(hex.to_string())),
        default_sound: None,
        embedded_in_calendar: None,
        // Todoist nests projects; surface the parent so the sidebar
        // builds the tree. Empty string ⇒ top-level.
        parent_id: entry.parent_id.filter(|s| !s.is_empty()),
        read_only: false,
    }
}

fn map_section(entry: SectionEntry, list_id: &str) -> Section {
    Section {
        id: entry.id,
        list_id: list_id.to_string(),
        name: entry.name.unwrap_or_default(),
        order: entry.order.max(0) as u32,
    }
}

fn map_task(entry: TaskEntry, list_id: &str) -> Task {
    let status = if entry.is_completed {
        TaskStatus::Completed
    } else {
        TaskStatus::Open
    };
    let priority = todoist_priority_to_aperio(entry.priority);
    let (scheduled_date, scheduled_time) = extract_scheduled(&entry.due);
    let deadline_date = entry
        .deadline
        .as_ref()
        .and_then(|d| d.date.as_deref())
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    let created_at = entry
        .created_at
        .as_deref()
        .and_then(parse_rfc3339)
        .unwrap_or_else(Utc::now);
    let completed_at = entry.completed_at.as_deref().and_then(parse_rfc3339);

    Task {
        id: entry.id,
        list_id: list_id.to_string(),
        title: entry.content.unwrap_or_default(),
        description: entry.description.filter(|s| !s.is_empty()),
        status,
        priority,
        scheduled_date,
        scheduled_time,
        deadline_date,
        // Todoist's deadline is date-only. We surface it as
        // date+no-time and accept that any locally-set
        // `deadline_time` is lost on the next round-trip.
        deadline_time: None,
        recurrence: None,
        parent_id: entry.parent_id.filter(|s| !s.is_empty()),
        section_id: entry.section_id.filter(|s| !s.is_empty()),
        color_label: None,
        reminders: Vec::new(),
        sound: None,
        created_at,
        // Todoist's REST v2 doesn't expose `updated_at`. Re-use
        // `created_at` so the row sorts sensibly in the local
        // cache.
        updated_at: created_at,
        completed_at,
        etag: None,
    }
}

fn new_task_to_create_body(list_id: &str, new: &NewTask) -> CreateTaskBody {
    warn_on_unsupported_fields(
        new.recurrence.is_some(),
        !new.reminders.is_empty(),
        new.parent_id.as_deref(),
    );
    let (due_date, due_datetime) = format_scheduled(new.scheduled_date, new.scheduled_time);
    CreateTaskBody {
        project_id: Some(list_id.to_string()),
        content: Some(new.title.clone()),
        description: new.description.clone().filter(|s| !s.is_empty()),
        priority: Some(aperio_priority_to_todoist(new.priority)),
        parent_id: new.parent_id.clone().filter(|s| !s.is_empty()),
        section_id: new.section_id.clone().filter(|s| !s.is_empty()),
        due_date,
        due_datetime,
        deadline_date: new.deadline_date.map(format_date),
    }
}

fn task_to_update_body(task: &Task) -> UpdateTaskBody {
    warn_on_unsupported_fields(
        task.recurrence.is_some(),
        !task.reminders.is_empty(),
        task.parent_id.as_deref(),
    );
    let (due_date, due_datetime) = format_scheduled(task.scheduled_date, task.scheduled_time);
    UpdateTaskBody {
        content: Some(task.title.clone()),
        description: task.description.clone().filter(|s| !s.is_empty()),
        priority: Some(aperio_priority_to_todoist(task.priority)),
        due_date,
        due_datetime,
        deadline_date: task.deadline_date.map(format_date),
    }
}

fn warn_on_unsupported_fields(has_recurrence: bool, has_reminders: bool, parent_id: Option<&str>) {
    if has_recurrence {
        tracing::warn!(
            "Todoist adapter dropping recurrence on write — Todoist uses natural-language due_string for recurrence, not Aperio's enum",
        );
    }
    if has_reminders {
        tracing::warn!(
            "Todoist adapter dropping reminders on write — Todoist's /reminders surface is not wired yet",
        );
    }
    if let Some(pid) = parent_id {
        if !pid.is_empty() {
            tracing::warn!(
                parent_id = pid,
                "Todoist adapter dropping subtask parent on update — moving a task between projects/parents needs the Sync API",
            );
        }
    }
}

// ── Status / priority mapping ──────────────────────────────────────────

/// Todoist's wire priority is INVERTED relative to the UI:
///
///   - `priority: 1` = "no priority" / lowest (the default)
///   - `priority: 2` = "Priority 3" in the UI
///   - `priority: 3` = "Priority 2" in the UI
///   - `priority: 4` = "Priority 1" in the UI (highest)
///
/// We collapse the bottom two bands into Aperio's Low bucket and
/// keep 3 ↔ Medium, 4 ↔ High.
fn todoist_priority_to_aperio(raw: i32) -> TaskPriority {
    match raw {
        4 => TaskPriority::High,
        3 => TaskPriority::Medium,
        _ => TaskPriority::Low,
    }
}

fn aperio_priority_to_todoist(p: TaskPriority) -> i32 {
    match p {
        TaskPriority::Low => 2,
        TaskPriority::Medium => 3,
        TaskPriority::High => 4,
    }
}

// ── Date handling ──────────────────────────────────────────────────────

fn extract_scheduled(due: &Option<DueEntry>) -> (Option<NaiveDate>, Option<NaiveTime>) {
    let Some(due) = due else {
        return (None, None);
    };
    // `datetime` is the more specific of the two; if Todoist sent
    // it, prefer it over `date`. Both fields are documented to
    // never appear together on output, but defending in depth keeps
    // the mapping clean against future schema surprises.
    if let Some(dt_raw) = due.datetime.as_deref() {
        if let Some(dt) = parse_rfc3339(dt_raw) {
            return (Some(dt.date_naive()), Some(dt.time()));
        }
    }
    if let Some(d_raw) = due.date.as_deref() {
        if let Ok(d) = NaiveDate::parse_from_str(d_raw, "%Y-%m-%d") {
            return (Some(d), None);
        }
    }
    (None, None)
}

fn format_scheduled(
    date: Option<NaiveDate>,
    time: Option<NaiveTime>,
) -> (Option<String>, Option<String>) {
    let Some(d) = date else {
        return (None, None);
    };
    match time {
        Some(t) => {
            let naive = d.and_time(t);
            let utc = Utc.from_utc_datetime(&naive);
            (
                None,
                Some(utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            )
        }
        None => (Some(format_date(d)), None),
    }
}

fn format_date(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn parse_rfc3339(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

// ── Todoist colour palette → hex ───────────────────────────────────────

/// Todoist projects carry a named colour (e.g. `berry_red`,
/// `sky_blue`) rather than a hex value. The palette is fixed and
/// documented; we translate to the hex form Aperio's
/// `ContainerColor` expects. Unknown names fall through to `None`
/// so the renderer's fallback chain (color-label → container
/// default → palette) still works.
fn todoist_color_to_hex(name: &str) -> Option<&'static str> {
    match name {
        "berry_red" => Some("#b8256f"),
        "red" => Some("#db4035"),
        "orange" => Some("#ff9933"),
        "yellow" => Some("#fad000"),
        "olive_green" => Some("#afb83b"),
        "lime_green" => Some("#7ecc49"),
        "green" => Some("#299438"),
        "mint_green" => Some("#6accbc"),
        "teal" => Some("#158fad"),
        "sky_blue" => Some("#14aaf5"),
        "light_blue" => Some("#96c3eb"),
        "blue" => Some("#4073ff"),
        "grape" => Some("#884dff"),
        "violet" => Some("#af38eb"),
        "lavender" => Some("#eb96eb"),
        "magenta" => Some("#e05194"),
        "salmon" => Some("#ff8d85"),
        "charcoal" => Some("#808080"),
        "grey" => Some("#b8b8b8"),
        "taupe" => Some("#ccac93"),
        _ => None,
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Conservative percent-encoder. Todoist IDs are short numeric
/// strings in practice (long-form snowflake-ish), so this rarely
/// substitutes anything, but the helper guards against odd values
/// in tests + future schema drift.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    fn fixture_client(server_url: &str) -> TodoistClient {
        TodoistClient::with_base_url_for_tests(
            "test-token".into(),
            reqwest::Client::new(),
            server_url.to_string(),
        )
    }

    // ── Priority mapping ───────────────────────────────────────

    #[test]
    fn priority_round_trips_through_buckets() {
        // Todoist 1 = "no priority" — collapse to Low.
        assert_eq!(todoist_priority_to_aperio(1), TaskPriority::Low);
        assert_eq!(todoist_priority_to_aperio(2), TaskPriority::Low);
        assert_eq!(todoist_priority_to_aperio(3), TaskPriority::Medium);
        assert_eq!(todoist_priority_to_aperio(4), TaskPriority::High);

        assert_eq!(aperio_priority_to_todoist(TaskPriority::Low), 2);
        assert_eq!(aperio_priority_to_todoist(TaskPriority::Medium), 3);
        assert_eq!(aperio_priority_to_todoist(TaskPriority::High), 4);
    }

    // ── Date mapping ───────────────────────────────────────────

    #[test]
    fn extract_scheduled_prefers_datetime_over_date() {
        let due = Some(DueEntry {
            date: Some("2026-05-22".into()),
            datetime: Some("2026-05-22T14:30:00Z".into()),
        });
        let (d, t) = extract_scheduled(&due);
        assert_eq!(d, Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()));
        assert_eq!(t, Some(NaiveTime::from_hms_opt(14, 30, 0).unwrap()));
    }

    #[test]
    fn extract_scheduled_falls_back_to_date_only() {
        let due = Some(DueEntry {
            date: Some("2026-05-22".into()),
            datetime: None,
        });
        let (d, t) = extract_scheduled(&due);
        assert_eq!(d, Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()));
        assert!(t.is_none());
    }

    #[test]
    fn extract_scheduled_handles_absent_due() {
        assert_eq!(extract_scheduled(&None), (None, None));
    }

    #[test]
    fn format_scheduled_chooses_date_when_time_absent() {
        let d = Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap());
        let (date_field, dt_field) = format_scheduled(d, None);
        assert_eq!(date_field.as_deref(), Some("2026-05-22"));
        assert!(dt_field.is_none());
    }

    #[test]
    fn format_scheduled_chooses_datetime_when_time_present() {
        let d = Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap());
        let t = Some(NaiveTime::from_hms_opt(14, 30, 0).unwrap());
        let (date_field, dt_field) = format_scheduled(d, t);
        assert!(date_field.is_none());
        assert_eq!(dt_field.as_deref(), Some("2026-05-22T14:30:00Z"));
    }

    #[test]
    fn format_scheduled_returns_pair_of_nones_without_date() {
        assert_eq!(format_scheduled(None, None), (None, None));
        // Time alone is meaningless without a date — defence in
        // depth: the wire shape would be rejected by Todoist anyway.
        let t = Some(NaiveTime::from_hms_opt(14, 30, 0).unwrap());
        assert_eq!(format_scheduled(None, t), (None, None));
    }

    // ── Colour palette ─────────────────────────────────────────

    #[test]
    fn color_palette_maps_common_names() {
        assert_eq!(todoist_color_to_hex("berry_red"), Some("#b8256f"));
        assert_eq!(todoist_color_to_hex("sky_blue"), Some("#14aaf5"));
        assert_eq!(todoist_color_to_hex("grey"), Some("#b8b8b8"));
    }

    #[test]
    fn color_palette_yields_none_for_unknown() {
        assert!(todoist_color_to_hex("not_a_real_color").is_none());
        assert!(todoist_color_to_hex("").is_none());
    }

    // ── map_project ────────────────────────────────────────────

    #[test]
    fn map_project_carries_translated_color() {
        let list = map_project(ProjectEntry {
            id: "1234".into(),
            name: Some("Work".into()),
            color: Some("sky_blue".into()),
            parent_id: None,
        });
        assert_eq!(list.id, "1234");
        assert_eq!(list.name, "Work");
        assert_eq!(list.color.unwrap().hex, "#14aaf5");
    }

    #[test]
    fn map_project_drops_unknown_color() {
        let list = map_project(ProjectEntry {
            id: "1".into(),
            name: Some("Stuff".into()),
            color: Some("not_in_palette".into()),
            parent_id: None,
        });
        assert!(list.color.is_none());
    }

    #[test]
    fn map_project_defaults_name() {
        let list = map_project(ProjectEntry {
            id: "1".into(),
            name: None,
            color: None,
            parent_id: None,
        });
        assert_eq!(list.name, "Inbox");
    }

    // ── map_task ───────────────────────────────────────────────

    #[test]
    fn map_task_translates_priority_and_deadline() {
        let entry = TaskEntry {
            id: "T1".into(),
            project_id: "P1".into(),
            content: Some("Submit invoice".into()),
            description: Some("Q2 client".into()),
            is_completed: false,
            priority: 4,
            due: Some(DueEntry {
                date: Some("2026-05-22".into()),
                datetime: None,
            }),
            deadline: Some(DeadlineEntry {
                date: Some("2026-05-25".into()),
            }),
            parent_id: None,
            section_id: None,
            created_at: Some("2026-05-01T10:00:00Z".into()),
            completed_at: None,
        };
        let task = map_task(entry, "P1");
        assert_eq!(task.id, "T1");
        assert_eq!(task.list_id, "P1");
        assert_eq!(task.title, "Submit invoice");
        assert_eq!(task.description.as_deref(), Some("Q2 client"));
        assert_eq!(task.status, TaskStatus::Open);
        assert_eq!(task.priority, TaskPriority::High);
        assert_eq!(
            task.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
        );
        assert!(task.scheduled_time.is_none());
        assert_eq!(
            task.deadline_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 25).unwrap()),
        );
        assert!(task.deadline_time.is_none());
    }

    #[test]
    fn map_task_marks_completed_with_timestamp() {
        let entry = TaskEntry {
            id: "T1".into(),
            project_id: "P1".into(),
            content: Some("Done".into()),
            description: None,
            is_completed: true,
            priority: 1,
            due: None,
            deadline: None,
            parent_id: None,
            section_id: None,
            created_at: Some("2026-05-01T10:00:00Z".into()),
            completed_at: Some("2026-05-22T15:00:00Z".into()),
        };
        let task = map_task(entry, "P1");
        assert_eq!(task.status, TaskStatus::Completed);
        assert!(task.completed_at.is_some());
        // priority 1 = "no priority" — collapses to Low.
        assert_eq!(task.priority, TaskPriority::Low);
    }

    #[test]
    fn map_task_handles_timed_due() {
        let entry = TaskEntry {
            id: "T1".into(),
            project_id: "P1".into(),
            content: Some("Standup".into()),
            description: None,
            is_completed: false,
            priority: 1,
            due: Some(DueEntry {
                date: Some("2026-05-22".into()),
                datetime: Some("2026-05-22T09:30:00Z".into()),
            }),
            deadline: None,
            parent_id: None,
            section_id: None,
            created_at: None,
            completed_at: None,
        };
        let task = map_task(entry, "P1");
        assert_eq!(
            task.scheduled_time,
            Some(NaiveTime::from_hms_opt(9, 30, 0).unwrap()),
        );
    }

    // ── Body shape (NewTask → wire) ────────────────────────────

    fn sample_new_task() -> NewTask {
        NewTask {
            title: "Buy bread".into(),
            description: Some("Bakery".into()),
            status: TaskStatus::Open,
            priority: TaskPriority::High,
            scheduled_date: Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
            scheduled_time: None,
            deadline_date: Some(NaiveDate::from_ymd_opt(2026, 5, 23).unwrap()),
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
        }
    }

    #[test]
    fn create_body_carries_scheduled_as_due_date() {
        let body = new_task_to_create_body("P1", &sample_new_task());
        assert_eq!(body.project_id.as_deref(), Some("P1"));
        assert_eq!(body.content.as_deref(), Some("Buy bread"));
        assert_eq!(body.priority, Some(4));
        assert_eq!(body.due_date.as_deref(), Some("2026-05-22"));
        assert!(body.due_datetime.is_none());
        assert_eq!(body.deadline_date.as_deref(), Some("2026-05-23"));
    }

    #[test]
    fn create_body_uses_datetime_when_time_set() {
        let mut nt = sample_new_task();
        nt.scheduled_time = Some(NaiveTime::from_hms_opt(8, 0, 0).unwrap());
        let body = new_task_to_create_body("P1", &nt);
        assert!(body.due_date.is_none());
        assert_eq!(body.due_datetime.as_deref(), Some("2026-05-22T08:00:00Z"));
    }

    #[test]
    fn create_body_omits_due_fields_when_dates_absent() {
        let mut nt = sample_new_task();
        nt.scheduled_date = None;
        nt.scheduled_time = None;
        nt.deadline_date = None;
        let body = new_task_to_create_body("P1", &nt);
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("due_date").is_none());
        assert!(json.get("due_datetime").is_none());
        assert!(json.get("deadline_date").is_none());
    }

    // ── End-to-end via mockito ────────────────────────────────

    #[tokio::test]
    async fn list_task_lists_decodes_projects() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/projects")
            .with_status(200)
            .with_body(
                r#"[{"id":"1","name":"Inbox","color":"grey"},{"id":"2","name":"Work","color":"sky_blue"}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let lists = list_task_lists(&client).await.unwrap();
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].id, "1");
        assert_eq!(lists[0].color.as_ref().unwrap().hex, "#b8b8b8");
        assert_eq!(lists[1].id, "2");
        assert_eq!(lists[1].color.as_ref().unwrap().hex, "#14aaf5");
    }

    #[tokio::test]
    async fn get_tasks_filters_by_project() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/tasks?project_id=P1")
            .with_status(200)
            .with_body(
                r#"[{"id":"T1","project_id":"P1","content":"One","is_completed":false,"priority":3}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let tasks = get_tasks(&client, "P1").await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "T1");
        assert_eq!(tasks[0].priority, TaskPriority::Medium);
    }

    #[tokio::test]
    async fn delete_task_round_trips_against_204() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("DELETE", "/tasks/T1")
            .with_status(204)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        delete_task(&client, "T1").await.unwrap();
    }

    #[tokio::test]
    async fn delete_task_surfaces_404_as_http_error() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("DELETE", "/tasks/missing")
            .with_status(404)
            .with_body(r#"{"error":"Task not found"}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let err = delete_task(&client, "missing").await.unwrap_err();
        match err {
            crate::error::TodoistError::Http { status, .. } => {
                assert_eq!(status, 404);
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unauthorised_surfaces_as_http_401() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/projects")
            .with_status(401)
            .with_body(r#"{"error":"Forbidden","error_tag":"AUTH_INVALID_TOKEN"}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let err = list_task_lists(&client).await.unwrap_err();
        match err {
            crate::error::TodoistError::Http { status, .. } => {
                assert_eq!(status, 401);
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_task_calls_close_when_status_completed() {
        let mut server = Server::new_async().await;
        let post = server
            .mock("POST", "/tasks")
            .with_status(200)
            .with_body(
                r#"{"id":"T1","project_id":"P1","content":"Buy bread","is_completed":false,"priority":4}"#,
            )
            .create_async()
            .await;
        let close = server
            .mock("POST", "/tasks/T1/close")
            .with_status(204)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let mut nt = sample_new_task();
        nt.status = TaskStatus::Completed;
        let task = create_task(&client, "P1", nt).await.unwrap();
        post.assert_async().await;
        close.assert_async().await;
        assert_eq!(task.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn update_task_fires_close_when_status_completed() {
        let mut server = Server::new_async().await;
        let post = server
            .mock("POST", "/tasks/T1")
            .with_status(200)
            .with_body(
                r#"{"id":"T1","project_id":"P1","content":"Buy bread","is_completed":false,"priority":3}"#,
            )
            .create_async()
            .await;
        let close = server
            .mock("POST", "/tasks/T1/close")
            .with_status(204)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let task = Task {
            id: "T1".into(),
            list_id: "P1".into(),
            title: "Buy bread".into(),
            description: None,
            status: TaskStatus::Completed,
            priority: TaskPriority::Medium,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: None,
            etag: None,
        };
        let result = update_task(&client, &task).await.unwrap();
        post.assert_async().await;
        close.assert_async().await;
        assert_eq!(result.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn update_task_fires_reopen_when_status_open() {
        let mut server = Server::new_async().await;
        let post = server
            .mock("POST", "/tasks/T1")
            .with_status(200)
            .with_body(
                r#"{"id":"T1","project_id":"P1","content":"Buy bread","is_completed":true,"priority":3}"#,
            )
            .create_async()
            .await;
        let reopen = server
            .mock("POST", "/tasks/T1/reopen")
            .with_status(204)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let task = Task {
            id: "T1".into(),
            list_id: "P1".into(),
            title: "Buy bread".into(),
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
            reminders: Vec::new(),
            sound: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: None,
            etag: None,
        };
        let result = update_task(&client, &task).await.unwrap();
        post.assert_async().await;
        reopen.assert_async().await;
        assert_eq!(result.status, TaskStatus::Open);
    }

    // ── Nested projects + sections ─────────────────────────────

    #[test]
    fn map_project_surfaces_parent_id() {
        let top = map_project(ProjectEntry {
            id: "1".into(),
            name: Some("Parent".into()),
            color: None,
            parent_id: None,
        });
        assert!(top.parent_id.is_none());
        let child = map_project(ProjectEntry {
            id: "2".into(),
            name: Some("Child".into()),
            color: None,
            parent_id: Some("1".into()),
        });
        assert_eq!(child.parent_id.as_deref(), Some("1"));
    }

    #[test]
    fn map_task_carries_section_id() {
        let entry = TaskEntry {
            id: "T1".into(),
            project_id: "P1".into(),
            content: Some("Grouped".into()),
            parent_id: None,
            section_id: Some("S9".into()),
            ..Default::default()
        };
        assert_eq!(map_task(entry, "P1").section_id.as_deref(), Some("S9"));
        // Empty string ⇒ ungrouped.
        let loose = TaskEntry {
            id: "T2".into(),
            section_id: Some(String::new()),
            ..Default::default()
        };
        assert!(map_task(loose, "P1").section_id.is_none());
    }

    #[test]
    fn create_body_carries_section_id() {
        let mut nt = sample_new_task();
        nt.section_id = Some("S9".into());
        let body = new_task_to_create_body("P1", &nt);
        assert_eq!(body.section_id.as_deref(), Some("S9"));
        // Absent section ⇒ field omitted from the wire body.
        let plain = new_task_to_create_body("P1", &sample_new_task());
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("section_id").is_none());
    }

    #[tokio::test]
    async fn list_sections_decodes_project_sections() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/sections?project_id=P1")
            .with_status(200)
            .with_body(
                r#"[{"id":"S1","project_id":"P1","order":1,"name":"To Do"},{"id":"S2","project_id":"P1","order":2,"name":"Doing"}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let sections = list_sections(&client, "P1").await.unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].id, "S1");
        assert_eq!(sections[0].name, "To Do");
        assert_eq!(sections[0].list_id, "P1");
        assert_eq!(sections[1].order, 2);
    }

    #[tokio::test]
    async fn create_task_list_posts_project_and_maps_it() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("POST", "/projects")
            .with_status(200)
            .with_body(r#"{"id":"P9","name":"New Project","parent_id":"P1"}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let list = create_task_list(&client, "New Project", Some("P1"))
            .await
            .unwrap();
        assert_eq!(list.id, "P9");
        assert_eq!(list.name, "New Project");
        assert_eq!(list.parent_id.as_deref(), Some("P1"));
    }

    #[tokio::test]
    async fn delete_task_list_hits_delete_endpoint() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("DELETE", "/projects/P7")
            .with_status(204)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        delete_task_list(&client, "P7").await.unwrap();
        m.assert_async().await;
    }
}
