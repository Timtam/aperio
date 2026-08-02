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
//!
//! Of those keys the user types exactly two — `client_id` and `client_secret`,
//! the only ones `plugin.json` declares as form fields. The three token keys
//! are filled in by the host after the sign-in: `client_secret` comes back from
//! the keychain (declared in the `oauth_client_secret` slot, so it never
//! reaches the account row, which syncs unencrypted whenever end-to-end
//! encryption is off), and `access_token` / `refresh_token` come from the slots
//! the manifest's `oauth` block names. `expires_at` and `scope` are optional —
//! see [`assume_expired`].

use std::os::raw::{c_char, c_void};

use base64::Engine as _;
use cal_adapter_google::drive::{DriveSyncAdapter, GoogleDriveAccountConfig};
use cal_adapter_google::{GoogleAdapter, TokenSet};
use cal_core::adapter::{Capability, Credentials as CalCredentials};
use cal_core::types::{AttendeeStatus, ContactPhoto, DateRange, NewContact, NewEvent, NewTask};
use cal_core::{CalendarFeature, ContactsFeature, TasksFeature};
use chrono::{DateTime, Utc};
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::{
    AdapterVtable, CalendarVtable, ContactsVtable, SyncVtable, TasksVtable,
};
use plugin_sdk::{
    decode_args, error_response, ok_response, open_instance_with, sync_error_to_response,
    PluginInstance,
};
use serde::Deserialize;
use sync_core::{DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter};

/// One Google account, in both of the roles it can now play.
///
/// `PluginInstance<T>` carries exactly one type, and this plugin serves two
/// families of vtable whose adapters are separate structs. So the instance is
/// the pair, and each FFI shim projects into the half it belongs to.
///
/// Both halves are optional, and which one is present is decided by what the
/// host handed over rather than by a flag:
///
/// - `cal` needs an access token. An account created as a Drive SYNC TARGET
///   never had one — the old `googledrive` schema had no such field — so its
///   calendar side cannot be built, and saying so is better than building an
///   adapter that 403s on every call.
/// - `drive` needs a refresh token, which is what mints Drive's own access
///   tokens. An account whose grant has lapsed has none, and then there is
///   nothing to sync through either.
///
/// Neither absence is an error at open time. A calendars-only Google account is
/// perfectly usable, and so is a Drive-only one; the refusal comes when
/// something actually asks the missing half to do work, and it names the repair.
pub struct GoogleAccount {
    cal: Option<GoogleAdapter>,
    drive: Option<DriveSyncAdapter>,
}

/// The calendar/tasks/contacts half, or the refusal that says how to get it.
///
/// A `googledrive` row adopted by this plugin reaches here: it is a real Google
/// account, it just signed in for Drive alone. Re-connecting it runs the
/// current consent, which asks for both, so the repair is the ordinary one the
/// accounts screen already offers for an OAuth account.
fn cal_half(account: &GoogleAccount) -> Result<&GoogleAdapter, cal_core::error::Error> {
    account.cal.as_ref().ok_or_else(|| {
        cal_core::error::Error::authentication(
            "this account was connected for Drive storage only; reconnect it to use its \
             calendars, tasks and contacts",
        )
    })
}

/// The Drive half, or the refusal that says why it is missing.
fn drive_half(account: &GoogleAccount) -> Result<&DriveSyncAdapter, sync_core::SyncError> {
    account.drive.as_ref().ok_or_else(|| {
        sync_core::SyncError::Auth(
            "this Google account holds no refresh token, so it cannot reach Drive; reconnect it"
                .to_string(),
        )
    })
}

// The dispatch helpers, by hand rather than through `cal_dispatch_helpers!` /
// `sync_dispatch_helpers!`: both macros emit `instance` / `dispatch` /
// `dispatch_unit` at crate root against ONE adapter type, so a plugin that
// serves two families cannot use either. `plugin_sdk`'s underlying functions are
// generic over the instance type and unconstrained, which is all this needs.

#[allow(dead_code)]
fn instance<'a>(
    handle: *mut c_void,
) -> Result<&'a PluginInstance<GoogleAccount>, PluginCallResult> {
    plugin_sdk::instance::<GoogleAccount>(handle)
}

fn dispatch<T, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static GoogleAdapter) -> Fut,
    Fut: std::future::Future<Output = cal_core::error::Result<T>>,
{
    plugin_sdk::cal_dispatch::<GoogleAccount, T, _, _>(handle, move |acct| async move {
        call(cal_half(acct)?).await
    })
}

fn dispatch_unit<F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    F: FnOnce(&'static GoogleAdapter) -> Fut,
    Fut: std::future::Future<Output = cal_core::error::Result<()>>,
{
    plugin_sdk::cal_dispatch_unit::<GoogleAccount, _, _>(handle, move |acct| async move {
        call(cal_half(acct)?).await
    })
}

fn sync_dispatch<T, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static DriveSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<T>>,
{
    plugin_sdk::sync_dispatch::<GoogleAccount, T, _, _>(handle, move |acct| async move {
        call(drive_half(acct)?).await
    })
}

fn sync_dispatch_unit<F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    F: FnOnce(&'static DriveSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<()>>,
{
    plugin_sdk::sync_dispatch_unit::<GoogleAccount, _, _>(handle, move |acct| async move {
        call(drive_half(acct)?).await
    })
}

#[derive(Debug, Deserialize)]
struct InitConfig {
    client_id: String,
    client_secret: String,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Absent means "assume stale". The schema-driven host merges the two
    /// tokens by keychain slot and has nowhere to say when one expires, so
    /// demanding this key would make a correctly declared account fail to open.
    /// The epoch is also exactly what the older per-kind host path sends
    /// verbatim — the API client refreshes lazily on a 401 either way, so the
    /// persisted access token never has to be fresh across a restart.
    #[serde(default = "assume_expired")]
    expires_at: DateTime<Utc>,
    #[serde(default)]
    scope: Option<String>,
    /// The folder under My Drive that holds the dataset when this account is
    /// used as the sync target. Absent or blank means `Aperio`, which is also
    /// what a row written by the retired `googledrive` adapter carries when the
    /// user never changed it.
    #[serde(default)]
    folder_name: String,
}

fn assume_expired() -> DateTime<Utc> {
    DateTime::UNIX_EPOCH
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(config_json: *const c_char) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig =
            serde_json::from_str(json).map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.client_id.trim().is_empty() || cfg.client_secret.trim().is_empty() {
            return Err("client_id and client_secret must not be empty".to_string());
        }
        // Neither token is demanded here. Which halves this account can serve
        // is READ OFF what arrived, because the answer differs per account and
        // refusing to open at all would take a working half down with the
        // missing one. See [`GoogleAccount`].
        if cfg.access_token.trim().is_empty()
            && cfg
                .refresh_token
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
        {
            return Err(
                "this account carries neither an access token nor a refresh token".to_string(),
            );
        }
        let drive = match cfg.refresh_token.as_deref().map(str::trim) {
            Some(refresh) if !refresh.is_empty() => Some(
                DriveSyncAdapter::new(
                    GoogleDriveAccountConfig {
                        client_id: cfg.client_id.clone(),
                        client_secret: cfg.client_secret.clone(),
                        folder_name: cfg.folder_name.clone(),
                    },
                    refresh,
                )
                .map_err(|e| format!("Drive adapter ctor failed: {e:?}"))?,
            ),
            _ => None,
        };
        let cal = (!cfg.access_token.trim().is_empty()).then(|| {
            GoogleAdapter::new(
                cfg.client_id,
                cfg.client_secret,
                TokenSet {
                    access_token: cfg.access_token,
                    refresh_token: cfg.refresh_token,
                    expires_at: cfg.expires_at,
                    scope: cfg.scope,
                },
            )
        });
        Ok(GoogleAccount { cal, drive })
    })
}

/// # Safety
/// FFI export; `handle` must be the pointer returned by
/// [`plugin_open_instance`].
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<GoogleAccount>::drop_handle(handle);
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
    // Synchronous, so it cannot go through `dispatch`; the same projection by
    // hand. An account with no calendar half declares no calendar capability,
    // which is the truthful answer rather than an error — the caller is asking
    // what this account can do, and the reply is "none of this".
    let caps: Vec<Capability> = match inst.plugin().cal.as_ref() {
        Some(cal) => cal_core::Adapter::capabilities(cal).to_vec(),
        None => Vec::new(),
    };
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
    // Also synchronous. No calendar half means no cached colour, which is the
    // same answer this returns for a calendar it has not listed yet.
    let color = inst
        .plugin()
        .cal
        .as_ref()
        .and_then(|cal| cal.calendar_color(&calendar_id));
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

// ── SyncFeature (Google Drive) ─────────────────────────────
//
// Lifted verbatim from the retired `sync-adapter-googledrive-plugin`; only the
// projection into the account's Drive half is new. The adapter crate itself did
// not move.

unsafe extern "C" fn ffi_test_connection(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    sync_dispatch_unit(h, |p| async move { p.test_connection().await })
}

unsafe extern "C" fn ffi_fetch_meta(h: *mut c_void, _a: *const u8, _l: usize) -> PluginCallResult {
    sync_dispatch(h, |p| async move { p.fetch_meta().await })
}

unsafe extern "C" fn ffi_push_meta(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let meta: MetaJson = match decode_args(a, l) {
        Ok(m) => m,
        Err(r) => return r,
    };
    sync_dispatch_unit(h, |p| async move { p.push_meta(&meta).await })
}

unsafe extern "C" fn ffi_fetch_new_logs(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let cursor: DeviceCursor = match decode_args(a, l) {
        Ok(c) => c,
        Err(r) => return r,
    };
    sync_dispatch(h, |p| async move { p.fetch_new_logs(&cursor).await })
}

unsafe extern "C" fn ffi_push_log(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let log: LogFile = match decode_args(a, l) {
        Ok(l) => l,
        Err(r) => return r,
    };
    sync_dispatch_unit(h, |p| async move { p.push_log(&log).await })
}

unsafe extern "C" fn ffi_fetch_snapshot(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    sync_dispatch(h, |p| async move { p.fetch_snapshot().await })
}

unsafe extern "C" fn ffi_push_snapshot(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let snap: Snapshot = match decode_args(a, l) {
        Ok(s) => s,
        Err(r) => return r,
    };
    sync_dispatch_unit(h, |p| async move { p.push_snapshot(&snap).await })
}

unsafe extern "C" fn ffi_delete_log(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let name: LogFileName = match decode_args(a, l) {
        Ok(n) => n,
        Err(r) => return r,
    };
    sync_dispatch_unit(h, |p| async move { p.delete_log(&name).await })
}

#[derive(Debug, Deserialize)]
struct PushSoundAssetArgs {
    hash: String,
    extension: String,
    bytes_base64: String,
}

unsafe extern "C" fn ffi_push_sound_asset(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: PushSoundAssetArgs = match decode_args(a, l) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let bytes = match base64::engine::general_purpose::STANDARD.decode(args.bytes_base64.as_bytes())
    {
        Ok(b) => b,
        Err(err) => {
            return error_response(
                plugin_sdk::plugin_core::ffi::PLUGIN_CALL_ERR_INVALID,
                &format!("bad base64: {err}"),
            )
        }
    };
    sync_dispatch_unit(h, move |p| {
        let hash = args.hash;
        let extension = args.extension;
        async move { p.push_sound_asset(&hash, &extension, &bytes).await }
    })
}

#[derive(Debug, Deserialize)]
struct FetchSoundAssetArgs {
    hash: String,
    extension: String,
}

unsafe extern "C" fn ffi_fetch_sound_asset(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: FetchSoundAssetArgs = match decode_args(a, l) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let inst = match instance(h) {
        Ok(i) => i,
        Err(r) => return r,
    };
    // Hand-rolled rather than through `sync_dispatch`: the payload is bytes and
    // has to be base64'd on the way out, which the generic marshaller does not
    // do. Same shape as the retired plugin's.
    let account: &'static GoogleAccount =
        unsafe { std::mem::transmute::<&GoogleAccount, &'static GoogleAccount>(inst.plugin()) };
    let outcome = inst.runtime().block_on(async move {
        drive_half(account)?
            .fetch_sound_asset(&args.hash, &args.extension)
            .await
    });
    match outcome {
        Ok(None) => ok_response(&Option::<String>::None),
        Ok(Some(bytes)) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            ok_response(&Some(b64))
        }
        Err(err) => sync_error_to_response(err),
    }
}

pub static SYNC_VTABLE: SyncVtable = SyncVtable {
    test_connection: Some(ffi_test_connection),
    fetch_meta: Some(ffi_fetch_meta),
    push_meta: Some(ffi_push_meta),
    fetch_new_logs: Some(ffi_fetch_new_logs),
    push_log: Some(ffi_push_log),
    fetch_snapshot: Some(ffi_fetch_snapshot),
    push_snapshot: Some(ffi_push_snapshot),
    delete_log: Some(ffi_delete_log),
    push_sound_asset: Some(ffi_push_sound_asset),
    fetch_sound_asset: Some(ffi_fetch_sound_asset),
    ..SyncVtable::empty()
};

pub static ADAPTER_VTABLE: AdapterVtable = AdapterVtable {
    calendar: &CALENDAR_VTABLE,
    tasks: &TASKS_VTABLE,
    contacts: &CONTACTS_VTABLE,
    sync: &SYNC_VTABLE,
    ..AdapterVtable::empty()
};

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.cal-adapter-google",
    name: "Aperio Google",
    version: "0.1.0",
    plugin_type: "adapter",
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

#[cfg(test)]
mod tests {

    /// The manifest ships beside this crate and is the ONLY thing that tells
    /// the host how to set up a Google account. Parsing it here means a typo
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
            .expect("Google declares an account schema");
        let known = [
            "client_id",
            "client_secret",
            "access_token",
            "refresh_token",
            "expires_at",
            "scope",
            "folder_name",
        ];
        for field in &schema.fields {
            assert!(
                known.contains(&field.key.as_str()),
                "schema field `{}` is not read by InitConfig",
                field.key
            );
        }
        let oauth = schema.oauth.expect("Google signs in via OAuth");
        for key in [
            Some(oauth.client_id_field.as_str()),
            oauth.client_secret_field.as_deref(),
            oauth.refresh_token_field.as_deref(),
            oauth.access_token_field.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                known.contains(&key),
                "oauth key `{key}` is not read by InitConfig"
            );
        }
    }

    #[test]
    fn only_what_the_user_can_type_is_asked_for() {
        // The four token keys `InitConfig` also reads are filled by the host
        // after the sign-in dance. Declaring one as a form field would put an
        // empty box on the connect form for something nobody can type.
        //
        // `folder_name` IS typeable and therefore does appear — it is optional
        // and only means anything once the account is picked as the place the
        // data is stored, which is what its hint says.
        let schema = manifest().account.unwrap();
        let keys: Vec<&str> = schema.fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["client_id", "client_secret", "folder_name"]);
        assert!(
            !schema.field("folder_name").unwrap().required,
            "a Google account that never holds the dataset must not have to name a folder",
        );
    }

    /// The adoption, asserted on the manifest that ships.
    ///
    /// A `googledrive` row is what a device carried before Drive folded into
    /// this adapter. Dropping this line would not fail anything at build time —
    /// it would leave those rows resolving to no plugin at all, which the host
    /// reads as an unknown kind and therefore as something that TRAVELS
    /// between devices.
    #[test]
    fn the_retired_drive_adapters_kind_is_adopted() {
        let m = manifest();
        assert_eq!(m.adapter_kind.as_deref(), Some("google"));
        assert!(m.serves_kind("googledrive"), "{:?}", m.adopts_adapter_kinds);
        assert!(m.adopts_kind("googledrive"));
        assert!(
            m.capabilities
                .contains(&plugin_sdk::plugin_core::Capability::Sync),
            "adopting the kind is pointless without the capability that serves it",
        );
    }

    #[test]
    fn the_client_secret_is_routed_away_from_the_account_row() {
        use plugin_sdk::plugin_core::account_schema::AccountSecretSlot;
        let schema = manifest().account.unwrap();
        assert_eq!(
            schema.field("client_secret").unwrap().secret_slot,
            Some(AccountSecretSlot::OauthClientSecret)
        );
        // The client id is NOT a secret: it travels in every authorization URL
        // the user's own browser visits, and the host needs it in the row to
        // record which registration an account belongs to.
        assert!(!schema.field("client_id").unwrap().is_secret());
    }

    #[test]
    fn an_access_token_is_requested_because_the_adapter_is_handed_one() {
        // Unlike Webex, this adapter does not mint a token on first use — its
        // `TokenSet` is built at open time and `access_token` must be there.
        let oauth = manifest().account.unwrap().oauth.unwrap();
        assert_eq!(oauth.access_token_field.as_deref(), Some("access_token"));
        assert_eq!(oauth.refresh_token_field.as_deref(), Some("refresh_token"));
        assert_eq!(oauth.builtin_provider.as_deref(), Some("google"));
    }

    #[test]
    fn no_capability_token_is_asked_for() {
        // This plugin exports nothing onto the host channel — it never reports
        // a rotated credential back — so it has no use for the authority.
        assert!(!manifest().account.unwrap().host_channel);
    }

    #[test]
    fn an_init_config_without_an_expiry_still_opens() {
        // What the schema-driven host sends: the two client values plus the
        // tokens it merges by keychain slot, and nothing to say when the access
        // token dies. Requiring `expires_at` would reject exactly that.
        let cfg: super::InitConfig = serde_json::from_str(
            r#"{"client_id":"id","client_secret":"s","access_token":"ya29.x",
                "refresh_token":"1//r"}"#,
        )
        .expect("the host's declared merge is a valid init config");
        assert_eq!(cfg.expires_at, super::assume_expired());
        assert!(cfg.scope.is_none());
    }

    #[test]
    fn both_form_labels_are_translated_in_both_languages() {
        let m = manifest();
        let schema = m.account.as_ref().unwrap();
        for field in &schema.fields {
            for lang in ["en", "de"] {
                // The raw map, not `lookup` — that one falls back to English,
                // so it would call a missing German string a success.
                let catalogue = m.strings.0.get(lang).expect("declared language");
                for key in [field.label_key.as_deref(), field.hint_key.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    assert!(catalogue.contains_key(key), "`{key}` has no {lang} string");
                }
            }
        }
        assert!(m.strings.has_fallback());
    }
}
