//! SFTP sync adapter packaged as a plugin (DESIGN.md §20).
//!
//! ## Init config
//!
//! ```json
//! {
//!   "host": "ssh.example.com",
//!   "port": 22,
//!   "user": "alice",
//!   "path": "/home/alice/aperio",
//!   "auth_method": "password",
//!   "password": "…",
//!   "key_path": "/home/alice/.ssh/id_ed25519",
//!   "key_passphrase": "",
//!   "pinned_fingerprint": "SHA256:…"
//! }
//! ```

use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use plugin_sdk::plugin_core::abi::OpenInstanceResult;
use plugin_sdk::plugin_core::ffi::{PluginCallResult, PLUGIN_CALL_ERR_INTERNAL};
use plugin_sdk::plugin_core::vtables::SyncVtable;
use plugin_sdk::{
    decode_args, error_response, ok_empty_response, ok_response,
    open_instance_with, sync_error_to_response, PluginInstance,
};
use serde::Deserialize;
use sync_adapter_sftp::{
    HostKeyVerifier, InMemoryHostKeyVerifier, SftpAuth, SftpSyncAdapter,
};
use sync_core::{DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter};

#[derive(Debug, Deserialize)]
struct InitConfig {
    host: String,
    #[serde(default = "default_port")]
    port: u16,
    user: String,
    path: String,
    #[serde(default = "default_auth_method")]
    auth_method: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    key_path: String,
    #[serde(default)]
    key_passphrase: String,
    #[serde(default)]
    pinned_fingerprint: String,
}

fn default_port() -> u16 { 22 }
fn default_auth_method() -> String { "password".to_string() }

/// # Safety
/// FFI export; `config_json` must be NUL-terminated UTF-8.
pub unsafe extern "C" fn plugin_open_instance(
    config_json: *const c_char,
) -> OpenInstanceResult {
    open_instance_with(config_json, |json| {
        let cfg: InitConfig = serde_json::from_str(json)
            .map_err(|e| format!("malformed init config: {e}"))?;
        if cfg.host.trim().is_empty() || cfg.user.trim().is_empty() || cfg.path.trim().is_empty() {
            return Err("host, user and path must not be empty".to_string());
        }
        let auth = match cfg.auth_method.as_str() {
            "password" => {
                if cfg.password.is_empty() {
                    return Err("password auth requires non-empty password".to_string());
                }
                SftpAuth::Password { password: cfg.password }
            }
            "key" => {
                if cfg.key_path.trim().is_empty() {
                    return Err("key auth requires key_path".to_string());
                }
                let passphrase = if cfg.key_passphrase.is_empty() { None } else { Some(cfg.key_passphrase) };
                SftpAuth::PrivateKey {
                    path: PathBuf::from(cfg.key_path.trim()),
                    passphrase,
                }
            }
            other => return Err(format!("unknown auth_method: {other}")),
        };
        let verifier: Arc<dyn HostKeyVerifier> = if cfg.pinned_fingerprint.trim().is_empty() {
            Arc::new(InMemoryHostKeyVerifier::new())
        } else {
            let host_port = format!("{}:{}", cfg.host.trim(), cfg.port);
            Arc::new(InMemoryHostKeyVerifier::with_known(
                &host_port,
                cfg.pinned_fingerprint.trim(),
            ))
        };
        Ok(SftpSyncAdapter::new(
            cfg.host.trim(),
            cfg.port,
            cfg.user.trim(),
            auth,
            PathBuf::from(cfg.path.trim()),
            verifier,
        ))
    })
}

/// # Safety
/// FFI export.
pub unsafe extern "C" fn plugin_close_instance(handle: *mut c_void) {
    PluginInstance::<SftpSyncAdapter>::drop_handle(handle);
}

fn instance<'a>(
    handle: *mut c_void,
) -> Result<&'a PluginInstance<SftpSyncAdapter>, PluginCallResult> {
    unsafe { PluginInstance::<SftpSyncAdapter>::from_handle(handle) }
        .ok_or_else(|| error_response(PLUGIN_CALL_ERR_INTERNAL, "null instance handle"))
}

fn dispatch<T, F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    T: serde::Serialize,
    F: FnOnce(&'static SftpSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<T>>,
{
    let inst = match instance(handle) { Ok(i) => i, Err(r) => return r };
    let p: &'static SftpSyncAdapter = unsafe {
        std::mem::transmute::<&SftpSyncAdapter, &'static SftpSyncAdapter>(inst.plugin())
    };
    match inst.runtime().block_on(call(p)) {
        Ok(v) => ok_response(&v),
        Err(e) => sync_error_to_response(e),
    }
}

fn dispatch_unit<F, Fut>(handle: *mut c_void, call: F) -> PluginCallResult
where
    F: FnOnce(&'static SftpSyncAdapter) -> Fut,
    Fut: std::future::Future<Output = sync_core::SyncResult<()>>,
{
    let inst = match instance(handle) { Ok(i) => i, Err(r) => return r };
    let p: &'static SftpSyncAdapter = unsafe {
        std::mem::transmute::<&SftpSyncAdapter, &'static SftpSyncAdapter>(inst.plugin())
    };
    match inst.runtime().block_on(call(p)) {
        Ok(()) => ok_empty_response(),
        Err(e) => sync_error_to_response(e),
    }
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
struct PushSoundAssetArgs { hash: String, extension: String, bytes_base64: String }

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
struct FetchSoundAssetArgs { hash: String, extension: String }

unsafe extern "C" fn ffi_fetch_sound_asset(h: *mut c_void, a: *const u8, l: usize) -> PluginCallResult {
    let args: FetchSoundAssetArgs = match decode_args(a, l) { Ok(a) => a, Err(r) => return r };
    let inst = match instance(h) { Ok(i) => i, Err(r) => return r };
    let p: &'static SftpSyncAdapter = unsafe {
        std::mem::transmute::<&SftpSyncAdapter, &'static SftpSyncAdapter>(inst.plugin())
    };
    let outcome = inst.runtime().block_on(async move {
        p.fetch_sound_asset(&args.hash, &args.extension).await
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
    id: "com.aperio.sync-adapter-sftp",
    name: "Aperio SFTP",
    version: "0.1.0",
    plugin_type: "sync-adapter",
    vtable: SYNC_VTABLE,
    open_instance: plugin_open_instance,
    close_instance: plugin_close_instance,
}
