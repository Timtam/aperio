//! Local-filesystem sync adapter packaged as a plugin
//! (DESIGN.md §20 PoC).
//!
//! Wraps [`sync_adapter_local::LocalFsSyncAdapter`] with the
//! C-ABI surface plugin-core's manager expects. Demonstrates the
//! complete plugin pipeline: manifest → cdylib → vtable → FFI
//! fn per trait method → host-side `FfiSyncAdapter` shim.
//!
//! This crate is the canonical PoC for the plugin system. If
//! every test in here passes, the entire pipeline from
//! plugin-core's ABI through plugin-sdk's helpers through the
//! adapter crate is wired up correctly.
//!
//! ## Init config
//!
//! The host calls the plugin's `init()` lifecycle hook with a
//! JSON document of the form:
//!
//! ```json
//! { "remote_root": "/mnt/nas/aperio" }
//! ```
//!
//! The adapter constructor takes nothing else, so that's the
//! whole config surface. `init()` parses this, builds the
//! `LocalFsSyncAdapter`, and stores it in
//! [`PLUGIN_INSTANCE`]. Subsequent vtable calls read through
//! the singleton.
//!
//! ## Per-method FFI shape
//!
//! Every vtable fn follows the same 5-step pattern:
//!
//!   1. Borrow the plugin + runtime from [`PLUGIN_INSTANCE`].
//!      A missing init returns `PLUGIN_CALL_ERR_INTERNAL`.
//!   2. Decode JSON args via [`plugin_sdk::decode_args`]. Void-
//!      arg methods skip this.
//!   3. `block_on` the trait method.
//!   4. Map the [`SyncResult`] into a [`PluginCallResult`] via
//!      [`plugin_sdk::ok_response`] /
//!      [`plugin_sdk::sync_error_to_response`].
//!
//! The boilerplate per method is six lines, repeated ten times
//! for the ten methods on [`SyncAdapter`]. The Cookie-cutter
//! pattern is exactly what a future `aperio_plugin_export!`
//! proc-macro would auto-generate; for now we hand-roll it,
//! deliberately, so the pattern's friction informs the macro
//! design later.

use std::os::raw::{c_char, c_int};
use std::path::PathBuf;

use base64::Engine as _;
use plugin_sdk::plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};
use plugin_sdk::plugin_core::vtables::SyncVtable;
use plugin_sdk::{
    decode_args, error_response, ok_empty_response, ok_response,
    sync_error_to_response, PluginSingleton,
};
use serde::Deserialize;
use sync_adapter_local::LocalFsSyncAdapter;
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter,
};
use tracing::warn;

// ─────────────────────────────────────────────────────────────
// Singleton + init/destroy
// ─────────────────────────────────────────────────────────────

/// Holds the singleton adapter instance + the plugin's
/// dedicated tokio runtime.
pub static PLUGIN_INSTANCE: PluginSingleton<LocalFsSyncAdapter> =
    PluginSingleton::new();

/// Init-time config the host passes via `config_json`. Keys
/// match the plugin's manifest documentation; missing /
/// malformed values surface as `PLUGIN_ERR_INVALID_CONFIG`
/// from the lifecycle hook.
#[derive(Debug, Deserialize)]
struct InitConfig {
    remote_root: String,
}

/// Lifecycle `init` callback. Builds the
/// [`LocalFsSyncAdapter`] from the JSON config + installs it
/// in the singleton.
///
/// # Safety
///
/// `config_json` is a NUL-terminated UTF-8 C string per the
/// ABI contract. May be NULL or empty (`""`) — both are
/// rejected with `PLUGIN_ERR_INVALID_CONFIG`.
#[allow(clippy::missing_safety_doc)] // safety covered in module docs
pub unsafe extern "C" fn plugin_init(config_json: *const c_char) -> c_int {
    let raw = if config_json.is_null() {
        return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
    } else {
        std::ffi::CStr::from_ptr(config_json)
    };
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
            warn!(?err, "sync-adapter-local-plugin: malformed init config");
            return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
        }
    };
    if cfg.remote_root.trim().is_empty() {
        return plugin_sdk::plugin_core::PLUGIN_ERR_INVALID_CONFIG;
    }
    let adapter = LocalFsSyncAdapter::new(PathBuf::from(cfg.remote_root));
    match PLUGIN_INSTANCE.init(adapter) {
        Ok(()) => plugin_sdk::plugin_core::PLUGIN_OK,
        Err(err) => {
            warn!(?err, "sync-adapter-local-plugin: singleton init failed");
            plugin_sdk::plugin_core::PLUGIN_ERR_INIT
        }
    }
}

/// Lifecycle `destroy` callback. Tokio runtime + adapter live
/// inside the `OnceLock` and the OS reclaims them when the
/// library unloads — there's nothing additional to do here.
/// We expose the symbol so `declare_lifecycle!` can wire it up
/// + so a future async-tokio-shutdown story has a place to
///   hook in.
///
/// # Safety
///
/// FFI export. Host calls this exactly once after the last
/// vtable method returns. No precondition on host state — the
/// fn body is empty.
pub unsafe extern "C" fn plugin_destroy() {
    // intentionally empty for now
}

// ─────────────────────────────────────────────────────────────
// Helpers shared by every FFI fn
// ─────────────────────────────────────────────────────────────

/// Run an async closure on the plugin's runtime, mapping the
/// `SyncResult<T>` into a [`PluginCallResult`]. Encapsulates
/// the "fetch singleton / handle no-init / spawn / encode"
/// boilerplate so each FFI fn body stays a single match.
fn dispatch<T, F, Fut>(call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static LocalFsSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<T>>,
{
    let Some((plugin, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(
            PLUGIN_CALL_ERR_INTERNAL,
            "plugin not initialised",
        );
    };
    // SAFETY: `plugin` is borrowed from a `OnceLock` that lives
    // for the lifetime of the loaded library. The future we
    // construct here borrows it; we drive it to completion
    // inside this stack frame via `block_on`, so the lifetime
    // never escapes.
    let plugin_static: &'static LocalFsSyncAdapter = unsafe {
        std::mem::transmute::<&LocalFsSyncAdapter, &'static LocalFsSyncAdapter>(plugin)
    };
    let fut = call(plugin_static);
    match rt.block_on(fut) {
        Ok(value) => ok_response(&value),
        Err(err) => sync_error_to_response(err),
    }
}

/// Same as [`dispatch`] but for trait methods that return
/// `SyncResult<()>` — empty payload on success.
fn dispatch_unit<F, Fut>(call: F) -> PluginCallResult
where
    F: FnOnce(&'static LocalFsSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<()>>,
{
    let Some((plugin, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(
            PLUGIN_CALL_ERR_INTERNAL,
            "plugin not initialised",
        );
    };
    let plugin_static: &'static LocalFsSyncAdapter = unsafe {
        std::mem::transmute::<&LocalFsSyncAdapter, &'static LocalFsSyncAdapter>(plugin)
    };
    let fut = call(plugin_static);
    match rt.block_on(fut) {
        Ok(()) => ok_empty_response(),
        Err(err) => sync_error_to_response(err),
    }
}

// ─────────────────────────────────────────────────────────────
// FFI fn per SyncAdapter trait method
// ─────────────────────────────────────────────────────────────
//
// Each fn is `unsafe extern "C"`, takes the JSON args pointer
// + length, and returns a `PluginCallResult`. The vtable
// (declared at the bottom of this file) plugs these into the
// matching slot on [`SyncVtable`].

unsafe extern "C" fn ffi_test_connection(
    _args_ptr: *const u8,
    _args_len: usize,
) -> PluginCallResult {
    dispatch_unit(|plugin| async move { plugin.test_connection().await })
}

unsafe extern "C" fn ffi_fetch_meta(
    _args_ptr: *const u8,
    _args_len: usize,
) -> PluginCallResult {
    dispatch(|plugin| async move { plugin.fetch_meta().await })
}

unsafe extern "C" fn ffi_push_meta(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let meta: MetaJson = match decode_args(args_ptr, args_len) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    dispatch_unit(|plugin| async move { plugin.push_meta(&meta).await })
}

unsafe extern "C" fn ffi_fetch_new_logs(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let cursor: DeviceCursor = match decode_args(args_ptr, args_len) {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    dispatch(|plugin| async move { plugin.fetch_new_logs(&cursor).await })
}

unsafe extern "C" fn ffi_push_log(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let log: LogFile = match decode_args(args_ptr, args_len) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    dispatch_unit(|plugin| async move { plugin.push_log(&log).await })
}

unsafe extern "C" fn ffi_fetch_snapshot(
    _args_ptr: *const u8,
    _args_len: usize,
) -> PluginCallResult {
    dispatch(|plugin| async move { plugin.fetch_snapshot().await })
}

unsafe extern "C" fn ffi_push_snapshot(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let snap: Snapshot = match decode_args(args_ptr, args_len) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    dispatch_unit(|plugin| async move { plugin.push_snapshot(&snap).await })
}

unsafe extern "C" fn ffi_delete_log(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let name: LogFileName = match decode_args(args_ptr, args_len) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    dispatch_unit(|plugin| async move { plugin.delete_log(&name).await })
}

/// Wire shape of [`SyncAdapter::push_sound_asset`] args.
/// `bytes_base64` carries the file contents — same convention
/// the host's `FfiSyncAdapter` shim uses on its end.
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
    let args: PushSoundAssetArgs = match decode_args(args_ptr, args_len) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let bytes = match base64::engine::general_purpose::STANDARD
        .decode(args.bytes_base64.as_bytes())
    {
        Ok(b) => b,
        Err(err) => {
            return error_response(
                plugin_sdk::plugin_core::PLUGIN_CALL_ERR_INVALID,
                &format!("bad base64 in sound asset: {err}"),
            );
        }
    };
    dispatch_unit(move |plugin| {
        let hash = args.hash;
        let extension = args.extension;
        async move {
            plugin
                .push_sound_asset(&hash, &extension, &bytes)
                .await
        }
    })
}

/// Wire shape of [`SyncAdapter::fetch_sound_asset`] args.
#[derive(Debug, Deserialize)]
struct FetchSoundAssetArgs {
    hash: String,
    extension: String,
}

unsafe extern "C" fn ffi_fetch_sound_asset(
    args_ptr: *const u8,
    args_len: usize,
) -> PluginCallResult {
    let args: FetchSoundAssetArgs = match decode_args(args_ptr, args_len) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    // The host's shim expects `Option<String>` (base64) — convert
    // the adapter's `Option<Vec<u8>>` before encoding the OK
    // response.
    let Some((plugin, rt)) = PLUGIN_INSTANCE.parts() else {
        return error_response(
            PLUGIN_CALL_ERR_INTERNAL,
            "plugin not initialised",
        );
    };
    let plugin_static: &'static LocalFsSyncAdapter = unsafe {
        std::mem::transmute::<&LocalFsSyncAdapter, &'static LocalFsSyncAdapter>(plugin)
    };
    let outcome = rt.block_on(async move {
        plugin_static
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

// ─────────────────────────────────────────────────────────────
// Vtable + lifecycle declarations
// ─────────────────────────────────────────────────────────────

/// The actual [`SyncVtable`] handed to the host. Built from the
/// per-method FFI fns above + the empty default so any future
/// method appended to the trait surface defaults to "not
/// implemented" until we wire it in here.
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

// SAFETY of the casts in `dispatch` + `dispatch_unit`: the
// plugin instance lives in a process-wide OnceLock for the
// lifetime of the loaded library. The `'static` we synthesise
// stays inside the function, never escapes via the returned
// PluginCallResult (which carries serialised bytes, not
// borrowed references). The lifetime-shortening is required
// only because `async move` futures can't borrow non-`'static`
// references when used through the trait machinery; the actual
// runtime lifetime is correct.

plugin_sdk::declare_lifecycle! {
    id: "com.aperio.sync-adapter-local",
    name: "Aperio Local Filesystem",
    version: "0.1.0",
    plugin_type: "sync-adapter",
    vtable: SYNC_VTABLE,
    init: plugin_init,
    destroy: plugin_destroy,
}
