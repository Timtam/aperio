//! Smoke test for the Vikunja plugin (P4 cal). Tasks-only —
//! same null-slot pattern as Todoist.

use std::ffi::CString;
use std::sync::{Arc, Mutex};

use plugin_core::{
    abi::AperioPlugin, manager::PluginManager, manifest::PluginManifest,
    shim::{FfiCalendarAdapter, FfiContactsAdapter, FfiTasksAdapter},
    Capability, PluginType, ABI_VERSION,
};

fn shared_setup() {
    static DONE: Mutex<bool> = Mutex::new(false);
    let mut done = DONE.lock().unwrap();
    if *done {
        return;
    }
    let cfg = serde_json::json!({
        "server_url": "https://vikunja.example.invalid",
        "token": "test-token"
    });
    let c = CString::new(cfg.to_string()).unwrap();
    let rc = unsafe { cal_adapter_vikunja_plugin::plugin_init(c.as_ptr()) };
    assert_eq!(rc, plugin_core::PLUGIN_OK);
    *done = true;
}

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
    }
}

fn register() -> PluginManager {
    let m = PluginManager::new("0.1.0");
    let d: *mut AperioPlugin =
        unsafe { cal_adapter_vikunja_plugin::aperio_plugin_create() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) =
        cal_adapter_vikunja_plugin::aperio_plugin_destroy;
    m.register_static(manifest(), d, dx).unwrap();
    m
}

#[test]
fn vikunja_plugin_wraps_through_ffi_tasks_adapter() {
    shared_setup();
    let manager = register();
    let loaded = manager.get("com.aperio.cal-adapter-vikunja").unwrap();
    let _adapter: Arc<FfiTasksAdapter> = Arc::new(
        FfiTasksAdapter::new(loaded).expect("tasks slot present"),
    );
}

#[test]
fn vikunja_plugin_has_no_calendar_or_contacts_slots() {
    shared_setup();
    let manager = register();
    let loaded = manager.get("com.aperio.cal-adapter-vikunja").unwrap();
    assert!(FfiCalendarAdapter::new(loaded.clone()).is_none());
    assert!(FfiContactsAdapter::new(loaded).is_none());
}
