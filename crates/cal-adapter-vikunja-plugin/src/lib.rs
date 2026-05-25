//! Vikunja tasks adapter packaged as a plugin (DESIGN.md §20).
//!
//! Single-capability tasks adapter — same shape as
//! cal-adapter-todoist-plugin, but Vikunja additionally needs
//! the self-hosted instance's `server_url` because there's no
//! single canonical Vikunja endpoint.
//!
//! ## Init config
//!
//! ```json
//! {
//!   "server_url": "https://vikunja.example.com",
//!   "token": "…"
//! }
//! ```

use std::os::raw::{c_char, c_int};

use cal_adapter_vikunja::VikunjaAdapter;
use cal_core::adapter::{AuthToken, Capability, Credentials as CalCredentials};
use cal_core::error::Result as CalResult;
use cal_core::types::NewTask;
use cal_core::TasksFeature;
use plugin_sdk::plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};
use plugin_sdk::plugin_core::vtables::{CalendarAdapterVtable, TasksVtable};
use plugin_sdk::{
    cal_error_to_response, decode_args, error_response, ok_empty_response,
    ok_response, PluginSingleton,
};
use serde::Deserialize;
use tracing::warn;

pub static PLUGIN_INSTANCE: PluginSingleton<VikunjaAdapter> =
    PluginSingleton::new();

#[derive(Debug, Deserialize)]
struct InitConfig {
    server_url: String,
    token: String,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_init(config_json: *const c_char) -> c_int {
    if config_json.is_null() {
        return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
    }
    let json_str = match std::ffi::CStr::from_ptr(config_json).to_str() {
        Ok(s) => s,
        Err(_) => return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG,
    };
    let cfg: InitConfig = match serde_json::from_str(json_str) {
        Ok(c) => c,
        Err(err) => {
            warn!(?err, "cal-adapter-vikunja-plugin: malformed init config");
            return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
        }
    };
    if cfg.server_url.trim().is_empty() || cfg.token.trim().is_empty() {
        return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
    }
    let adapter = match VikunjaAdapter::new(cfg.server_url.trim(), cfg.token) {
        Ok(a) => a,
        Err(err) => {
            warn!(?err, "cal-adapter-vikunja-plugin: adapter ctor failed");
            return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
        }
    };
    match PLUGIN_INSTANCE.init(adapter) {
        Ok(()) => plugin_sdk::plugin_core::PLUGIN_OK,
        Err(_) => plugin_sdk::plugin_core::PLUGIN_ERR_INIT,
    }
}

/// # Safety
/// FFI export; empty teardown.
pub unsafe extern "C" fn plugin_destroy() {}

fn dispatch<T, F, Fut>(call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static VikunjaAdapter) -> Fut,
    Fut: std::future::Future<Output = CalResult<T>>,
{
    let Some((p, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let p_static: &'static VikunjaAdapter =
        unsafe { std::mem::transmute::<&VikunjaAdapter, &'static VikunjaAdapter>(p) };
    match rt.block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => cal_error_to_response(e),
    }
}

fn dispatch_unit<F, Fut>(call: F) -> PluginCallResult
where
    F: FnOnce(&'static VikunjaAdapter) -> Fut,
    Fut: std::future::Future<Output = CalResult<()>>,
{
    let Some((p, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let p_static: &'static VikunjaAdapter =
        unsafe { std::mem::transmute::<&VikunjaAdapter, &'static VikunjaAdapter>(p) };
    match rt.block_on(call(p_static)) {
        Ok(()) => ok_empty_response(),
        Err(e) => cal_error_to_response(e),
    }
}

// ── Adapter base ───────────────────────────────────────────

unsafe extern "C" fn ffi_authenticate(a: *const u8, l: usize) -> PluginCallResult {
    let creds: CalCredentials = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    let Some((p, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let p_static: &'static VikunjaAdapter =
        unsafe { std::mem::transmute::<&VikunjaAdapter, &'static VikunjaAdapter>(p) };
    let outcome: CalResult<AuthToken> = rt.block_on(async move {
        cal_core::Adapter::authenticate(p_static, creds).await
    });
    match outcome {
        Ok(v) => ok_response(&v),
        Err(e) => cal_error_to_response(e),
    }
}

unsafe extern "C" fn ffi_capabilities(_a: *const u8, _l: usize) -> PluginCallResult {
    let Some(p) = PLUGIN_INSTANCE.get() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let caps: Vec<Capability> = cal_core::Adapter::capabilities(p).to_vec();
    ok_response(&caps)
}

// ── TasksFeature ───────────────────────────────────────────

unsafe extern "C" fn ffi_list_task_lists(_a: *const u8, _l: usize) -> PluginCallResult {
    dispatch(|p| async move { p.list_task_lists().await })
}

unsafe extern "C" fn ffi_get_tasks(a: *const u8, l: usize) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move { p.get_tasks(&list_id).await })
}

#[derive(Debug, Deserialize)]
struct CreateTaskArgs {
    list_id: String,
    task: NewTask,
}

unsafe extern "C" fn ffi_create_task(a: *const u8, l: usize) -> PluginCallResult {
    let args: CreateTaskArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move { p.create_task(&args.list_id, args.task).await })
}

unsafe extern "C" fn ffi_update_task(a: *const u8, l: usize) -> PluginCallResult {
    let task: cal_core::Task = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move { p.update_task(task).await })
}

unsafe extern "C" fn ffi_delete_task(a: *const u8, l: usize) -> PluginCallResult {
    let task_id: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch_unit(move |p| async move { p.delete_task(&task_id).await })
}

#[derive(Debug, Deserialize)]
struct RenameTaskListArgs {
    list_id: String,
    new_name: String,
}

unsafe extern "C" fn ffi_rename_task_list(a: *const u8, l: usize) -> PluginCallResult {
    let args: RenameTaskListArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch_unit(move |p| async move {
        p.rename_task_list(&args.list_id, &args.new_name).await
    })
}

#[no_mangle]
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

#[no_mangle]
pub static ADAPTER_VTABLE: CalendarAdapterVtable = CalendarAdapterVtable {
    vtable_version: plugin_sdk::plugin_core::ABI_VERSION,
    calendar: std::ptr::null(),
    tasks: &TASKS_VTABLE,
    contacts: std::ptr::null(),
};

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.cal-adapter-vikunja",
    name: "Aperio Vikunja",
    version: "0.1.0",
    plugin_type: "calendar-adapter",
    vtable: ADAPTER_VTABLE,
    init: plugin_init,
    destroy: plugin_destroy,
}
