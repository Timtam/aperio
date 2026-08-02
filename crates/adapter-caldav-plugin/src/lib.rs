//! CalDAV + CardDAV calendar/tasks/contacts adapter packaged
//! as a plugin (DESIGN.md §20).
//!
//! Multi-capability adapter — fills all three slots of
//! [`AdapterVtable`]: calendar (VEVENT), tasks (VTODO)
//! and contacts (CardDAV VCARD). One [`CaldavAdapter`] instance
//! per opened handle services every surface so the discovery +
//! listing caches stay coherent across reads.
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
//! via `config_json`. ABI v2 supports many parallel
//! `open_instance` calls so multiple CalDAV accounts on the same
//! Aperio session each get their own independent adapter
//! instance (DESIGN.md §6.4).
//!
//! ## What the account schema declares, and what it does not
//!
//! `plugin.json` declares three fields — `server_url`, `username`
//! and `secret` — under exactly the keys this module
//! deserialises. The credential field is called `secret` and not
//! `password`: the *slot* it is kept in is `password`, but the
//! key it is merged back under at open time is the schema field's
//! own key, and this adapter reads `secret`.
//!
//! `auth_kind` is deliberately NOT a declared field. It is one of
//! two magic tokens (`basic` / `bearer`), the schema has no way to
//! present a choice between them, and a free-text box would let a
//! typo turn into a `malformed init config` at connect time. There
//! is also nothing to choose yet: every caller in the tree passes
//! `basic`, and `bearer` exists only for a future OAuth flow. So
//! the default lives where it belongs — in `InitConfig`'s
//! `#[serde(default)]`, which yields [`AuthKind::Basic`] for the
//! key's absence, exactly what the old host form's fixed
//! `auth_kind: "basic"` produced.

use std::os::raw::{c_char, c_void};

use adapter_caldav::{AuthKind, CaldavAccountConfig, CaldavAdapter, Credentials};
use cal_core::adapter::{Capability, Credentials as CalCredentials};
use cal_core::types::{AttendeeStatus, ContactPhoto, DateRange, NewContact, NewEvent, NewTask};
use cal_core::{CalendarFeature, ContactsFeature, TasksFeature};
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::{
    AdapterVtable, CalendarVtable, ContactsVtable, TasksVtable,
};
use plugin_sdk::{decode_args, ok_response, open_instance_with, PluginInstance};
use serde::Deserialize;

plugin_sdk::cal_dispatch_helpers!(CaldavAdapter);

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
pub unsafe extern "C" fn plugin_open_instance(config_json: *const c_char) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig =
            serde_json::from_str(json).map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.server_url.trim().is_empty()
            || cfg.username.trim().is_empty()
            || cfg.secret.is_empty()
        {
            return Err("server_url, username and secret must not be empty".to_string());
        }
        let credentials = Credentials::new(
            CaldavAccountConfig {
                server_url: cfg.server_url,
                username: cfg.username,
                auth_kind: cfg.auth_kind,
            },
            cfg.secret,
        );
        CaldavAdapter::new(credentials, None).map_err(|e| format!("adapter ctor failed: {e:?}"))
    })
}

/// # Safety
/// FFI export; `handle` must be the pointer returned by
/// [`plugin_open_instance`].
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<CaldavAdapter>::drop_handle(handle);
}

// ─────────────────────────────────────────────────────────────
// Adapter base trait
// ─────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────
// CalendarFeature
// ─────────────────────────────────────────────────────────────

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
    occurrence: chrono::DateTime<chrono::Utc>,
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

// ─────────────────────────────────────────────────────────────
// TasksFeature
// ─────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────
// ContactsFeature
// ─────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────
// Vtables
// ─────────────────────────────────────────────────────────────

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
    id: "com.aperio.cal-adapter-caldav",
    name: "Aperio CalDAV",
    version: "0.1.0",
    plugin_type: "adapter",
    vtable: ADAPTER_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest ships beside this crate and is the ONLY thing that tells
    /// the host how to set up a CalDAV account. Parsing it here means a typo
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
            .expect("CalDAV declares an account schema");
        let known = ["server_url", "username", "auth_kind", "secret"];
        for field in &schema.fields {
            assert!(
                known.contains(&field.key.as_str()),
                "schema field `{}` is not read by InitConfig",
                field.key
            );
        }
        assert!(
            schema.oauth.is_none(),
            "CalDAV signs in with a password, so there is no OAuth key to check"
        );
    }

    #[test]
    fn the_credential_is_named_for_what_the_plugin_reads_not_for_the_slot() {
        use plugin_sdk::plugin_core::account_schema::{AccountFieldKind, AccountSecretSlot};
        // The old host form called this field `password` and the plugin has
        // always deserialised `secret`. The schema keeps the two apart on
        // purpose: the KEY is `secret`, because that is what `InitConfig`
        // reads and what the host merges the keychain value back under; the
        // SLOT is `password`, because that is where every existing CalDAV
        // account's credential already sits.
        let schema = manifest().account.unwrap();
        let secret = schema.field("secret").expect("the credential field");
        assert_eq!(secret.kind, AccountFieldKind::Secret);
        assert_eq!(secret.secret_slot, Some(AccountSecretSlot::Password));
        assert!(
            schema.field("password").is_none(),
            "a `password` key would be collected and then dropped on the floor"
        );
        // Neither of the other two may carry a slot: config_json syncs in the
        // clear, and a server URL kept in the keychain would be unreadable.
        assert!(!schema.field("server_url").unwrap().is_secret());
        assert!(!schema.field("username").unwrap().is_secret());
    }

    #[test]
    fn auth_kind_is_carried_by_the_default_rather_than_asked_for() {
        // Not a field: it is a magic token with two values, the schema has no
        // way to offer a choice, and a free-text box would let a typo become a
        // parse failure at connect time. Absence has to mean exactly what the
        // old host form's fixed `auth_kind: "basic"` meant.
        let schema = manifest().account.unwrap();
        assert!(schema.field("auth_kind").is_none());
        let cfg: InitConfig = serde_json::from_str(
            r#"{"server_url":"https://dav.test/","username":"toni","secret":"hunter2"}"#,
        )
        .expect("the three declared fields are a complete init config");
        assert_eq!(cfg.auth_kind, AuthKind::Basic);
    }

    #[test]
    fn every_declared_label_and_hint_key_has_a_string_in_both_languages() {
        // Deliberately NOT `lookup`: that falls back to English, so a German
        // string this catalogue never carried would still "resolve" and the
        // gap would stay invisible until a German user opened the form. Read
        // each language's map directly.
        let m = manifest();
        let schema = m.account.as_ref().expect("CalDAV declares a schema");
        for field in &schema.fields {
            assert!(
                field.label_key.is_some(),
                "field `{}` has no label_key",
                field.key
            );
            for key in [field.label_key.as_deref(), field.hint_key.as_deref()]
                .into_iter()
                .flatten()
            {
                for lang in ["en", "de"] {
                    let present = m
                        .strings
                        .0
                        .get(lang)
                        .and_then(|map| map.get(key))
                        .is_some_and(|s| !s.trim().is_empty());
                    assert!(present, "`{key}` has no `{lang}` string");
                }
            }
        }
    }
}
