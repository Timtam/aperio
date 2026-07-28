//! Cisco WebEx videoconference adapter packaged as a plugin
//! (DESIGN.md §11 + §20).
//!
//! ## Init config
//!
//! ```json
//! {
//!   "client_id": "…",
//!   "client_secret": "…",
//!   "refresh_token": "…",
//!   "site_url": "example.webex.com",
//!   "use_personal_room": false,
//!   "send_webex_emails": false
//! }
//! ```
//!
//! The host merges the two CREDENTIALS — `client_secret` and `refresh_token` —
//! in from the keychain at open time; only the rest is persisted in the account
//! row. That split matters: the account row is appended to the sync event log
//! unencrypted whenever end-to-end encryption is off, so a secret living there
//! would travel to the user's own sync target in the clear.
//!
//! Unlike Teams (Microsoft Graph) and Meet (Google Calendar), Webex does not
//! piggy-back on any of Aperio's calendar adapters — it runs its own OAuth flow
//! against the Webex Meetings REST API, so the refresh token lives in a
//! Webex-specific keychain slot and the host runs a separate sign-in for each
//! Webex account.

use std::os::raw::{c_char, c_void};

use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::VcVtable;
use plugin_sdk::{decode_args, open_instance_with, PluginInstance};
use serde::Deserialize;
use vc_adapter_webex::{WebexAccountConfig, WebexAdapter};
use vc_core::{MeetingId, NewMeeting, VcAdapter};

plugin_sdk::vc_dispatch_helpers!(WebexAdapter);

#[derive(Debug, Deserialize)]
struct InitConfig {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    #[serde(default)]
    site_url: Option<String>,
    #[serde(default)]
    use_personal_room: bool,
    #[serde(default)]
    send_webex_emails: bool,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(config_json: *const c_char) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig =
            serde_json::from_str(json).map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.client_id.trim().is_empty()
            || cfg.client_secret.trim().is_empty()
            || cfg.refresh_token.trim().is_empty()
        {
            return Err("client_id, client_secret and refresh_token must not be empty".to_string());
        }
        Ok(WebexAdapter::new(
            WebexAccountConfig {
                client_id: cfg.client_id,
                site_url: cfg.site_url.filter(|s| !s.trim().is_empty()),
                use_personal_room: cfg.use_personal_room,
                send_webex_emails: cfg.send_webex_emails,
            },
            cfg.client_secret,
            cfg.refresh_token,
        ))
    })
}

/// # Safety
/// FFI export.
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<WebexAdapter>::drop_handle(handle);
}

unsafe extern "C" fn ffi_test_connection(
    h: *mut c_void,
    _a: *const u8,
    _l: usize,
) -> PluginCallResult {
    dispatch_unit(h, |p| async move { p.test_connection().await })
}

unsafe extern "C" fn ffi_create_meeting(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let spec: NewMeeting = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.create_meeting(spec).await })
}

unsafe extern "C" fn ffi_get_meeting(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let id: MeetingId = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch(h, move |p| async move { p.get_meeting(&id).await })
}

unsafe extern "C" fn ffi_delete_meeting(
    h: *mut c_void,
    a: *const u8,
    l: usize,
) -> PluginCallResult {
    let id: MeetingId = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move { p.delete_meeting(&id).await })
}

pub static VC_VTABLE: VcVtable = VcVtable {
    test_connection: Some(ffi_test_connection),
    create_meeting: Some(ffi_create_meeting),
    get_meeting: Some(ffi_get_meeting),
    delete_meeting: Some(ffi_delete_meeting),
    ..VcVtable::empty()
};

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.vc-adapter-webex",
    name: "Aperio Cisco WebEx",
    version: "0.1.0",
    plugin_type: "videoconference-adapter",
    vtable: VC_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}
