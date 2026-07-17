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
use cal_core::adapter::{Capability, Credentials as CalCredentials};
use cal_core::types::{AttendeeStatus, ContactPhoto, DateRange, NewContact, NewEvent, NewTask};
use cal_core::{CalendarFeature, ContactsFeature, TasksFeature};
use chrono::{DateTime, Utc};
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::{
    CalendarAdapterVtable, CalendarVtable, ContactsVtable, TasksVtable,
};
use plugin_sdk::{decode_args, ok_response, open_instance_with, PluginInstance};
use serde::Deserialize;

plugin_sdk::cal_dispatch_helpers!(GoogleAdapter);

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
pub unsafe extern "C" fn plugin_open_instance(config_json: *const c_char) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig =
            serde_json::from_str(json).map_err(|e| format!("malformed init config: {e}"))?;
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

// ── Adapter base ───────────────────────────────────────────

unsafe extern "C" fn ffi_authenticate(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let creds: CalCredentials = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
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

// ── CalendarFeature ────────────────────────────────────────

unsafe extern "C" fn ffi_list_calendars(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch(h, |p| async move { p.list_calendars().await })
}

#[derive(Debug, Deserialize)]
struct GetEventsArgs {
    calendar_id: String,
    range: DateRange,
}

unsafe extern "C" fn ffi_get_events(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: GetEventsArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.get_events(&args.calendar_id, args.range).await
    })
}

#[derive(Debug, Deserialize)]
struct GetEventsDeltaArgs {
    calendar_id: String,
    range: DateRange,
    since_token: Option<String>,
}

unsafe extern "C" fn ffi_get_events_delta(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: GetEventsDeltaArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.get_events_delta(&args.calendar_id, args.range, args.since_token.as_deref())
            .await
    })
}

#[derive(Debug, Deserialize)]
struct CreateEventArgs {
    calendar_id: String,
    event: NewEvent,
}

unsafe extern "C" fn ffi_create_event(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: CreateEventArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.create_event(&args.calendar_id, args.event).await
    })
}

unsafe extern "C" fn ffi_update_event(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let event: cal_core::Event = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.update_event(event).await })
}

#[derive(Debug, Deserialize)]
struct DeleteEventArgs {
    event_id: String,
    #[serde(default)]
    send_cancellations: bool,
}

unsafe extern "C" fn ffi_delete_event(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: DeleteEventArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.delete_event(&args.event_id, args.send_cancellations)
            .await
    })
}

#[derive(Debug, Deserialize)]
struct GetFreeBusyArgs {
    emails: Vec<String>,
    range: DateRange,
}

unsafe extern "C" fn ffi_get_free_busy(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: GetFreeBusyArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        let refs: Vec<&str> = args.emails.iter().map(|s| s.as_str()).collect();
        p.get_free_busy(&refs, args.range).await
    })
}

unsafe extern "C" fn ffi_calendar_color(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let calendar_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let inst = match instance(h) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let color = inst.plugin().calendar_color(&calendar_id);
    ok_response(&color)
}

#[derive(Debug, Deserialize)]
struct AddExdateArgs {
    event_id: String,
    occurrence: DateTime<Utc>,
    #[serde(default)]
    send_cancellations: bool,
}

unsafe extern "C" fn ffi_add_event_exdate(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: AddExdateArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.add_event_exdate(&args.event_id, args.occurrence, args.send_cancellations)
            .await
    })
}

#[derive(Debug, Deserialize)]
struct RenameCalendarArgs {
    calendar_id: String,
    new_name: String,
}

unsafe extern "C" fn ffi_rename_calendar(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: RenameCalendarArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.rename_calendar(&args.calendar_id, &args.new_name).await
    })
}

// ── TasksFeature ───────────────────────────────────────────

unsafe extern "C" fn ffi_list_task_lists(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch(h, |p| async move { p.list_task_lists().await })
}

unsafe extern "C" fn ffi_get_tasks(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.get_tasks(&list_id).await })
}

#[derive(Debug, Deserialize)]
struct CreateTaskArgs {
    list_id: String,
    task: NewTask,
}

unsafe extern "C" fn ffi_create_task(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: CreateTaskArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.create_task(&args.list_id, args.task).await
    })
}

unsafe extern "C" fn ffi_update_task(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let task: cal_core::Task = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.update_task(task).await })
}

unsafe extern "C" fn ffi_delete_task(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let task_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
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
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.rename_task_list(&args.list_id, &args.new_name).await
    })
}

#[derive(Debug, Deserialize)]
struct CreateTaskListArgs {
    name: String,
    parent_id: Option<String>,
}

unsafe extern "C" fn ffi_create_task_list(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: CreateTaskListArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.create_task_list(&args.name, args.parent_id.as_deref())
            .await
    })
}

unsafe extern "C" fn ffi_delete_task_list(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(
        h,
        move |p| async move { p.delete_task_list(&list_id).await },
    )
}

// ── ContactsFeature ────────────────────────────────────────

unsafe extern "C" fn ffi_list_contact_lists(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch(h, |p| async move { p.list_contact_lists().await })
}

unsafe extern "C" fn ffi_get_contacts(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let list_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.get_contacts(&list_id).await })
}

#[derive(Debug, Deserialize)]
struct GetContactsDeltaArgs {
    list_id: String,
    since_token: Option<String>,
}

unsafe extern "C" fn ffi_get_contacts_delta(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: GetContactsDeltaArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.get_contacts_delta(&args.list_id, args.since_token.as_deref())
            .await
    })
}

unsafe extern "C" fn ffi_search_contacts(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let query: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.search_contacts(&query).await })
}

#[derive(Debug, Deserialize)]
struct CreateContactArgs {
    list_id: String,
    contact: NewContact,
}

unsafe extern "C" fn ffi_create_contact(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: CreateContactArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.create_contact(&args.list_id, args.contact).await
    })
}

unsafe extern "C" fn ffi_update_contact(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let contact: cal_core::Contact = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.update_contact(contact).await })
}

unsafe extern "C" fn ffi_delete_contact(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let contact_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(
        h,
        move |p| async move { p.delete_contact(&contact_id).await },
    )
}

#[derive(Debug, Deserialize)]
struct RenameContactListArgs {
    list_id: String,
    new_name: String,
}

unsafe extern "C" fn ffi_rename_contact_list(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: RenameContactListArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.rename_contact_list(&args.list_id, &args.new_name).await
    })
}

unsafe extern "C" fn ffi_get_contact_photo(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let contact_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(
        h,
        move |p| async move { p.get_contact_photo(&contact_id).await },
    )
}

#[derive(Debug, Deserialize)]
struct SetContactPhotoArgs {
    contact_id: String,
    photo: ContactPhoto,
}

unsafe extern "C" fn ffi_set_contact_photo(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: SetContactPhotoArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.set_contact_photo(&args.contact_id, args.photo).await
    })
}

unsafe extern "C" fn ffi_delete_contact_photo(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let contact_id: String = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.delete_contact_photo(&contact_id).await
    })
}

unsafe extern "C" fn ffi_invalidate_contacts_cache(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch_unit(h, |p| async move { p.invalidate_contacts_cache().await })
}

// ── Vtables ────────────────────────────────────────────────

unsafe extern "C" fn ffi_current_user_email(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch(h, |p| async move { p.current_user_email().await })
}

#[derive(Debug, Deserialize)]
struct RespondToEventArgs {
    event_id: String,
    status: AttendeeStatus,
    send_response: bool,
}

unsafe extern "C" fn ffi_respond_to_event(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: RespondToEventArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move {
        p.respond_to_event(&args.event_id, args.status, args.send_response)
            .await
    })
}

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
    get_events_delta: Some(ffi_get_events_delta),
    current_user_email: Some(ffi_current_user_email),
    respond_to_event: Some(ffi_respond_to_event),
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
    create_task_list: Some(ffi_create_task_list),
    delete_task_list: Some(ffi_delete_task_list),
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
    get_contacts_delta: Some(ffi_get_contacts_delta),
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

// The handler supports the desktop "full" loopback dance (no `phase`, the
// historical contract — backward compatible) AND a host-driven split for
// mobile: `phase:"authorize"` returns {authorize_url, pkce_verifier, state} for
// the host to open in a native auth session; `phase:"exchange"` swaps the
// returned code for tokens. The host holds verifier/state between the two
// calls (the adapter is stateless across phases).
#[derive(Debug, serde::Deserialize)]
struct InteractiveAuthArgs {
    client_id: String,
    /// Required for "full" + "exchange"; absent in "authorize".
    #[serde(default)]
    client_secret: Option<String>,
    /// `"authorize"` | `"exchange"` | absent/`"full"` (desktop loopback).
    #[serde(default)]
    phase: Option<String>,
    /// authorize + exchange: the caller-supplied redirect URI (mobile scheme).
    #[serde(default)]
    redirect_uri: Option<String>,
    /// exchange: the auth code + the verifier/state from the authorize phase.
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    pkce_verifier: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    returned_state: Option<String>,
}

/// Validate the OAuth CSRF `state`: the redirect's returned state must be
/// present, non-empty, and equal to the one issued at authorize. Fails CLOSED —
/// a missing or empty value on either side is rejected, since this guards the
/// token exchange + account creation.
fn verify_oauth_state(issued: Option<&str>, returned: Option<&str>) -> Result<(), String> {
    let issued = issued.unwrap_or_default().trim();
    let returned = returned.unwrap_or_default().trim();
    if issued.is_empty() || returned.is_empty() || issued != returned {
        return Err("OAuth state mismatch (possible CSRF) — aborting".to_string());
    }
    Ok(())
}

async fn plugin_interactive_auth(args_json: String) -> Result<Vec<u8>, String> {
    let args: InteractiveAuthArgs = serde_json::from_str(&args_json)
        .map_err(|e| format!("malformed interactive_auth args: {e}"))?;
    let client_id = args.client_id.trim();
    if client_id.is_empty() {
        return Err("client_id must not be empty".to_string());
    }
    match args.phase.as_deref() {
        Some("authorize") => {
            let redirect_uri = args
                .redirect_uri
                .ok_or_else(|| "redirect_uri is required in the authorize phase".to_string())?;
            let authz = GoogleAdapter::oauth_authorize(client_id, &redirect_uri)
                .map_err(|e| format!("Google authorize: {e}"))?;
            serde_json::to_vec(&authz).map_err(|e| format!("serialise authorize response: {e}"))
        }
        Some("exchange") => {
            let secret = args
                .client_secret
                .ok_or_else(|| "client_secret is required in the exchange phase".to_string())?;
            if secret.trim().is_empty() {
                return Err("client_secret must not be empty".to_string());
            }
            let code = args
                .code
                .ok_or_else(|| "code is required in the exchange phase".to_string())?;
            let verifier = args
                .pkce_verifier
                .ok_or_else(|| "pkce_verifier is required in the exchange phase".to_string())?;
            let redirect_uri = args
                .redirect_uri
                .ok_or_else(|| "redirect_uri is required in the exchange phase".to_string())?;
            // CSRF: the redirect's `state` must equal the one issued at authorize.
            // Fail CLOSED — this guards token minting + account creation, so a
            // missing/empty state on either side aborts (see verify_oauth_state).
            verify_oauth_state(args.state.as_deref(), args.returned_state.as_deref())?;
            let tokens = GoogleAdapter::oauth_exchange(
                client_id,
                secret.trim(),
                code.trim(),
                verifier.trim(),
                &redirect_uri,
            )
            .await
            .map_err(|e| format!("Google exchange: {e}"))?;
            serde_json::to_vec(&tokens).map_err(|e| format!("serialise TokenSet: {e}"))
        }
        None | Some("full") => {
            let secret = args
                .client_secret
                .ok_or_else(|| "client_secret must not be empty".to_string())?;
            if secret.trim().is_empty() {
                return Err("client_id and client_secret must not be empty".to_string());
            }
            let tokens = GoogleAdapter::authenticate_interactive(client_id, secret.trim())
                .await
                .map_err(|e| format!("Google OAuth: {e}"))?;
            serde_json::to_vec(&tokens).map_err(|e| format!("serialise TokenSet: {e}"))
        }
        Some(other) => Err(format!("unknown interactive_auth phase: {other}")),
    }
}

plugin_sdk::declare_interactive_auth! {
    handler: plugin_interactive_auth,
}
