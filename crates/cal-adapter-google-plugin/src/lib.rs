//! Google Calendar/Tasks/Contacts adapter packaged as a plugin
//! (DESIGN.md §20).
//!
//! ## Init config
//!
//! ```json
//! {
//!   "client_id": "…",
//!   "client_secret": "…",
//!   "access_token": "ya29.…",
//!   "refresh_token": "1//…",
//!   "expires_at": "2030-01-01T00:00:00Z",
//!   "scope": null
//! }
//! ```
//!
//! Google's OAuth dance happens host-side via
//! `GoogleAdapter::authenticate_interactive` — the resulting
//! [`TokenSet`] (plus the Cloud Console client id / secret) is
//! threaded into the plugin via `config_json`. ABI v2 supports
//! N independent Google accounts per loaded library.

use std::os::raw::{c_char, c_void};

use cal_adapter_google::{GoogleAdapter, TokenSet};
use cal_core::adapter::{AuthToken, Capability, Credentials as CalCredentials};
use cal_core::error::Result as CalResult;
use cal_core::types::{ContactPhoto, DateRange, NewContact, NewEvent, NewTask};
use cal_core::{CalendarFeature, ContactsFeature, TasksFeature};
use chrono::{DateTime, Utc};
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};
use plugin_sdk::plugin_core::vtables::{
    CalendarAdapterVtable, CalendarVtable, ContactsVtable, TasksVtable,
};
use plugin_sdk::{
    cal_error_to_response, decode_args, error_response, ok_empty_response,
    ok_response, open_instance_with, PluginInstance,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct InitConfig {
    client_id: String,
    client_secret: String,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_at: DateTime<Utc>,
    #[serde(default)]
    scope: Option<String>,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(
    config_json: *const c_char,
) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig = serde_json::from_str(json)
            .map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.client_id.trim().is_empty()
            || cfg.client_secret.trim().is_empty()
            || cfg.access_token.trim().is_empty()
        {
            return Err("client_id, client_secret and access_token must not be empty".to_string());
        }
        let tokens = TokenSet {
            access_token: cfg.access_token,
            refresh_token: cfg.refresh_token,
            expires_at: cfg.expires_at,
            scope: cfg.scope,
        };
        Ok(GoogleAdapter::new(cfg.client_id, cfg.client_secret, tokens))
    })
}

/// # Safety
/// FFI export; `handle` must be the pointer returned by
/// [`plugin_open_instance`].
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<GoogleAdapter>::drop_handle(handle);
}

fn instance<'a>(
    handle: *mut c_void,
) -> Result<&'a PluginInstance<GoogleAdapter>, PluginCallResult> {
    unsafe { PluginInstance::<GoogleAdapter>::from_handle(handle) }
        .ok_or_else(|| error_response(PLUGIN_CALL_ERR_INTERNAL, "null instance handle"))
}

fn dispatch<T, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static GoogleAdapter) -> Fut,
    Fut: std::future::Future<Output = CalResult<T>>,
{
    let inst = match instance(handle) { Ok(i) => i, Err(r) => return r };
    let p_static: &'static GoogleAdapter =
        unsafe { std::mem::transmute::<&GoogleAdapter, &'static GoogleAdapter>(inst.plugin()) };
    match inst.runtime().block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => cal_error_to_response(e),
    }
}

fn dispatch_unit<F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    F: FnOnce(&'static GoogleAdapter) -> Fut,
    Fut: std::future::Future<Output = CalResult<()>>,
{
    let inst = match instance(handle) { Ok(i) => i, Err(r) => return r };
    let p_static: &'static GoogleAdapter =
        unsafe { std::mem::transmute::<&GoogleAdapter, &'static GoogleAdapter>(inst.plugin()) };
    match inst.runtime().block_on(call(p_static)) {
        Ok(()) => ok_empty_response(),
        Err(e) => cal_error_to_response(e),
    }
}

// ── Adapter base ───────────────────────────────────────────

unsafe extern "C" fn ffi_authenticate(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let creds: CalCredentials = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    let inst = match instance(h) { Ok(i) => i, Err(r) => return r };
    let p_static: &'static GoogleAdapter =
        unsafe { std::mem::transmute::<&GoogleAdapter, &'static GoogleAdapter>(inst.plugin()) };
    let outcome: CalResult<AuthToken> = inst.runtime().block_on(async move {
        cal_core::Adapter::authenticate(p_static, creds).await
    });
    match outcome {
        Ok(v) => ok_response(&v),
        Err(e) => cal_error_to_response(e),
    }
}

unsafe extern "C" fn ffi_capabilities(h: *mut c_void, _a: *const u8, _l: usize) -> PluginCallResult {
    let inst = match instance(h) { Ok(i) => i, Err(r) => return r };
    let caps: Vec<Capability> = cal_core::Adapter::capabilities(inst.plugin()).to_vec();
    ok_response(&caps)
}

// ── CalendarFeature ────────────────────────────────────────

unsafe extern "C" fn ffi_list_calendars(h: *mut c_void, _a: *const u8, _l: usize) -> PluginCallResult {
    dispatch(h, |p| async move { p.list_calendars().await })
}

#[derive(Debug, Deserialize)]
struct GetEventsArgs { calendar_id: String, range: DateRange }

unsafe extern "C" fn ffi_get_events(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: GetEventsArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move { p.get_events(&args.calendar_id, args.range).await })
}

#[derive(Debug, Deserialize)]
struct CreateEventArgs { calendar_id: String, event: NewEvent }

unsafe extern "C" fn ffi_create_event(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: CreateEventArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move { p.create_event(&args.calendar_id, args.event).await })
}

unsafe extern "C" fn ffi_update_event(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let event: cal_core::Event = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move { p.update_event(event).await })
}

unsafe extern "C" fn ffi_delete_event(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let event_id: String = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(h, move |p| async move { p.delete_event(&event_id).await })
}

#[derive(Debug, Deserialize)]
struct GetFreeBusyArgs { emails: Vec<String>, range: DateRange }

unsafe extern "C" fn ffi_get_free_busy(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: GetFreeBusyArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move {
        let refs: Vec<&str> = args.emails.iter().map(|s| s.as_str()).collect();
        p.get_free_busy(&refs, args.range).await
    })
}

unsafe extern "C" fn ffi_calendar_color(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let calendar_id: String = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    let inst = match instance(h) { Ok(i) => i, Err(r) => return r };
    let color = inst.plugin().calendar_color(&calendar_id);
    ok_response(&color)
}

#[derive(Debug, Deserialize)]
struct AddExdateArgs { event_id: String, occurrence: DateTime<Utc> }

unsafe extern "C" fn ffi_add_event_exdate(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: AddExdateArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(h, move |p| async move { p.add_event_exdate(&args.event_id, args.occurrence).await })
}

#[derive(Debug, Deserialize)]
struct RenameCalendarArgs { calendar_id: String, new_name: String }

unsafe extern "C" fn ffi_rename_calendar(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: RenameCalendarArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(h, move |p| async move { p.rename_calendar(&args.calendar_id, &args.new_name).await })
}

// ── TasksFeature ───────────────────────────────────────────

unsafe extern "C" fn ffi_list_task_lists(h: *mut c_void, _a: *const u8, _l: usize) -> PluginCallResult {
    dispatch(h, |p| async move { p.list_task_lists().await })
}

unsafe extern "C" fn ffi_get_tasks(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move { p.get_tasks(&list_id).await })
}

#[derive(Debug, Deserialize)]
struct CreateTaskArgs { list_id: String, task: NewTask }

unsafe extern "C" fn ffi_create_task(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: CreateTaskArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move { p.create_task(&args.list_id, args.task).await })
}

unsafe extern "C" fn ffi_update_task(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let task: cal_core::Task = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move { p.update_task(task).await })
}

unsafe extern "C" fn ffi_delete_task(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let task_id: String = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(h, move |p| async move { p.delete_task(&task_id).await })
}

#[derive(Debug, Deserialize)]
struct RenameTaskListArgs { list_id: String, new_name: String }

unsafe extern "C" fn ffi_rename_task_list(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: RenameTaskListArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(h, move |p| async move { p.rename_task_list(&args.list_id, &args.new_name).await })
}

// ── ContactsFeature ────────────────────────────────────────

unsafe extern "C" fn ffi_list_contact_lists(h: *mut c_void, _a: *const u8, _l: usize) -> PluginCallResult {
    dispatch(h, |p| async move { p.list_contact_lists().await })
}

unsafe extern "C" fn ffi_get_contacts(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move { p.get_contacts(&list_id).await })
}

unsafe extern "C" fn ffi_search_contacts(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let query: String = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move { p.search_contacts(&query).await })
}

#[derive(Debug, Deserialize)]
struct CreateContactArgs { list_id: String, contact: NewContact }

unsafe extern "C" fn ffi_create_contact(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: CreateContactArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move { p.create_contact(&args.list_id, args.contact).await })
}

unsafe extern "C" fn ffi_update_contact(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let contact: cal_core::Contact = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move { p.update_contact(contact).await })
}

unsafe extern "C" fn ffi_delete_contact(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let contact_id: String = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(h, move |p| async move { p.delete_contact(&contact_id).await })
}

#[derive(Debug, Deserialize)]
struct RenameContactListArgs { list_id: String, new_name: String }

unsafe extern "C" fn ffi_rename_contact_list(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: RenameContactListArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(h, move |p| async move { p.rename_contact_list(&args.list_id, &args.new_name).await })
}

unsafe extern "C" fn ffi_get_contact_photo(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let contact_id: String = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(h, move |p| async move { p.get_contact_photo(&contact_id).await })
}

#[derive(Debug, Deserialize)]
struct SetContactPhotoArgs { contact_id: String, photo: ContactPhoto }

unsafe extern "C" fn ffi_set_contact_photo(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: SetContactPhotoArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(h, move |p| async move { p.set_contact_photo(&args.contact_id, args.photo).await })
}

unsafe extern "C" fn ffi_delete_contact_photo(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let contact_id: String = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(h, move |p| async move { p.delete_contact_photo(&contact_id).await })
}

unsafe extern "C" fn ffi_invalidate_contacts_cache(h: *mut c_void, _a: *const u8, _l: usize) -> PluginCallResult {
    dispatch_unit(h, |p| async move { p.invalidate_contacts_cache().await })
}

// ── Vtables ────────────────────────────────────────────────

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

pub static ADAPTER_VTABLE: CalendarAdapterVtable = CalendarAdapterVtable {
    vtable_version: plugin_sdk::plugin_core::ABI_VERSION,
    calendar: &CALENDAR_VTABLE,
    tasks: &TASKS_VTABLE,
    contacts: &CONTACTS_VTABLE,
};

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.cal-adapter-google",
    name: "Aperio Google",
    version: "0.1.0",
    plugin_type: "calendar-adapter",
    vtable: ADAPTER_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}

// ─────────────────────────────────────────────────────────────
// Interactive auth (OAuth 2.0 PKCE flow)
// ─────────────────────────────────────────────────────────────
//
// The host's `connect_google_account` Tauri command calls
// `PluginManager::interactive_auth` with the Cloud Console
// credentials; this handler runs the loopback OAuth dance via
// the adapter crate's existing runner + returns the resulting
// `TokenSet` as JSON bytes the host can persist into the
// keychain.

#[derive(Debug, serde::Deserialize)]
struct InteractiveAuthArgs {
    client_id: String,
    client_secret: String,
}

async fn plugin_interactive_auth(args_json: String) -> Result<Vec<u8>, String> {
    let args: InteractiveAuthArgs = serde_json::from_str(&args_json)
        .map_err(|e| format!("malformed interactive_auth args: {e}"))?;
    if args.client_id.trim().is_empty() || args.client_secret.trim().is_empty() {
        return Err("client_id and client_secret must not be empty".to_string());
    }
    let tokens = GoogleAdapter::authenticate_interactive(
        args.client_id.trim(),
        args.client_secret.trim(),
    )
    .await
    .map_err(|e| format!("Google OAuth: {e}"))?;
    serde_json::to_vec(&tokens)
        .map_err(|e| format!("serialise TokenSet: {e}"))
}

plugin_sdk::declare_interactive_auth! {
    handler: plugin_interactive_auth,
}
