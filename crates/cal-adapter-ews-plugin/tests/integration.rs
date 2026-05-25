//! Smoke test for the EWS plugin (ABI v2).

use std::sync::Arc;

use plugin_core::{
    abi::AperioPlugin, manager::PluginManager, manifest::PluginManifest,
    shim::{FfiCalendarAdapter, FfiContactsAdapter, FfiTasksAdapter},
    Capability, PluginType, ABI_VERSION,
};

fn manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.cal-adapter-ews".into(),
        name: "Aperio Exchange (EWS)".into(),
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
    }
}

fn register() -> PluginManager {
    let m = PluginManager::new("0.1.0");
    let d: *mut AperioPlugin =
        unsafe { cal_adapter_ews_plugin::aperio_plugin_create() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) =
        cal_adapter_ews_plugin::aperio_plugin_destroy;
    m.register_static(manifest(), d, dx).unwrap();
    m
}

fn open_one(manager: &PluginManager, endpoint: &str) -> Arc<plugin_core::LoadedInstance> {
    let loaded = manager.get("com.aperio.cal-adapter-ews").unwrap();
    let cfg = serde_json::json!({
        "endpoint": endpoint,
        "username": "alice@example.invalid",
        "password": "hunter2",
    });
    manager.open_instance(loaded, &cfg.to_string()).expect("open")
}

#[test]
fn ews_plugin_exposes_all_three_surfaces() {
    let manager = register();
    let inst = open_one(&manager, "https://mail.example.invalid/EWS/Exchange.asmx");
    let _cal: Arc<FfiCalendarAdapter> = Arc::new(
        FfiCalendarAdapter::new(inst.clone()).expect("calendar slot present"),
    );
    let _tasks: Arc<FfiTasksAdapter> = Arc::new(
        FfiTasksAdapter::new(inst.clone()).expect("tasks slot present"),
    );
    let _contacts: Arc<FfiContactsAdapter> = Arc::new(
        FfiContactsAdapter::new(inst).expect("contacts slot present"),
    );
}

#[test]
fn multiple_ews_servers_get_distinct_handles() {
    let manager = register();
    let on_prem = open_one(&manager, "https://exchange.intern.invalid/EWS/Exchange.asmx");
    let kerio = open_one(&manager, "https://kerio.intern.invalid/EWS/Exchange.asmx");
    assert_ne!(on_prem.handle() as usize, kerio.handle() as usize);
}

#[test]
fn manifest_capabilities_match_vtable() {
    let m = manifest();
    assert!(m.has_capability(&Capability::Calendar));
    assert!(m.has_capability(&Capability::Tasks));
    assert!(m.has_capability(&Capability::Contacts));
}
