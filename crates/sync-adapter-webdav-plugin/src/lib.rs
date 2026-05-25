//! WebDAV sync adapter packaged as a plugin (DESIGN.md §20).
//!
//! ## Init config
//!
//! ```json
//! {
//!   "url": "https://cloud.example.com/remote.php/dav/files/alice/aperio/",
//!   "user": "alice",
//!   "password": "hunter2"
//! }
//! ```
//!
//! `user` + `password` are both optional — an empty `user`
//! constructs a [`WebDavCredentials::None`] adapter, which is
//! the right shape for read-only public test datasets.

use std::os::raw::{c_char, c_void};

use base64::Engine as _;
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::PluginCallResult;
use plugin_sdk::plugin_core::vtables::SyncVtable;
use plugin_sdk::{
    decode_args, error_response, ok_response, open_instance_with,
    sync_error_to_response, PluginInstance,
};
use serde::Deserialize;
use sync_adapter_webdav::{WebDavCredentials, WebDavSyncAdapter};
use sync_core::{DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter};

plugin_sdk::sync_dispatch_helpers!(WebDavSyncAdapter);

#[derive(Debug, Deserialize)]
struct InitConfig {
    url: String,
    #[serde(default)]
    user: String,
    #[serde(default)]
    password: String,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(
    config_json: *const c_char,
) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig = serde_json::from_str(json)
            .map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.url.trim().is_empty() {
            return Err("url must not be empty".to_string());
        }
        let credentials = if cfg.user.trim().is_empty() {
            WebDavCredentials::None
        } else {
            WebDavCredentials::basic(cfg.user.trim(), &cfg.password)
        };
        WebDavSyncAdapter::new(cfg.url.trim(), credentials)
            .map_err(|e| format!("adapter ctor failed: {e:?}"))
    })
}

/// # Safety
/// FFI export.
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<WebDavSyncAdapter>::drop_handle(handle);
}

unsafe extern "C" fn ffi_test_connection(h: *mut c_void, _a: *const u8, _l: usize) -> PluginCallResult {
    dispatch_unit(h, |p| async move { p.test_connection().await })
}

unsafe extern "C" fn ffi_fetch_meta(h: *mut c_void, _a: *const u8, _l: usize) -> PluginCallResult {
    dispatch(h, |p| async move { p.fetch_meta().await })
}

unsafe extern "C" fn ffi_push_meta(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let meta: MetaJson = match decode_args(a, l) { Ok(m) => m, Err(r) => return r };
    dispatch_unit(h, |p| async move { p.push_meta(&meta).await })
}

unsafe extern "C" fn ffi_fetch_new_logs(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let cursor: DeviceCursor = match decode_args(a, l) { Ok(c) => c, Err(r) => return r };
    dispatch(h, |p| async move { p.fetch_new_logs(&cursor).await })
}

unsafe extern "C" fn ffi_push_log(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let log: LogFile = match decode_args(a, l) { Ok(l) => l, Err(r) => return r };
    dispatch_unit(h, |p| async move { p.push_log(&log).await })
}

unsafe extern "C" fn ffi_fetch_snapshot(h: *mut c_void, _a: *const u8, _l: usize) -> PluginCallResult {
    dispatch(h, |p| async move { p.fetch_snapshot().await })
}

unsafe extern "C" fn ffi_push_snapshot(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let snap: Snapshot = match decode_args(a, l) { Ok(s) => s, Err(r) => return r };
    dispatch_unit(h, |p| async move { p.push_snapshot(&snap).await })
}

unsafe extern "C" fn ffi_delete_log(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let name: LogFileName = match decode_args(a, l) { Ok(n) => n, Err(r) => return r };
    dispatch_unit(h, |p| async move { p.delete_log(&name).await })
}

#[derive(Debug, Deserialize)]
struct PushSoundAssetArgs {
    hash: String,
    extension: String,
    bytes_base64: String,
}

unsafe extern "C" fn ffi_push_sound_asset(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: PushSoundAssetArgs = match decode_args(a, l) { Ok(a) => a, Err(r) => return r };
    let bytes = match base64::engine::general_purpose::STANDARD.decode(args.bytes_base64.as_bytes()) {
        Ok(b) => b,
        Err(err) => return error_response(
            plugin_sdk::plugin_core::ffi::PLUGIN_CALL_ERR_INVALID,
            &format!("bad base64: {err}"),
        ),
    };
    dispatch_unit(h, move |p| {
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

unsafe extern "C" fn ffi_fetch_sound_asset(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: FetchSoundAssetArgs = match decode_args(a, l) { Ok(a) => a, Err(r) => return r };
    let inst = match instance(h) { Ok(i) => i, Err(r) => return r };
    let p_static: &'static WebDavSyncAdapter = unsafe {
        std::mem::transmute::<&WebDavSyncAdapter, &'static WebDavSyncAdapter>(inst.plugin())
    };
    let outcome = inst.runtime().block_on(async move {
        p_static.fetch_sound_asset(&args.hash, &args.extension).await
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

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.sync-adapter-webdav",
    name: "Aperio WebDAV",
    version: "0.1.0",
    plugin_type: "sync-adapter",
    vtable: SYNC_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}
