//! Task list and task commands.

use cal_adapter_local::LocalAdapter;
use cal_core::{NewTask, Task, TaskList, TasksFeature};
use serde::Deserialize;
use tauri::State;

use super::CommandResult;
use crate::reminders::SchedulerHandle;

#[derive(Debug, Deserialize)]
pub struct CreateTaskListRequest {
    pub name: String,
    pub embedded_in_calendar: Option<String>,
}

#[tauri::command]
pub async fn list_task_lists(adapter: State<'_, LocalAdapter>) -> CommandResult<Vec<TaskList>> {
    Ok(adapter.list_task_lists().await?)
}

#[tauri::command]
pub async fn create_task_list(
    adapter: State<'_, LocalAdapter>,
    request: CreateTaskListRequest,
) -> CommandResult<TaskList> {
    Ok(adapter.create_task_list(&request.name, None, None, request.embedded_in_calendar)?)
}

#[tauri::command]
pub async fn delete_task_list(adapter: State<'_, LocalAdapter>, id: String) -> CommandResult<()> {
    Ok(adapter.delete_task_list(&id)?)
}

#[tauri::command]
pub async fn get_tasks(
    adapter: State<'_, LocalAdapter>,
    list_id: String,
) -> CommandResult<Vec<Task>> {
    Ok(adapter.get_tasks(&list_id).await?)
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub list_id: String,
    #[serde(flatten)]
    pub task: NewTask,
}

#[tauri::command]
pub async fn create_task(
    adapter: State<'_, LocalAdapter>,
    scheduler: State<'_, SchedulerHandle>,
    request: CreateTaskRequest,
) -> CommandResult<Task> {
    let task = adapter.create_task(&request.list_id, request.task).await?;
    scheduler.invalidate();
    Ok(task)
}

#[tauri::command]
pub async fn update_task(
    adapter: State<'_, LocalAdapter>,
    scheduler: State<'_, SchedulerHandle>,
    task: Task,
) -> CommandResult<Task> {
    let task = adapter.update_task(task).await?;
    scheduler.invalidate();
    Ok(task)
}

#[tauri::command]
pub async fn delete_task(
    adapter: State<'_, LocalAdapter>,
    scheduler: State<'_, SchedulerHandle>,
    id: String,
) -> CommandResult<()> {
    adapter.delete_task(&id).await?;
    scheduler.invalidate();
    Ok(())
}
