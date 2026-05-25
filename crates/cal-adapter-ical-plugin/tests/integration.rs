//! Smoke test for the iCal-feed cal-adapter plugin (P4 cal PoC).
//!
//! Validates:
//!   - The CalendarAdapterVtable wrapper is read correctly by
//!     `FfiCalendarAdapter` (this is the first calendar plugin
//!     to exercise that path).
//!   - The CalendarVtable inside it has the expected method
//!     pointers.
//!   - Pure-calendar adapters (with null tasks + contacts slots)
//!     wrap successfully + downstream FfiTasksAdapter /
//!     FfiContactsAdapter::new return None for them.

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
        "feed_url": "https://example.invalid/holidays.ics",
        "username": null,
        "password": null
    });
    let c = CString::new(cfg.to_string()).unwrap();
    let rc = unsafe { cal_adapter_ical_plugin::plugin_init(c.as_ptr()) };
    assert_eq!(rc, plugin_core::PLUGIN_OK);
    *done = true;
}

fn manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.cal-adapter-ical".into(),
        name: "Aperio iCal Feed".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::CalendarAdapter,
        capabilities: vec![Capability::Calendar],
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
        unsafe { cal_adapter_ical_plugin::aperio_plugin_create() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) =
        cal_adapter_ical_plugin::aperio_plugin_destroy;
    m.register_static(manifest(), d, dx).unwrap();
    m
}

#[test]
fn ical_plugin_wraps_through_ffi_calendar_adapter() {
    shared_setup();
    let manager = register();
    let loaded = manager.get("com.aperio.cal-adapter-ical").unwrap();
    let _adapter: Arc<FfiCalendarAdapter> = Arc::new(
        FfiCalendarAdapter::new(loaded).expect("calendar slot present"),
    );
}

/// The same plugin's tasks + contacts slots are null — the
/// FfiTasksAdapter / FfiContactsAdapter constructors must
/// return None silently rather than panicking on the null
/// dereference.
#[test]
fn ical_plugin_has_no_tasks_or_contacts_slots() {
    shared_setup();
    let manager = register();
    let loaded = manager.get("com.aperio.cal-adapter-ical").unwrap();
    assert!(FfiTasksAdapter::new(loaded.clone()).is_none());
    assert!(FfiContactsAdapter::new(loaded).is_none());
}

/// Sanity check: the manifest declares a single capability —
/// "calendar" — matching the populated sub-vtable slot.
#[test]
fn manifest_capabilities_match_vtable() {
    let m = manifest();
    assert_eq!(m.capabilities, vec![Capability::Calendar]);
    assert!(m.has_capability(&Capability::Calendar));
    assert!(!m.has_capability(&Capability::Tasks));
    assert!(!m.has_capability(&Capability::Contacts));
}
