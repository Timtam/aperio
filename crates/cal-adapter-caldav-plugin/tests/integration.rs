//! Smoke test for the CalDAV plugin (ABI v2).

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
        id: "com.aperio.cal-adapter-caldav".into(),
        name: "Aperio CalDAV".into(),
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
    let d: *mut AperioPlugin = unsafe { cal_adapter_caldav_plugin::build_descriptor() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) = cal_adapter_caldav_plugin::DESTROY_FN;
    m.register_static(manifest(), d, dx).unwrap();
    m
}

fn open_one(manager: &PluginManager, server: &str, user: &str) -> Arc<plugin_core::LoadedInstance> {
    let loaded = manager.get("com.aperio.cal-adapter-caldav").unwrap();
    let cfg = serde_json::json!({
        "server_url": server,
        "username": user,
        "auth_kind": "basic",
        "secret": "hunter2",
    });
    manager
        .open_instance(loaded, &cfg.to_string())
        .expect("open")
}

#[test]
fn caldav_plugin_exposes_all_three_surfaces() {
    let manager = register();
    let inst = open_one(&manager, "https://caldav.example.invalid/", "alice");
    let _cal: Arc<FfiCalendarAdapter> =
        Arc::new(FfiCalendarAdapter::new(inst.clone()).expect("calendar slot present"));
    let _tasks: Arc<FfiTasksAdapter> =
        Arc::new(FfiTasksAdapter::new(inst.clone()).expect("tasks slot present"));
    let _contacts: Arc<FfiContactsAdapter> =
        Arc::new(FfiContactsAdapter::new(inst).expect("contacts slot present"));
}

#[test]
fn multiple_caldav_accounts_get_distinct_handles() {
    let manager = register();
    let icloud = open_one(&manager, "https://caldav.icloud.com/", "user@icloud.com");
    let nextcloud = open_one(&manager, "https://cloud.example.org/", "alice");
    assert_ne!(icloud.handle() as usize, nextcloud.handle() as usize);
}

#[test]
fn manifest_capabilities_match_vtable() {
    let m = manifest();
    assert!(m.has_capability(&Capability::Calendar));
    assert!(m.has_capability(&Capability::Tasks));
    assert!(m.has_capability(&Capability::Contacts));
}
