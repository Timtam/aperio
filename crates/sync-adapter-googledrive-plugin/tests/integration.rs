//! Smoke test for the Google Drive sync plugin (ABI v2).

use std::sync::Arc;

use plugin_core::{
    abi::AperioPlugin, manager::PluginManager, manifest::PluginManifest,
    shim::FfiSyncAdapter, PluginType, ABI_VERSION,
};

fn manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.sync-adapter-googledrive".into(),
        name: "Aperio Google Drive".into(),
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

fn make_manager() -> PluginManager {
    let m = PluginManager::new("0.1.0");
    let d: *mut AperioPlugin =
        unsafe { sync_adapter_googledrive_plugin::aperio_plugin_create() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) =
        sync_adapter_googledrive_plugin::aperio_plugin_destroy;
    m.register_static(manifest(), d, dx).expect("register");
    m
}

fn open_one(manager: &PluginManager, refresh_token: &str) -> Arc<plugin_core::LoadedInstance> {
    let loaded = manager.get("com.aperio.sync-adapter-googledrive").expect("registered");
    let cfg = serde_json::json!({
        "client_id": "test-client-id",
        "client_secret": "test-client-secret",
        "folder_name": "Aperio",
        "refresh_token": refresh_token,
    });
    manager.open_instance(loaded, &cfg.to_string()).expect("open")
}

#[test]
fn plugin_loads_and_wraps_through_ffi_sync_adapter() {
    let manager = make_manager();
    let inst = open_one(&manager, "test-refresh-token");
    let _adapter: Arc<FfiSyncAdapter> = Arc::new(
        FfiSyncAdapter::new(inst).expect("vtable surface"),
    );
}

#[test]
fn multiple_drive_accounts_get_distinct_handles() {
    let manager = make_manager();
    let a = open_one(&manager, "refresh-account-a");
    let b = open_one(&manager, "refresh-account-b");
    assert_ne!(a.handle() as usize, b.handle() as usize);
}
