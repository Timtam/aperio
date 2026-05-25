//! Smoke test for the FTPS sync plugin (P4). Same shape as
//! sync-adapter-webdav-plugin's smoke test — just confirms the
//! FTPS-specific InitConfig + constructor + manifest line up.

use std::ffi::CString;
use std::sync::{Arc, Mutex};

use plugin_core::{
    abi::AperioPlugin, manager::PluginManager, manifest::PluginManifest,
    shim::FfiSyncAdapter, PluginType, ABI_VERSION,
};

fn shared_setup() {
    static DONE: Mutex<bool> = Mutex::new(false);
    let mut done = DONE.lock().unwrap();
    if *done {
        return;
    }
    let cfg = serde_json::json!({
        "host": "ftp.example.invalid",
        "port": 21,
        "user": "tester",
        "password": "swordfish",
        "path": "/aperio",
        "mode": "explicit"
    });
    let c = CString::new(cfg.to_string()).unwrap();
    let rc = unsafe { sync_adapter_ftp_plugin::plugin_init(c.as_ptr()) };
    assert_eq!(rc, plugin_core::PLUGIN_OK);
    *done = true;
}

fn manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.sync-adapter-ftp".into(),
        name: "Aperio FTPS".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::SyncAdapter,
        capabilities: vec![],
        abi_version: ABI_VERSION,
        min_app_version: "0.1.0".into(),
        author: Some("Aperio Contributors".into()),
        description: Some("Bundled".into()),
        signed: false,
    }
}

fn make_adapter() -> (PluginManager, Arc<FfiSyncAdapter>) {
    let m = PluginManager::new("0.1.0");
    let d: *mut AperioPlugin =
        unsafe { sync_adapter_ftp_plugin::aperio_plugin_create() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) =
        sync_adapter_ftp_plugin::aperio_plugin_destroy;
    m.register_static(manifest(), d, dx).unwrap();
    let l = m.get("com.aperio.sync-adapter-ftp").unwrap();
    (m, Arc::new(FfiSyncAdapter::new(l).unwrap()))
}

#[test]
fn plugin_loads_and_wraps_through_ffi_sync_adapter() {
    shared_setup();
    let (_m, _a) = make_adapter();
}
