//! Local-filesystem sync adapter packaged as a plugin
//! (DESIGN.md §20).
//!
//! Wraps [`sync_adapter_local::LocalFsSyncAdapter`] with the
//! C-ABI surface plugin-core's manager expects. Canonical PoC
//! for the entire pipeline: manifest → cdylib → vtable → FFI
//! fn per trait method → host-side `FfiSyncAdapter` shim.
//!
//! ## Init config
//!
//! ```json
//! { "remote_root": "/mnt/nas/aperio" }
//! ```
//!
//! ABI v2: the host calls `open_instance` once per account it
//! wants to wire up (so you can sync to multiple NAS roots from
//! a single Aperio session — see DESIGN.md §19.6). Every vtable
//! method routes through the per-instance handle.

use std::os::raw::{c_char, c_void};
use std::path::PathBuf;

use base64::Engine as _;
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};
use plugin_sdk::plugin_core::vtables::SyncVtable;
use plugin_sdk::{
    decode_args, error_response, ok_empty_response, ok_response,
    open_instance_with, sync_error_to_response, PluginInstance,
};
use serde::Deserialize;
use sync_adapter_local::LocalFsSyncAdapter;
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter,
};

#[derive(Debug, Deserialize)]
struct InitConfig {
    remote_root: String,
}

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(
    config_json: *const c_char,
) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig = serde_json::from_str(json)
            .map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.remote_root.trim().is_empty() {
            return Err("remote_root must not be empty".to_string());
        }
        Ok(LocalFsSyncAdapter::new(PathBuf::from(cfg.remote_root)))
    })
}

/// # Safety
/// FFI export; `handle` must be the pointer returned by
/// [`plugin_open_instance`].
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<LocalFsSyncAdapter>::drop_handle(handle);
}

fn instance<'a>(
    handle: *mut c_void,
) -> Result<&'a PluginInstance<LocalFsSyncAdapter>, PluginCallResult> {
    unsafe { PluginInstance::<LocalFsSyncAdapter>::from_handle(handle) }
        .ok_or_else(|| error_response(PLUGIN_CALL_ERR_INTERNAL, "null instance handle"))
}

fn dispatch<T, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static LocalFsSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<T>>,
{
    let inst = match instance(handle) { Ok(i) => i, Err(r) => return r };
    let plugin_static: &'static LocalFsSyncAdapter = unsafe {
        std::mem::transmute::<&LocalFsSyncAdapter, &'static LocalFsSyncAdapter>(inst.plugin())
    };
    match inst.runtime().block_on(call(plugin_static)) {
        Ok(value) => ok_response(&value),
        Err(err) => sync_error_to_response(err),
    }
}

fn dispatch_unit<F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    F: FnOnce(&'static LocalFsSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<()>>,
{
    let inst = match instance(handle) { Ok(i) => i, Err(r) => return r };
    let plugin_static: &'static LocalFsSyncAdapter = unsafe {
        std::mem::transmute::<&LocalFsSyncAdapter, &'static LocalFsSyncAdapter>(inst.plugin())
    };
    match inst.runtime().block_on(call(plugin_static)) {
        Ok(()) => ok_empty_response(),
        Err(err) => sync_error_to_response(err),
    }
}

// ─────────────────────────────────────────────────────────────
// FFI fn per SyncAdapter trait method
// ─────────────────────────────────────────────────────────────

unsafe extern "C" fn ffi_test_connection(
    h: *mut c_void,
    _args_ptr: *const u8,
    _args_len: usize,
) -> PluginCallResult {
    dispatch_unit(h, |plugin| async move { plugin.test_connection().await })
}

unsafe extern "C" fn ffi_fetch_meta(
    h: *mut c_void,
    _args_ptr: *const u8,
    _args_len: usize,
) -> PluginCallResult {
    dispatch(h, |plugin| async move { plugin.fetch_meta().await })
}

unsafe extern "C" fn ffi_push_meta(
    h: *mut c_void,
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let meta: MetaJson = match decode_args(args_ptr, args_len) {
        Ok(m) => m, Err(r) => return r,
    };
    dispatch_unit(h, |plugin| async move { plugin.push_meta(&meta).await })
}

unsafe extern "C" fn ffi_fetch_new_logs(
    h: *mut c_void,
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let cursor: DeviceCursor = match decode_args(args_ptr, args_len) {
        Ok(c) => c, Err(r) => return r,
    };
    dispatch(h, |plugin| async move { plugin.fetch_new_logs(&cursor).await })
}

unsafe extern "C" fn ffi_push_log(
    h: *mut c_void,
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let log: LogFile = match decode_args(args_ptr, args_len) {
        Ok(l) => l, Err(r) => return r,
    };
    dispatch_unit(h, |plugin| async move { plugin.push_log(&log).await })
}

unsafe extern "C" fn ffi_fetch_snapshot(
    h: *mut c_void,
    _args_ptr: *const u8,
    _args_len: usize,
) -> PluginCallResult {
    dispatch(h, |plugin| async move { plugin.fetch_snapshot().await })
}

unsafe extern "C" fn ffi_push_snapshot(
    h: *mut c_void,
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let snap: Snapshot = match decode_args(args_ptr, args_len) {
        Ok(s) => s, Err(r) => return r,
    };
    dispatch_unit(h, |plugin| async move { plugin.push_snapshot(&snap).await })
}

unsafe extern "C" fn ffi_delete_log(
    h: *mut c_void,
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let name: LogFileName = match decode_args(args_ptr, args_len) {
        Ok(n) => n, Err(r) => return r,
    };
    dispatch_unit(h, |plugin| async move { plugin.delete_log(&name).await })
}

#[derive(Debug, Deserialize)]
struct PushSoundAssetArgs {
    hash: String,
    extension: String,
    bytes_base64: String,
}

unsafe extern "C" fn ffi_push_sound_asset(
    h: *mut c_void,
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let args: PushSoundAssetArgs = match decode_args(args_ptr, args_len) {
        Ok(a) => a, Err(r) => return r,
    };
    let bytes = match base64::engine::general_purpose::STANDARD
        .decode(args.bytes_base64.as_bytes())
    {
        Ok(b) => b,
        Err(err) => {
            return error_response(
                plugin_sdk::plugin_core::ffi::PLUGIN_CALL_ERR_INVALID,
                &format!("bad base64 in sound asset: {err}"),
            );
        }
    };
    dispatch_unit(h, move |plugin| {
        let hash = args.hash;
        let extension = args.extension;
        async move { plugin.push_sound_asset(&hash, &extension, &bytes).await }
    })
}

#[derive(Debug, Deserialize)]
struct FetchSoundAssetArgs {
    hash: String,
    extension: String,
}

unsafe extern "C" fn ffi_fetch_sound_asset(
    h: *mut c_void,
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let args: FetchSoundAssetArgs = match decode_args(args_ptr, args_len) {
        Ok(a) => a, Err(r) => return r,
    };
    let inst = match instance(h) { Ok(i) => i, Err(r) => return r };
    let plugin_static: &'static LocalFsSyncAdapter = unsafe {
        std::mem::transmute::<&LocalFsSyncAdapter, &'static LocalFsSyncAdapter>(inst.plugin())
    };
    let outcome = inst.runtime().block_on(async move {
        plugin_static.fetch_sound_asset(&args.hash, &args.extension).await
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
    id: "com.aperio.sync-adapter-local",
    name: "Aperio Local Filesystem",
    version: "0.1.0",
    plugin_type: "sync-adapter",
    vtable: SYNC_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}
