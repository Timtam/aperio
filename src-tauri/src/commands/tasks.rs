//! Task list and task commands.

use cal_adapter_local::LocalAdapter;
use cal_core::{NewTask, Task, TaskList, TasksFeature};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use sync_core::{EventPayload, IdPayload, SyncEvent};
use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;
use crate::overrides::{apply_to_task_lists, OverridesRepo};
use crate::registry::{AdapterRegistry, LOCAL_ID};
use crate::reminders::SchedulerHandle;

/// Wire-format TaskList enriched with the owning account id. Same
/// shape + rationale as `CalendarRow` — the frontend uses it to
/// group containers by source for the account-aware sidebar.
#[derive(Debug, Serialize)]
pub struct TaskListRow {
    #[serde(flatten)]
    pub inner: TaskList,
    pub account_id: String,
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
    Ok(out
        .into_iter()
        .map(|list| {
            let account_id = registry
                .account_for_task_list(&list.id)
                .unwrap_or_else(|| LOCAL_ID.to_string());
            TaskListRow {
                inner: list,
                account_id,
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
    let list =
        adapter.create_task_list(&request.name, None, None, request.embedded_in_calendar)?;
    if let Ok(fields) = serde_json::to_value(&list) {
        event_log.append(SyncEvent::TaskListCreated(EventPayload {
            id: list.id.clone(),
            fields,
        }));
    }
    Ok(TaskListRow {
        inner: list,
        account_id: LOCAL_ID.to_string(),
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
            color_label: task.color_label.clone(),
            reminders: task.reminders.clone(),
            sound: task.sound.clone(),
        };

        let created = if target_account == LOCAL_ID {
            adapter
                .create_task(&task.list_id, new_payload)
                .await?
        } else {
            let Some(ext) = registry.task_adapter(&target_account) else {
                return Err(CommandError {
                    code: "not_found",
                    message: format!(
                        "target task list '{}' is not routable",
                        task.list_id,
                    ),
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
