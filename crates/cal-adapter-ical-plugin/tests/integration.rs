//! Smoke test for the iCal-feed plugin (ABI v2).

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
        id: "com.aperio.cal-adapter-ical".into(),
        name: "Aperio iCal Feed".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::Adapter,
        capabilities: vec![Capability::Calendar],
        abi_version: ABI_VERSION,
        min_app_version: "0.1.0".into(),
        author: Some("Aperio Contributors".into()),
        description: Some("Bundled".into()),
        signed: false,
        recurrence: Default::default(),
        tasks: Default::default(),
        account: None,
        adapter_kind: None,
        adopts_adapter_kinds: Vec::new(),
        strings: Default::default(),
    }
}

fn register() -> PluginManager {
    let m = PluginManager::new("0.1.0");
    let d: *mut AperioPlugin = unsafe { cal_adapter_ical_plugin::build_descriptor() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) = cal_adapter_ical_plugin::DESTROY_FN;
    m.register_static(manifest(), d, dx).unwrap();
    m
}

fn open_one(manager: &PluginManager, feed_url: &str) -> Arc<plugin_core::LoadedInstance> {
    let loaded = manager.get("com.aperio.cal-adapter-ical").unwrap();
    let cfg = serde_json::json!({
        "feed_url": feed_url,
        "username": null,
        "password": null,
    });
    manager
        .open_instance(loaded, &cfg.to_string())
        .expect("open")
}

#[test]
fn ical_plugin_wraps_through_ffi_calendar_adapter() {
    let manager = register();
    let inst = open_one(&manager, "https://example.invalid/holidays.ics");
    let _adapter: Arc<FfiCalendarAdapter> =
        Arc::new(FfiCalendarAdapter::new(inst).expect("calendar slot present"));
}

#[test]
fn ical_plugin_has_no_tasks_or_contacts_slots() {
    let manager = register();
    let inst = open_one(&manager, "https://example.invalid/feed.ics");
    assert!(FfiTasksAdapter::new(inst.clone()).is_none());
    assert!(FfiContactsAdapter::new(inst).is_none());
}

#[test]
fn multiple_ical_feeds_get_distinct_handles() {
    let manager = register();
    let a = open_one(&manager, "https://a.invalid/feed.ics");
    let b = open_one(&manager, "https://b.invalid/feed.ics");
    assert_ne!(a.handle() as usize, b.handle() as usize);
}

#[test]
fn manifest_capabilities_match_vtable() {
    let m = manifest();
    assert_eq!(m.capabilities, vec![Capability::Calendar]);
    assert!(m.has_capability(&Capability::Calendar));
    assert!(!m.has_capability(&Capability::Tasks));
    assert!(!m.has_capability(&Capability::Contacts));
}
