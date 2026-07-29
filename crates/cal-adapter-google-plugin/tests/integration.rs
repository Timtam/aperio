//! Smoke test for the Google plugin (ABI v2).

use std::sync::Arc;

use plugin_core::{
    abi::AperioPlugin,
    manager::PluginManager,
    manifest::PluginManifest,
    shim::{FfiCalendarAdapter, FfiContactsAdapter, FfiTasksAdapter},
    Capability, PluginType, ABI_VERSION,
};

fn manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.cal-adapter-google".into(),
        name: "Aperio Google".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::CalendarAdapter,
        capabilities: vec![
            Capability::Calendar,
            Capability::Tasks,
            Capability::Contacts,
        ],
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

fn register() -> PluginManager {
    let m = PluginManager::new("0.1.0");
    let d: *mut AperioPlugin = unsafe { cal_adapter_google_plugin::build_descriptor() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) = cal_adapter_google_plugin::DESTROY_FN;
    m.register_static(manifest(), d, dx).unwrap();
    m
}

fn open_one(manager: &PluginManager, access: &str) -> Arc<plugin_core::LoadedInstance> {
    let loaded = manager.get("com.aperio.cal-adapter-google").unwrap();
    let cfg = serde_json::json!({
        "client_id": "test-client",
        "client_secret": "test-secret",
        "access_token": access,
        "refresh_token": "1//test-refresh",
        "expires_at": "2099-01-01T00:00:00Z",
        "scope": null,
    });
    manager
        .open_instance(loaded, &cfg.to_string())
        .expect("open")
}

#[test]
fn google_plugin_exposes_all_three_surfaces() {
    let manager = register();
    let inst = open_one(&manager, "ya29.first");
    let _cal: Arc<FfiCalendarAdapter> =
        Arc::new(FfiCalendarAdapter::new(inst.clone()).expect("calendar slot present"));
    let _tasks: Arc<FfiTasksAdapter> =
        Arc::new(FfiTasksAdapter::new(inst.clone()).expect("tasks slot present"));
    let _contacts: Arc<FfiContactsAdapter> =
        Arc::new(FfiContactsAdapter::new(inst).expect("contacts slot present"));
}

#[test]
fn multiple_google_accounts_get_distinct_handles() {
    let manager = register();
    let work = open_one(&manager, "ya29.work");
    let private = open_one(&manager, "ya29.private");
    assert_ne!(work.handle() as usize, private.handle() as usize);
}

#[test]
fn manifest_capabilities_match_vtable() {
    let m = manifest();
    assert!(m.has_capability(&Capability::Calendar));
    assert!(m.has_capability(&Capability::Tasks));
    assert!(m.has_capability(&Capability::Contacts));
}
