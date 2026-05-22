//! Task list and task commands.

use cal_adapter_local::LocalAdapter;
use cal_core::{NewTask, Task, TaskList, TasksFeature};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
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
    request: CreateTaskListRequest,
) -> CommandResult<TaskListRow> {
    let list =
        adapter.create_task_list(&request.name, None, None, request.embedded_in_calendar)?;
    Ok(TaskListRow {
        inner: list,
        account_id: LOCAL_ID.to_string(),
    })
}

#[tauri::command]
pub async fn delete_task_list(adapter: State<'_, LocalAdapter>, id: String) -> CommandResult<()> {
    Ok(adapter.delete_task_list(&id)?)
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
    request: CreateTaskRequest,
) -> CommandResult<Task> {
    let account = registry
        .account_for_task_list(&request.list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    let task = if account == LOCAL_ID {
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
    scheduler.invalidate();
    Ok(task)
}

#[tauri::command]
pub async fn update_task(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    scheduler: State<'_, SchedulerHandle>,
    task: Task,
) -> CommandResult<Task> {
    let account = registry
        .account_for_task_list(&task.list_id)
        .unwrap_or_else(|| LOCAL_ID.to_string());
    let updated = if account == LOCAL_ID {
        adapter.update_task(task).await?
    } else {
        let Some(ext) = registry.task_adapter(&account) else {
            return Err(CommandError {
                code: "not_found",
                message: format!("task list '{}' is not routable", task.list_id),
            });
        };
        ext.update_task(task).await?
    };
    scheduler.invalidate();
    Ok(updated)
}

#[tauri::command]
pub async fn delete_task(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    scheduler: State<'_, SchedulerHandle>,
    id: String,
    list_id: Option<String>,
) -> CommandResult<()> {
    let account = list_id
        .as_deref()
        .and_then(|lid| registry.account_for_task_list(lid))
        .unwrap_or_else(|| LOCAL_ID.to_string());
    if account == LOCAL_ID {
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
    scheduler.invalidate();
    Ok(())
}
