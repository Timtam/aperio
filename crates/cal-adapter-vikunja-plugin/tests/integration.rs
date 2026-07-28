//! Smoke test for the Vikunja plugin (ABI v2).

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
        id: "com.aperio.cal-adapter-vikunja".into(),
        name: "Aperio Vikunja".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::CalendarAdapter,
        capabilities: vec![Capability::Tasks],
        abi_version: ABI_VERSION,
        min_app_version: "0.1.0".into(),
        author: Some("Aperio Contributors".into()),
        description: Some("Bundled".into()),
        signed: false,
        recurrence: Default::default(),
        tasks: Default::default(),
        account: None,
        adapter_kind: None,
    }
}

fn register() -> PluginManager {
    let m = PluginManager::new("0.1.0");
    let d: *mut AperioPlugin = unsafe { cal_adapter_vikunja_plugin::build_descriptor() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) = cal_adapter_vikunja_plugin::DESTROY_FN;
    m.register_static(manifest(), d, dx).unwrap();
    m
}

fn open_one(
    manager: &PluginManager,
    server_url: &str,
    token: &str,
) -> Arc<plugin_core::LoadedInstance> {
    let loaded = manager.get("com.aperio.cal-adapter-vikunja").unwrap();
    let cfg = serde_json::json!({ "server_url": server_url, "token": token });
    manager
        .open_instance(loaded, &cfg.to_string())
        .expect("open")
}

#[test]
fn vikunja_plugin_wraps_through_ffi_tasks_adapter() {
    let manager = register();
    let inst = open_one(&manager, "https://a.example.invalid", "tok-a");
    let _adapter: Arc<FfiTasksAdapter> =
        Arc::new(FfiTasksAdapter::new(inst).expect("tasks slot present"));
}

#[test]
fn vikunja_plugin_has_no_calendar_or_contacts_slots() {
    let manager = register();
    let inst = open_one(&manager, "https://b.example.invalid", "tok-b");
    assert!(FfiCalendarAdapter::new(inst.clone()).is_none());
    assert!(FfiContactsAdapter::new(inst).is_none());
}

#[test]
fn multiple_vikunja_instances_get_distinct_handles() {
    let manager = register();
    let a = open_one(&manager, "https://server-a.example.invalid", "tok-a");
    let b = open_one(&manager, "https://server-b.example.invalid", "tok-b");
    assert_ne!(a.handle() as usize, b.handle() as usize);
}
