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
use crate::manager::{InFlightGuard, LoadedInstance};
use crate::vtables::{CalendarAdapterVtable, TasksVtable};

use super::call::{call_method, decode_payload, encode_args, CallOutcome};

pub struct FfiTasksAdapter {
    _instance: Arc<LoadedInstance>,
    handle_addr: usize,
    vtable: VtableSnapshot,
    capabilities: Vec<Capability>,
    /// In-flight counter handle shared with the
    /// [`crate::manager::LoadedPlugin`]. Every FFI-dispatching
    /// trait method brackets its body with an [`InFlightGuard`]
    /// derived from this Arc so the host's unload path can
    /// observe a deterministic "is anything in flight" gate.
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
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
    /// Wrap a loaded plugin instance's tasks surface. Returns
    /// `None` if the plugin doesn't declare the tasks capability
    /// (the [`CalendarAdapterVtable::tasks`] slot is null) or
    /// the sub-vtable fails the minimum-surface check.
    pub fn new(instance: Arc<LoadedInstance>) -> Option<Self> {
        let plugin = instance.plugin().clone();
        let raw = plugin.vtable_ptr();
        if raw.is_null() {
            warn!(
                plugin_id = %plugin.manifest.id,
                "tasks plugin has NULL vtable; refusing to wrap",
            );
            return None;
        }
        // SAFETY: the manifest says plugin_type =
        // "calendar-adapter", so the vtable is a
        // CalendarAdapterVtable per the ABI contract.
        let outer: &CalendarAdapterVtable = unsafe { &*(raw as *const CalendarAdapterVtable) };
        if outer.tasks.is_null() {
            return None;
        }
        // SAFETY: outer.tasks is non-null per the check above +
        // points at a static in the plugin's library; the
        // LoadedPlugin Arc inside the instance keeps it alive.
        let vtable_ref: &TasksVtable = unsafe { &*outer.tasks };
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
        let handle_addr = instance.handle() as usize;
        let in_flight = Arc::clone(plugin.in_flight_handle());
        Some(Self {
            _instance: instance,
            handle_addr,
            vtable: snapshot,
            capabilities,
            in_flight,
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
    instance_addr: usize,
    args: &A,
) -> Result<T>
where
    T: serde::de::DeserializeOwned,
    A: Serialize,
{
    let bytes = encode_args(args).map_err(|e| Error::Internal(format!("encode args: {e}")))?;
    let outcome = call_method(method, instance_addr, bytes).await;
    if outcome.is_ok() {
        decode_payload(&outcome.bytes)
            .map_err(|e| Error::Protocol(format!("decode plugin response: {e}")))
    } else {
        Err(status_to_cal_error(outcome))
    }
}

async fn call_for_unit<A: Serialize>(
    method: Option<crate::vtables::VtableMethodFn>,
    instance_addr: usize,
    args: &A,
) -> Result<()> {
    let bytes = encode_args(args).map_err(|e| Error::Internal(format!("encode args: {e}")))?;
    let outcome = call_method(method, instance_addr, bytes).await;
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
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_then_decode(self.vtable.authenticate, self.handle_addr, &credentials).await
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl TasksFeature for FfiTasksAdapter {
    async fn list_task_lists(&self) -> Result<Vec<TaskList>> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_then_decode(self.vtable.list_task_lists, self.handle_addr, &()).await
    }

    async fn get_tasks(&self, list_id: &str) -> Result<Vec<Task>> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_then_decode(self.vtable.get_tasks, self.handle_addr, &list_id).await
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> Result<Task> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        let args = CreateTaskArgs { list_id, task };
        call_then_decode(self.vtable.create_task, self.handle_addr, &args).await
    }

    async fn update_task(&self, task: Task) -> Result<Task> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_then_decode(self.vtable.update_task, self.handle_addr, &task).await
    }

    async fn delete_task(&self, task_id: &str) -> Result<()> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_for_unit(self.vtable.delete_task, self.handle_addr, &task_id).await
    }

    async fn rename_task_list(&self, list_id: &str, new_name: &str) -> Result<()> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        let args = RenameTaskListArgs { list_id, new_name };
        call_for_unit(self.vtable.rename_task_list, self.handle_addr, &args).await
    }
}
