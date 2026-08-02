//! Exchange Web Services (EWS) adapter packaged as a plugin
//! (DESIGN.md §20).
//!
//! ## Init config
//!
//! ```json
//! {
//!   "endpoint": "https://mail.example.org/EWS/Exchange.asmx",
//!   "username": "alice@example.org",
//!   "password": "…"
//! }
//! ```
//!
//! EWS auth is per-request HTTP Basic; the host pre-extracts the
//! password from the platform keychain and threads it in via
//! `config_json`. ABI v2 supports N independent EWS endpoints
//! per loaded library.

use std::os::raw::{c_char, c_void};

use adapter_ews::{BasicCredentials, EwsAdapter};
use cal_core::adapter::{Capability, Credentials as CalCredentials};
use cal_core::types::{AttendeeStatus, ContactPhoto, DateRange, NewContact, NewEvent, NewTask};
use cal_core::{CalendarFeature, ContactsFeature, TasksFeature};
use chrono::{DateTime, Utc};
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::{
    AdapterVtable, CalendarVtable, ContactsVtable, TasksVtable,
};
use plugin_sdk::{decode_args, ok_response, open_instance_with, PluginInstance};
use serde::Deserialize;

plugin_sdk::cal_dispatch_helpers!(EwsAdapter);

#[derive(Debug, Deserialize)]
struct InitConfig {
    endpoint: String,
    username: String,
    password: String,
    /// Optional per-account state directory the host computed
    /// off its data_dir. Hands the adapter a stable filesystem
    /// location to cache the SyncFolderItems cookie + the
    /// per-folder item snapshots so a restart doesn't trigger
    /// a full re-sync. Absent on the smoke-test path (one-shot
    /// ephemeral instance) — the adapter then runs purely in
    /// memory.
    #[serde(default)]
    state_dir: Option<String>,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(config_json: *const c_char) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig =
            serde_json::from_str(json).map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.endpoint.trim().is_empty()
            || cfg.username.trim().is_empty()
            || cfg.password.is_empty()
        {
            return Err("endpoint, username and password must not be empty".to_string());
        }
        let creds = BasicCredentials {
            username: cfg.username,
            password: cfg.password,
        };
        let mut adapter = EwsAdapter::new(cfg.endpoint, creds);
        if let Some(dir) = cfg.state_dir {
            adapter = adapter.with_state_dir(std::path::PathBuf::from(dir));
        }
        Ok(adapter)
    })
}

/// # Safety
/// FFI export; `handle` must be the pointer returned by
/// [`plugin_open_instance`].
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<EwsAdapter>::drop_handle(handle);
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
struct GetTasksDeltaArgs {
    list_id: String,
    since_token: Option<String>,
}

unsafe extern "C" fn ffi_get_tasks_delta(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: GetTasksDeltaArgs = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.get_tasks_delta(&args.list_id, args.since_token.as_deref())
            .await
    })
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
    get_tasks_delta: Some(ffi_get_tasks_delta),
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

pub static ADAPTER_VTABLE: AdapterVtable = AdapterVtable {
    calendar: &CALENDAR_VTABLE,
    tasks: &TASKS_VTABLE,
    contacts: &CONTACTS_VTABLE,
    ..AdapterVtable::empty()
};

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.cal-adapter-ews",
    name: "Aperio Exchange (EWS)",
    version: "0.1.0",
    plugin_type: "adapter",
    vtable: ADAPTER_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}

// ─────────────────────────────────────────────────────────────
// Service discovery (Microsoft POX Autodiscover)
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DiscoverArgs {
    email: String,
    password: String,
}

async fn plugin_discover(args_json: String) -> Result<Vec<u8>, String> {
    let args: DiscoverArgs =
        serde_json::from_str(&args_json).map_err(|e| format!("malformed discover args: {e}"))?;
    let email = args.email.trim();
    let password = args.password.as_str();
    if email.is_empty() {
        return Err("email must not be empty".to_string());
    }
    if password.is_empty() {
        return Err("password must not be empty".to_string());
    }
    let http = adapter_ews::discover_client().map_err(|e| format!("build discover client: {e}"))?;
    let endpoints = adapter_ews::discover(email, password, &http)
        .await
        .map_err(|e| format!("EWS Autodiscover: {e}"))?;
    serde_json::to_vec(&endpoints).map_err(|e| format!("serialise DiscoveredEndpoints: {e}"))
}

plugin_sdk::declare_discover! {
    handler: plugin_discover,
}

#[cfg(test)]
mod tests {
    use plugin_sdk::plugin_core::account_schema::{AccountFieldKind, AccountSecretSlot};

    /// The manifest ships beside this crate and is the ONLY thing that tells
    /// the host how to set up an EWS account. Parsing it here means a typo
    /// fails the build rather than the first user who tries to connect.
    fn manifest() -> plugin_sdk::plugin_core::manifest::PluginManifest {
        plugin_sdk::plugin_core::manifest::PluginManifest::from_bytes(include_bytes!(
            "../plugin.json"
        ))
        .expect("plugin.json parses and its account schema validates")
    }

    #[test]
    fn every_schema_field_is_a_key_the_init_config_actually_reads() {
        // The schema and `InitConfig` are two descriptions of the same thing,
        // in two languages, and nothing but this test connects them. A field
        // the host faithfully collects and merges under a name the plugin does
        // not deserialise is silently dropped — the account connects, and then
        // behaves as though the setting were never set.
        let schema = manifest()
            .account
            .expect("the EWS adapter declares an account schema");
        let known = ["endpoint", "username", "password", "state_dir"];
        for field in &schema.fields {
            assert!(
                known.contains(&field.key.as_str()),
                "schema field `{}` is not read by InitConfig",
                field.key
            );
        }
        // No OAuth: EWS authenticates with HTTP Basic on every request.
        assert!(schema.oauth.is_none());
    }

    #[test]
    fn the_state_directory_is_not_asked_of_the_user() {
        // `state_dir` is a key `InitConfig` reads, but the host computes it off
        // its own data dir — a field for it would ask the user to name a
        // filesystem path they have no way to choose correctly, and one wrong
        // answer would put the sync cookie cache outside the portable data
        // layout.
        assert!(manifest().account.unwrap().field("state_dir").is_none());
    }

    #[test]
    fn only_the_password_leaves_the_account_row() {
        let schema = manifest().account.unwrap();
        assert_eq!(
            schema.field("password").unwrap().secret_slot,
            Some(AccountSecretSlot::Password)
        );
        // The endpoint and the user name are what the account row IS — the host
        // shows them back to the user, and both are handed straight to
        // `InitConfig` from `config_json`.
        assert!(!schema.field("endpoint").unwrap().is_secret());
        assert!(!schema.field("username").unwrap().is_secret());
    }

    #[test]
    fn all_three_fields_are_required_because_basic_auth_has_no_anonymous_mode() {
        // `plugin_open_instance` refuses an empty endpoint, username or
        // password. A form that let any of them through would turn a mistake
        // the user can still fix into an account that fails at open time.
        let schema = manifest().account.unwrap();
        for key in ["endpoint", "username", "password"] {
            assert!(
                schema.field(key).unwrap().required,
                "`{key}` must be required"
            );
        }
        // `url` rather than `text` so mobile offers the URL keyboard.
        assert_eq!(
            schema.field("endpoint").unwrap().kind,
            AccountFieldKind::Url
        );
    }

    #[test]
    fn every_declared_label_and_hint_resolves_in_both_languages() {
        // A `label_key` that no catalogue answers degrades to the verbatim
        // English label — silently, and only for the reader whose language is
        // missing. Checking both declared languages here is what keeps that
        // from shipping.
        let manifest = manifest();
        let schema = manifest.account.as_ref().unwrap();
        assert_eq!(
            manifest.strings.languages(),
            vec!["de".to_string(), "en".to_string()]
        );
        for field in &schema.fields {
            for lang in ["en", "de"] {
                // The map directly, NOT `lookup` — that one falls back to
                // English, so it would answer for a German string that isn't
                // there and this test would pass on a half-translated form.
                let catalogue = manifest.strings.0.get(lang).expect("a declared language");
                for key in [field.label_key.as_deref(), field.hint_key.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    assert!(
                        catalogue.contains_key(key),
                        "`{key}` has no {lang} translation"
                    );
                }
            }
        }
    }
}
