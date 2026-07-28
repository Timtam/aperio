//! Smoke test for the SFTP sync plugin (ABI v2).

use std::sync::Arc;

use plugin_core::{
    abi::AperioPlugin, manager::PluginManager, manifest::PluginManifest, shim::FfiSyncAdapter,
    PluginType, ABI_VERSION,
};

fn manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.sync-adapter-sftp".into(),
        name: "Aperio SFTP".into(),
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
    }
}

fn make_manager() -> PluginManager {
    let m = PluginManager::new("0.1.0");
    let d: *mut AperioPlugin = unsafe { sync_adapter_sftp_plugin::build_descriptor() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) = sync_adapter_sftp_plugin::DESTROY_FN;
    m.register_static(manifest(), d, dx).expect("register");
    m
}

fn open_one(manager: &PluginManager, host: &str) -> Arc<plugin_core::LoadedInstance> {
    let loaded = manager
        .get("com.aperio.sync-adapter-sftp")
        .expect("registered");
    let cfg = serde_json::json!({
        "host": host,
        "port": 22,
        "user": "alice",
        "path": "/home/alice/aperio",
        "auth_method": "password",
        "password": "swordfish",
        "pinned_fingerprint": "SHA256:abcd1234567890",
    });
    manager
        .open_instance(loaded, &cfg.to_string())
        .expect("open")
}

#[test]
fn plugin_loads_and_wraps_through_ffi_sync_adapter() {
    let manager = make_manager();
    let inst = open_one(&manager, "ssh.example.invalid");
    let _adapter: Arc<FfiSyncAdapter> =
        Arc::new(FfiSyncAdapter::new(inst).expect("vtable surface"));
}

#[test]
fn multiple_sftp_servers_get_distinct_handles() {
    let manager = make_manager();
    let a = open_one(&manager, "ssh-a.example.invalid");
    let b = open_one(&manager, "ssh-b.example.invalid");
    assert_ne!(a.handle() as usize, b.handle() as usize);
}
