//! Google Meet videoconference adapter packaged as a plugin
//! (DESIGN.md §11 + §20).
//!
//! ## Init config
//!
//! ```json
//! {
//!   "client_id": "…",
//!   "client_secret": "…",
//!   "refresh_token": "…"
//! }
//! ```
//!
//! Google Meet links are minted through the Calendar API's
//! `conferenceData` field, which uses the standard Google
//! OAuth flow (confidential client — both `client_id` and
//! `client_secret` are required). Rather than running its
//! own sign-in dance, this plugin re-uses the refresh token
//! the host has already minted for the Google calendar
//! adapter: the `refresh_token` field is sourced from the
//! shared keychain slot the `cal-adapter-google` adapter
//! uses, so the user only needs to authorise Google once.
//! The current trait impl is a stub: all four FFI methods
//! route through to `MeetAdapter`'s `Unsupported` returns
//! until the REST layer lands.

use std::os::raw::{c_char, c_void};

use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::{AdapterVtable, VcVtable};
use plugin_sdk::{decode_args, open_instance_with, PluginInstance};
use serde::Deserialize;
use vc_adapter_meet::{MeetAccountConfig, MeetAdapter};
use vc_core::{MeetingId, MeetingRemoval, NewMeeting, VcAdapter};

plugin_sdk::vc_dispatch_helpers!(MeetAdapter);

#[derive(Debug, Deserialize)]
struct InitConfig {
    client_id: String,
    client_secret: String,
    refresh_token: String,
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
        Ok(MeetAdapter::new(
            MeetAccountConfig {
                client_id: cfg.client_id,
                client_secret: cfg.client_secret,
            },
            cfg.refresh_token,
        ))
    })
}

/// # Safety
/// FFI export.
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<MeetAdapter>::drop_handle(handle);
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
    let removal: MeetingRemoval = match decode_args(a, l) {
        Ok(v) => v,
        Err(r) => return r,
    };
    dispatch_unit(h, move |p| async move { p.delete_meeting(removal).await })
}

pub static VC_VTABLE: VcVtable = VcVtable {
    test_connection: Some(ffi_test_connection),
    create_meeting: Some(ffi_create_meeting),
    get_meeting: Some(ffi_get_meeting),
    delete_meeting: Some(ffi_delete_meeting),
    ..VcVtable::empty()
};

/// The outer vtable the host reads. One pointer per feature family;
/// this plugin serves videoconference and leaves the rest null.
pub static ADAPTER_VTABLE: AdapterVtable = AdapterVtable {
    videoconference: &VC_VTABLE,
    ..AdapterVtable::empty()
};

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.vc-adapter-meet",
    name: "Aperio Google Meet",
    version: "0.1.0",
    plugin_type: "adapter",
    vtable: ADAPTER_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}
