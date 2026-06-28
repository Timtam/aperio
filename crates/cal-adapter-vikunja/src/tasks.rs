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
//! Recurrence maps the shapes Vikunja can store: daily / weekly become a
//! `repeat_after` seconds period (mode 0) and monthly uses Vikunja's
//! monthly mode (mode 1). Yearly, a weekday picker, an explicit
//! day-of-month and the COUNT / UNTIL end modes have no Vikunja
//! equivalent — the adapter declares a restricted `recurrence`
//! capability so the task editor greys those out, and any such rule
//! arriving from elsewhere is dropped with a `tracing::warn` rather than
//! approximated. See `recurrence_from_vikunja` / `recurrence_to_vikunja`.
//!
//! Out of scope for Phase 6g.1 (logged with `tracing::warn` on
//! write so we know the field is being dropped):
//!
//!   - Reminders (Vikunja's `reminders[]` is rich enough — relative
//!     periods, multiple reminders — but Aperio's per-task reminder
//!     edit UI doesn't surface a multi-row editor for it yet).
//!   - Subtasks via `related_tasks`.
//!   - Labels (the field is there in `Task`, but Aperio's
//!     ColorLabel system is local-only at the moment).

use cal_core::{
    MemberRight, NewTask, RecurrenceFrequency, Section, Task, TaskList, TaskListShare,
    TaskPriority, TaskRecurrence, TaskStatus, TaskUser,
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
            // Skip Vikunja's pseudo-projects. `GET /projects` also returns
            // the Favorites collection (id -1) once anything is favorited
            // and saved filters (negative ids). They aren't real
            // containers: creating a task in one fails server-side with
            // error 3001 ("This project does not exist"), and a favorited
            // task would also double up (it still lives in its real
            // project, so it'd appear under both ids). Real projects have
            // positive, auto-increment ids.
            if entry.id < 1 {
                continue;
            }
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
    // Resolve the kanban view once so each task's per-view bucket — which
    // Vikunja ≥0.24 keeps on the view, not on the task — can be stitched
    // back into its section. `None` on section-less / older servers, where
    // `map_task` falls back to the flat `bucket_id`.
    let view = kanban_view(client, project_id).await;
    let kanban_view_id = view.as_ref().map(|v| v.id);
    // The "done bucket" (DESIGN §8.2) is Vikunja's done-status mechanism, not
    // a user section — a task it filed there on completion must not surface as
    // if it lived in that section, so we blank it out below.
    let done_bucket = view
        .as_ref()
        .map(|v| v.done_bucket_id)
        .filter(|&id| id != 0);
    let done_bucket_str = done_bucket.map(|id| id.to_string());
    let mut out = Vec::new();
    let mut page: u32 = 1;
    loop {
        // `filter_by=done&filter_value=false&filter_value=true` would
        // be a more selective fetch but Aperio displays open + done
        // side by side, so we ask for everything. `expand=buckets` adds
        // the per-view bucket memberships (≥0.24); older servers ignore
        // the unknown param and keep returning the flat `bucket_id`.
        let path = format!("/projects/{project_id}/tasks?page={page}&per_page=50&expand=buckets");
        let entries: Vec<TaskEntry> = client.get_json(&path).await?;
        if entries.is_empty() {
            break;
        }
        let len = entries.len();
        for entry in entries {
            // Prefer the per-view bucket from the expanded `buckets`
            // array; `map_task` falls back to the flat `bucket_id`.
            let section_override = kanban_view_id.and_then(|vid| {
                entry
                    .buckets
                    .iter()
                    .find(|b| b.project_view_id == vid && b.id != 0)
                    .map(|b| b.id.to_string())
            });
            let mut task = map_task(entry, list_id);
            if let Some(section) = section_override {
                task.section_id = Some(section);
            }
            // Drop the done bucket — it isn't a user section.
            if task.section_id.is_some() && task.section_id.as_deref() == done_bucket_str.as_deref()
            {
                task.section_id = None;
            }
            out.push(task);
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
    let Some(view) = kanban_view(client, project_id).await else {
        return Ok(Vec::new());
    };
    let path = format!("/projects/{project_id}/views/{}/buckets", view.id);
    let buckets: Vec<BucketEntry> = match client.get_json(&path).await {
        Ok(b) => b,
        Err(_) => return Ok(Vec::new()),
    };
    // The done bucket is Vikunja's done-status mechanism, not a user section —
    // never offer it as a manageable/pickable section (DESIGN §8.2).
    Ok(buckets
        .into_iter()
        .filter(|b| view.done_bucket_id == 0 || b.id != view.done_bucket_id)
        .map(|b| map_bucket(b, list_id))
        .collect())
}

/// Resolve `(project_id, kanban_view_id)` for a section-management call,
/// erroring if the project has no kanban view — Vikunja ≥0.24 hangs
/// buckets off the view, so without one there's nothing to manage.
async fn section_view(client: &VikunjaClient, list_id: &str) -> VikunjaResult<(i64, KanbanView)> {
    let project_id = parse_id(list_id, "task list id")?;
    let Some(view) = kanban_view(client, project_id).await else {
        return Err(VikunjaError::Protocol(
            "project has no kanban view — sections can't be managed".into(),
        ));
    };
    Ok((project_id, view))
}

/// Reject a section-management op aimed at the done bucket — renaming or
/// deleting it would silently re-point / clear Vikunja's done mechanism
/// (DESIGN §8.2). `list_sections` already hides it, so this only fires on a
/// stale id or a direct API misuse.
fn reject_done_bucket(view: &KanbanView, bucket_id: i64) -> VikunjaResult<()> {
    if view.done_bucket_id != 0 && bucket_id == view.done_bucket_id {
        return Err(VikunjaError::Protocol(
            "that bucket is Vikunja's done bucket, not a user section".into(),
        ));
    }
    Ok(())
}

/// `PUT /projects/{p}/views/{v}/buckets` with `{ title }` — create a
/// kanban bucket (Aperio section). Color is never sent (it's a local
/// override).
pub async fn create_section(
    client: &VikunjaClient,
    list_id: &str,
    name: &str,
) -> VikunjaResult<Section> {
    let (project_id, view) = section_view(client, list_id).await?;
    let path = format!("/projects/{project_id}/views/{}/buckets", view.id);
    let body = serde_json::json!({ "title": name });
    let entry: BucketEntry = client.put_json(&path, &body).await?;
    Ok(map_bucket(entry, list_id))
}

/// `POST /projects/{p}/views/{v}/buckets/{id}` with `{ title }` — rename
/// a bucket.
pub async fn update_section(
    client: &VikunjaClient,
    list_id: &str,
    section_id: &str,
    new_name: &str,
) -> VikunjaResult<Section> {
    let (project_id, view) = section_view(client, list_id).await?;
    let bucket_id = parse_id(section_id, "section id")?;
    reject_done_bucket(&view, bucket_id)?;
    let path = format!(
        "/projects/{project_id}/views/{}/buckets/{bucket_id}",
        view.id
    );
    let body = serde_json::json!({ "title": new_name });
    let entry: BucketEntry = client.post_json(&path, &body).await?;
    Ok(map_bucket(entry, list_id))
}

/// `DELETE /projects/{p}/views/{v}/buckets/{id}` — remove a bucket; its
/// tasks fall back to the view's default bucket at the source.
pub async fn delete_section(
    client: &VikunjaClient,
    list_id: &str,
    section_id: &str,
) -> VikunjaResult<()> {
    let (project_id, view) = section_view(client, list_id).await?;
    let bucket_id = parse_id(section_id, "section id")?;
    reject_done_bucket(&view, bucket_id)?;
    let path = format!(
        "/projects/{project_id}/views/{}/buckets/{bucket_id}",
        view.id
    );
    client.delete(&path).await
}

/// Resolve a project's kanban view (id + default bucket), if it has one.
/// One `GET /projects/{id}/views`; `None` (error swallowed) on servers
/// without the views endpoint, matching `list_sections`' graceful
/// degradation.
async fn kanban_view(client: &VikunjaClient, project_id: i64) -> Option<KanbanView> {
    let views: Vec<ViewEntry> = client
        .get_json(&format!("/projects/{project_id}/views"))
        .await
        .ok()?;
    views
        .into_iter()
        .find(|v| v.view_kind.as_deref() == Some("kanban"))
        .map(|v| KanbanView {
            id: v.id,
            default_bucket_id: v.default_bucket_id,
            done_bucket_id: v.done_bucket_id,
        })
}

/// The lowest-`position` bucket of a kanban view — Vikunja's implicit
/// default when the view sets no explicit `default_bucket_id`. `None` if
/// the buckets can't be read. `exclude` (the done bucket, `0` = none) is
/// skipped so we never fall back to filing a task as "done".
async fn leftmost_bucket(
    client: &VikunjaClient,
    project_id: i64,
    view_id: i64,
    exclude: i64,
) -> Option<i64> {
    let buckets: Vec<BucketEntry> = client
        .get_json(&format!("/projects/{project_id}/views/{view_id}/buckets"))
        .await
        .ok()?;
    buckets
        .into_iter()
        .filter(|b| exclude == 0 || b.id != exclude)
        .min_by(|a, b| {
            a.position
                .partial_cmp(&b.position)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|b| b.id)
}

/// A bucket id seen as an Aperio section — `None` for the done bucket, which
/// is Vikunja's done-status mechanism rather than a user section (DESIGN §8.2).
fn open_section(bucket: i64, done_bucket_id: i64) -> Option<i64> {
    (done_bucket_id == 0 || bucket != done_bucket_id).then_some(bucket)
}

/// Read the task's current bucket id *in the given kanban view*
/// (`GET /tasks/{id}?expand=buckets`). `None` when the server doesn't
/// populate it (older Vikunja / no `expand` support / task in no bucket
/// of this view) — callers treat `None` as "can't determine" and skip
/// any move rather than risk placing the task in the wrong bucket.
async fn current_bucket(client: &VikunjaClient, task_id: i64, view_id: i64) -> Option<i64> {
    let entry: TaskEntry = client
        .get_json(&format!("/tasks/{task_id}?expand=buckets"))
        .await
        .ok()?;
    entry
        .buckets
        .iter()
        .find(|b| b.project_view_id == view_id && b.id != 0)
        .map(|b| b.id)
}

/// Best-effort: place `task` in its target kanban bucket on Vikunja
/// ≥0.24, where the bucket lives on a per-project kanban *view* and is
/// set via a dedicated endpoint (the task body's `bucket_id` is ignored).
///
/// It first reads the task's current bucket and moves **only when it
/// actually changed**, so an unrelated edit never reorders the card. A
/// `section_id` of `None` maps to the view's default (then leftmost)
/// bucket, since Vikunja kanban has no "ungrouped" state. Degrades
/// silently — a server without the view/bucket endpoints, or an
/// unreadable current bucket, skips the move — and never fails the
/// surrounding task edit.
///
/// Returns the bucket the task is in **after** the call (so the caller
/// reports the real section, not an optimistic guess): the target on a
/// successful move, the unchanged current bucket on a no-op / failed /
/// skipped move, or `None` when it genuinely can't be determined (no
/// kanban view, or the current bucket couldn't be read).
async fn move_task_bucket(client: &VikunjaClient, task: &Task, task_id: i64) -> Option<i64> {
    let project_id = parse_id(&task.list_id, "task list id").ok()?;
    let Some(view) = kanban_view(client, project_id).await else {
        if task.section_id.is_some() {
            tracing::warn!(
                project_id,
                "vikunja: no kanban view — section change not applied",
            );
        }
        return None;
    };
    let Some(current) = current_bucket(client, task_id, view.id).await else {
        // Can't read where the task currently sits → don't risk a wrong
        // move. (Older server / no `expand` support.)
        if task.section_id.is_some() {
            tracing::warn!(
                task_id,
                "vikunja: can't read current bucket — section change not applied",
            );
        }
        return None;
    };
    let done = view.done_bucket_id;
    // Resolve the bucket the task should sit in. The done bucket is NEVER a
    // valid target — it's Vikunja's done-status mechanism, not a section — so
    // an explicit section pointing at it (the read path filters it, but be
    // defensive), or a default/leftmost that *is* it, falls through to a real
    // open bucket. That fall-through is what lets a reopened task actually
    // leave the done bucket when the project's only/default bucket coincides
    // with it (DESIGN §8.2).
    let explicit = task
        .section_id
        .as_deref()
        .and_then(|s| parse_id(s, "section id").ok())
        .filter(|&b| done == 0 || b != done);
    let target = match explicit {
        Some(b) => b,
        // No (real) section → the view's default open bucket, else the
        // leftmost open bucket; both skip the done bucket.
        None => {
            if view.default_bucket_id != 0 && view.default_bucket_id != done {
                view.default_bucket_id
            } else {
                match leftmost_bucket(client, project_id, view.id, done).await {
                    Some(b) => b,
                    // Only the done bucket exists — nothing open to move to.
                    None => return open_section(current, done),
                }
            }
        }
    };
    if target == current {
        return open_section(current, done); // already in the right bucket.
    }
    let path = format!(
        "/projects/{project_id}/views/{}/buckets/{target}/tasks",
        view.id
    );
    let body = serde_json::json!({ "task_id": task_id, "bucket_id": target });
    let moved: VikunjaResult<serde_json::Value> = client.post_json(&path, &body).await;
    match moved {
        // `target` is a non-done bucket by construction.
        Ok(_) => Some(target),
        Err(err) => {
            tracing::warn!(
                ?err,
                task_id,
                target,
                "vikunja: bucket move failed — task edit preserved",
            );
            // The move didn't apply → the task is still in `current`.
            open_section(current, done)
        }
    }
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
    // Honour the requested section. A fresh task lands in the view's default
    // bucket, so relocate it the way `update_task` does — otherwise a
    // sectioned task (e.g. a spawned recurring instance, DESIGN §9.12) drifts
    // to the default column. Best-effort: a move failure never fails the
    // create, and the done bucket is filtered inside `move_task_bucket`.
    if let Some(requested) = task.section_id.clone() {
        let mut probe = mapped.clone();
        probe.section_id = Some(requested);
        if let Some(bucket) = move_task_bucket(client, &probe, new_id).await {
            mapped.section_id = Some(bucket.to_string());
        }
        // `None` ⇒ the move couldn't be determined; keep the default bucket
        // `map_task` already resolved rather than claim the requested section.
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
    // Apply a section (kanban bucket) change via the dedicated per-view
    // endpoint — the task body's `bucket_id` is ignored on ≥0.24. This is
    // a best-effort follow-up (like assignees) that never fails the edit.
    //
    // EXCEPT when we're completing the task: Vikunja files a done task into
    // its kanban "done bucket", so `current_bucket` reads it there. Since
    // that differs from the task's own section, `move_task_bucket` would move
    // it back to a regular bucket — and moving a task *out* of the done
    // bucket flips it undone again, so it'd reappear as open in its section.
    // A done task's bucket is irrelevant in Aperio (the Done group ignores
    // sections), so leave it where Vikunja put it.
    let effective_section = if matches!(task.status, TaskStatus::Completed) {
        None
    } else {
        move_task_bucket(client, task, task_id).await
    };
    let mut mapped = map_task(entry, &task.list_id);
    mapped.assignees = task.assignees.clone();
    // Reflect where the task actually ended up — the field PUT doesn't
    // move buckets and its response can't carry the per-view bucket, so
    // `map_task` drops it. `None` means the move couldn't be determined
    // (no kanban view / unreadable current bucket); keep `map_task`'s
    // value rather than optimistically claiming the requested section.
    if let Some(bucket) = effective_section {
        mapped.section_id = Some(bucket.to_string());
    }
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
                right: Some(vikunja_right_to_member(e.permission)),
                pending: false,
            }
        })
        .collect())
}

/// `GET /users?s=` — directory search for users to add as members. The
/// returned `TaskUser.id` carries the USERNAME (the membership add key).
///
/// Errors propagate (rather than collapsing to an empty list) so the
/// members dialog can tell "no matches" apart from "the request failed"
/// and surface the latter to the user. A genuine no-match is a `200`
/// with an empty array, which still maps to `Ok(vec![])`.
pub async fn search_users(client: &VikunjaClient, query: &str) -> VikunjaResult<Vec<TaskUser>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let users: Vec<VikunjaUser> = client
        .get_json(&format!("/users?s={}", encode_query(query)))
        .await?;
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

/// Body for `PUT /projects/{id}/users`. The share is keyed on the
/// USERNAME, which Vikunja resolves server-side via `GetUserByUsername`.
///
/// Vikunja renamed two body fields over time, and we send the old + new
/// name for each so current and legacy servers both bind the value (an
/// unknown extra field is ignored):
///   - the user key: `user_id` (≤ 0.x) → `username` (current). Sending
///     only `user_id` left `Username` empty on current servers, so the
///     lookup hit error 1005 ("The user does not exist").
///   - the level: `right` (≤ 0.x) → `permission` (current). Sending only
///     `right` left `Permission` at 0 on current servers, so every share
///     was created/updated as Read regardless of the chosen level.
fn share_user_body(member_ref: &str, right: Option<MemberRight>) -> serde_json::Value {
    let perm = member_right_to_vikunja(right);
    serde_json::json!({
        "username": member_ref,
        "user_id": member_ref,
        "permission": perm,
        "right": perm,
    })
}

/// Body for `POST /projects/{id}/users/{user}` (change a share's level).
/// Sends both `permission` (current) and `right` (legacy) — see
/// [`share_user_body`].
fn member_permission_body(right: MemberRight) -> serde_json::Value {
    let perm = member_right_to_vikunja(Some(right));
    serde_json::json!({ "permission": perm, "right": perm })
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
    let body = share_user_body(member_ref, right);
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
    let body = member_permission_body(right);
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
    // Vikunja renamed this field `right` → `permission` (model
    // `UserWithPermission`). Accept either so the share level reads back
    // correctly on both current and legacy servers; without this it
    // always defaulted to 0 → every member showed "Read".
    #[serde(default, rename = "permission", alias = "right")]
    permission: i32,
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

/// One entry of `GET /projects/{id}/views`. We need the kanban view's
/// id and its default bucket; other kinds (`list`, `gantt`, `table`) are
/// ignored.
#[derive(Debug, Deserialize)]
struct ViewEntry {
    id: i64,
    #[serde(default)]
    view_kind: Option<String>,
    /// Bucket that "bucket-less" tasks land in (Vikunja ≥0.24). `0` ⇒
    /// unset, in which case Vikunja uses the leftmost bucket. This is the
    /// target for moving a task to "no section" (Vikunja kanban has no
    /// ungrouped state).
    #[serde(default)]
    default_bucket_id: i64,
    /// Bucket Vikunja treats as "done": marking a task done files it here,
    /// and moving a task out of it flips it back to undone. `0` ⇒ none set.
    /// Aperio must NOT treat it as a user section (DESIGN §8.2).
    #[serde(default)]
    done_bucket_id: i64,
}

/// The kanban view of a project, resolved once before reading or moving
/// a task's bucket (Vikunja ≥0.24 hangs buckets off the view).
struct KanbanView {
    id: i64,
    default_bucket_id: i64,
    /// `0` when the view has no done bucket; see [`ViewEntry::done_bucket_id`].
    done_bucket_id: i64,
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

/// One per-view bucket membership of a task, returned with
/// `?expand=buckets` (Vikunja ≥0.24, where a task's bucket lives per
/// kanban *view* rather than on the task itself). We pick the entry
/// whose `project_view_id` matches the project's kanban view.
#[derive(Debug, Default, Deserialize)]
struct TaskBucketRef {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    project_view_id: i64,
}

/// Vikunja Task. Fields we don't surface yet (labels, attachments,
/// repeat_after, reminders, related_tasks) are simply not declared —
/// serde tolerates unknown fields by default.
#[derive(Debug, Default, Deserialize, Serialize)]
struct TaskEntry {
    // Skip a zero id on write. The create body (`new_task_to_body`) carries
    // id 0; sending it is at best noise, and `project_id` below is the real
    // hazard. Reads are unaffected (skip_serializing_if only touches writes).
    #[serde(default, skip_serializing_if = "is_zero")]
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
    // CRITICAL: never serialize a zero project_id. The create endpoint is
    // `PUT /projects/{id}/tasks` — the project comes from the URL path — but
    // current Vikunja also binds `project_id` from the request body and lets
    // it WIN over the path. The body builders set this to 0, so sending it
    // pinned every create to project 0 → "project does not exist" (error
    // 3001). Omitting a zero lets the URL define the project; a real value
    // (e.g. a future move) is still sent. Reads are unaffected.
    #[serde(default, skip_serializing_if = "is_zero")]
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
    /// Recurrence period in SECONDS. Paired with `repeat_mode`. `0` ⇒ no
    /// recurrence. Always serialized (no skip) so an update that clears
    /// recurrence sends `0` and actually clears it server-side.
    #[serde(default)]
    repeat_after: i64,
    /// Recurrence mode: `0` repeat `repeat_after` from the due date,
    /// `1` monthly (period ignored), `2` repeat `repeat_after` from the
    /// completion date. Aperio maps daily/weekly → mode 0 and monthly →
    /// mode 1; see `recurrence_from_vikunja` / `recurrence_to_vikunja`.
    #[serde(default)]
    repeat_mode: i32,
    /// Assignees of the task. Read-only on this struct: Vikunja sets
    /// them via the dedicated `…/assignees` endpoints, so we never send
    /// them in the create/update body (`skip_serializing`).
    #[serde(default, skip_serializing)]
    assignees: Option<Vec<VikunjaUser>>,
    /// Per-view bucket memberships, populated with `?expand=buckets`
    /// (Vikunja ≥0.24). Read-only; used to resolve the task's section
    /// for the project's kanban view. Empty on older servers, which use
    /// the flat `bucket_id` above.
    #[serde(default, skip_serializing)]
    buckets: Vec<TaskBucketRef>,
}

// ── Mappers ────────────────────────────────────────────────────────────

fn map_project(entry: ProjectEntry) -> TaskList {
    TaskList {
        color_label: None,
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
        // Vikunja buckets carry no color of their own.
        color_label: None,
        order: entry.position.max(0.0) as u32,
    }
}

const SECONDS_PER_DAY: i64 = 86_400;
const SECONDS_PER_WEEK: i64 = 604_800;

/// Vikunja recurrence → Aperio. Vikunja stores a plain period
/// (`repeat_after` seconds) with a `repeat_mode`; only the shapes Aperio
/// can express survive:
///   - `repeat_mode == 1` → monthly, interval 1 (the day comes from the
///     task's due date).
///   - otherwise a positive `repeat_after` → weekly when it's a whole
///     number of weeks, else daily when it's a whole number of days.
///
/// Sub-day periods, and `repeat_mode == 2`'s "from completion" anchor,
/// have no task-recurrence equivalent and collapse (the anchor is lost).
/// `None` ⇒ no recurrence.
fn recurrence_from_vikunja(repeat_after: i64, repeat_mode: i32) -> Option<TaskRecurrence> {
    let simple = |frequency, interval: i64| {
        Some(TaskRecurrence {
            frequency,
            interval: interval.max(1) as u32,
            day_of_week: None,
            day_of_month: None,
            end: None,
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
        })
    };
    if repeat_mode == 1 {
        return simple(RecurrenceFrequency::Monthly, 1);
    }
    if repeat_after <= 0 {
        return None;
    }
    if repeat_after % SECONDS_PER_WEEK == 0 {
        simple(RecurrenceFrequency::Weekly, repeat_after / SECONDS_PER_WEEK)
    } else if repeat_after % SECONDS_PER_DAY == 0 {
        simple(RecurrenceFrequency::Daily, repeat_after / SECONDS_PER_DAY)
    } else {
        // A period that isn't a whole number of days (e.g. an hourly
        // repeat set in Vikunja directly) can't be shown in Aperio's
        // day-granular task recurrence — leave it unset rather than lie.
        None
    }
}

/// Aperio recurrence → Vikunja `(repeat_after_seconds, repeat_mode)`.
/// Returns `None` for shapes Vikunja can't store (yearly). The task
/// recurrence capability greys those out in the UI, so this is a
/// defensive fallback (e.g. a task carrying a yearly rule from another
/// list). Weekday selection, an explicit day-of-month and the COUNT /
/// UNTIL end modes are likewise dropped (also gated off in the UI):
/// Vikunja's monthly mode just repeats on the due date's day.
fn recurrence_to_vikunja(rec: &TaskRecurrence) -> Option<(i64, i32)> {
    let interval = i64::from(rec.interval.max(1));
    match rec.frequency {
        RecurrenceFrequency::Daily => Some((interval * SECONDS_PER_DAY, 0)),
        RecurrenceFrequency::Weekly => Some((interval * SECONDS_PER_WEEK, 0)),
        RecurrenceFrequency::Monthly => Some((0, 1)),
        RecurrenceFrequency::Yearly => None,
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

    // DESIGN §9.12: the description carries an Aperio-Extras block for fields
    // Vikunja can't store natively. Strip it back out so the user-facing
    // description stays clean, then overlay the carried fields — the bag's
    // recurrence (when present) is authoritative over Vikunja's lossy
    // repeat_after/repeat_mode projection.
    let (clean_description, extras) = cal_core::extras::extract(entry.description.as_deref());
    let mut recurrence = recurrence_from_vikunja(entry.repeat_after, entry.repeat_mode);
    let mut resurface_date = None;
    let mut series_id = None;
    let mut effort = cal_core::TaskEffort::default();
    if let Some(extras) = &extras {
        cal_core::apply_task_extras(
            extras,
            &mut recurrence,
            &mut resurface_date,
            &mut series_id,
            &mut effort,
        );
    }

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
        description: clean_description.filter(|s| !s.is_empty()),
        status,
        priority,
        effort,
        scheduled_date: scheduled.map(|dt| dt.date_naive()),
        scheduled_time: scheduled.map(|dt| dt.time()).filter(non_midnight),
        deadline_date: deadline.map(|dt| dt.date_naive()),
        deadline_time: deadline.map(|dt| dt.time()).filter(non_midnight),
        // parent_id + reminders are intentionally dropped on read;
        // documented in the module preamble. Recurrence round-trips the
        // shapes Vikunja can store (daily/weekly periods + monthly) plus the
        // on-demand axes carried in the extras block.
        recurrence,
        parent_id: None,
        resurface_date,
        series_id,
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

/// DESIGN §9.12: fold the Aperio-only fields (on-demand recurrence axes,
/// `resurface_date`, `series_id`) into a visible extras block on the
/// description, and decide what — if anything — to put in Vikunja's own
/// recurrence fields. A plain scheduled rule projects to native
/// `repeat_after/repeat_mode`; anything with a non-native aspect rides the
/// bag instead, so Vikunja's own scheduler doesn't fight Aperio's spawner.
fn vikunja_body_extras(
    description: Option<&str>,
    recurrence: Option<&TaskRecurrence>,
    resurface_date: Option<NaiveDate>,
    series_id: Option<&str>,
    effort: cal_core::TaskEffort,
    op: &str,
) -> (Option<String>, i64, i32) {
    let extras = cal_core::extras_for_task(recurrence, resurface_date, series_id, effort);
    let description = cal_core::extras::embed(description, &extras).filter(|s| !s.is_empty());
    let native = recurrence.filter(|r| !cal_core::recurrence_needs_extras(r));
    let (repeat_after, repeat_mode) = recurrence_body(native, op);
    (description, repeat_after, repeat_mode)
}

fn new_task_to_body(new: &NewTask) -> TaskEntry {
    let (description, repeat_after, repeat_mode) = vikunja_body_extras(
        new.description.as_deref(),
        new.recurrence.as_ref(),
        new.resurface_date,
        new.series_id.as_deref(),
        new.effort,
        "create",
    );
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
        description,
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
        repeat_after,
        repeat_mode,
        assignees: None,
        buckets: Vec::new(),
    }
}

/// Resolve the `(repeat_after, repeat_mode)` to put in a task body.
/// `None` recurrence (and any rule Vikunja can't store) sends `0/0`,
/// which clears recurrence server-side. A set-but-unsupported shape
/// (yearly) only reaches here from a rule that originated elsewhere —
/// the recurrence capability greys it out in the UI — so we drop it with
/// a warn rather than approximate it.
fn recurrence_body(rec: Option<&TaskRecurrence>, op: &str) -> (i64, i32) {
    match rec {
        None => (0, 0),
        Some(r) => recurrence_to_vikunja(r).unwrap_or_else(|| {
            tracing::warn!(
                "Vikunja adapter dropping unsupported recurrence on {op} — only daily / weekly / monthly round-trip",
            );
            (0, 0)
        }),
    }
}

fn task_to_body(task: &Task) -> TaskEntry {
    let (description, repeat_after, repeat_mode) = vikunja_body_extras(
        task.description.as_deref(),
        task.recurrence.as_ref(),
        task.resurface_date,
        task.series_id.as_deref(),
        task.effort,
        "update",
    );
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
        description,
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
        repeat_after,
        repeat_mode,
        assignees: None,
        buckets: Vec::new(),
    }
}

// ── Priority mapping ───────────────────────────────────────────────────

/// Vikunja 0..=5 → Aperio Low/Medium/High. The thresholds match
/// Vikunja's own UI labelling (Low/Medium/High at 2/3/4 with 0–1
/// shown as "no priority"); we collapse the bottom band to Low so
/// the round-trip preserves the user-visible bucket.
// Vikunja's scale is 0=Unset, 1=Low, 2=Medium, 3=High, 4=Urgent, 5=DO NOW.
// Map by matching LABELS, not by spreading across the range: an earlier
// version mapped Medium→3 and High→5, so an Aperio "Medium" task showed up
// in Vikunja as "High" (and "High" as "DO NOW"). Aperio has three levels, so
// Vikunja's Urgent/DO NOW collapse to High on read (and write back as High);
// Unset reads as Low.
fn vikunja_priority_to_aperio(raw: i32) -> TaskPriority {
    match raw {
        i32::MIN..=1 => TaskPriority::Low,
        2 => TaskPriority::Medium,
        _ => TaskPriority::High,
    }
}

fn aperio_priority_to_vikunja(p: TaskPriority) -> i32 {
    match p {
        TaskPriority::Low => 1,
        TaskPriority::Medium => 2,
        TaskPriority::High => 3,
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
    fn priority_maps_by_label() {
        // Vikunja: 0 Unset, 1 Low, 2 Medium, 3 High, 4 Urgent, 5 DO NOW.
        assert_eq!(vikunja_priority_to_aperio(0), TaskPriority::Low); // unset → floor
        assert_eq!(vikunja_priority_to_aperio(1), TaskPriority::Low);
        assert_eq!(vikunja_priority_to_aperio(2), TaskPriority::Medium);
        assert_eq!(vikunja_priority_to_aperio(3), TaskPriority::High);
        assert_eq!(vikunja_priority_to_aperio(4), TaskPriority::High); // Urgent → High
        assert_eq!(vikunja_priority_to_aperio(5), TaskPriority::High); // DO NOW → High

        // Aperio's three levels map onto the SAME-labelled Vikunja values —
        // Medium → 2 (Medium), not 3 (which Vikunja labels "High").
        assert_eq!(aperio_priority_to_vikunja(TaskPriority::Low), 1);
        assert_eq!(aperio_priority_to_vikunja(TaskPriority::Medium), 2);
        assert_eq!(aperio_priority_to_vikunja(TaskPriority::High), 3);

        // The three Aperio levels round-trip exactly.
        for p in [TaskPriority::Low, TaskPriority::Medium, TaskPriority::High] {
            assert_eq!(vikunja_priority_to_aperio(aperio_priority_to_vikunja(p)), p);
        }
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
            buckets: Vec::new(),
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
            ..Default::default()
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
            buckets: Vec::new(),
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
            ..Default::default()
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
            buckets: Vec::new(),
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
            ..Default::default()
        };
        let task = map_task(entry, "1");
        assert!(task.scheduled_date.is_none());
        assert!(task.deadline_date.is_none());
    }

    #[test]
    fn extras_block_round_trips_backlog_recurrence() {
        use cal_core::{MonthDay, RecurrenceAnchor, RecurrenceEnd, RecurrencePlacement};
        // A seasonal backlog task — no native Vikunja recurrence expresses it.
        let mut task = task_fixture("7", "3", None);
        task.description = Some("Swap shoes".into());
        task.recurrence = Some(TaskRecurrence {
            frequency: RecurrenceFrequency::Yearly,
            interval: 1,
            day_of_week: None,
            day_of_month: None,
            end: Some(RecurrenceEnd::Never),
            anchor: RecurrenceAnchor::FromCompletion,
            placement: RecurrencePlacement::Backlog,
            fixed_dates: Some(vec![
                MonthDay { month: 4, day: 1 },
                MonthDay { month: 10, day: 1 },
            ]),
        });
        task.resurface_date = Some(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        task.series_id = Some("series-shoes".into());

        let body = task_to_body(&task);
        // User text + an Aperio block; native recurrence stays cleared so
        // Vikunja's scheduler doesn't double-spawn the backlog task.
        let desc = body.description.clone().unwrap();
        assert!(desc.starts_with("Swap shoes"));
        assert!(desc.contains("aperio:1:"));
        assert_eq!(body.repeat_after, 0);
        assert_eq!(body.repeat_mode, 0);

        // Read the body back as if the server echoed it.
        let entry = TaskEntry {
            id: 7,
            title: Some("Swap shoes".into()),
            description: body.description,
            repeat_after: body.repeat_after,
            repeat_mode: body.repeat_mode,
            ..Default::default()
        };
        let restored = map_task(entry, "3");
        assert_eq!(restored.description.as_deref(), Some("Swap shoes"));
        assert_eq!(restored.recurrence, task.recurrence);
        assert_eq!(restored.resurface_date, task.resurface_date);
        assert_eq!(restored.series_id.as_deref(), Some("series-shoes"));
    }

    #[test]
    fn plain_scheduled_recurrence_stays_in_native_fields() {
        // A daily rule has no non-native aspect, so it rides Vikunja's own
        // repeat fields and leaves the description block-free.
        let mut task = task_fixture("7", "3", None);
        task.description = Some("Standup".into());
        task.recurrence = Some(TaskRecurrence {
            frequency: RecurrenceFrequency::Daily,
            interval: 1,
            day_of_week: None,
            day_of_month: None,
            end: None,
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
        });
        let body = task_to_body(&task);
        assert_eq!(body.description.as_deref(), Some("Standup"));
        assert!(body.repeat_after > 0, "daily period rides the native field");
    }

    #[test]
    fn recurrence_maps_the_shapes_vikunja_can_store() {
        let rule = |frequency, interval| TaskRecurrence {
            frequency,
            interval,
            day_of_week: None,
            day_of_month: None,
            end: None,
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
        };
        // Aperio → Vikunja: daily/weekly become a seconds period (mode 0),
        // monthly uses Vikunja's monthly mode, yearly can't be stored.
        assert_eq!(
            recurrence_to_vikunja(&rule(RecurrenceFrequency::Daily, 3)),
            Some((3 * 86_400, 0)),
        );
        assert_eq!(
            recurrence_to_vikunja(&rule(RecurrenceFrequency::Weekly, 2)),
            Some((2 * 604_800, 0)),
        );
        assert_eq!(
            recurrence_to_vikunja(&rule(RecurrenceFrequency::Monthly, 1)),
            Some((0, 1)),
        );
        assert_eq!(
            recurrence_to_vikunja(&rule(RecurrenceFrequency::Yearly, 1)),
            None,
        );

        // Vikunja → Aperio: mode 1 → monthly; a weekly multiple → weekly;
        // a day multiple → daily; mode 2 keeps the interval (anchor lost);
        // sub-day / none → no recurrence.
        assert_eq!(
            recurrence_from_vikunja(0, 1).map(|r| r.frequency),
            Some(RecurrenceFrequency::Monthly),
        );
        let weekly = recurrence_from_vikunja(2 * 604_800, 0).expect("weekly");
        assert_eq!(weekly.frequency, RecurrenceFrequency::Weekly);
        assert_eq!(weekly.interval, 2);
        let daily = recurrence_from_vikunja(3 * 86_400, 2).expect("daily");
        assert_eq!(daily.frequency, RecurrenceFrequency::Daily);
        assert_eq!(daily.interval, 3);
        assert!(recurrence_from_vikunja(0, 0).is_none());
        assert!(recurrence_from_vikunja(3_600, 0).is_none());
    }

    #[test]
    fn new_task_body_carries_and_clears_recurrence() {
        let mut new = sample_new_task();
        new.recurrence = Some(TaskRecurrence {
            frequency: RecurrenceFrequency::Daily,
            interval: 1,
            day_of_week: None,
            day_of_month: None,
            end: None,
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
        });
        let body = new_task_to_body(&new);
        assert_eq!(body.repeat_after, 86_400);
        assert_eq!(body.repeat_mode, 0);
        // Clearing recurrence sends 0/0 so Vikunja actually drops it.
        new.recurrence = None;
        let cleared = new_task_to_body(&new);
        assert_eq!(cleared.repeat_after, 0);
        assert_eq!(cleared.repeat_mode, 0);
    }

    // ── Body shape (NewTask → wire) ────────────────────────────

    fn sample_new_task() -> NewTask {
        NewTask {
            assignees: Vec::new(),
            title: "Buy bread".into(),
            description: Some("Bakery".into()),
            status: TaskStatus::Open,
            priority: TaskPriority::High,
            effort: cal_core::TaskEffort::Medium,
            scheduled_date: Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
            scheduled_time: Some(NaiveTime::from_hms_opt(8, 0, 0).unwrap()),
            deadline_date: Some(NaiveDate::from_ymd_opt(2026, 5, 23).unwrap()),
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            resurface_date: None,
            series_id: None,
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
        // sample_new_task() is High → Vikunja 3 ("High"), not 5 ("DO NOW").
        assert_eq!(body.priority, 3);
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

    // ── Member sharing body ────────────────────────────────────

    #[test]
    fn share_body_sends_username_and_permission_keys() {
        // Current Vikunja keys the share on `username` + `permission`;
        // older servers read `user_id` + `right`. We send both names for
        // each — sending only the old names left the user unresolved
        // (error 1005) and the level stuck at Read.
        let body = share_user_body("alice", Some(MemberRight::Write));
        assert_eq!(body["username"], "alice");
        assert_eq!(body["user_id"], "alice");
        assert_eq!(body["permission"], 1);
        assert_eq!(body["right"], 1);
    }

    #[test]
    fn permission_body_sends_both_level_keys() {
        let admin = member_permission_body(MemberRight::Admin);
        assert_eq!(admin["permission"], 2);
        assert_eq!(admin["right"], 2);
        let read = member_permission_body(MemberRight::Read);
        assert_eq!(read["permission"], 0);
        assert_eq!(read["right"], 0);
    }

    #[test]
    fn member_reads_back_permission_field() {
        // A current-server row uses `permission`; a legacy row uses
        // `right`. Both must map to the same level.
        let modern: ProjectUserEntry =
            serde_json::from_str(r#"{"id":3,"username":"bob","permission":2}"#).unwrap();
        assert_eq!(modern.permission, 2);
        let legacy: ProjectUserEntry =
            serde_json::from_str(r#"{"id":3,"username":"bob","right":1}"#).unwrap();
        assert_eq!(legacy.permission, 1);
    }

    #[tokio::test]
    async fn add_member_puts_username_in_body() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("PUT", "/api/v1/projects/7/users")
            // The mock only matches when the request body carries the
            // `username` field — i.e. the 1005 regression is fixed.
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"username":"alice"}"#.into(),
            ))
            .with_status(200)
            .with_body(r#"{"id":1,"username":"alice","right":1}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        add_task_list_member(&client, "7", "alice", Some(MemberRight::Write))
            .await
            .unwrap();
        m.assert_async().await;
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
        // No kanban view → the per-view bucket stitch degrades and we
        // read the flat `bucket_id` (older-server / section-less path).
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        let _m = server
            .mock(
                "GET",
                "/api/v1/projects/3/tasks?page=1&per_page=50&expand=buckets",
            )
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
        // Vikunja priority 3 is "High".
        assert_eq!(tasks[0].priority, TaskPriority::High);
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
    async fn list_task_lists_skips_pseudo_projects() {
        // Vikunja injects Favorites (id -1) and saved filters (negative
        // ids) into GET /projects. They must be dropped — creating a task
        // in one returns error 3001 ("This project does not exist").
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/projects?page=1&per_page=50")
            .with_status(200)
            .with_body(
                r#"[{"id":-1,"title":"Favorites","parent_project_id":0},{"id":-2,"title":"My filter","parent_project_id":0},{"id":5,"title":"Real","parent_project_id":0}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let lists = list_task_lists(&client).await.unwrap();
        assert_eq!(lists.len(), 1, "only the real project should remain");
        assert_eq!(lists[0].id, "5");
        assert_eq!(lists[0].name, "Real");
    }

    #[tokio::test]
    async fn get_tasks_maps_bucket_to_section() {
        let mut server = Server::new_async().await;
        // Older-server / no-kanban-view path: the flat `bucket_id` on the
        // task maps straight to the section.
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        let _m = server
            .mock(
                "GET",
                "/api/v1/projects/3/tasks?page=1&per_page=50&expand=buckets",
            )
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
    async fn get_tasks_stitches_per_view_bucket() {
        // ≥0.24: the task carries no flat bucket_id; its bucket comes from
        // the expanded `buckets` array, matched to the project's kanban
        // view (id 11). A bucket from a *different* view must be ignored.
        let mut server = Server::new_async().await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(r#"[{"id":11,"view_kind":"kanban","default_bucket_id":5}]"#)
            .create_async()
            .await;
        let _m = server
            .mock(
                "GET",
                "/api/v1/projects/3/tasks?page=1&per_page=50&expand=buckets",
            )
            .with_status(200)
            .with_body(
                r#"[{"id":7,"title":"Grouped","project_id":3,"buckets":[{"id":9,"project_view_id":11},{"id":99,"project_view_id":22}]},{"id":8,"title":"Loose","project_id":3,"buckets":[]}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let tasks = get_tasks(&client, "3").await.unwrap();
        // Bucket 9 belongs to the kanban view (11); bucket 99 (view 22) is
        // ignored.
        assert_eq!(tasks[0].section_id.as_deref(), Some("9"));
        // No bucket in this view ⇒ ungrouped.
        assert!(tasks[1].section_id.is_none());
    }

    #[tokio::test]
    async fn get_tasks_blanks_the_done_bucket() {
        // Vikunja's done bucket (id 9 here) is a done-status mechanism, not a
        // user section — a task it filed there on completion must read back
        // section-less, not as if it lived in that bucket (DESIGN §8.2).
        let mut server = Server::new_async().await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(
                r#"[{"id":11,"view_kind":"kanban","default_bucket_id":5,"done_bucket_id":9}]"#,
            )
            .create_async()
            .await;
        let _m = server
            .mock(
                "GET",
                "/api/v1/projects/3/tasks?page=1&per_page=50&expand=buckets",
            )
            .with_status(200)
            .with_body(
                r#"[{"id":7,"title":"Filed done","project_id":3,"buckets":[{"id":9,"project_view_id":11}]},{"id":8,"title":"In a real bucket","project_id":3,"buckets":[{"id":12,"project_view_id":11}]}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let tasks = get_tasks(&client, "3").await.unwrap();
        // In the done bucket ⇒ no section.
        assert!(tasks[0].section_id.is_none());
        // In a regular bucket ⇒ that section.
        assert_eq!(tasks[1].section_id.as_deref(), Some("12"));
    }

    #[tokio::test]
    async fn update_task_never_moves_into_the_done_bucket() {
        // A move whose target resolves to the done bucket (id 9) must be
        // skipped — moving a task into the done bucket would mark it done.
        let mut server = Server::new_async().await;
        mount_update_prelude(&mut server).await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(
                r#"[{"id":11,"view_kind":"kanban","default_bucket_id":5,"done_bucket_id":9}]"#,
            )
            .create_async()
            .await;
        let _current = server
            .mock("GET", "/api/v1/tasks/7?expand=buckets")
            .with_status(200)
            .with_body(r#"{"id":7,"buckets":[{"id":5,"project_view_id":11}]}"#)
            .create_async()
            .await;
        let mv = server
            .mock("POST", "/api/v1/projects/3/views/11/buckets/9/tasks")
            .expect(0)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        // section_id "9" == the done bucket → the move is skipped.
        update_task(&client, &task_fixture("7", "3", Some("9")))
            .await
            .unwrap();
        mv.assert_async().await;
    }

    #[tokio::test]
    async fn update_task_reopens_out_of_done_bucket_when_default_is_done() {
        // The pathological case the user hit (buckets deleted down to one):
        // default_bucket_id == done_bucket_id (both 9). A reopened task sits
        // in the done bucket (current 9) with no section; it must be moved to
        // a real OPEN bucket (the leftmost non-done one, 5), not stranded.
        let mut server = Server::new_async().await;
        mount_update_prelude(&mut server).await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(
                r#"[{"id":11,"view_kind":"kanban","default_bucket_id":9,"done_bucket_id":9}]"#,
            )
            .create_async()
            .await;
        let _current = server
            .mock("GET", "/api/v1/tasks/7?expand=buckets")
            .with_status(200)
            .with_body(r#"{"id":7,"buckets":[{"id":9,"project_view_id":11}]}"#)
            .create_async()
            .await;
        // The default == done, so the move target falls back to the leftmost
        // OPEN bucket: 9 is excluded, 5 is the lowest-position survivor.
        let _buckets = server
            .mock("GET", "/api/v1/projects/3/views/11/buckets")
            .with_status(200)
            .with_body(r#"[{"id":9,"position":0},{"id":5,"position":1}]"#)
            .create_async()
            .await;
        let mv = server
            .mock("POST", "/api/v1/projects/3/views/11/buckets/5/tasks")
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        // No section, currently in the done bucket → relocated to bucket 5.
        let result = update_task(&client, &task_fixture("7", "3", None))
            .await
            .unwrap();
        mv.assert_async().await;
        assert_eq!(result.section_id.as_deref(), Some("5"));
    }

    #[tokio::test]
    async fn list_sections_hides_the_done_bucket() {
        // The done bucket is a status mechanism, not a user-pickable section —
        // list_sections must not surface it.
        let mut server = Server::new_async().await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(
                r#"[{"id":11,"view_kind":"kanban","default_bucket_id":5,"done_bucket_id":9}]"#,
            )
            .create_async()
            .await;
        let _buckets = server
            .mock("GET", "/api/v1/projects/3/views/11/buckets")
            .with_status(200)
            .with_body(
                r#"[{"id":5,"title":"To Do","position":0},{"id":9,"title":"Done","position":1},{"id":12,"title":"Doing","position":2}]"#,
            )
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let sections = list_sections(&client, "3").await.unwrap();
        let ids: Vec<&str> = sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["5", "12"], "the done bucket (9) is filtered out");
    }

    fn task_fixture(id: &str, list_id: &str, section_id: Option<&str>) -> Task {
        Task {
            assignees: Vec::new(),
            id: id.into(),
            list_id: list_id.into(),
            title: "Edit me".into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            effort: cal_core::TaskEffort::Medium,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            resurface_date: None,
            series_id: None,
            section_id: section_id.map(str::to_string),
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: None,
            etag: None,
        }
    }

    /// Mounts the field-update PUT + assignee-sync that every
    /// `update_task` issues before the bucket move.
    async fn mount_update_prelude(server: &mut Server) {
        server
            .mock("POST", "/api/v1/tasks/7")
            .with_status(200)
            .with_body(r#"{"id":7,"title":"Edit me","project_id":3}"#)
            .create_async()
            .await;
        server
            .mock("POST", "/api/v1/tasks/7/assignees/bulk")
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
    }

    #[tokio::test]
    async fn update_task_moves_bucket_when_section_changed() {
        let mut server = Server::new_async().await;
        mount_update_prelude(&mut server).await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(r#"[{"id":11,"view_kind":"kanban","default_bucket_id":5}]"#)
            .create_async()
            .await;
        // The task currently sits in bucket 9 of the kanban view.
        let _current = server
            .mock("GET", "/api/v1/tasks/7?expand=buckets")
            .with_status(200)
            .with_body(r#"{"id":7,"buckets":[{"id":9,"project_view_id":11}]}"#)
            .create_async()
            .await;
        // …and is moved to bucket 12 (the new section).
        let mv = server
            .mock("POST", "/api/v1/projects/3/views/11/buckets/12/tasks")
            // The body must carry `task_id` (path-redundant `bucket_id`
            // too) — a wrong field name would fail a real server.
            .match_body(mockito::Matcher::Json(
                serde_json::json!({ "task_id": 7, "bucket_id": 12 }),
            ))
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let result = update_task(&client, &task_fixture("7", "3", Some("12")))
            .await
            .unwrap();
        mv.assert_async().await;
        assert_eq!(result.section_id.as_deref(), Some("12"));
    }

    #[tokio::test]
    async fn update_task_skips_move_when_section_unchanged() {
        let mut server = Server::new_async().await;
        mount_update_prelude(&mut server).await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(r#"[{"id":11,"view_kind":"kanban","default_bucket_id":5}]"#)
            .create_async()
            .await;
        let _current = server
            .mock("GET", "/api/v1/tasks/7?expand=buckets")
            .with_status(200)
            .with_body(r#"{"id":7,"buckets":[{"id":9,"project_view_id":11}]}"#)
            .create_async()
            .await;
        // Desired section == current bucket → the move endpoint must NOT
        // be called (no reorder on an unrelated edit).
        let mv = server
            .mock("POST", "/api/v1/projects/3/views/11/buckets/9/tasks")
            .expect(0)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        update_task(&client, &task_fixture("7", "3", Some("9")))
            .await
            .unwrap();
        mv.assert_async().await;
    }

    #[tokio::test]
    async fn update_task_completed_skips_bucket_move() {
        // Vikunja files a done task into its kanban done bucket; moving it
        // back to its section bucket would flip it undone. So completing a
        // task must not touch the kanban bucket at all — the whole move path
        // (views lookup included) is skipped.
        let mut server = Server::new_async().await;
        mount_update_prelude(&mut server).await;
        let views = server
            .mock("GET", "/api/v1/projects/3/views")
            .expect(0)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let mut done = task_fixture("7", "3", Some("12"));
        done.status = TaskStatus::Completed;
        done.completed_at = Some(Utc::now());
        update_task(&client, &done).await.unwrap();
        views.assert_async().await;
    }

    #[tokio::test]
    async fn update_task_section_none_moves_to_default_bucket() {
        let mut server = Server::new_async().await;
        mount_update_prelude(&mut server).await;
        // Default bucket 5 — the target for "no section" (Vikunja kanban
        // has no ungrouped state).
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(r#"[{"id":11,"view_kind":"kanban","default_bucket_id":5}]"#)
            .create_async()
            .await;
        let _current = server
            .mock("GET", "/api/v1/tasks/7?expand=buckets")
            .with_status(200)
            .with_body(r#"{"id":7,"buckets":[{"id":9,"project_view_id":11}]}"#)
            .create_async()
            .await;
        let mv = server
            .mock("POST", "/api/v1/projects/3/views/11/buckets/5/tasks")
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let result = update_task(&client, &task_fixture("7", "3", None))
            .await
            .unwrap();
        mv.assert_async().await;
        // "No section" on Vikunja means the default bucket (there is no
        // ungrouped state), so the task reports that bucket — not `None`.
        assert_eq!(result.section_id.as_deref(), Some("5"));
    }

    #[tokio::test]
    async fn update_task_degrades_without_kanban_view() {
        let mut server = Server::new_async().await;
        mount_update_prelude(&mut server).await;
        // No kanban view → the move is skipped, but the field edit still
        // succeeds.
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let result = update_task(&client, &task_fixture("7", "3", Some("12")))
            .await
            .unwrap();
        assert_eq!(result.title, "Edit me");
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
    async fn create_section_puts_bucket_on_kanban_view() {
        let mut server = Server::new_async().await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(r#"[{"id":11,"view_kind":"kanban","default_bucket_id":21}]"#)
            .create_async()
            .await;
        let m = server
            .mock("PUT", "/api/v1/projects/3/views/11/buckets")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({ "title": "Backlog" }),
            ))
            .with_status(200)
            .with_body(r#"{"id":25,"title":"Backlog","position":3.0}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let section = create_section(&client, "3", "Backlog").await.unwrap();
        m.assert_async().await;
        assert_eq!(section.id, "25");
        assert_eq!(section.name, "Backlog");
        assert_eq!(section.list_id, "3");
    }

    #[tokio::test]
    async fn update_section_renames_bucket() {
        let mut server = Server::new_async().await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(r#"[{"id":11,"view_kind":"kanban"}]"#)
            .create_async()
            .await;
        let m = server
            .mock("POST", "/api/v1/projects/3/views/11/buckets/22")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({ "title": "Done" }),
            ))
            .with_status(200)
            .with_body(r#"{"id":22,"title":"Done","position":2.0}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let section = update_section(&client, "3", "22", "Done").await.unwrap();
        m.assert_async().await;
        assert_eq!(section.name, "Done");
    }

    #[tokio::test]
    async fn delete_section_deletes_bucket() {
        let mut server = Server::new_async().await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(r#"[{"id":11,"view_kind":"kanban"}]"#)
            .create_async()
            .await;
        let m = server
            .mock("DELETE", "/api/v1/projects/3/views/11/buckets/22")
            .with_status(200)
            .with_body(r#"{"message":"ok"}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        delete_section(&client, "3", "22").await.unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn create_section_errors_without_kanban_view() {
        let mut server = Server::new_async().await;
        let _views = server
            .mock("GET", "/api/v1/projects/3/views")
            .with_status(200)
            .with_body(r#"[{"id":10,"view_kind":"list"}]"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        // No kanban view → managing sections isn't possible.
        assert!(create_section(&client, "3", "Backlog").await.is_err());
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

    fn new_task_min(title: &str) -> NewTask {
        NewTask {
            title: title.into(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            effort: cal_core::TaskEffort::Medium,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            resurface_date: None,
            series_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            assignees: Vec::new(),
        }
    }

    #[test]
    fn create_body_omits_zero_project_and_id() {
        // A zero `project_id` in the body overrode the URL path on current
        // Vikunja, pinning every create to project 0 → error 3001. The
        // create body must carry neither it nor a zero id.
        let body = serde_json::to_value(new_task_to_body(&new_task_min("Play"))).unwrap();
        assert!(
            body.get("project_id").is_none(),
            "zero project_id leaked into the create body: {body}"
        );
        assert!(body.get("id").is_none(), "zero id leaked: {body}");
        assert_eq!(body["title"], "Play");
    }

    #[tokio::test]
    async fn create_task_targets_project_url_without_zero_project_id() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("PUT", "/api/v1/projects/5/tasks")
            // The mock matches only if the body does NOT pin project_id to 0.
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::PartialJsonString(r#"{"title":"Play"}"#.into()),
            ]))
            .with_status(200)
            .with_body(r#"{"id":99,"title":"Play","project_id":5}"#)
            .create_async()
            .await;
        let client = fixture_client(&server.url());
        let task = create_task(&client, "5", new_task_min("Play"))
            .await
            .unwrap();
        m.assert_async().await;
        assert_eq!(task.id, "99");
        assert_eq!(task.list_id, "5");
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
