//! Dropbox sync adapter packaged as a plugin (DESIGN.md §20).
//!
//! ## Init config
//!
//! ```json
//! {
//!   "client_id": "abcdef…",
//!   "client_secret": "",
//!   "base_path": "/aperio",
//!   "refresh_token": "…"
//! }
//! ```
//!
//! The OAuth dance itself stays host-side (it needs the
//! loopback HTTP server + the system browser, both of which are
//! easier to drive from the Tauri side). The host runs the
//! dance, stashes the refresh token in the keychain, and passes
//! it to the plugin via `refresh_token` in `config_json` at
//! init-time.

use std::os::raw::{c_char, c_int};

use plugin_sdk::plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};
use plugin_sdk::plugin_core::vtables::SyncVtable;
use plugin_sdk::{
    decode_args, error_response, ok_empty_response, ok_response,
    sync_error_to_response, PluginSingleton,
};
use serde::Deserialize;
use sync_adapter_dropbox::{DropboxAccountConfig, DropboxSyncAdapter};
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter,
};
use tracing::warn;

pub static PLUGIN_INSTANCE: PluginSingleton<DropboxSyncAdapter> =
    PluginSingleton::new();

#[derive(Debug, Deserialize)]
struct InitConfig {
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    base_path: String,
    refresh_token: String,
}

/// # Safety
/// FFI export; config_json must be NUL-terminated UTF-8.
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
            warn!(?err, "sync-adapter-dropbox-plugin: malformed init config");
            return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
        }
    };
    if cfg.client_id.trim().is_empty() || cfg.refresh_token.trim().is_empty() {
        return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
    }
    let adapter = match DropboxSyncAdapter::new(
        DropboxAccountConfig {
            client_id: cfg.client_id,
            client_secret: cfg.client_secret,
            base_path: cfg.base_path,
        },
        cfg.refresh_token,
    ) {
        Ok(a) => a,
        Err(err) => {
            warn!(?err, "sync-adapter-dropbox-plugin: adapter ctor failed");
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

fn dispatch<T, F, Fut>(call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static DropboxSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<T>>,
{
    let Some((p, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let p_static: &'static DropboxSyncAdapter =
        unsafe { std::mem::transmute::<&DropboxSyncAdapter, &'static DropboxSyncAdapter>(p) };
    match rt.block_on(call(p_static)) {
        Ok(v) => ok_response(&v),
        Err(e) => sync_error_to_response(e),
    }
}

fn dispatch_unit<F, Fut>(call: F) -> PluginCallResult
where
    F: FnOnce(&'static DropboxSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<()>>,
{
    let Some((p, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let p_static: &'static DropboxSyncAdapter =
        unsafe { std::mem::transmute::<&DropboxSyncAdapter, &'static DropboxSyncAdapter>(p) };
    match rt.block_on(call(p_static)) {
        Ok(()) => ok_empty_response(),
        Err(e) => sync_error_to_response(e),
    }
}

unsafe extern "C" fn ffi_test_connection(_: *const u8, _: usize) -> PluginCallResult {
    dispatch_unit(|p| async move { p.test_connection().await })
}
unsafe extern "C" fn ffi_fetch_meta(_: *const u8, _: usize) -> PluginCallResult {
    dispatch(|p| async move { p.fetch_meta().await })
}
unsafe extern "C" fn ffi_push_meta(a: *const u8, l: usize) -> PluginCallResult {
    let m: MetaJson = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(|p| async move { p.push_meta(&m).await })
}
unsafe extern "C" fn ffi_fetch_new_logs(a: *const u8, l: usize) -> PluginCallResult {
    let c: DeviceCursor = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch(|p| async move { p.fetch_new_logs(&c).await })
}
unsafe extern "C" fn ffi_push_log(a: *const u8, l: usize) -> PluginCallResult {
    let log: LogFile = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(|p| async move { p.push_log(&log).await })
}
unsafe extern "C" fn ffi_fetch_snapshot(_: *const u8, _: usize) -> PluginCallResult {
    dispatch(|p| async move { p.fetch_snapshot().await })
}
unsafe extern "C" fn ffi_push_snapshot(a: *const u8, l: usize) -> PluginCallResult {
    let s: Snapshot = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(|p| async move { p.push_snapshot(&s).await })
}
unsafe extern "C" fn ffi_delete_log(a: *const u8, l: usize) -> PluginCallResult {
    let n: LogFileName = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    dispatch_unit(|p| async move { p.delete_log(&n).await })
}

#[derive(Debug, Deserialize)]
struct PushSoundAssetArgs { hash: String, extension: String, bytes_base64: String }

unsafe extern "C" fn ffi_push_sound_asset(a: *const u8, l: usize) -> PluginCallResult {
    use base64::Engine as _;
    let args: PushSoundAssetArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    let bytes = match base64::engine::general_purpose::STANDARD.decode(args.bytes_base64.as_bytes()) {
        Ok(b) => b,
        Err(err) => return error_response(plugin_sdk::plugin_core::PLUGIN_CALL_ERR_INVALID, &format!("bad base64: {err}")),
    };
    dispatch_unit(move |p| {
        let h = args.hash;
        let e = args.extension;
        async move { p.push_sound_asset(&h, &e, &bytes).await }
    })
}

#[derive(Debug, Deserialize)]
struct FetchSoundAssetArgs { hash: String, extension: String }

unsafe extern "C" fn ffi_fetch_sound_asset(a: *const u8, l: usize) -> PluginCallResult {
    use base64::Engine as _;
    let args: FetchSoundAssetArgs = match decode_args(a, l) { Ok(v) => v, Err(r) => return r };
    let Some((p, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(PLUGIN_CALL_ERR_INTERNAL, "plugin not initialised");
    };
    let p_static: &'static DropboxSyncAdapter =
        unsafe { std::mem::transmute::<&DropboxSyncAdapter, &'static DropboxSyncAdapter>(p) };
    match rt.block_on(async move { p_static.fetch_sound_asset(&args.hash, &args.extension).await }) {
        Ok(None) => ok_response(&Option::<String>::None),
        Ok(Some(b)) => ok_response(&Some(base64::engine::general_purpose::STANDARD.encode(b))),
        Err(e) => sync_error_to_response(e),
    }
}

#[no_mangle]
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

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.sync-adapter-dropbox",
    name: "Aperio Dropbox",
    version: "0.1.0",
    plugin_type: "sync-adapter",
    vtable: SYNC_VTABLE,
    init: plugin_init,
    destroy: plugin_destroy,
}
