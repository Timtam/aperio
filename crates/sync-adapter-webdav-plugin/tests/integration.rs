//! Smoke test for the WebDAV sync plugin (ABI v2).

use std::sync::Arc;

use plugin_core::{
    abi::AperioPlugin, manager::PluginManager, manifest::PluginManifest, shim::FfiSyncAdapter,
    Capability, PluginType, ABI_VERSION,
};
use sync_core::SyncAdapter;

fn manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.sync-adapter-webdav".into(),
        name: "Aperio WebDAV".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::Adapter,
        capabilities: vec![Capability::Sync],
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
    let manager = PluginManager::new("0.1.0");
    let desc: *mut AperioPlugin = unsafe { sync_adapter_webdav_plugin::build_descriptor() };
    assert!(!desc.is_null());
    let destroy: unsafe extern "C" fn(*mut AperioPlugin) = sync_adapter_webdav_plugin::DESTROY_FN;
    manager
        .register_static(manifest(), desc, destroy)
        .expect("register");
    manager
}

fn open_one(manager: &PluginManager, url: &str) -> Arc<plugin_core::LoadedInstance> {
    let loaded = manager
        .get("com.aperio.sync-adapter-webdav")
        .expect("registered");
    let cfg = serde_json::json!({
        "url": url,
        "user": "tester",
        "password": "swordfish",
    });
    manager
        .open_instance(loaded, &cfg.to_string())
        .expect("open")
}

fn make_adapter() -> (PluginManager, Arc<FfiSyncAdapter>) {
    let manager = make_manager();
    let inst = open_one(&manager, "https://example.invalid/aperio/");
    let adapter = FfiSyncAdapter::new(inst).expect("vtable surface");
    (manager, Arc::new(adapter))
}

#[test]
fn plugin_loads_and_wraps_through_ffi_sync_adapter() {
    let (_m, _a) = make_adapter();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_connection_against_bogus_url_surfaces_network_error() {
    let (_m, adapter) = make_adapter();
    let err = adapter
        .test_connection()
        .await
        .expect_err("bogus URL must fail");
    match err {
        sync_core::SyncError::Network(_)
        | sync_core::SyncError::NotFound(_)
        | sync_core::SyncError::Io(_) => {}
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn rejects_empty_url_at_open() {
    let manager = make_manager();
    let loaded = manager
        .get("com.aperio.sync-adapter-webdav")
        .expect("registered");
    let bad = serde_json::json!({ "url": "", "user": "", "password": "" });
    let err = manager.open_instance(loaded, &bad.to_string()).unwrap_err();
    assert!(matches!(
        err,
        plugin_core::error::PluginError::InstanceOpen { .. }
    ));
}

#[test]
fn multiple_webdav_servers_get_distinct_handles() {
    let manager = make_manager();
    let nextcloud = open_one(&manager, "https://cloud.example.invalid/dav/");
    let owncloud = open_one(&manager, "https://owncloud.example.invalid/dav/");
    assert_ne!(nextcloud.handle() as usize, owncloud.handle() as usize);
}
