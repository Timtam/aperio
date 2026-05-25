//! iCal-feed calendar adapter packaged as a plugin
//! (DESIGN.md §20). Canonical pattern for calendar-adapter
//! plugins; the four big multi-capability adapters
//! (CalDAV, Google, MS Graph, EWS) follow this shape but fill
//! more than one slot in the [`CalendarAdapterVtable`].
//!
//! ## Init config
//!
//! ```json
//! {
//!   "feed_url": "https://example.com/calendar.ics",
//!   "username": null,
//!   "password": null
//! }
//! ```
//!
//! `username` + `password` are both optional — most public
//! feeds don't need them. When present, the adapter sends
//! HTTP Basic Auth.
//!
//! ## Vtable shape
//!
//! Single-capability calendar adapter — fills only the
//! `calendar` slot of [`CalendarAdapterVtable`]; `tasks` +
//! `contacts` stay null. iCal feeds are read-only at the
//! protocol level, so the write-side methods (`create_event`,
//! `update_event`, `delete_event`, `add_event_exdate`,
//! `rename_calendar`) are left at `None` and the host's shim
//! surfaces them as `cal_core::Error::Unsupported`.

use std::os::raw::{c_char, c_int};

use cal_core::adapter::{AuthToken, Capability, Credentials as CalCredentials};
use cal_core::error::Result as CalResult;
use cal_core::types::DateRange;
use cal_core::CalendarFeature;
use cal_adapter_ical::{Credentials as IcalCredentials, IcalAccountConfig, IcalAdapter};
use plugin_sdk::plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};
use plugin_sdk::plugin_core::vtables::{CalendarAdapterVtable, CalendarVtable};
use plugin_sdk::{
    cal_error_to_response, decode_args, error_response, ok_empty_response,
    ok_response, PluginSingleton,
};
use serde::Deserialize;
use tracing::warn;

pub static PLUGIN_INSTANCE: PluginSingleton<IcalAdapter> =
    PluginSingleton::new();

#[derive(Debug, Deserialize)]
struct InitConfig {
    feed_url: String,
    #[serde(default)]
    username: Option<String>,
    /// Host pre-extracts the password from keychain + threads
    /// it in via `config_json`. Same pattern as the OAuth-
    /// based sync plugins from P4 (sync).
    #[serde(default)]
    password: Option<String>,
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
            warn!(?err, "cal-adapter-ical-plugin: malformed init config");
            return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
        }
    };
    if cfg.feed_url.trim().is_empty() {
        return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
    }
    let credentials = IcalCredentials::new(
        IcalAccountConfig {
            feed_url: cfg.feed_url,
            username: cfg.username,
        },
        cfg.password,
    );
    let adapter = match IcalAdapter::new(credentials) {
        Ok(a) => a,
        Err(err) => {
            warn!(?err, "cal-adapter-ical-plugin: adapter ctor failed");
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
// Dispatch helpers — cal-core's CalendarFeature uses
// `cal_core::Result<T>` so the error mapping side hits
// cal_error_to_response (vs the sync plugins' sync_error_to_response).
// ─────────────────────────────────────────────────────────────

fn dispatch<T, F, Fut>(call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static IcalAdapter) -> Fut,
    Fut: std::future::Future<Output = CalResult<T>>,
{
    let Some((p, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let p_static: &'static IcalAdapter =
        unsafe { std::mem::transmute::<&IcalAdapter, &'static IcalAdapter>(p) };
    match rt.block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => cal_error_to_response(e),
    }
}

#[allow(dead_code)] // every iCal trait method that ever returns Result<()>
fn dispatch_unit<F, Fut>(call: F) -> PluginCallResult
where
    F: FnOnce(&'static IcalAdapter) -> Fut,
    Fut: std::future::Future<Output = CalResult<()>>,
{
    let Some((p, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let p_static: &'static IcalAdapter =
        unsafe { std::mem::transmute::<&IcalAdapter, &'static IcalAdapter>(p) };
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
    let p_static: &'static IcalAdapter =
        unsafe { std::mem::transmute::<&IcalAdapter, &'static IcalAdapter>(p) };
    let outcome: CalResult<AuthToken> = rt.block_on(async move {
        cal_core::Adapter::authenticate(p_static, creds).await
    });
    match outcome {
        Ok(v) => ok_response(&v),
        Err(e) => cal_error_to_response(e),
    }
}

unsafe extern "C" fn ffi_capabilities(
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    let Some(p) = PLUGIN_INSTANCE.get() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    // `Adapter::capabilities` is sync — no runtime needed.
    let caps: Vec<Capability> = cal_core::Adapter::capabilities(p).to_vec();
    ok_response(&caps)
}

// ─────────────────────────────────────────────────────────────
// CalendarFeature trait
// ─────────────────────────────────────────────────────────────

unsafe extern "C" fn ffi_list_calendars(
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
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
    // Sync-shape trait method — no runtime needed.
    let calendar_id: String = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    let Some(p) = PLUGIN_INSTANCE.get() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let color = p.calendar_color(&calendar_id);
    ok_response(&color)
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
    // iCal feeds are read-only at the protocol level — create /
    // update / delete / add_event_exdate / rename_calendar stay
    // unimplemented. The host shim returns Error::Unsupported
    // for any None slot, which is exactly the message the user
    // should see.
    create_event: None,
    update_event: None,
    delete_event: None,
    get_free_busy: Some(ffi_get_free_busy),
    calendar_color: Some(ffi_calendar_color),
    add_event_exdate: None,
    rename_calendar: None,
    ..CalendarVtable::empty()
};

#[no_mangle]
pub static ADAPTER_VTABLE: CalendarAdapterVtable = CalendarAdapterVtable {
    vtable_version: plugin_sdk::plugin_core::ABI_VERSION,
    calendar: &CALENDAR_VTABLE,
    tasks: std::ptr::null(),
    contacts: std::ptr::null(),
};

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.cal-adapter-ical",
    name: "Aperio iCal Feed",
    version: "0.1.0",
    plugin_type: "calendar-adapter",
    vtable: ADAPTER_VTABLE,
    init: plugin_init,
    destroy: plugin_destroy,
}
