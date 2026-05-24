//! `FfiTasksAdapter` — `cal_core::TasksFeature` impl that
//! dispatches across the FFI boundary into a loaded plugin's
//! [`crate::vtables::TasksVtable`].
//!
//! Same shape as [`super::calendar::FfiCalendarAdapter`]; see
//! that file's module doc for the canonical pattern.

use std::sync::Arc;

use async_trait::async_trait;
use cal_core::adapter::{Adapter, AuthToken, Capability, Credentials, TasksFeature};
use cal_core::error::{Error, Result};
use cal_core::types::{NewTask, Task, TaskList};
use serde::Serialize;
use tracing::warn;

use crate::ffi::*;
use crate::manager::LoadedPlugin;
use crate::vtables::TasksVtable;

use super::call::{call_method, decode_payload, encode_args, CallOutcome};

pub struct FfiTasksAdapter {
    _plugin: Arc<LoadedPlugin>,
    vtable: VtableSnapshot,
    capabilities: Vec<Capability>,
}

#[derive(Clone, Copy)]
struct VtableSnapshot {
    authenticate: Option<crate::vtables::VtableMethodFn>,
    list_task_lists: Option<crate::vtables::VtableMethodFn>,
    get_tasks: Option<crate::vtables::VtableMethodFn>,
    create_task: Option<crate::vtables::VtableMethodFn>,
    update_task: Option<crate::vtables::VtableMethodFn>,
    delete_task: Option<crate::vtables::VtableMethodFn>,
    rename_task_list: Option<crate::vtables::VtableMethodFn>,
}

impl FfiTasksAdapter {
    /// Wrap a loaded plugin's tasks vtable. Returns `None` if the
    /// vtable pointer is NULL or the minimum-surface check (a
    /// non-`None` `list_task_lists`) fails.
    pub fn new(plugin: Arc<LoadedPlugin>) -> Option<Self> {
        let raw = plugin.vtable_ptr();
        if raw.is_null() {
            warn!(
                plugin_id = %plugin.manifest.id,
                "tasks plugin has NULL vtable; refusing to wrap",
            );
            return None;
        }
        // SAFETY: the plugin's manifest declares plugin_type =
        // sync/calendar with tasks capability, so the vtable
        // points at a TasksVtable per the ABI contract.
        let vtable_ref: &TasksVtable = unsafe { &*(raw as *const TasksVtable) };
        if !vtable_ref.has_minimum_surface() {
            warn!(
                plugin_id = %plugin.manifest.id,
                "tasks plugin's vtable lacks list_task_lists; refusing to wrap",
            );
            return None;
        }
        let snapshot = VtableSnapshot {
            authenticate: vtable_ref.authenticate,
            list_task_lists: vtable_ref.list_task_lists,
            get_tasks: vtable_ref.get_tasks,
            create_task: vtable_ref.create_task,
            update_task: vtable_ref.update_task,
            delete_task: vtable_ref.delete_task,
            rename_task_list: vtable_ref.rename_task_list,
        };
        let capabilities = super::manifest_capabilities(&plugin.manifest.capabilities);
        Some(Self {
            _plugin: plugin,
            vtable: snapshot,
            capabilities,
        })
    }
}

/// Same helper as in the calendar shim — translates the plugin's
/// status into a `cal_core::Error`. Kept duplicated rather than
/// hoisted into shim/call.rs because the mapping is trait-side
/// (cal_core::Error here, sync_core::SyncError in sync.rs).
fn status_to_cal_error(outcome: CallOutcome) -> Error {
    let msg = outcome.message();
    match outcome.status {
        PLUGIN_CALL_ERR_UNSUPPORTED => Error::Unsupported(msg),
        PLUGIN_CALL_ERR_INVALID => Error::InvalidInput(msg),
        PLUGIN_CALL_ERR_AUTH => Error::Authentication(msg),
        PLUGIN_CALL_ERR_NETWORK => Error::Network(msg),
        PLUGIN_CALL_ERR_NOT_FOUND => Error::NotFound(msg),
        PLUGIN_CALL_ERR_PROTOCOL => Error::Protocol(msg),
        PLUGIN_CALL_ERR_CONFLICT => Error::Conflict(msg),
        PLUGIN_CALL_ERR_FORBIDDEN => Error::Forbidden(msg),
        PLUGIN_CALL_ERR_IO => Error::Internal(format!("plugin IO: {msg}")),
        PLUGIN_CALL_ERR_INTERNAL => Error::Internal(msg),
        other => Error::Internal(format!("plugin status {other}: {msg}")),
    }
}

async fn call_then_decode<T, A>(
    method: Option<crate::vtables::VtableMethodFn>,
    args: &A,
) -> Result<T>
where
    T: serde::de::DeserializeOwned,
    A: Serialize,
{
    let bytes = encode_args(args).map_err(|e| Error::Internal(format!(
        "encode args: {e}"
    )))?;
    let outcome = call_method(method, bytes).await;
    if outcome.is_ok() {
        decode_payload(&outcome.bytes).map_err(|e| Error::Protocol(format!(
            "decode plugin response: {e}"
        )))
    } else {
        Err(status_to_cal_error(outcome))
    }
}

async fn call_for_unit<A: Serialize>(
    method: Option<crate::vtables::VtableMethodFn>,
    args: &A,
) -> Result<()> {
    let bytes = encode_args(args).map_err(|e| Error::Internal(format!(
        "encode args: {e}"
    )))?;
    let outcome = call_method(method, bytes).await;
    if outcome.is_ok() {
        Ok(())
    } else {
        Err(status_to_cal_error(outcome))
    }
}

#[derive(Serialize)]
struct CreateTaskArgs<'a> {
    list_id: &'a str,
    task: NewTask,
}

#[derive(Serialize)]
struct RenameTaskListArgs<'a> {
    list_id: &'a str,
    new_name: &'a str,
}

#[async_trait]
impl Adapter for FfiTasksAdapter {
    async fn authenticate(&self, credentials: Credentials) -> Result<AuthToken> {
        call_then_decode(self.vtable.authenticate, &credentials).await
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl TasksFeature for FfiTasksAdapter {
    async fn list_task_lists(&self) -> Result<Vec<TaskList>> {
        call_then_decode(self.vtable.list_task_lists, &()).await
    }

    async fn get_tasks(&self, list_id: &str) -> Result<Vec<Task>> {
        call_then_decode(self.vtable.get_tasks, &list_id).await
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> Result<Task> {
        let args = CreateTaskArgs { list_id, task };
        call_then_decode(self.vtable.create_task, &args).await
    }

    async fn update_task(&self, task: Task) -> Result<Task> {
        call_then_decode(self.vtable.update_task, &task).await
    }

    async fn delete_task(&self, task_id: &str) -> Result<()> {
        call_for_unit(self.vtable.delete_task, &task_id).await
    }

    async fn rename_task_list(
        &self,
        list_id: &str,
        new_name: &str,
    ) -> Result<()> {
        let args = RenameTaskListArgs { list_id, new_name };
        call_for_unit(self.vtable.rename_task_list, &args).await
    }
}
