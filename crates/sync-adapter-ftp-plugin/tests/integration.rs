//! Smoke test for the FTPS sync plugin (ABI v2).

use std::sync::Arc;

use plugin_core::{
    abi::AperioPlugin, manager::PluginManager, manifest::PluginManifest, shim::FfiSyncAdapter,
    PluginType, ABI_VERSION,
};

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
        recurrence: Default::default(),
        tasks: Default::default(),
        account: None,
        adapter_kind: None,
        strings: Default::default(),
    }
}

fn make_manager() -> PluginManager {
    let m = PluginManager::new("0.1.0");
    let d: *mut AperioPlugin = unsafe { sync_adapter_ftp_plugin::build_descriptor() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) = sync_adapter_ftp_plugin::DESTROY_FN;
    m.register_static(manifest(), d, dx).expect("register");
    m
}

fn open_one(manager: &PluginManager, host: &str) -> Arc<plugin_core::LoadedInstance> {
    let loaded = manager
        .get("com.aperio.sync-adapter-ftp")
        .expect("registered");
    let cfg = serde_json::json!({
        "host": host,
        "port": 21,
        "user": "tester",
        "password": "swordfish",
        "path": "/aperio",
        "mode": "explicit",
    });
    manager
        .open_instance(loaded, &cfg.to_string())
        .expect("open")
}

#[test]
fn plugin_loads_and_wraps_through_ffi_sync_adapter() {
    let manager = make_manager();
    let inst = open_one(&manager, "ftp.example.invalid");
    let _adapter: Arc<FfiSyncAdapter> =
        Arc::new(FfiSyncAdapter::new(inst).expect("vtable surface"));
}

#[test]
fn multiple_ftp_servers_get_distinct_handles() {
    let manager = make_manager();
    let a = open_one(&manager, "ftp1.example.invalid");
    let b = open_one(&manager, "ftp2.example.invalid");
    assert_ne!(a.handle() as usize, b.handle() as usize);
}
