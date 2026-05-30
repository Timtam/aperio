//! Task list and task commands.

use cal_adapter_local::LocalAdapter;
use cal_core::{NewTask, Section, Task, TaskList, TasksFeature};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use sync_core::{EventPayload, IdPayload, SyncEvent};
use tauri::State;

use plugin_core::{PluginManager, TaskCapabilities};

use super::plugins::plugin_id_for_adapter_kind;
use super::{CommandError, CommandResult};
use crate::accounts::{AccountsRepo, AdapterKind};
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;
use crate::overrides::{apply_to_task_lists, OverridesRepo};
use crate::registry::{AdapterRegistry, LOCAL_ID};
use crate::reminders::SchedulerHandle;

/// Wire-format TaskList enriched with the owning account id. Same
/// shape + rationale as `CalendarRow` — the frontend uses it to
/// group containers by source for the account-aware sidebar.
///
/// `inner` is `serde(flatten)`ed, so `TaskList.parent_id` rides along
/// at the top level for free — the nested-project tree in the sidebar
/// reads it without a second round-trip.
#[derive(Debug, Serialize)]
pub struct TaskListRow {
    #[serde(flatten)]
    pub inner: TaskList,
    pub account_id: String,
    /// Task-organisation shapes the owning adapter supports (nested
    /// projects, sections, subtask depth, …), resolved from the
    /// account's plugin manifest. The frontend gates affordances on
    /// these — e.g. only shows "add section" where `sections` is true.
    /// Local + unknown sources report [`TaskCapabilities::default`].
    pub task_capabilities: TaskCapabilities,
}

/// The local SQLite store's task capabilities. Unlike a plugin-backed
/// account it has no manifest, so we hard-code what the store actually
/// supports: it nests projects (`task_lists.parent_id`) and groups
/// tasks into sections, on top of the cal-core-native subtasks /
/// recurrence / cross-list-move support the default already carries.
fn local_task_capabilities() -> TaskCapabilities {
    TaskCapabilities {
        nested_projects: true,
        sections: true,
        ..TaskCapabilities::default()
    }
}

/// Resolve an account's task capabilities from its plugin manifest.
/// Mirrors `recurrence_caps_for_account` in `calendars.rs`: the local
/// store reports its own capabilities; accounts whose plugin we can't
/// resolve fall back to the permissive cal-core-native default.
fn task_caps_for_account(
    account_id: &str,
    account_kinds: &std::collections::HashMap<String, AdapterKind>,
    plugin_manager: &PluginManager,
) -> TaskCapabilities {
    if account_id == LOCAL_ID {
        return local_task_capabilities();
    }
    let Some(kind) = account_kinds.get(account_id) else {
        return TaskCapabilities::default();
    };
    let Some(plugin_id) = plugin_id_for_adapter_kind(*kind) else {
        // No plugin for this kind — default capabilities.
        return TaskCapabilities::default();
    };
    plugin_manager
        .get_including_disabled(plugin_id)
        .map(|p| p.manifest.tasks.clone())
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskListRequest {
    pub name: String,
    pub embedded_in_calendar: Option<String>,
}

#[tauri::command]
pub async fn list_task_lists(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    db: State<'_, DbHandle>,
) -> CommandResult<Vec<TaskListRow>> {
    let local = adapter.list_task_lists().await?;
    for l in &local {
        registry.note_task_list_route(&l.id, LOCAL_ID);
    }
    let mut external = registry.list_external_task_lists().await;
    let mut out = local;
    out.append(&mut external);
    let shared = db.shared();
    let repo = OverridesRepo::new(&shared);
    apply_to_task_lists(&repo, &mut out);
    // Snapshot account_id → adapter_kind once so the per-row caps
    // lookup is a cheap map hit. Same permissive-on-failure default
    // as the calendar path: a read failure degrades to "every account
    // looks local" → full cal-core-native capabilities.
    let account_kinds: std::collections::HashMap<String, AdapterKind> = AccountsRepo::new(&shared)
        .list()
        .map(|accounts| {
            accounts
                .into_iter()
                .map(|a| (a.id, a.adapter_kind))
                .collect()
        })
        .unwrap_or_default();
    Ok(out
        .into_iter()
        .map(|list| {
            let account_id = registry
                .account_for_task_list(&list.id)
                .unwrap_or_else(|| LOCAL_ID.to_string());
            let task_capabilities =
                task_caps_for_account(&account_id, &account_kinds, &plugin_manager);
            TaskListRow {
                inner: list,
                account_id,
                task_capabilities,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn create_task_list(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: CreateTaskListRequest,
) -> CommandResult<TaskListRow> {
    let list = adapter.create_task_list(&request.name, None, None, request.embedded_in_calendar)?;
    if let Ok(fields) = serde_json::to_value(&list) {
        event_log.append(SyncEvent::TaskListCreated(EventPayload {
            id: list.id.clone(),
            fields,
        }));
    }
    Ok(TaskListRow {
        inner: list,
        account_id: LOCAL_ID.to_string(),
        // Freshly-created lists are always local — report the local
        // store's capabilities (nested projects + sections).
        task_capabilities: local_task_capabilities(),
    })
}

#[tauri::command]
pub async fn delete_task_list(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
) -> CommandResult<()> {
    adapter.delete_task_list(&id)?;
    event_log.append(SyncEvent::TaskListDeleted(IdPayload { id: id.clone() }));
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct ReparentTaskListRequest {
    pub id: String,
    /// New parent list id, or `None` to promote to top level.
    pub parent_id: Option<String>,
}

/// Reparent a local task list under another (or to the top level).
/// Local-store only — external-provider projects are reparented in
/// their own UI; the frontend gates the gesture to local lists. The
/// backend independently enforces the no-self / no-cycle invariant so a
/// buggy caller can't corrupt the tree, then emits a `task_list.updated`
/// event so the move propagates cross-device.
#[tauri::command]
pub async fn reparent_task_list(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: ReparentTaskListRequest,
) -> CommandResult<TaskList> {
    if let Some(parent) = &request.parent_id {
        if parent == &request.id {
            return Err(CommandError {
                code: "invalid",
                message: "a task list cannot be its own parent".into(),
            });
        }
        // Walk up the prospective parent's ancestor chain; reaching the
        // moved list means the move would form a cycle.
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(parent.clone());
        while let Some(cur) = cursor {
            if cur == request.id {
                return Err(CommandError {
                    code: "invalid",
                    message: "reparenting would create a cycle".into(),
                });
            }
            if !seen.insert(cur.clone()) {
                break;
            }
            cursor = adapter.get_task_list_by_id(&cur)?.and_then(|l| l.parent_id);
        }
    }

    let updated = adapter.reparent_task_list(&request.id, request.parent_id.as_deref())?;
    if let Ok(fields) = serde_json::to_value(&updated) {
        event_log.append(SyncEvent::TaskListUpdated(EventPayload {
            id: updated.id.clone(),
            fields,
        }));
    }
    Ok(updated)
}

#[tauri::command]
pub async fn get_tasks(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    list_id: String,
) -> CommandResult<Vec<Task>> {
    let account = registry
        .account_for_task_list(&list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(adapter.get_tasks(&list_id).await?);
    }
    let Some(ext) = registry.task_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("task list '{list_id}' is not routable"),
        });
    };
    Ok(ext.get_tasks(&list_id).await?)
}

/// List the sections (Vikunja buckets / Todoist sections) of one
/// list. Routes by the list's owning account exactly like
/// `get_tasks`: local lists hit the SQLite store, external lists hit
/// the provider adapter. Section-less backends return an empty list
/// via the `TasksFeature::list_sections` default.
#[tauri::command]
pub async fn get_sections(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    list_id: String,
) -> CommandResult<Vec<Section>> {
    let account = registry
        .account_for_task_list(&list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
        return Ok(adapter.list_sections(&list_id).await?);
    }
    let Some(ext) = registry.task_adapter(&account) else {
        return Err(CommandError {
            code: "not_found",
            message: format!("task list '{list_id}' is not routable"),
        });
    };
    Ok(ext.list_sections(&list_id).await?)
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub list_id: String,
    #[serde(flatten)]
    pub task: NewTask,
}

#[tauri::command]
pub async fn get_task_by_id(
    adapter: State<'_, LocalAdapter>,
    id: String,
) -> CommandResult<Option<Task>> {
    Ok(adapter.get_task_by_id(&id)?)
}

#[tauri::command]
pub async fn create_task(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: CreateTaskRequest,
) -> CommandResult<Task> {
    let account = registry
        .account_for_task_list(&request.list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    let is_local = account == LOCAL_ID;
    let task = if is_local {
        adapter.create_task(&request.list_id, request.task).await?
    } else {
        let Some(ext) = registry.task_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("task list '{}' is not routable", request.list_id),
            });
        };
        ext.create_task(&request.list_id, request.task).await?
    };
    if is_local {
        if let Ok(fields) = serde_json::to_value(&task) {
            event_log.append(SyncEvent::TaskCreated(EventPayload {
                id: task.id.clone(),
                fields,
            }));
        }
    }
    scheduler.invalidate();
    Ok(task)
}

#[tauri::command]
pub async fn update_task(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    task: Task,
    previous_list_id: Option<String>,
) -> CommandResult<Task> {
    let target_account = registry
        .account_for_task_list(&task.list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());

    // Cross-list move detection — same shape as `update_event`'s
    // cross-calendar move guard. The TaskDialog's list picker
    // doubles as a "move to another list" gesture; without this
    // hint, a save against a different list would PATCH the
    // wrong resource (CalDAV VTODO at the old URL → 412 with
    // If-Match; Google Tasks `tasks.patch` against the wrong
    // tasklist → 404; iCloud-shaped CalDAV → Conflict).
    let is_move = previous_list_id
        .as_deref()
        .map(|prev| prev != task.list_id)
        .unwrap_or(false);

    if is_move {
        let previous = previous_list_id.expect("checked above");
        let source_account = registry
            .account_for_task_list(&previous)
            .unwrap_or_else(|| LOCAL_ID.to_string());

        // Local ↔ Local: the LocalAdapter does the move via a
        // single SQL UPDATE on the list_id column. No
        // create+delete dance needed.
        if source_account == LOCAL_ID && target_account == LOCAL_ID {
            let updated = adapter.update_task(task).await?;
            // Local↔Local task move = single SQL UPDATE on list_id.
            // Emit one TaskUpdated with the full row.
            if let Ok(fields) = serde_json::to_value(&updated) {
                event_log.append(SyncEvent::TaskUpdated(EventPayload {
                    id: updated.id.clone(),
                    fields,
                }));
            }
            scheduler.invalidate();
            return Ok(updated);
        }

        // Cross-list move involving an external adapter — works
        // across providers too (Google Tasks → Todoist, iCloud
        // CalDAV-VTODO → Microsoft To Do, etc.). The new task
        // gets a fresh adapter-assigned id; the frontend's
        // dataVersion bump on dialog-close forces a refetch that
        // surfaces the new id naturally without any caller
        // having to translate the old id to the new one. Create
        // BEFORE delete so a half-failed move never leaves the
        // user with nothing.
        let new_payload = NewTask {
            title: task.title.clone(),
            description: task.description.clone(),
            status: task.status,
            priority: task.priority,
            scheduled_date: task.scheduled_date,
            scheduled_time: task.scheduled_time,
            deadline_date: task.deadline_date,
            deadline_time: task.deadline_time,
            recurrence: task.recurrence.clone(),
            parent_id: task.parent_id.clone(),
            section_id: None,
            color_label: task.color_label.clone(),
            reminders: task.reminders.clone(),
            sound: task.sound.clone(),
        };

        let created = if target_account == LOCAL_ID {
            adapter.create_task(&task.list_id, new_payload).await?
        } else {
            let Some(ext) = registry.task_adapter(&target_account) else {
                return Err(CommandError {
                    code: "not_found",
                    message: format!("target task list '{}' is not routable", task.list_id,),
                });
            };
            ext.create_task(&task.list_id, new_payload).await?
        };

        // Delete from source. Warn-but-continue on failure: the
        // create at the target already succeeded, and a bubbled
        // error here would tempt the user to retry, doubling
        // the duplicate. A leftover row in the source list is
        // the lesser evil.
        let delete_result = if source_account == LOCAL_ID {
            adapter
                .delete_task(&task.id)
                .await
                .map_err(CommandError::from)
        } else if let Some(ext) = registry.task_adapter(&source_account) {
            ext.delete_task(&task.id).await.map_err(CommandError::from)
        } else {
            Ok(())
        };
        if let Err(err) = delete_result {
            tracing::warn!(
                task_id = %task.id,
                source = %previous,
                target = %task.list_id,
                code = %err.code,
                message = %err.message,
                "delete from source task list failed after move; duplicate may exist",
            );
        }

        // Sync-event emission: same shape as the event move —
        // each LOCAL side emits its own event, external sides
        // stay silent and rely on the provider's sync mesh.
        if target_account == LOCAL_ID {
            if let Ok(fields) = serde_json::to_value(&created) {
                event_log.append(SyncEvent::TaskCreated(EventPayload {
                    id: created.id.clone(),
                    fields,
                }));
            }
        }
        if source_account == LOCAL_ID {
            event_log.append(SyncEvent::TaskDeleted(IdPayload {
                id: task.id.clone(),
            }));
        }

        scheduler.invalidate();
        return Ok(created);
    }

    // Plain in-place update.
    let is_local = target_account == LOCAL_ID;
    let updated = if is_local {
        adapter.update_task(task).await?
    } else {
        let Some(ext) = registry.task_adapter(&target_account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("task list '{}' is not routable", task.list_id),
            });
        };
        ext.update_task(task).await?
    };
    if is_local {
        if let Ok(fields) = serde_json::to_value(&updated) {
            event_log.append(SyncEvent::TaskUpdated(EventPayload {
                id: updated.id.clone(),
                fields,
            }));
        }
    }
    scheduler.invalidate();
    Ok(updated)
}

#[tauri::command]
pub async fn delete_task(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
    list_id: Option<String>,
) -> CommandResult<()> {
    let account = list_id
        .as_deref()
        .and_then(|lid| registry.account_for_task_list(lid))
        .unwrap_or_else(|| LOCAL_ID.to_string());
    let is_local = account == LOCAL_ID;
    if is_local {
        adapter.delete_task(&id).await?;
    } else {
        let Some(ext) = registry.task_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("account '{account}' is not routable"),
            });
        };
        ext.delete_task(&id).await?;
    }
    if is_local {
        event_log.append(SyncEvent::TaskDeleted(IdPayload { id: id.clone() }));
    }
    scheduler.invalidate();
    Ok(())
}

// ── Section commands ────────────────────────────────────────────────
//
// Sections are currently a local-store concept: the user creates and
// reorders them on local lists, and they propagate cross-device via the
// `section.*` event log. External-provider sections (Vikunja buckets,
// Todoist sections) are read-only here — they surface through
// `get_sections` but are managed in the provider's own UI, so these
// mutation commands always target the local adapter.

#[derive(Debug, Deserialize)]
pub struct CreateSectionRequest {
    pub list_id: String,
    pub name: String,
    /// Display order; defaults to 0 (the frontend appends with the
    /// current section count to keep new sections at the bottom).
    #[serde(default)]
    pub position: u32,
}

#[tauri::command]
pub async fn create_section(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: CreateSectionRequest,
) -> CommandResult<Section> {
    let section = adapter.create_section(&request.list_id, &request.name, request.position)?;
    if let Ok(fields) = serde_json::to_value(&section) {
        event_log.append(SyncEvent::SectionCreated(EventPayload {
            id: section.id.clone(),
            fields,
        }));
    }
    Ok(section)
}

#[tauri::command]
pub async fn update_section(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    section: Section,
) -> CommandResult<Section> {
    let updated = adapter.update_section(section)?;
    if let Ok(fields) = serde_json::to_value(&updated) {
        event_log.append(SyncEvent::SectionUpdated(EventPayload {
            id: updated.id.clone(),
            fields,
        }));
    }
    Ok(updated)
}

#[tauri::command]
pub async fn delete_section(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
) -> CommandResult<()> {
    adapter.delete_section(&id)?;
    event_log.append(SyncEvent::SectionDeleted(IdPayload { id: id.clone() }));
    Ok(())
}
