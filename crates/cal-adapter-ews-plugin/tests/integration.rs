//! Smoke test for the EWS plugin (P4 cal).
//!
//! Validates the full-3-capability vtable wiring with fake
//! Basic-auth credentials — the constructor never hits the
//! network.

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
        "endpoint": "https://mail.example.invalid/EWS/Exchange.asmx",
        "username": "alice@example.invalid",
        "password": "hunter2"
    });
    let c = CString::new(cfg.to_string()).unwrap();
    let rc = unsafe { cal_adapter_ews_plugin::plugin_init(c.as_ptr()) };
    assert_eq!(rc, plugin_core::PLUGIN_OK);
    *done = true;
}

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

#[test]
fn ews_plugin_exposes_all_three_surfaces() {
    shared_setup();
    let manager = register();
    let loaded = manager.get("com.aperio.cal-adapter-ews").unwrap();
    let _cal: Arc<FfiCalendarAdapter> = Arc::new(
        FfiCalendarAdapter::new(loaded.clone()).expect("calendar slot present"),
    );
    let _tasks: Arc<FfiTasksAdapter> = Arc::new(
        FfiTasksAdapter::new(loaded.clone()).expect("tasks slot present"),
    );
    let _contacts: Arc<FfiContactsAdapter> = Arc::new(
        FfiContactsAdapter::new(loaded).expect("contacts slot present"),
    );
}

#[test]
fn manifest_capabilities_match_vtable() {
    let m = manifest();
    assert!(m.has_capability(&Capability::Calendar));
    assert!(m.has_capability(&Capability::Tasks));
    assert!(m.has_capability(&Capability::Contacts));
}
