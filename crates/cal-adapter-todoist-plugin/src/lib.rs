//! Todoist tasks adapter packaged as a plugin (DESIGN.md §20).
//!
//! Single-capability tasks adapter — fills only the
//! `tasks` slot in [`CalendarAdapterVtable`].
//!
//! ## Init config
//!
//! ```json
//! { "token": "…" }
//! ```
//!
//! Todoist auth is a long-lived API token; the OAuth dance
//! (when used) stays host-side + the host threads the token in
//! via `config_json` per ABI v2's `open_instance` hook.

use std::os::raw::{c_char, c_void};

use cal_adapter_todoist::TodoistAdapter;
use cal_core::adapter::{Capability, Credentials as CalCredentials};
use cal_core::types::NewTask;
use cal_core::TasksFeature;
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::{CalendarAdapterVtable, TasksVtable};
use plugin_sdk::{decode_args, ok_response, open_instance_with, PluginInstance};
use serde::Deserialize;

plugin_sdk::cal_dispatch_helpers!(TodoistAdapter);

#[derive(Debug, Deserialize)]
struct InitConfig {
    token: String,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(
    config_json: *const c_char,
) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig = serde_json::from_str(json)
            .map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.token.trim().is_empty() {
            return Err("token must not be empty".to_string());
        }
        Ok(TodoistAdapter::new(cfg.token))
    })
}

/// # Safety
/// FFI export; `handle` must be the pointer returned by
/// [`plugin_open_instance`].
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<TodoistAdapter>::drop_handle(handle);
}

// ── Adapter base ───────────────────────────────────────────

unsafe extern "C" fn ffi_authenticate(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let creds: CalCredentials = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        cal_core::Adapter::authenticate(p, creds).await
    })
}

unsafe extern "C" fn ffi_capabilities(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    let inst = match instance(h) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let caps: Vec<Capability> = cal_core::Adapter::capabilities(inst.plugin()).to_vec();
    ok_response(&caps)
}

// ── TasksFeature ───────────────────────────────────────────

unsafe extern "C" fn ffi_list_task_lists(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch(h, |p| async move { p.list_task_lists().await })
}

unsafe extern "C" fn ffi_get_tasks(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.get_tasks(&list_id).await })
}

#[derive(Debug, Deserialize)]
struct CreateTaskArgs {
    list_id: String,
    task: NewTask,
}

unsafe extern "C" fn ffi_create_task(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: CreateTaskArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.create_task(&args.list_id, args.task).await })
}

unsafe extern "C" fn ffi_update_task(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let task: cal_core::Task = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.update_task(task).await })
}

unsafe extern "C" fn ffi_delete_task(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let task_id: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move { p.delete_task(&task_id).await })
}

#[derive(Debug, Deserialize)]
struct RenameTaskListArgs {
    list_id: String,
    new_name: String,
}

unsafe extern "C" fn ffi_rename_task_list(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: RenameTaskListArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.rename_task_list(&args.list_id, &args.new_name).await
    })
}

pub static TASKS_VTABLE: TasksVtable = TasksVtable {
    authenticate: Some(ffi_authenticate),
    capabilities: Some(ffi_capabilities),
    list_task_lists: Some(ffi_list_task_lists),
    get_tasks: Some(ffi_get_tasks),
    create_task: Some(ffi_create_task),
    update_task: Some(ffi_update_task),
    delete_task: Some(ffi_delete_task),
    rename_task_list: Some(ffi_rename_task_list),
    ..TasksVtable::empty()
};

pub static ADAPTER_VTABLE: CalendarAdapterVtable = CalendarAdapterVtable {
    vtable_version: plugin_sdk::plugin_core::ABI_VERSION,
    calendar: std::ptr::null(),
    tasks: &TASKS_VTABLE,
    contacts: std::ptr::null(),
};

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.cal-adapter-todoist",
    name: "Aperio Todoist",
    version: "0.1.0",
    plugin_type: "calendar-adapter",
    vtable: ADAPTER_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}
