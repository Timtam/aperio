//! Vikunja Tasks API mapping.
//!
//! Vikunja's data model (since the 2023 schema cleanup):
//!
//!   - **Projects** play the role of Aperio's `TaskList`. Every task
//!     belongs to exactly one project. Projects nest via
//!     `parent_project_id` (the old namespace concept is gone, but
//!     project-under-project nesting replaced it) — we surface that as
//!     `TaskList.parent_id` so the sidebar renders the tree. Projects
//!     carry a `hex_color` which we expose via `TaskList.color`.
//!   - **Buckets** are a project's kanban columns. We map them to
//!     Aperio `Section`s (`list_sections`) and a task's `bucket_id` to
//!     `Task.section_id`. Buckets live on a per-project kanban *view*
//!     in current Vikunja; the lookup degrades to "no sections" on
//!     servers that don't expose the view/bucket endpoints.
//!   - **Tasks** are the data items. Two independent date slots —
//!     `start_date` and `due_date` — map cleanly onto Aperio's
//!     `scheduled_*` / `deadline_*` pair (DESIGN.md §9.7 documents
//!     the alignment).
//!   - **Subtasks** in Vikunja are not a `parent_id` field but a
//!     symmetric `related_tasks` relation with kind `parenttask` /
//!     `subtask`. Out of scope for the first cut — Aperio surfaces a
//!     flat list and the `Task.parent_id` round-trips as `None`.
//!
//! Status mapping (DESIGN.md §9.7):
//!
//!   - Aperio Open / InProgress  → Vikunja `done = false`
//!   - Aperio Completed          → Vikunja `done = true` (Vikunja
//!     also sets `done_at` to the server clock)
//!   - Aperio Cancelled          → Vikunja `done = false` (Vikunja
//!     has no equivalent; the cancelled marker only exists locally)
//!
//! Date semantics:
//!
//!   - Vikunja uses RFC 3339 timestamps with a fixed sentinel of
//!     `"0001-01-01T00:00:00Z"` to mean "no date". We treat any
//!     timestamp before the year 1900 as "unset" on the way in and
//!     emit the sentinel on the way out when a slot is empty.
//!   - `start_date` / `due_date` carry meaningful times in Vikunja's
//!     own UI. We round-trip the local components (date + time) as
//!     UTC-tagged datetimes — same trade-off Outlook makes for
//!     tasks (sharing a task across timezones loses the original
//!     local interpretation, but the round-trip with the SAME user
//!     stays stable).
//!
//! Out of scope for Phase 6g.1 (logged with `tracing::warn` on
//! write so we know the field is being dropped):
//!
//!   - Recurrence (Vikunja's `repeat_after` + `repeat_mode` differs
//!     from Aperio's calendar-style recurrence enum; would need a
//!     dedicated mapper).
//!   - Reminders (Vikunja's `reminders[]` is rich enough — relative
//!     periods, multiple reminders — but Aperio's per-task reminder
//!     edit UI doesn't surface a multi-row editor for it yet).
//!   - Subtasks via `related_tasks`.
//!   - Labels (the field is there in `Task`, but Aperio's
//!     ColorLabel system is local-only at the moment).

use cal_core::{
    MemberRight, NewTask, Section, Task, TaskList, TaskListShare, TaskPriority, TaskStatus,
    TaskUser,
};
use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::api::VikunjaClient;
use crate::error::{VikunjaError, VikunjaResult};

// ── Public adapter-side surface ────────────────────────────────────────

/// `GET /projects`. Vikunja lists every project the authenticated
/// user has read access to, including shared ones. The response is
/// paginated via `?page=` + the `X-Pagination-Total-Pages` header;
/// we walk every page so the sidebar sees the full list.
pub async fn list_task_lists(client: &VikunjaClient) -> VikunjaResult<Vec<TaskList>> {
    let mut out = Vec::new();
    let mut page: u32 = 1;
    loop {
        let path = format!("/projects?page={page}&per_page=50");
        let entries: Vec<ProjectEntry> = client.get_json(&path).await?;
        if entries.is_empty() {
            break;
        }
        let len = entries.len();
        for entry in entries {
            // Vikunja returns shared / read-only projects through
            // the same endpoint; we currently flag them all
            // writable because the user's API token already
            // captures the access scope (a write against a
            // read-only project surfaces as a 403 at the wire
            // level, which the error mapper routes to
            // `Forbidden`). A future patch can read
            // `is_archived` / the per-user right and flip
            // `read_only` accordingly.
            out.push(map_project(entry));
        }
        if len < 50 {
            break;
        }
        page += 1;
        // Belt-and-braces guard: a misbehaving server shouldn't be
        // able to keep us looping forever.
        if page > 200 {
            tracing::warn!(
                "vikunja list_task_lists stopped after 200 pages — server returned unbounded data",
            );
            break;
        }
    }
    Ok(out)
}

/// `GET /projects/{id}/tasks`. Vikunja's "all tasks in this project"
/// endpoint with the same page-walk logic as `list_task_lists`.
pub async fn get_tasks(client: &VikunjaClient, list_id: &str) -> VikunjaResult<Vec<Task>> {
    let project_id = parse_id(list_id, "task list id")?;
    let mut out = Vec::new();
    let mut page: u32 = 1;
    loop {
        // `filter_by=done&filter_value=false&filter_value=true` would
        // be a more selective fetch but Aperio displays open + done
        // side by side, so we ask for everything.
        let path = format!("/projects/{project_id}/tasks?page={page}&per_page=50");
        let entries: Vec<TaskEntry> = client.get_json(&path).await?;
        if entries.is_empty() {
            break;
        }
        let len = entries.len();
        for entry in entries {
            out.push(map_task(entry, list_id));
        }
        if len < 50 {
            break;
        }
        page += 1;
        if page > 1_000 {
            tracing::warn!(
                project_id,
                "vikunja get_tasks stopped after 1000 pages — server returned unbounded data",
            );
            break;
        }
    }
    Ok(out)
}

/// List a project's kanban buckets as Aperio sections.
///
/// Current Vikunja (≥ 0.22) hangs buckets off a per-project *view* of
/// kind `kanban`, so we resolve that view first, then pull its
/// buckets. The lookup degrades gracefully: a project with no kanban
/// view — or a server that doesn't expose either endpoint — yields no
/// sections rather than an error, matching the `list_sections` default
/// for section-less backends.
pub async fn list_sections(client: &VikunjaClient, list_id: &str) -> VikunjaResult<Vec<Section>> {
    let project_id = parse_id(list_id, "task list id")?;
    let views: Vec<ViewEntry> = match client
        .get_json(&format!("/projects/{project_id}/views"))
        .await
    {
        Ok(v) => v,
        // No view endpoint (older server) → no sections to surface.
        Err(_) => return Ok(Vec::new()),
    };
    let Some(kanban) = views
        .into_iter()
        .find(|v| v.view_kind.as_deref() == Some("kanban"))
    else {
        return Ok(Vec::new());
    };
    let path = format!("/projects/{project_id}/views/{}/buckets", kanban.id);
    let buckets: Vec<BucketEntry> = match client.get_json(&path).await {
        Ok(b) => b,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(buckets
        .into_iter()
        .map(|b| map_bucket(b, list_id))
        .collect())
}

/// Replace a task's assignee set via `POST /tasks/{id}/assignees/bulk`.
/// Vikunja's bulk endpoint takes the FULL desired list and syncs to it
/// (adds missing, drops extras), so an empty list clears all assignees.
/// `TaskUser.id` is the stringified Vikunja numeric user id; entries
/// that don't parse are skipped.
async fn set_assignees(
    client: &VikunjaClient,
    task_id: i64,
    assignees: &[TaskUser],
) -> VikunjaResult<()> {
    let ids: Vec<serde_json::Value> = assignees
        .iter()
        .filter_map(|a| a.id.parse::<i64>().ok())
        .map(|id| serde_json::json!({ "id": id }))
        .collect();
    let body = serde_json::json!({ "assignees": ids });
    let _: serde_json::Value = client
        .post_json(&format!("/tasks/{task_id}/assignees/bulk"), &body)
        .await?;
    Ok(())
}

/// `PUT /projects/{id}/tasks`. Vikunja returns the freshly-created
/// task in the response, so we can map it back into Aperio's `Task`
/// directly without a follow-up GET. Assignees are applied afterwards
/// (separate endpoint) since the create body doesn't carry them.
pub async fn create_task(
    client: &VikunjaClient,
    list_id: &str,
    task: NewTask,
) -> VikunjaResult<Task> {
    let project_id = parse_id(list_id, "task list id")?;
    let path = format!("/projects/{project_id}/tasks");
    let body = new_task_to_body(&task);
    let entry: TaskEntry = client.put_json(&path, &body).await?;
    let new_id = entry.id;
    let mut mapped = map_task(entry, list_id);
    // A fresh task has no assignees, so skip the round-trip unless the
    // caller asked for some.
    if !task.assignees.is_empty() {
        set_assignees(client, new_id, &task.assignees).await?;
        mapped.assignees = task.assignees;
    }
    Ok(mapped)
}

/// `POST /tasks/{id}`. Vikunja accepts a partial body and returns
/// the merged result. We send every user-visible field so the
/// server's view matches the local one without diffing logic.
pub async fn update_task(client: &VikunjaClient, task: &Task) -> VikunjaResult<Task> {
    let task_id = parse_id(&task.id, "task id")?;
    let path = format!("/tasks/{task_id}");
    let body = task_to_body(task);
    let entry: TaskEntry = client.post_json(&path, &body).await?;
    // Sync the assignee set on every update so the picker can both add
    // and clear assignees (the bulk endpoint takes the full list).
    set_assignees(client, task_id, &task.assignees).await?;
    let mut mapped = map_task(entry, &task.list_id);
    mapped.assignees = task.assignees.clone();
    Ok(mapped)
}

/// `DELETE /tasks/{id}`.
pub async fn delete_task(client: &VikunjaClient, task_id: &str) -> VikunjaResult<()> {
    let id = parse_id(task_id, "task id")?;
    let path = format!("/tasks/{id}");
    client.delete(&path).await
}

/// `POST /projects/{id}` with the renamed `title`. Vikunja accepts a
/// partial body — we only set the title to avoid clobbering colour
/// or other project-level fields a future patch might also be
/// editing.
pub async fn rename_task_list(
    client: &VikunjaClient,
    list_id: &str,
    new_name: &str,
) -> VikunjaResult<()> {
    let project_id = parse_id(list_id, "task list id")?;
    let path = format!("/projects/{project_id}");
    let body = serde_json::json!({ "title": new_name });
    let _: serde_json::Value = client.post_json(&path, &body).await?;
    Ok(())
}

/// `PUT /projects` — create a project. `parent_id` nests it under
/// another project (Vikunja's `parent_project_id`); `None` ⇒ top
/// level. Returns the created project mapped to a `TaskList`.
pub async fn create_task_list(
    client: &VikunjaClient,
    name: &str,
    parent_id: Option<&str>,
) -> VikunjaResult<TaskList> {
    let mut body = serde_json::json!({ "title": name });
    if let Some(parent) = parent_id {
        let pid = parse_id(parent, "parent project id")?;
        body["parent_project_id"] = serde_json::json!(pid);
    }
    let created: ProjectEntry = client.put_json("/projects", &body).await?;
    Ok(map_project(created))
}

/// `DELETE /projects/{id}` — remove a project (with its tasks) at the
/// source.
pub async fn delete_task_list(client: &VikunjaClient, list_id: &str) -> VikunjaResult<()> {
    let project_id = parse_id(list_id, "task list id")?;
    let path = format!("/projects/{project_id}");
    client.delete(&path).await
}

/// `GET /projects/{id}/projectusers` — the users with access to the
/// project, i.e. the valid assignee pool (DESIGN §9.7). Degrades to an
/// empty list on servers that don't expose the endpoint (older Vikunja)
/// so the picker shows no candidates rather than erroring.
pub async fn list_task_list_members(
    client: &VikunjaClient,
    list_id: &str,
) -> VikunjaResult<Vec<TaskUser>> {
    let project_id = parse_id(list_id, "task list id")?;
    let users: Vec<VikunjaUser> = match client
        .get_json(&format!("/projects/{project_id}/projectusers"))
        .await
    {
        Ok(u) => u,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(users.into_iter().map(map_user).collect())
}

/// `GET /user` — the authenticated account's own identity ("me"),
/// used to tell "assigned to me" from "assigned to someone else".
pub async fn current_user(client: &VikunjaClient) -> VikunjaResult<Option<TaskUser>> {
    let u: VikunjaUser = client.get_json("/user").await?;
    Ok(Some(map_user(u)))
}

// ── Membership / sharing (DESIGN §9.7) ──────────────────────────────────
//
// Vikunja keys *project shares* on the USERNAME (add body + remove/right
// path all use it), so the membership `TaskUser.id` carries the username
// — distinct from the assignee path, which keys on the numeric user id
// (the bulk-assignee endpoint wants that). The two never mix: one feeds
// the members dialog, the other the assignee picker.

fn vikunja_right_to_member(raw: i32) -> MemberRight {
    match raw {
        2 => MemberRight::Admin,
        1 => MemberRight::Write,
        _ => MemberRight::Read,
    }
}

fn member_right_to_vikunja(right: Option<MemberRight>) -> i32 {
    match right {
        Some(MemberRight::Admin) => 2,
        Some(MemberRight::Write) => 1,
        _ => 0,
    }
}

/// The username Vikunja keys a share on, falling back to the numeric id
/// when the username is absent from the response.
fn member_ref_of(id: i64, username: &Option<String>) -> String {
    username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string())
}

/// Minimal percent-encoder for a query/path segment (no extra dep).
fn encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `GET /projects/{id}/users` — the editable direct user shares (with
/// rights), distinct from `projectusers` (the read-only effective pool).
pub async fn list_task_list_shares(
    client: &VikunjaClient,
    list_id: &str,
) -> VikunjaResult<Vec<TaskListShare>> {
    let project_id = parse_id(list_id, "task list id")?;
    let entries: Vec<ProjectUserEntry> = match client
        .get_json(&format!("/projects/{project_id}/users"))
        .await
    {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(entries
        .into_iter()
        .map(|e| {
            let display = e
                .name
                .clone()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| e.username.clone().filter(|s| !s.trim().is_empty()))
                .unwrap_or_else(|| format!("User {}", e.id));
            TaskListShare {
                user: TaskUser {
                    id: member_ref_of(e.id, &e.username),
                    name: display,
                    email: e.email.filter(|s| !s.trim().is_empty()),
                },
                right: Some(vikunja_right_to_member(e.right)),
                pending: false,
            }
        })
        .collect())
}

/// `GET /users?s=` — directory search for users to add as members. The
/// returned `TaskUser.id` carries the USERNAME (the membership add key).
pub async fn search_users(client: &VikunjaClient, query: &str) -> VikunjaResult<Vec<TaskUser>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let users: Vec<VikunjaUser> = match client
        .get_json(&format!("/users?s={}", encode_query(query)))
        .await
    {
        Ok(u) => u,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(users
        .into_iter()
        .filter_map(|u| {
            let username = u.username.clone();
            let mut tu = map_user(u);
            tu.id = member_ref_of(0, &username);
            (tu.id != "0").then_some(tu)
        })
        .collect())
}

/// `PUT /projects/{id}/users` — share with a user (by username) at a
/// right level. Immediate; no invitation flow.
pub async fn add_task_list_member(
    client: &VikunjaClient,
    list_id: &str,
    member_ref: &str,
    right: Option<MemberRight>,
) -> VikunjaResult<()> {
    let project_id = parse_id(list_id, "task list id")?;
    let body = serde_json::json!({
        "user_id": member_ref,
        "right": member_right_to_vikunja(right),
    });
    let _: serde_json::Value = client
        .put_json(&format!("/projects/{project_id}/users"), &body)
        .await?;
    Ok(())
}

/// `DELETE /projects/{id}/users/{member}` — revoke a user's share.
pub async fn remove_task_list_member(
    client: &VikunjaClient,
    list_id: &str,
    member_ref: &str,
) -> VikunjaResult<()> {
    let project_id = parse_id(list_id, "task list id")?;
    client
        .delete(&format!(
            "/projects/{project_id}/users/{}",
            encode_query(member_ref)
        ))
        .await
}

/// `POST /projects/{id}/users/{member}` — change a user's right.
pub async fn set_task_list_member_right(
    client: &VikunjaClient,
    list_id: &str,
    member_ref: &str,
    right: MemberRight,
) -> VikunjaResult<()> {
    let project_id = parse_id(list_id, "task list id")?;
    let body = serde_json::json!({ "right": member_right_to_vikunja(Some(right)) });
    let _: serde_json::Value = client
        .post_json(
            &format!("/projects/{project_id}/users/{}", encode_query(member_ref)),
            &body,
        )
        .await?;
    Ok(())
}

/// A `GET /projects/{id}/users` row: the embedded user fields + the
/// share's `right` (0/1/2).
#[derive(Debug, Deserialize)]
struct ProjectUserEntry {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    right: i32,
}

// ── JSON wire shapes ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ProjectEntry {
    id: i64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    hex_color: Option<String>,
    /// Parent project id for Vikunja's nested-project tree. `0` is
    /// Vikunja's "no parent" sentinel (top-level project).
    #[serde(default)]
    parent_project_id: i64,
}

/// One entry of `GET /projects/{id}/views`. We only need the kanban
/// view's id; other kinds (`list`, `gantt`, `table`) are ignored.
#[derive(Debug, Deserialize)]
struct ViewEntry {
    id: i64,
    #[serde(default)]
    view_kind: Option<String>,
}

/// A Vikunja user, as returned inline on a task's `assignees`, by
/// `GET /projects/{id}/projectusers`, and by `GET /user`.
#[derive(Debug, Deserialize)]
struct VikunjaUser {
    id: i64,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

/// Map a Vikunja user to Aperio's `TaskUser`. `name` is Vikunja's
/// optional full name; fall back to the username, then a synthetic
/// label, so the picker always shows something readable.
fn map_user(u: VikunjaUser) -> TaskUser {
    let name = u
        .name
        .filter(|s| !s.trim().is_empty())
        .or_else(|| u.username.filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| format!("User {}", u.id));
    TaskUser {
        id: u.id.to_string(),
        name,
        email: u.email.filter(|s| !s.trim().is_empty()),
    }
}

/// A kanban bucket → Aperio section.
#[derive(Debug, Deserialize)]
struct BucketEntry {
    id: i64,
    #[serde(default)]
    title: Option<String>,
    /// Vikunja orders buckets by a float `position`; we collapse it to
    /// the unsigned section order (display order only, exact value is
    /// immaterial).
    #[serde(default)]
    position: f64,
}

/// Vikunja Task. Fields we don't surface yet (labels, attachments,
/// repeat_after, reminders, related_tasks) are simply not declared —
/// serde tolerates unknown fields by default.
#[derive(Debug, Default, Deserialize, Serialize)]
struct TaskEntry {
    #[serde(default)]
    id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default)]
    done: bool,
    /// RFC 3339 timestamp; Vikunja emits the sentinel
    /// `"0001-01-01T00:00:00Z"` for "no date".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    done_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_date: Option<String>,
    /// Vikunja priority is 0..=5 with 0 = unset. We map 0–1 → Low,
    /// 2–3 → Medium, 4–5 → High, and write Aperio's enum back as
    /// 1 / 3 / 5.
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    project_id: i64,
    /// Kanban bucket the task sits in, mapped to Aperio's section.
    /// `0` ⇒ ungrouped (Vikunja's "no bucket" sentinel). Vikunja
    /// versions that moved buckets fully per-view may omit this on the
    /// task; serde's default keeps such tasks ungrouped. Read-only for
    /// now — skipped on write so an update never disturbs bucket
    /// placement (section write-back isn't wired yet).
    #[serde(default, skip_serializing_if = "is_zero")]
    bucket_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hex_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated: Option<String>,
    /// Assignees of the task. Read-only on this struct: Vikunja sets
    /// them via the dedicated `…/assignees` endpoints, so we never send
    /// them in the create/update body (`skip_serializing`).
    #[serde(default, skip_serializing)]
    assignees: Option<Vec<VikunjaUser>>,
}

// ── Mappers ────────────────────────────────────────────────────────────

fn map_project(entry: ProjectEntry) -> TaskList {
    TaskList {
        id: entry.id.to_string(),
        name: entry.title.unwrap_or_else(|| "Project".into()),
        // Vikunja's `hex_color` is a 6-char string without the `#`;
        // ContainerColor expects "#RRGGBB". Empty or whitespace
        // means "no colour set".
        color: entry
            .hex_color
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                let raw = s.trim_start_matches('#');
                cal_core::ContainerColor::native(format!("#{raw}"))
            }),
        default_sound: None,
        embedded_in_calendar: None,
        // Vikunja nests projects; surface the parent so the sidebar can
        // build the tree. `0` is the "top-level" sentinel.
        parent_id: (entry.parent_project_id != 0).then(|| entry.parent_project_id.to_string()),
        read_only: false,
    }
}

fn map_bucket(entry: BucketEntry, list_id: &str) -> Section {
    Section {
        id: entry.id.to_string(),
        list_id: list_id.to_string(),
        name: entry.title.unwrap_or_else(|| "Bucket".into()),
        order: entry.position.max(0.0) as u32,
    }
}

fn map_task(entry: TaskEntry, list_id: &str) -> Task {
    let status = if entry.done {
        TaskStatus::Completed
    } else {
        TaskStatus::Open
    };
    let priority = vikunja_priority_to_aperio(entry.priority);
    let scheduled = entry.start_date.as_deref().and_then(parse_vikunja_datetime);
    let deadline = entry.due_date.as_deref().and_then(parse_vikunja_datetime);
    let completed_at = entry.done_at.as_deref().and_then(parse_vikunja_datetime);
    let created_at = entry
        .created
        .as_deref()
        .and_then(parse_vikunja_datetime)
        .unwrap_or_else(Utc::now);
    let updated_at = entry
        .updated
        .as_deref()
        .and_then(parse_vikunja_datetime)
        .unwrap_or(created_at);

    Task {
        assignees: entry
            .assignees
            .unwrap_or_default()
            .into_iter()
            .map(map_user)
            .collect(),
        id: entry.id.to_string(),
        list_id: list_id.to_string(),
        title: entry.title.unwrap_or_default(),
        description: entry.description.filter(|s| !s.is_empty()),
        status,
        priority,
        scheduled_date: scheduled.map(|dt| dt.date_naive()),
        scheduled_time: scheduled.map(|dt| dt.time()).filter(non_midnight),
        deadline_date: deadline.map(|dt| dt.date_naive()),
        deadline_time: deadline.map(|dt| dt.time()).filter(non_midnight),
        // Recurrence + parent_id + reminders are intentionally
        // dropped on read; documented in the module preamble.
        recurrence: None,
        parent_id: None,
        // Kanban bucket → section. `0` is Vikunja's "no bucket".
        section_id: (entry.bucket_id != 0).then(|| entry.bucket_id.to_string()),
        color_label: None,
        reminders: Vec::new(),
        sound: None,
        created_at,
        updated_at,
        completed_at,
        // Vikunja doesn't emit an ETag — we rely on the
        // last-writer-wins behaviour the REST API documents.
        etag: None,
    }
}

fn new_task_to_body(new: &NewTask) -> TaskEntry {
    if new.recurrence.is_some() {
        tracing::warn!(
            "Vikunja adapter dropping recurrence on create — schema mismatch with Aperio's calendar-style RRULE",
        );
    }
    if !new.reminders.is_empty() {
        tracing::warn!(
            "Vikunja adapter dropping reminders on create — Vikunja's reminders[] schema not surfaced yet",
        );
    }
    if new.parent_id.is_some() {
        tracing::warn!(
            "Vikunja adapter dropping parent_id on create — subtasks need a separate related_tasks call",
        );
    }
    TaskEntry {
        id: 0,
        title: Some(new.title.clone()),
        description: new.description.clone().filter(|s| !s.is_empty()),
        done: matches!(new.status, TaskStatus::Completed),
        done_at: None,
        due_date: combine_date_time(new.deadline_date, new.deadline_time),
        start_date: combine_date_time(new.scheduled_date, new.scheduled_time),
        priority: aperio_priority_to_vikunja(new.priority),
        project_id: 0,
        bucket_id: 0,
        hex_color: None,
        created: None,
        updated: None,
        assignees: None,
    }
}

fn task_to_body(task: &Task) -> TaskEntry {
    if task.recurrence.is_some() {
        tracing::warn!(
            "Vikunja adapter dropping recurrence on update — schema mismatch with Aperio's calendar-style RRULE",
        );
    }
    if !task.reminders.is_empty() {
        tracing::warn!(
            "Vikunja adapter dropping reminders on update — Vikunja's reminders[] schema not surfaced yet",
        );
    }
    if task.parent_id.is_some() {
        tracing::warn!(
            "Vikunja adapter dropping parent_id on update — subtasks need a separate related_tasks call",
        );
    }
    TaskEntry {
        id: parse_id(&task.id, "task id").unwrap_or(0),
        title: Some(task.title.clone()),
        description: task.description.clone().filter(|s| !s.is_empty()),
        done: matches!(task.status, TaskStatus::Completed),
        // We never write `done_at` ourselves — Vikunja sets it
        // server-side when `done` flips to true.
        done_at: None,
        due_date: combine_date_time(task.deadline_date, task.deadline_time),
        start_date: combine_date_time(task.scheduled_date, task.scheduled_time),
        priority: aperio_priority_to_vikunja(task.priority),
        project_id: parse_id(&task.list_id, "task list id").unwrap_or(0),
        bucket_id: 0,
        hex_color: None,
        created: None,
        updated: None,
        assignees: None,
    }
}

// ── Priority mapping ───────────────────────────────────────────────────

/// Vikunja 0..=5 → Aperio Low/Medium/High. The thresholds match
/// Vikunja's own UI labelling (Low/Medium/High at 2/3/4 with 0–1
/// shown as "no priority"); we collapse the bottom band to Low so
/// the round-trip preserves the user-visible bucket.
fn vikunja_priority_to_aperio(raw: i32) -> TaskPriority {
    match raw {
        i32::MIN..=1 => TaskPriority::Low,
        2..=3 => TaskPriority::Medium,
        _ => TaskPriority::High,
    }
}

fn aperio_priority_to_vikunja(p: TaskPriority) -> i32 {
    match p {
        TaskPriority::Low => 1,
        TaskPriority::Medium => 3,
        TaskPriority::High => 5,
    }
}

// ── Date handling ──────────────────────────────────────────────────────

/// Parse a Vikunja datetime, returning `None` for the
/// `0001-01-01T00:00:00Z` sentinel Vikunja emits for "unset". We
/// guard with `< 1900` rather than an exact string match because
/// some Vikunja versions emit `+00:00` instead of `Z`.
fn parse_vikunja_datetime(raw: &str) -> Option<DateTime<Utc>> {
    let dt = DateTime::parse_from_rfc3339(raw).ok()?;
    if dt.year() < 1900 {
        return None;
    }
    Some(dt.with_timezone(&Utc))
}

/// Combine an Aperio date + optional time into a Vikunja-style
/// RFC 3339 datetime string, returning `None` when the date is
/// absent so the caller's `skip_serializing_if` strips the field
/// entirely. Returning the `0001-01-01` sentinel is the alternative
/// — same wire effect — but omitting the field is cleaner when the
/// API contract supports it (Vikunja does).
fn combine_date_time(date: Option<NaiveDate>, time: Option<NaiveTime>) -> Option<String> {
    let d = date?;
    let t = time.unwrap_or_else(|| NaiveTime::from_hms_opt(0, 0, 0).unwrap());
    let naive = d.and_time(t);
    let utc = Utc.from_utc_datetime(&naive);
    Some(utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn non_midnight(t: &NaiveTime) -> bool {
    *t != NaiveTime::from_hms_opt(0, 0, 0).unwrap()
}

/// `skip_serializing_if` predicate for the read-only `bucket_id` slot.
fn is_zero(n: &i64) -> bool {
    *n == 0
}

// ── Helpers ────────────────────────────────────────────────────────────

fn parse_id(raw: &str, what: &'static str) -> VikunjaResult<i64> {
    raw.parse::<i64>().map_err(|_| {
        VikunjaError::Config(format!("{what} must be a positive integer, got '{raw}'"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    fn fixture_client(server_url: &str) -> VikunjaClient {
        VikunjaClient::new(server_url, "test-token".into(), reqwest::Client::new())
            .expect("fixture client")
    }

    // ── Priority mapping ───────────────────────────────────────

    #[test]
    fn priority_round_trips_through_buckets() {
        assert_eq!(vikunja_priority_to_aperio(0), TaskPriority::Low);
        assert_eq!(vikunja_priority_to_aperio(1), TaskPriority::Low);
        assert_eq!(vikunja_priority_to_aperio(2), TaskPriority::Medium);
        assert_eq!(vikunja_priority_to_aperio(3), TaskPriority::Medium);
        assert_eq!(vikunja_priority_to_aperio(4), TaskPriority::High);
        assert_eq!(vikunja_priority_to_aperio(5), TaskPriority::High);

        assert_eq!(aperio_priority_to_vikunja(TaskPriority::Low), 1);
        assert_eq!(aperio_priority_to_vikunja(TaskPriority::Medium), 3);
        assert_eq!(aperio_priority_to_vikunja(TaskPriority::High), 5);
    }

    // ── Date helpers ───────────────────────────────────────────

    #[test]
    fn parse_drops_sentinel_dates() {
        assert!(parse_vikunja_datetime("0001-01-01T00:00:00Z").is_none());
        assert!(parse_vikunja_datetime("not-a-date").is_none());
    }

    #[test]
    fn parse_accepts_real_dates() {
        let dt = parse_vikunja_datetime("2026-05-22T14:00:00Z").unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 5);
        assert_eq!(dt.day(), 22);
    }

    #[test]
    fn combine_date_time_emits_iso_for_date_only() {
        let d = Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap());
        assert_eq!(
            combine_date_time(d, None).as_deref(),
            Some("2026-05-22T00:00:00Z"),
        );
    }

    #[test]
    fn combine_date_time_includes_time_when_set() {
        let d = Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap());
        let t = Some(NaiveTime::from_hms_opt(14, 30, 0).unwrap());
        assert_eq!(
            combine_date_time(d, t).as_deref(),
            Some("2026-05-22T14:30:00Z"),
        );
    }

    #[test]
    fn combine_date_time_returns_none_without_date() {
        assert_eq!(combine_date_time(None, None), None);
        // Time alone is meaningless without a date — Aperio's data
        // model enforces this too via the CHECK constraint, but we
        // defend in depth here.
        let t = Some(NaiveTime::from_hms_opt(14, 30, 0).unwrap());
        assert_eq!(combine_date_time(None, t), None);
    }

    // ── map_project ───────────────────────────────────────────

    #[test]
    fn map_project_carries_hex_color() {
        let list = map_project(ProjectEntry {
            id: 42,
            title: Some("Work".into()),
            hex_color: Some("ff8800".into()),
            parent_project_id: 0,
        });
        assert_eq!(list.id, "42");
        assert_eq!(list.name, "Work");
        assert_eq!(list.color.as_ref().map(|c| c.hex.as_str()), Some("#ff8800"),);
    }

    #[test]
    fn map_project_defaults_title_when_missing() {
        let list = map_project(ProjectEntry {
            id: 1,
            title: None,
            hex_color: None,
            parent_project_id: 0,
        });
        assert_eq!(list.name, "Project");
        assert!(list.color.is_none());
    }

    #[test]
    fn map_project_handles_hex_with_leading_hash() {
        let list = map_project(ProjectEntry {
            id: 1,
            title: Some("Stuff".into()),
            hex_color: Some("#abcdef".into()),
            parent_project_id: 0,
        });
        assert_eq!(list.color.unwrap().hex, "#abcdef");
    }

    // ── map_task ──────────────────────────────────────────────

    #[test]
    fn map_task_pulls_dates_into_separate_slots() {
        let entry = TaskEntry {
            assignees: None,
            id: 99,
            title: Some("Submit invoice".into()),
            description: Some("Q2".into()),
            done: false,
            done_at: None,
            due_date: Some("2026-05-25T12:00:00Z".into()),
            start_date: Some("2026-05-22T00:00:00Z".into()),
            priority: 4,
            project_id: 7,
            bucket_id: 0,
            hex_color: None,
            created: Some("2026-05-01T10:00:00Z".into()),
            updated: Some("2026-05-02T11:00:00Z".into()),
        };
        let task = map_task(entry, "7");
        assert_eq!(task.id, "99");
        assert_eq!(task.list_id, "7");
        assert_eq!(task.title, "Submit invoice");
        assert_eq!(task.description.as_deref(), Some("Q2"));
        assert_eq!(task.status, TaskStatus::Open);
        assert_eq!(task.priority, TaskPriority::High);
        assert_eq!(
            task.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
        );
        // Start date had midnight — we drop the time so the UI
        // treats it as a date-only "do it on" marker.
        assert!(task.scheduled_time.is_none());
        assert_eq!(
            task.deadline_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 25).unwrap()),
        );
        assert_eq!(
            task.deadline_time,
            Some(NaiveTime::from_hms_opt(12, 0, 0).unwrap()),
        );
    }

    #[test]
    fn map_task_marks_completed_when_done() {
        let entry = TaskEntry {
            assignees: None,
            id: 1,
            title: Some("Done".into()),
            description: None,
            done: true,
            done_at: Some("2026-05-22T15:00:00Z".into()),
            due_date: None,
            start_date: None,
            priority: 0,
            project_id: 1,
            bucket_id: 0,
            hex_color: None,
            created: Some("2026-05-20T10:00:00Z".into()),
            updated: Some("2026-05-22T15:00:00Z".into()),
        };
        let task = map_task(entry, "1");
        assert_eq!(task.status, TaskStatus::Completed);
        assert!(task.completed_at.is_some());
        // Priority 0 collapses to Low (the "no priority" bucket
        // round-trips to a real Aperio value).
        assert_eq!(task.priority, TaskPriority::Low);
    }

    #[test]
    fn map_task_drops_sentinel_dates() {
        let entry = TaskEntry {
            assignees: None,
            id: 1,
            title: Some("No dates".into()),
            description: None,
            done: false,
            done_at: None,
            due_date: Some("0001-01-01T00:00:00Z".into()),
            start_date: Some("0001-01-01T00:00:00Z".into()),
            priority: 0,
            project_id: 1,
            bucket_id: 0,
            hex_color: None,
            created: None,
            updated: None,
        };
        let task = map_task(entry, "1");
        assert!(task.scheduled_date.is_none());
        assert!(task.deadline_date.is_none());
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
            scheduled_time: Some(NaiveTime::from_hms_opt(8, 0, 0).unwrap()),
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
    fn new_task_body_serialises_both_date_slots() {
        let body = new_task_to_body(&sample_new_task());
        assert_eq!(body.title.as_deref(), Some("Buy bread"));
        assert_eq!(body.start_date.as_deref(), Some("2026-05-22T08:00:00Z"));
        assert_eq!(body.due_date.as_deref(), Some("2026-05-23T00:00:00Z"));
        assert_eq!(body.priority, 5);
        assert!(!body.done);
    }

    #[test]
    fn new_task_body_omits_empty_date_slots_on_serialise() {
        let mut nt = sample_new_task();
        nt.scheduled_date = None;
        nt.scheduled_time = None;
        nt.deadline_date = None;
        nt.deadline_time = None;
        let body = new_task_to_body(&nt);
        // Round-trip through serde_json and check the keys aren't
        // present — that's what `skip_serializing_if` guarantees.
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("start_date").is_none());
        assert!(json.get("due_date").is_none());
    }

    // ── End-to-end via mockito ────────────────────────────────

    #[tokio::test]
    async fn delete_round_trips_against_200() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("DELETE", "/api/v1/tasks/42")
            .with_status(200)
            .with_body(r#"{"message":"Successfully deleted."}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        delete_task(&client, "42").await.unwrap();
    }

    #[tokio::test]
    async fn delete_surfaces_404_as_http_error() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("DELETE", "/api/v1/tasks/9999")
            .with_status(404)
            .with_body(r#"{"code":40400,"message":"Task not found"}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let err = delete_task(&client, "9999").await.unwrap_err();
        match err {
            VikunjaError::Http { status, .. } => assert_eq!(status, 404),
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_task_lists_decodes_projects() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/projects?page=1&per_page=50")
            .with_status(200)
            .with_body(
                r#"[{"id":1,"title":"Inbox","hex_color":""},{"id":2,"title":"Work","hex_color":"ff8800"}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let lists = list_task_lists(&client).await.unwrap();
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].id, "1");
        assert_eq!(lists[0].name, "Inbox");
        assert!(lists[0].color.is_none());
        assert_eq!(lists[1].id, "2");
        assert_eq!(lists[1].color.as_ref().unwrap().hex, "#ff8800");
    }

    #[tokio::test]
    async fn get_tasks_decodes_response() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/projects/3/tasks?page=1&per_page=50")
            .with_status(200)
            .with_body(
                r#"[{"id":7,"title":"One","done":false,"priority":3,"project_id":3},{"id":8,"title":"Two","done":true,"priority":0,"project_id":3}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let tasks = get_tasks(&client, "3").await.unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].title, "One");
        assert_eq!(tasks[0].status, TaskStatus::Open);
        assert_eq!(tasks[0].priority, TaskPriority::Medium);
        assert_eq!(tasks[1].status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn unauthorised_surfaces_as_http_401() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/projects?page=1&per_page=50")
            .with_status(401)
            .with_body(r#"{"code":401,"message":"missing or invalid token"}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let err = list_task_lists(&client).await.unwrap_err();
        match err {
            VikunjaError::Http { status, .. } => assert_eq!(status, 401),
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn parse_id_rejects_garbage() {
        let err = parse_id("not-a-number", "task id").unwrap_err();
        match err {
            VikunjaError::Config(msg) => assert!(msg.contains("task id")),
            other => panic!("expected Config, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_task_lists_surfaces_parent_project_id() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/projects?page=1&per_page=50")
            .with_status(200)
            .with_body(
                r#"[{"id":1,"title":"Parent","parent_project_id":0},{"id":2,"title":"Child","parent_project_id":1}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let lists = list_task_lists(&client).await.unwrap();
        // Top-level project → no parent.
        assert_eq!(lists[0].id, "1");
        assert!(lists[0].parent_id.is_none());
        // Nested project → parent surfaced as the parent project id.
        assert_eq!(lists[1].id, "2");
        assert_eq!(lists[1].parent_id.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn get_tasks_maps_bucket_to_section() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/projects/3/tasks?page=1&per_page=50")
            .with_status(200)
            .with_body(
                r#"[{"id":7,"title":"Grouped","project_id":3,"bucket_id":9},{"id":8,"title":"Loose","project_id":3,"bucket_id":0}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let tasks = get_tasks(&client, "3").await.unwrap();
        assert_eq!(tasks[0].section_id.as_deref(), Some("9"));
        // bucket_id 0 (or absent) ⇒ ungrouped.
        assert!(tasks[1].section_id.is_none());
    }

    #[tokio::test]
    async fn list_sections_maps_kanban_buckets() {
        let mut server = Server::new_async().await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(r#"[{"id":10,"view_kind":"list"},{"id":11,"view_kind":"kanban"}]"#)
            .create_async()
            .await;
        let _buckets = server
            .mock("GET", "/api/v1/projects/3/views/11/buckets")
            .with_status(200)
            .with_body(
                r#"[{"id":21,"title":"To Do","position":1.0},{"id":22,"title":"Doing","position":2.0}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let sections = list_sections(&client, "3").await.unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].id, "21");
        assert_eq!(sections[0].name, "To Do");
        assert_eq!(sections[0].list_id, "3");
        assert_eq!(sections[1].name, "Doing");
    }

    #[tokio::test]
    async fn list_sections_empty_without_kanban_view() {
        let mut server = Server::new_async().await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(r#"[{"id":10,"view_kind":"list"}]"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        // No kanban view → no sections, no error.
        assert!(list_sections(&client, "3").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_task_list_puts_project_and_maps_it() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("PUT", "/api/v1/projects")
            .with_status(200)
            .with_body(r#"{"id":12,"title":"New Project","parent_project_id":3}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let list = create_task_list(&client, "New Project", Some("3"))
            .await
            .unwrap();
        assert_eq!(list.id, "12");
        assert_eq!(list.name, "New Project");
        assert_eq!(list.parent_id.as_deref(), Some("3"));
    }

    #[tokio::test]
    async fn delete_task_list_hits_delete_endpoint() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("DELETE", "/api/v1/projects/7")
            .with_status(200)
            .with_body(r#"{"message":"Successfully deleted."}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        delete_task_list(&client, "7").await.unwrap();
        m.assert_async().await;
    }
}
