//! WebDAV sync adapter packaged as a plugin (DESIGN.md §20).
//!
//! Wraps [`sync_adapter_webdav::WebDavSyncAdapter`] with the
//! C-ABI surface plugin-core's manager expects. Follows the
//! sync-adapter-local-plugin pattern from the P3 PoC almost
//! verbatim — the only differences are the `InitConfig` shape
//! and the constructor call inside `plugin_init`.
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

use std::os::raw::{c_char, c_int};

use plugin_sdk::plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};
use plugin_sdk::plugin_core::vtables::SyncVtable;
use plugin_sdk::{
    decode_args, error_response, ok_empty_response, ok_response,
    sync_error_to_response, PluginSingleton,
};
use serde::Deserialize;
use sync_adapter_webdav::{WebDavCredentials, WebDavSyncAdapter};
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter,
};
use tracing::warn;

pub static PLUGIN_INSTANCE: PluginSingleton<WebDavSyncAdapter> =
    PluginSingleton::new();

#[derive(Debug, Deserialize)]
struct InitConfig {
    url: String,
    #[serde(default)]
    user: String,
    #[serde(default)]
    password: String,
}

/// # Safety
///
/// FFI export. `config_json` must be a NUL-terminated UTF-8 C
/// string per the ABI contract; NULL / empty / bad-utf8 are
/// rejected.
pub unsafe extern "C" fn plugin_init(config_json: *const c_char) -> c_int {
    if config_json.is_null() {
        return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
    }
    let raw = std::ffi::CStr::from_ptr(config_json);
    let json_str = match raw.to_str() {
        Ok(s) => s,
        Err(_) => return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG,
    };
    if json_str.is_empty() {
        return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
    }
    let cfg: InitConfig = match serde_json::from_str(json_str) {
        Ok(c) => c,
        Err(err) => {
            warn!(?err, "sync-adapter-webdav-plugin: malformed init config");
            return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
        }
    };
    if cfg.url.trim().is_empty() {
        return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
    }
    let credentials = if cfg.user.trim().is_empty() {
        WebDavCredentials::None
    } else {
        WebDavCredentials::basic(cfg.user.trim(), &cfg.password)
    };
    let adapter = match WebDavSyncAdapter::new(cfg.url.trim(), credentials) {
        Ok(a) => a,
        Err(err) => {
            warn!(?err, "sync-adapter-webdav-plugin: adapter ctor failed");
            return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
        }
    };
    match PLUGIN_INSTANCE.init(adapter) {
        Ok(()) => plugin_sdk::plugin_core::PLUGIN_OK,
        Err(err) => {
            warn!(?err, "sync-adapter-webdav-plugin: singleton init failed");
            plugin_sdk::plugin_core::PLUGIN_ERR_INIT
        }
    }
}

/// # Safety
///
/// FFI export. Host calls this once after the last vtable call.
pub unsafe extern "C" fn plugin_destroy() {
    // intentionally empty
}

// ─────────────────────────────────────────────────────────────
// Shared dispatch helpers — identical to sync-adapter-local-plugin.
// If this boilerplate grows tedious across the 5 sync-adapter
// plugins, a future plugin-sdk macro could collapse it into a
// single `sync_adapter_plugin!` invocation. For now we keep it
// hand-rolled so the FFI shape stays grep-friendly.
// ─────────────────────────────────────────────────────────────

fn dispatch<T, F, Fut>(call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static WebDavSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<T>>,
{
    let Some((plugin, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(
            PLUGIN_CALL_ERR_INTERNAL,
            "plugin not initialised",
        );
    };
    let plugin_static: &'static WebDavSyncAdapter = unsafe {
        std::mem::transmute::<&WebDavSyncAdapter, &'static WebDavSyncAdapter>(plugin)
    };
    let fut = call(plugin_static);
    match rt.block_on(fut) {
        Ok(value) => ok_response(&value),
        Err(err) => sync_error_to_response(err),
    }
}

fn dispatch_unit<F, Fut>(call: F) -> PluginCallResult
where
    F: FnOnce(&'static WebDavSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<()>>,
{
    let Some((plugin, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(
            PLUGIN_CALL_ERR_INTERNAL,
            "plugin not initialised",
        );
    };
    let plugin_static: &'static WebDavSyncAdapter = unsafe {
        std::mem::transmute::<&WebDavSyncAdapter, &'static WebDavSyncAdapter>(plugin)
    };
    let fut = call(plugin_static);
    match rt.block_on(fut) {
        Ok(()) => ok_empty_response(),
        Err(err) => sync_error_to_response(err),
    }
}

// ─────────────────────────────────────────────────────────────
// Per-method FFI fns
// ─────────────────────────────────────────────────────────────

unsafe extern "C" fn ffi_test_connection(
    _args_ptr: *const u8,
    _args_len: usize,
) -> PluginCallResult {
    dispatch_unit(|p| async move { p.test_connection().await })
}

unsafe extern "C" fn ffi_fetch_meta(
    _args_ptr: *const u8,
    _args_len: usize,
) -> PluginCallResult {
    dispatch(|p| async move { p.fetch_meta().await })
}

unsafe extern "C" fn ffi_push_meta(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let meta: MetaJson = match decode_args(args_ptr, args_len) {
        Ok(m) => m,
        Err(r) => return r,
    };
    dispatch_unit(|p| async move { p.push_meta(&meta).await })
}

unsafe extern "C" fn ffi_fetch_new_logs(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let cursor: DeviceCursor = match decode_args(args_ptr, args_len) {
        Ok(c) => c,
        Err(r) => return r,
    };
    dispatch(|p| async move { p.fetch_new_logs(&cursor).await })
}

unsafe extern "C" fn ffi_push_log(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let log: LogFile = match decode_args(args_ptr, args_len) {
        Ok(l) => l,
        Err(r) => return r,
    };
    dispatch_unit(|p| async move { p.push_log(&log).await })
}

unsafe extern "C" fn ffi_fetch_snapshot(
    _args_ptr: *const u8,
    _args_len: usize,
) -> PluginCallResult {
    dispatch(|p| async move { p.fetch_snapshot().await })
}

unsafe extern "C" fn ffi_push_snapshot(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let snap: Snapshot = match decode_args(args_ptr, args_len) {
        Ok(s) => s,
        Err(r) => return r,
    };
    dispatch_unit(|p| async move { p.push_snapshot(&snap).await })
}

unsafe extern "C" fn ffi_delete_log(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let name: LogFileName = match decode_args(args_ptr, args_len) {
        Ok(n) => n,
        Err(r) => return r,
    };
    dispatch_unit(|p| async move { p.delete_log(&name).await })
}

#[derive(Debug, Deserialize)]
struct PushSoundAssetArgs {
    hash: String,
    extension: String,
    bytes_base64: String,
}

unsafe extern "C" fn ffi_push_sound_asset(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    use base64::Engine as _;
    let args: PushSoundAssetArgs = match decode_args(args_ptr, args_len) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let bytes = match base64::engine::general_purpose::STANDARD
        .decode(args.bytes_base64.as_bytes())
    {
        Ok(b) => b,
        Err(err) => {
            return error_response(
                plugin_sdk::plugin_core::PLUGIN_CALL_ERR_INVALID,
                &format!("bad base64: {err}"),
            );
        }
    };
    dispatch_unit(move |p| {
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
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    use base64::Engine as _;
    let args: FetchSoundAssetArgs = match decode_args(args_ptr, args_len) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let Some((plugin, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(
            PLUGIN_CALL_ERR_INTERNAL,
            "plugin not initialised",
        );
    };
    let p_static: &'static WebDavSyncAdapter = unsafe {
        std::mem::transmute::<&WebDavSyncAdapter, &'static WebDavSyncAdapter>(plugin)
    };
    let outcome = rt.block_on(async move {
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
    id: "com.aperio.sync-adapter-webdav",
    name: "Aperio WebDAV",
    version: "0.1.0",
    plugin_type: "sync-adapter",
    vtable: SYNC_VTABLE,
    init: plugin_init,
    destroy: plugin_destroy,
}
