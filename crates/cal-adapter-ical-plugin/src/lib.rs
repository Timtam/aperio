//! iCal-feed calendar adapter packaged as a plugin
//! (DESIGN.md §20).
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

use std::os::raw::{c_char, c_void};

use cal_core::adapter::{AuthToken, Capability, Credentials as CalCredentials};
use cal_core::error::Result as CalResult;
use cal_core::types::DateRange;
use cal_core::CalendarFeature;
use cal_adapter_ical::{Credentials as IcalCredentials, IcalAccountConfig, IcalAdapter};
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};
use plugin_sdk::plugin_core::vtables::{CalendarAdapterVtable, CalendarVtable};
use plugin_sdk::{
    cal_error_to_response, decode_args, error_response, ok_response,
    open_instance_with, PluginInstance,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct InitConfig {
    feed_url: String,
    #[serde(default)]
    username: Option<String>,
    /// Host pre-extracts the password from keychain + threads
    /// it in via `config_json`.
    #[serde(default)]
    password: Option<String>,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(
    config_json: *const c_char,
) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig = serde_json::from_str(json)
            .map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.feed_url.trim().is_empty() {
            return Err("feed_url must not be empty".to_string());
        }
        let credentials = IcalCredentials::new(
            IcalAccountConfig {
                feed_url: cfg.feed_url,
                username: cfg.username,
            },
            cfg.password,
        );
        IcalAdapter::new(credentials)
            .map_err(|e| format!("adapter ctor failed: {e:?}"))
    })
}

/// # Safety
/// FFI export; `handle` must be the pointer returned by
/// [`plugin_open_instance`].
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<IcalAdapter>::drop_handle(handle);
}

fn instance<'a>(
    handle: *mut c_void,
) -> Result<&'a PluginInstance<IcalAdapter>, PluginCallResult> {
    unsafe { PluginInstance::<IcalAdapter>::from_handle(handle) }
        .ok_or_else(|| error_response(PLUGIN_CALL_ERR_INTERNAL, "null instance handle"))
}

fn dispatch<T, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static IcalAdapter) -> Fut,
    Fut: std::future::Future<Output = CalResult<T>>,
{
    let inst = match instance(handle) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let p_static: &'static IcalAdapter =
        unsafe { std::mem::transmute::<&IcalAdapter, &'static IcalAdapter>(inst.plugin()) };
    match inst.runtime().block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => cal_error_to_response(e),
    }
}

// ─────────────────────────────────────────────────────────────
// Adapter base trait
// ─────────────────────────────────────────────────────────────

unsafe extern "C" fn ffi_authenticate(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let creds: CalCredentials = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    let inst = match instance(h) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let p_static: &'static IcalAdapter =
        unsafe { std::mem::transmute::<&IcalAdapter, &'static IcalAdapter>(inst.plugin()) };
    let outcome: CalResult<AuthToken> = inst.runtime().block_on(async move {
        cal_core::Adapter::authenticate(p_static, creds).await
    });
    match outcome {
        Ok(v) => ok_response(&v),
        Err(e) => cal_error_to_response(e),
    }
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
// CalendarFeature trait
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

unsafe extern "C" fn ffi_get_events(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: GetEventsArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
    };
    dispatch(h, move |p| async move {
        p.get_events(&args.calendar_id, args.range).await
    })
}

#[derive(Debug, Deserialize)]
struct GetFreeBusyArgs {
    emails: Vec<String>,
    range: DateRange,
}

unsafe extern "C" fn ffi_get_free_busy(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let args: GetFreeBusyArgs = match decode_args(a, l) {
        Ok(v) => v, Err(r) => return r,
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
        Ok(v) => v, Err(r) => return r,
    };
    let inst = match instance(h) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let color = inst.plugin().calendar_color(&calendar_id);
    ok_response(&color)
}

// ─────────────────────────────────────────────────────────────
// Vtables
// ─────────────────────────────────────────────────────────────

pub static CALENDAR_VTABLE: CalendarVtable = CalendarVtable {
    authenticate: Some(ffi_authenticate),
    capabilities: Some(ffi_capabilities),
    list_calendars: Some(ffi_list_calendars),
    get_events: Some(ffi_get_events),
    create_event: None,
    update_event: None,
    delete_event: None,
    get_free_busy: Some(ffi_get_free_busy),
    calendar_color: Some(ffi_calendar_color),
    add_event_exdate: None,
    rename_calendar: None,
    ..CalendarVtable::empty()
};

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
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}
