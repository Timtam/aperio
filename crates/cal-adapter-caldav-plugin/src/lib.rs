//! CalDAV + CardDAV calendar/tasks/contacts adapter packaged
//! as a plugin (DESIGN.md §20).
//!
//! Multi-capability adapter — fills all three slots of
//! [`CalendarAdapterVtable`]: calendar (VEVENT), tasks (VTODO)
//! and contacts (CardDAV VCARD). One [`CaldavAdapter`] instance
//! services every surface so the discovery + listing caches
//! stay coherent across reads.
//!
//! ## Init config
//!
//! ```json
//! {
//!   "server_url": "https://caldav.example.com/",
//!   "username": "alice",
//!   "auth_kind": "basic",
//!   "secret": "…"
//! }
//! ```
//!
//! `secret` is the per-account password / bearer token; the host
//! pre-extracts it from the platform keychain and threads it in
//! via `config_json` — same pattern as the OAuth-based sync
//! plugins from P4 (sync). `auth_kind` defaults to `"basic"` and
//! accepts the snake-case variants of [`AuthKind`] (`basic` |
//! `bearer`).

use std::os::raw::{c_char, c_int};

use cal_adapter_caldav::{
    AuthKind, CaldavAccountConfig, CaldavAdapter, Credentials,
};
use cal_core::adapter::{AuthToken, Capability, Credentials as CalCredentials};
use cal_core::error::Result as CalResult;
use cal_core::types::{ContactPhoto, DateRange, NewContact, NewEvent, NewTask};
use cal_core::{CalendarFeature, ContactsFeature, TasksFeature};
use plugin_sdk::plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};
use plugin_sdk::plugin_core::vtables::{
    CalendarAdapterVtable, CalendarVtable, ContactsVtable, TasksVtable,
};
use plugin_sdk::{
    cal_error_to_response, decode_args, error_response, ok_empty_response,
    ok_response, PluginSingleton,
};
use serde::Deserialize;
use tracing::warn;

pub static PLUGIN_INSTANCE: PluginSingleton<CaldavAdapter> =
    PluginSingleton::new();

#[derive(Debug, Deserialize)]
struct InitConfig {
    server_url: String,
    username: String,
    #[serde(default)]
    auth_kind: AuthKind,
    secret: String,
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
            warn!(?err, "cal-adapter-caldav-plugin: malformed init config");
            return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
        }
    };
    if cfg.server_url.trim().is_empty()
        || cfg.username.trim().is_empty()
        || cfg.secret.is_empty()
    {
        return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
    }
    let credentials = Credentials::new(
        CaldavAccountConfig {
            server_url: cfg.server_url,
            username: cfg.username,
            auth_kind: cfg.auth_kind,
        },
        cfg.secret,
    );
    let adapter = match CaldavAdapter::new(credentials, None) {
        Ok(a) => a,
        Err(err) => {
            warn!(?err, "cal-adapter-caldav-plugin: adapter ctor failed");
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

// ─────────────────────────────────────────────────────────────
// Dispatch helpers
// ─────────────────────────────────────────────────────────────

fn dispatch<T, F, Fut>(call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static CaldavAdapter) -> Fut,
    Fut: std::future::Future<Output = CalResult<T>>,
{
    let Some((p, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let p_static: &'static CaldavAdapter =
        unsafe { std::mem::transmute::<&CaldavAdapter, &'static CaldavAdapter>(p) };
    match rt.block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => cal_error_to_response(e),
    }
}

fn dispatch_unit<F, Fut>(call: F) -> PluginCallResult
where
    F: FnOnce(&'static CaldavAdapter) -> Fut,
    Fut: std::future::Future<Output = CalResult<()>>,
{
    let Some((p, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let p_static: &'static CaldavAdapter =
        unsafe { std::mem::transmute::<&CaldavAdapter, &'static CaldavAdapter>(p) };
    match rt.block_on(call(p_static)) {
        Ok(()) => ok_empty_response(),
        Err(e) => cal_error_to_response(e),
    }
}

// ─────────────────────────────────────────────────────────────
// Adapter base trait
// ─────────────────────────────────────────────────────────────

unsafe extern "C" fn ffi_authenticate(a: *const u8, l: usize) -> PluginCallResult {
    let creds: CalCredentials = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    let Some((p, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let p_static: &'static CaldavAdapter =
        unsafe { std::mem::transmute::<&CaldavAdapter, &'static CaldavAdapter>(p) };
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

// ─────────────────────────────────────────────────────────────
// CalendarFeature
// ─────────────────────────────────────────────────────────────

unsafe extern "C" fn ffi_list_calendars(_a: *const u8, _l: usize) -> PluginCallResult {
    dispatch(|p| async move { p.list_calendars().await })
}

#[derive(Debug, Deserialize)]
struct GetEventsArgs {
    calendar_id: String,
    range: DateRange,
}

unsafe extern "C" fn ffi_get_events(a: *const u8, l: usize) -> PluginCallResult {
    let args: GetEventsArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move {
        p.get_events(&args.calendar_id, args.range).await
    })
}

#[derive(Debug, Deserialize)]
struct CreateEventArgs {
    calendar_id: String,
    event: NewEvent,
}

unsafe extern "C" fn ffi_create_event(a: *const u8, l: usize) -> PluginCallResult {
    let args: CreateEventArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move {
        p.create_event(&args.calendar_id, args.event).await
    })
}

unsafe extern "C" fn ffi_update_event(a: *const u8, l: usize) -> PluginCallResult {
    let event: cal_core::Event = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move { p.update_event(event).await })
}

unsafe extern "C" fn ffi_delete_event(a: *const u8, l: usize) -> PluginCallResult {
    let event_id: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch_unit(move |p| async move { p.delete_event(&event_id).await })
}

#[derive(Debug, Deserialize)]
struct GetFreeBusyArgs {
    emails: Vec<String>,
    range: DateRange,
}

unsafe extern "C" fn ffi_get_free_busy(a: *const u8, l: usize) -> PluginCallResult {
    let args: GetFreeBusyArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move {
        let refs: Vec<&str> = args.emails.iter().map(|s| s.as_str()).collect();
        p.get_free_busy(&refs, args.range).await
    })
}

unsafe extern "C" fn ffi_calendar_color(a: *const u8, l: usize) -> PluginCallResult {
    let calendar_id: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    let Some(p) = PLUGIN_INSTANCE.get() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let color = p.calendar_color(&calendar_id);
    ok_response(&color)
}

#[derive(Debug, Deserialize)]
struct AddExdateArgs {
    event_id: String,
    occurrence: chrono::DateTime<chrono::Utc>,
}

unsafe extern "C" fn ffi_add_event_exdate(a: *const u8, l: usize) -> PluginCallResult {
    let args: AddExdateArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch_unit(move |p| async move {
        p.add_event_exdate(&args.event_id, args.occurrence).await
    })
}

#[derive(Debug, Deserialize)]
struct RenameCalendarArgs {
    calendar_id: String,
    new_name: String,
}

unsafe extern "C" fn ffi_rename_calendar(a: *const u8, l: usize) -> PluginCallResult {
    let args: RenameCalendarArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch_unit(move |p| async move {
        p.rename_calendar(&args.calendar_id, &args.new_name).await
    })
}

// ─────────────────────────────────────────────────────────────
// TasksFeature
// ─────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────
// ContactsFeature
// ─────────────────────────────────────────────────────────────

unsafe extern "C" fn ffi_list_contact_lists(_a: *const u8, _l: usize) -> PluginCallResult {
    dispatch(|p| async move { p.list_contact_lists().await })
}

unsafe extern "C" fn ffi_get_contacts(a: *const u8, l: usize) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move { p.get_contacts(&list_id).await })
}

unsafe extern "C" fn ffi_search_contacts(a: *const u8, l: usize) -> PluginCallResult {
    let query: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move { p.search_contacts(&query).await })
}

#[derive(Debug, Deserialize)]
struct CreateContactArgs {
    list_id: String,
    contact: NewContact,
}

unsafe extern "C" fn ffi_create_contact(a: *const u8, l: usize) -> PluginCallResult {
    let args: CreateContactArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move {
        p.create_contact(&args.list_id, args.contact).await
    })
}

unsafe extern "C" fn ffi_update_contact(a: *const u8, l: usize) -> PluginCallResult {
    let contact: cal_core::Contact = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move { p.update_contact(contact).await })
}

unsafe extern "C" fn ffi_delete_contact(a: *const u8, l: usize) -> PluginCallResult {
    let contact_id: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch_unit(move |p| async move { p.delete_contact(&contact_id).await })
}

#[derive(Debug, Deserialize)]
struct RenameContactListArgs {
    list_id: String,
    new_name: String,
}

unsafe extern "C" fn ffi_rename_contact_list(a: *const u8, l: usize) -> PluginCallResult {
    let args: RenameContactListArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch_unit(move |p| async move {
        p.rename_contact_list(&args.list_id, &args.new_name).await
    })
}

unsafe extern "C" fn ffi_get_contact_photo(a: *const u8, l: usize) -> PluginCallResult {
    let contact_id: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(move |p| async move { p.get_contact_photo(&contact_id).await })
}

#[derive(Debug, Deserialize)]
struct SetContactPhotoArgs {
    contact_id: String,
    photo: ContactPhoto,
}

unsafe extern "C" fn ffi_set_contact_photo(a: *const u8, l: usize) -> PluginCallResult {
    let args: SetContactPhotoArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch_unit(move |p| async move {
        p.set_contact_photo(&args.contact_id, args.photo).await
    })
}

unsafe extern "C" fn ffi_delete_contact_photo(a: *const u8, l: usize) -> PluginCallResult {
    let contact_id: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch_unit(move |p| async move { p.delete_contact_photo(&contact_id).await })
}

unsafe extern "C" fn ffi_invalidate_contacts_cache(
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch_unit(|p| async move { p.invalidate_contacts_cache().await })
}

// ─────────────────────────────────────────────────────────────
// Vtables
// ─────────────────────────────────────────────────────────────

#[no_mangle]
pub static CALENDAR_VTABLE: CalendarVtable = CalendarVtable {
    authenticate: Some(ffi_authenticate),
    capabilities: Some(ffi_capabilities),
    list_calendars: Some(ffi_list_calendars),
    get_events: Some(ffi_get_events),
    create_event: Some(ffi_create_event),
    update_event: Some(ffi_update_event),
    delete_event: Some(ffi_delete_event),
    get_free_busy: Some(ffi_get_free_busy),
    calendar_color: Some(ffi_calendar_color),
    add_event_exdate: Some(ffi_add_event_exdate),
    rename_calendar: Some(ffi_rename_calendar),
    ..CalendarVtable::empty()
};

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
pub static CONTACTS_VTABLE: ContactsVtable = ContactsVtable {
    authenticate: Some(ffi_authenticate),
    capabilities: Some(ffi_capabilities),
    list_contact_lists: Some(ffi_list_contact_lists),
    get_contacts: Some(ffi_get_contacts),
    search_contacts: Some(ffi_search_contacts),
    create_contact: Some(ffi_create_contact),
    update_contact: Some(ffi_update_contact),
    delete_contact: Some(ffi_delete_contact),
    rename_contact_list: Some(ffi_rename_contact_list),
    get_contact_photo: Some(ffi_get_contact_photo),
    set_contact_photo: Some(ffi_set_contact_photo),
    delete_contact_photo: Some(ffi_delete_contact_photo),
    invalidate_contacts_cache: Some(ffi_invalidate_contacts_cache),
    ..ContactsVtable::empty()
};

#[no_mangle]
pub static ADAPTER_VTABLE: CalendarAdapterVtable = CalendarAdapterVtable {
    vtable_version: plugin_sdk::plugin_core::ABI_VERSION,
    calendar: &CALENDAR_VTABLE,
    tasks: &TASKS_VTABLE,
    contacts: &CONTACTS_VTABLE,
};

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.cal-adapter-caldav",
    name: "Aperio CalDAV",
    version: "0.1.0",
    plugin_type: "calendar-adapter",
    vtable: ADAPTER_VTABLE,
    init: plugin_init,
    destroy: plugin_destroy,
}
