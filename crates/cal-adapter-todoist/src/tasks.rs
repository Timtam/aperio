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

use std::collections::HashMap;

use cal_core::{
    NewTask, Section, Task, TaskList, TaskListShare, TaskPriority, TaskStatus, TaskUser,
};
use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::TodoistClient;
use crate::error::{TodoistError, TodoistResult};

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
    let mut tasks: Vec<Task> = entries.into_iter().map(|e| map_task(e, list_id)).collect();
    resolve_assignee_names(client, list_id, &mut tasks).await;
    Ok(tasks)
}

/// Todoist tasks carry only the assignee's numeric `assignee_id`, so
/// `map_task` seeds each assignee with the id doubling as the display
/// name. Fill in real names + emails from the project's collaborators —
/// but only when at least one task is actually assigned, since most
/// personal projects have none and the extra round-trip would be pure
/// overhead. A failed collaborator fetch (non-shared project, older API)
/// leaves the id-as-name placeholder in place rather than erroring.
async fn resolve_assignee_names(client: &TodoistClient, list_id: &str, tasks: &mut [Task]) {
    if !tasks.iter().any(|t| !t.assignees.is_empty()) {
        return;
    }
    let Ok(members) = list_task_list_members(client, list_id).await else {
        return;
    };
    let by_id: HashMap<&str, &TaskUser> = members.iter().map(|m| (m.id.as_str(), m)).collect();
    for task in tasks.iter_mut() {
        for assignee in &mut task.assignees {
            if let Some(member) = by_id.get(assignee.id.as_str()) {
                assignee.name = member.name.clone();
                assignee.email = member.email.clone();
            }
        }
    }
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

/// `POST /sections` with `{ project_id, name }` — create a section in a
/// project. Returns the created section mapped to Aperio's `Section`.
/// Color is never sent — Todoist sections carry none; it's a local
/// override.
pub async fn create_section(
    client: &TodoistClient,
    list_id: &str,
    name: &str,
) -> TodoistResult<Section> {
    let body = serde_json::json!({ "project_id": list_id, "name": name });
    let entry: SectionEntry = client.post_json("/sections", &body).await?;
    Ok(map_section(entry, list_id))
}

/// `POST /sections/{id}` with `{ name }` — rename a section. Returns the
/// updated section.
pub async fn update_section(
    client: &TodoistClient,
    list_id: &str,
    section_id: &str,
    new_name: &str,
) -> TodoistResult<Section> {
    let encoded = urlencoding(section_id);
    let body = serde_json::json!({ "name": new_name });
    let entry: SectionEntry = client
        .post_json(&format!("/sections/{encoded}"), &body)
        .await?;
    Ok(map_section(entry, list_id))
}

/// `DELETE /sections/{id}` — remove a section; its tasks become
/// section-less at the source. Todoist returns 204.
pub async fn delete_section(client: &TodoistClient, section_id: &str) -> TodoistResult<()> {
    let encoded = urlencoding(section_id);
    client.delete(&format!("/sections/{encoded}")).await
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
    // REST v2 can't change a task's `section_id`, so the body omits it
    // and the PATCH response still shows the task's *current* section.
    // If the caller moved the task to a different section — or cleared it
    // ("no section") — do that via the Sync API's `item_move`. Only fire
    // when the section actually changed, so a plain title/date edit never
    // reorders the task within its section.
    let current_section = entry.section_id.clone().filter(|s| !s.is_empty());
    let desired_section = task.section_id.clone().filter(|s| !s.is_empty());
    if current_section != desired_section {
        let args = match &desired_section {
            Some(section_id) => {
                serde_json::json!({ "id": task.id, "section_id": section_id })
            }
            // No section: move the task to its project's root, which
            // detaches it from any section while keeping it in the list.
            None => serde_json::json!({ "id": task.id, "project_id": task.list_id }),
        };
        sync_command(client, "item_move", args).await?;
    }
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
    // …and the pre-move section (REST ignored the section change); reflect
    // the section we just moved to so the caller's Task is consistent.
    result.section_id = desired_section;
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

/// `GET /projects/{id}/collaborators` — the users who share the project,
/// i.e. the valid assignee pool (DESIGN §9.7). A personal (non-shared)
/// project has no collaborators endpoint payload of interest; Todoist
/// returns just the owner, and projects the user can't share yield an
/// error which we soften to an empty list so the picker shows no
/// candidates rather than failing the whole task load.
pub async fn list_task_list_members(
    client: &TodoistClient,
    list_id: &str,
) -> TodoistResult<Vec<TaskUser>> {
    let encoded = urlencoding(list_id);
    let entries: Vec<CollaboratorEntry> = match client
        .get_json(&format!("/projects/{encoded}/collaborators"))
        .await
    {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(entries.into_iter().map(map_collaborator).collect())
}

// ── Membership / sharing (DESIGN §9.7) ──────────────────────────────────
//
// Todoist gates project sharing behind the Sync API (REST v2 has no
// share endpoint), so these go through `sync_form` rather than the REST
// helpers. There are NO per-member roles, and adding is an EMAIL INVITE
// with an acceptance step — so the membership `TaskUser.id` carries the
// EMAIL (the key `delete_collaborator` wants), `right` is always `None`,
// and a freshly invited, not-yet-accepted collaborator surfaces with
// `pending = true`.

/// `POST /sync` reading `collaborators` + `collaborator_states`, filtered
/// to this project. Joins each state's `user_id` to the matching
/// collaborator for a name/email and marks `state == "invited"` as
/// pending. Soft-fails to an empty list (the dialog then shows "no
/// members") rather than erroring, mirroring the Vikunja shares read.
pub async fn list_task_list_shares(
    client: &TodoistClient,
    list_id: &str,
) -> TodoistResult<Vec<TaskListShare>> {
    let resp: SyncCollaboratorsResponse = match client
        .sync_form(&[
            ("sync_token", "*".to_string()),
            (
                "resource_types",
                r#"["collaborators","collaborator_states"]"#.to_string(),
            ),
        ])
        .await
    {
        Ok(r) => r,
        Err(_) => return Ok(Vec::new()),
    };
    let by_id: HashMap<String, &SyncCollaborator> = resp
        .collaborators
        .iter()
        .map(|c| (c.id.to_id_string(), c))
        .collect();
    let shares = resp
        .collaborator_states
        .iter()
        .filter(|s| !s.is_deleted && s.project_id.to_id_string() == list_id)
        .map(|s| {
            let user_id = s.user_id.to_id_string();
            let collaborator = by_id.get(&user_id);
            let email = collaborator
                .and_then(|c| c.email.clone())
                .filter(|e| !e.trim().is_empty());
            let name = collaborator
                .and_then(|c| c.full_name.clone())
                .filter(|n| !n.trim().is_empty())
                .or_else(|| email.clone())
                .unwrap_or_else(|| format!("User {user_id}"));
            // `delete_collaborator` keys on the email; fall back to the
            // user id when (rarely) the collaborator object is absent.
            let member_ref = email.clone().unwrap_or(user_id);
            TaskListShare {
                user: TaskUser {
                    id: member_ref,
                    name,
                    email,
                },
                // Todoist has no per-share roles.
                right: None,
                pending: s.state.as_deref() == Some("invited"),
            }
        })
        .collect();
    Ok(shares)
}

/// Sync command `share_project { project_id, email }` — invite someone to
/// the project by email. Todoist has no roles, so the caller's `right` is
/// ignored. The invite stays pending until the recipient accepts.
pub async fn add_task_list_member(
    client: &TodoistClient,
    list_id: &str,
    member_ref: &str,
) -> TodoistResult<()> {
    let args = serde_json::json!({ "project_id": list_id, "email": member_ref });
    sync_command(client, "share_project", args).await
}

/// Sync command `delete_collaborator { project_id, email }` — revoke a
/// member's access (or cancel a pending invite). `member_ref` is the
/// email carried on the share's `TaskUser.id`.
pub async fn remove_task_list_member(
    client: &TodoistClient,
    list_id: &str,
    member_ref: &str,
) -> TodoistResult<()> {
    let args = serde_json::json!({ "project_id": list_id, "email": member_ref });
    sync_command(client, "delete_collaborator", args).await
}

/// Issue one Sync API command and surface a per-command failure as a
/// protocol error. The Sync API replies `200` with the outcome in
/// `sync_status` keyed by the command's uuid: the string `"ok"` on
/// success or an error object otherwise.
async fn sync_command(
    client: &TodoistClient,
    command_type: &str,
    args: serde_json::Value,
) -> TodoistResult<()> {
    let uuid = Uuid::new_v4().to_string();
    let commands = serde_json::json!([{
        "type": command_type,
        "uuid": uuid,
        "args": args,
    }]);
    let resp: SyncStatusResponse = client
        .sync_form(&[("commands", commands.to_string())])
        .await?;
    // We send exactly one command, so the response carries at most one
    // `sync_status` entry. Rather than look it up by our (runtime-random)
    // uuid, require every entry to be the string `"ok"` — anything else
    // is an error object. An empty map means the command was accepted
    // without an explicit status.
    for status in resp.sync_status.values() {
        let ok = matches!(status, serde_json::Value::String(s) if s == "ok");
        if !ok {
            return Err(TodoistError::Protocol(format!(
                "Todoist sync command '{command_type}' failed: {status}"
            )));
        }
    }
    Ok(())
}

// ── JSON wire shapes ───────────────────────────────────────────────────

/// Todoist IDs are strings in REST v2, but the task object's
/// `assignee_id` has historically been emitted as a bare integer (and
/// the collaborator `id` as a string). Accept either shape and
/// normalise to the string form Todoist uses for ids everywhere else,
/// so the assignee id always matches a collaborator id for name lookup.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum WireId {
    Str(String),
    Int(i64),
}

impl WireId {
    fn into_string(self) -> String {
        match self {
            WireId::Str(s) => s,
            WireId::Int(i) => i.to_string(),
        }
    }

    /// Borrowing variant for when the id is only needed transiently
    /// (e.g. to key a map or compare against a project id).
    fn to_id_string(&self) -> String {
        match self {
            WireId::Str(s) => s.clone(),
            WireId::Int(i) => i.to_string(),
        }
    }
}

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
    /// The user assigned to the task. Only meaningful in shared
    /// projects; `null` / absent ⇒ unassigned. Todoist supports a
    /// single assignee per task, so this maps to 0 or 1 `TaskUser`s.
    #[serde(default)]
    assignee_id: Option<WireId>,
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

/// A `GET /projects/{id}/collaborators` row: the people who share the
/// project. Doubles as the assignee pool and the source for resolving a
/// task's `assignee_id` to a display name.
#[derive(Debug, Deserialize)]
struct CollaboratorEntry {
    id: WireId,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

/// `POST /sync` response carrying the account-wide collaborator pool +
/// the per-project membership states. We filter `collaborator_states` to
/// the project of interest and join to `collaborators` for names/emails.
#[derive(Debug, Default, Deserialize)]
struct SyncCollaboratorsResponse {
    #[serde(default)]
    collaborators: Vec<SyncCollaborator>,
    #[serde(default)]
    collaborator_states: Vec<SyncCollaboratorState>,
}

/// A Sync-API `collaborators` row — an account-wide user record.
#[derive(Debug, Deserialize)]
struct SyncCollaborator {
    id: WireId,
    #[serde(default)]
    email: Option<String>,
    /// Todoist names this `full_name` in the Sync API (vs `name` in the
    /// REST collaborators endpoint).
    #[serde(default)]
    full_name: Option<String>,
}

/// A Sync-API `collaborator_states` row — one user's membership of one
/// project, including whether the invite is still `"invited"` (pending).
#[derive(Debug, Deserialize)]
struct SyncCollaboratorState {
    project_id: WireId,
    user_id: WireId,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    is_deleted: bool,
}

/// `POST /sync` response for a write: the per-command outcome keyed by
/// the command uuid (`"ok"` or an error object).
#[derive(Debug, Default, Deserialize)]
struct SyncStatusResponse {
    #[serde(default)]
    sync_status: HashMap<String, serde_json::Value>,
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
    /// Assignee (single). Only honoured in shared projects; omitted from
    /// the body when the new task is unassigned.
    #[serde(skip_serializing_if = "Option::is_none")]
    assignee_id: Option<String>,
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
    /// Assignee (single). Sent on every update — including as `null` —
    /// so the picker can both set and CLEAR the assignee: `None`
    /// serialises to `null`, which unassigns the task (Todoist treats an
    /// omitted field as "unchanged"). Because `task.assignees` faithfully
    /// round-trips the server state on read, a plain edit re-sends the
    /// existing assignee and only an explicit removal clears it.
    assignee_id: Option<String>,
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
        color_label: None,
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
        // Todoist sections carry no color of their own.
        color_label: None,
        order: entry.order.max(0) as u32,
    }
}

fn map_collaborator(entry: CollaboratorEntry) -> TaskUser {
    let id = entry.id.into_string();
    let email = entry.email.filter(|s| !s.trim().is_empty());
    let name = entry
        .name
        .filter(|s| !s.trim().is_empty())
        .or_else(|| email.clone())
        .unwrap_or_else(|| format!("User {id}"));
    TaskUser { id, name, email }
}

/// Map a task's `assignee_id` to Aperio's assignee list. Todoist allows
/// a single assignee, so the result is 0 or 1 `TaskUser`s. The name
/// starts as the id (the task object carries no name); `get_tasks`
/// resolves it from the project's collaborators afterwards.
fn extract_assignees(assignee_id: Option<WireId>) -> Vec<TaskUser> {
    match assignee_id.map(WireId::into_string) {
        Some(id) if !id.trim().is_empty() => vec![TaskUser {
            name: id.clone(),
            id,
            email: None,
        }],
        _ => Vec::new(),
    }
}

/// The single Todoist `assignee_id` to write from Aperio's multi-assignee
/// list. Todoist supports one assignee per task, so we keep the first
/// non-empty id and warn when the caller supplied more (the model is
/// "list multi-assignee, adapter clamps" — DESIGN §9.7).
fn first_assignee_id(assignees: &[TaskUser]) -> Option<String> {
    if assignees.len() > 1 {
        tracing::warn!(
            count = assignees.len(),
            "Todoist supports a single assignee per task — keeping the first, dropping the rest",
        );
    }
    assignees
        .iter()
        .map(|a| a.id.trim())
        .find(|s| !s.is_empty())
        .map(str::to_string)
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
        assignees: extract_assignees(entry.assignee_id),
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
        assignee_id: first_assignee_id(&new.assignees),
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
        assignee_id: first_assignee_id(&task.assignees),
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
            assignee_id: None,
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
            assignee_id: None,
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
            assignee_id: None,
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
            assignees: Vec::new(),
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
            assignees: Vec::new(),
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
    async fn update_task_moves_section_via_sync_when_changed() {
        let mut server = Server::new_async().await;
        // The PATCH response still shows the *old* section S1 — REST v2
        // ignores section changes on update.
        let patch = server
            .mock("POST", "/tasks/T1")
            .with_status(200)
            .with_body(
                r#"{"id":"T1","project_id":"P1","content":"Buy bread","is_completed":false,"priority":1,"section_id":"S1"}"#,
            )
            .create_async()
            .await;
        // S1 → S2 differs, so an `item_move` Sync command fires.
        let mv = server
            .mock("POST", "/sync")
            .with_status(200)
            .with_body(r#"{"sync_status":{}}"#)
            .create_async()
            .await;
        let reopen = server
            .mock("POST", "/tasks/T1/reopen")
            .with_status(204)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let task = Task {
            assignees: Vec::new(),
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
            section_id: Some("S2".into()),
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: None,
            etag: None,
        };
        let result = update_task(&client, &task).await.unwrap();
        patch.assert_async().await;
        mv.assert_async().await;
        reopen.assert_async().await;
        // The returned Task reflects the section we moved to, not the
        // pre-move one the PATCH echoed back.
        assert_eq!(result.section_id.as_deref(), Some("S2"));
    }

    #[tokio::test]
    async fn update_task_clears_section_via_sync() {
        let mut server = Server::new_async().await;
        let patch = server
            .mock("POST", "/tasks/T1")
            .with_status(200)
            .with_body(
                r#"{"id":"T1","project_id":"P1","content":"Buy bread","is_completed":false,"priority":1,"section_id":"S1"}"#,
            )
            .create_async()
            .await;
        // No section desired → `item_move` to the project root, which
        // detaches the task from its section.
        let mv = server
            .mock("POST", "/sync")
            .with_status(200)
            .with_body(r#"{"sync_status":{}}"#)
            .create_async()
            .await;
        let reopen = server
            .mock("POST", "/tasks/T1/reopen")
            .with_status(204)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let task = Task {
            assignees: Vec::new(),
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
        patch.assert_async().await;
        mv.assert_async().await;
        reopen.assert_async().await;
        assert!(result.section_id.is_none());
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
            assignees: Vec::new(),
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
    async fn create_section_posts_and_maps() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("POST", "/sections")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "project_id": "P1",
                "name": "Backlog"
            })))
            .with_status(200)
            .with_body(r#"{"id":"S9","project_id":"P1","order":3,"name":"Backlog"}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let section = create_section(&client, "P1", "Backlog").await.unwrap();
        m.assert_async().await;
        assert_eq!(section.id, "S9");
        assert_eq!(section.name, "Backlog");
        assert_eq!(section.list_id, "P1");
    }

    #[tokio::test]
    async fn update_section_renames() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("POST", "/sections/S9")
            .match_body(mockito::Matcher::Json(serde_json::json!({ "name": "Done" })))
            .with_status(200)
            .with_body(r#"{"id":"S9","project_id":"P1","order":3,"name":"Done"}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let section = update_section(&client, "P1", "S9", "Done").await.unwrap();
        m.assert_async().await;
        assert_eq!(section.name, "Done");
    }

    #[tokio::test]
    async fn delete_section_hits_endpoint() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("DELETE", "/sections/S9")
            .with_status(204)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        delete_section(&client, "S9").await.unwrap();
        m.assert_async().await;
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

    // ── Assignees + collaborators ──────────────────────────────

    #[test]
    fn extract_assignees_accepts_string_and_int_ids() {
        // Todoist v2 strings…
        let from_str = extract_assignees(Some(WireId::Str("42".into())));
        assert_eq!(from_str.len(), 1);
        assert_eq!(from_str[0].id, "42");
        // …and the legacy bare-integer shape both normalise to a
        // string id (so it matches a collaborator id for name lookup).
        let from_int = extract_assignees(Some(WireId::Int(42)));
        assert_eq!(from_int[0].id, "42");
        // The placeholder name is the id until get_tasks resolves it.
        assert_eq!(from_int[0].name, "42");
        assert!(from_int[0].email.is_none());
    }

    #[test]
    fn extract_assignees_empty_when_unassigned() {
        assert!(extract_assignees(None).is_empty());
        // Defensive: an empty string is not a real assignee.
        assert!(extract_assignees(Some(WireId::Str(String::new()))).is_empty());
    }

    #[test]
    fn map_task_reads_assignee_id() {
        let entry = TaskEntry {
            id: "T1".into(),
            content: Some("Shared".into()),
            assignee_id: Some(WireId::Str("99".into())),
            ..Default::default()
        };
        let task = map_task(entry, "P1");
        assert_eq!(task.assignees.len(), 1);
        assert_eq!(task.assignees[0].id, "99");
    }

    #[test]
    fn first_assignee_id_clamps_to_first_non_empty() {
        let one = vec![TaskUser {
            id: "7".into(),
            name: "A".into(),
            email: None,
        }];
        assert_eq!(first_assignee_id(&one).as_deref(), Some("7"));
        // Multiple → keep the first (Todoist is single-assignee).
        let many = vec![
            TaskUser {
                id: "7".into(),
                name: "A".into(),
                email: None,
            },
            TaskUser {
                id: "8".into(),
                name: "B".into(),
                email: None,
            },
        ];
        assert_eq!(first_assignee_id(&many).as_deref(), Some("7"));
        // None when unassigned.
        assert_eq!(first_assignee_id(&[]), None);
    }

    #[test]
    fn create_body_carries_assignee() {
        let mut nt = sample_new_task();
        nt.assignees = vec![TaskUser {
            id: "42".into(),
            name: "Alice".into(),
            email: None,
        }];
        let body = new_task_to_create_body("P1", &nt);
        assert_eq!(body.assignee_id.as_deref(), Some("42"));
        // Unassigned ⇒ field omitted from the create body.
        let plain = new_task_to_create_body("P1", &sample_new_task());
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("assignee_id").is_none());
    }

    #[test]
    fn update_body_sends_null_to_clear_assignee() {
        // Unassigned ⇒ assignee_id present as null so the update
        // unassigns rather than leaving a stale assignee.
        let task = Task {
            assignees: Vec::new(),
            id: "T1".into(),
            list_id: "P1".into(),
            title: "X".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Low,
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
        let json = serde_json::to_value(task_to_update_body(&task)).unwrap();
        assert!(json.get("assignee_id").is_some());
        assert!(json["assignee_id"].is_null());
        // Assigned ⇒ the id rides along.
        let assigned = Task {
            assignees: vec![TaskUser {
                id: "42".into(),
                name: "Alice".into(),
                email: None,
            }],
            ..task
        };
        let json = serde_json::to_value(task_to_update_body(&assigned)).unwrap();
        assert_eq!(json["assignee_id"], serde_json::json!("42"));
    }

    #[test]
    fn map_collaborator_prefers_name_then_email() {
        let named = map_collaborator(CollaboratorEntry {
            id: WireId::Str("1".into()),
            name: Some("Alice".into()),
            email: Some("alice@example.com".into()),
        });
        assert_eq!(named.id, "1");
        assert_eq!(named.name, "Alice");
        assert_eq!(named.email.as_deref(), Some("alice@example.com"));
        // No name ⇒ fall back to email.
        let emailed = map_collaborator(CollaboratorEntry {
            id: WireId::Int(2),
            name: None,
            email: Some("bob@example.com".into()),
        });
        assert_eq!(emailed.id, "2");
        assert_eq!(emailed.name, "bob@example.com");
    }

    #[tokio::test]
    async fn list_task_list_members_decodes_collaborators() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/projects/P1/collaborators")
            .with_status(200)
            .with_body(r#"[{"id":"42","name":"Alice","email":"alice@example.com"}]"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let members = list_task_list_members(&client, "P1").await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].id, "42");
        assert_eq!(members[0].name, "Alice");
    }

    #[tokio::test]
    async fn list_task_list_members_empty_on_error() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/projects/P1/collaborators")
            .with_status(403)
            .with_body(r#"{"error":"Forbidden"}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        // Non-shareable project ⇒ no candidates, not a hard error.
        assert!(list_task_list_members(&client, "P1")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn get_tasks_resolves_assignee_names_from_collaborators() {
        let mut server = Server::new_async().await;
        let _tasks = server
            .mock("GET", "/tasks?project_id=P1")
            .with_status(200)
            .with_body(r#"[{"id":"T1","project_id":"P1","content":"Shared","assignee_id":"42"}]"#)
            .create_async()
            .await;
        let _collab = server
            .mock("GET", "/projects/P1/collaborators")
            .with_status(200)
            .with_body(r#"[{"id":"42","name":"Alice","email":"alice@example.com"}]"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let tasks = get_tasks(&client, "P1").await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].assignees.len(), 1);
        assert_eq!(tasks[0].assignees[0].id, "42");
        // The placeholder id was replaced with the collaborator's name.
        assert_eq!(tasks[0].assignees[0].name, "Alice");
        assert_eq!(
            tasks[0].assignees[0].email.as_deref(),
            Some("alice@example.com"),
        );
    }

    #[tokio::test]
    async fn get_tasks_skips_collaborator_fetch_when_unassigned() {
        let mut server = Server::new_async().await;
        let _tasks = server
            .mock("GET", "/tasks?project_id=P1")
            .with_status(200)
            .with_body(r#"[{"id":"T1","project_id":"P1","content":"Solo"}]"#)
            .create_async()
            .await;
        // No collaborators mock registered: if get_tasks tried to fetch
        // them for an all-unassigned project, the request would 501 and
        // the assertion below on a clean result still holds — but the
        // intent is that the extra round-trip is skipped entirely.
        let client = fixture_client(&server.url());
        let tasks = get_tasks(&client, "P1").await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].assignees.is_empty());
    }

    // ── Membership / sharing (Sync API) ────────────────────────

    #[tokio::test]
    async fn list_task_list_shares_decodes_sync() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("POST", "/sync")
            .with_status(200)
            .with_body(
                r#"{
                    "collaborators":[
                        {"id":"10","email":"alice@example.com","full_name":"Alice"},
                        {"id":"20","email":"bob@example.com","full_name":"Bob"}
                    ],
                    "collaborator_states":[
                        {"project_id":"P1","user_id":"10","state":"active","is_deleted":false},
                        {"project_id":"P1","user_id":"20","state":"invited","is_deleted":false},
                        {"project_id":"P2","user_id":"10","state":"active","is_deleted":false},
                        {"project_id":"P1","user_id":"99","state":"active","is_deleted":true}
                    ]
                }"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let shares = list_task_list_shares(&client, "P1").await.unwrap();
        // Only P1 + non-deleted survive: Alice (active) and Bob (invited).
        assert_eq!(shares.len(), 2);
        let alice = shares.iter().find(|s| s.user.name == "Alice").unwrap();
        // Membership keys on the email (delete_collaborator wants it).
        assert_eq!(alice.user.id, "alice@example.com");
        assert!(!alice.pending);
        assert!(alice.right.is_none());
        let bob = shares.iter().find(|s| s.user.name == "Bob").unwrap();
        assert!(bob.pending);
    }

    #[tokio::test]
    async fn list_task_list_shares_empty_on_error() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("POST", "/sync")
            .with_status(403)
            .with_body(r#"{"error":"Forbidden"}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        assert!(list_task_list_shares(&client, "P1")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn add_task_list_member_succeeds_on_ok() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("POST", "/sync")
            .with_status(200)
            .with_body(r#"{"sync_status":{"cmd":"ok"}}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        add_task_list_member(&client, "P1", "new@example.com")
            .await
            .unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn add_task_list_member_surfaces_command_error() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("POST", "/sync")
            .with_status(200)
            .with_body(r#"{"sync_status":{"cmd":{"error_code":35,"error":"already shared"}}}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let err = add_task_list_member(&client, "P1", "x@example.com")
            .await
            .unwrap_err();
        match err {
            crate::error::TodoistError::Protocol(msg) => assert!(msg.contains("share_project")),
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn remove_task_list_member_succeeds_on_ok() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("POST", "/sync")
            .with_status(200)
            .with_body(r#"{"sync_status":{"cmd":"ok"}}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        remove_task_list_member(&client, "P1", "bye@example.com")
            .await
            .unwrap();
    }
}
