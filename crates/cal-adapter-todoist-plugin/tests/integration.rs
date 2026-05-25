//! Smoke test for the Todoist plugin (ABI v2).
//!
//! Validates:
//!   - Single-capability tasks adapter wires through
//!     FfiTasksAdapter; FfiCalendarAdapter + FfiContactsAdapter
//!     return None silently for the null slots.
//!   - The new instance-handle ABI: two independent open_instance
//!     calls on the same loaded library yield two distinct
//!     handles + can be closed independently (DESIGN.md §6.4).

use std::sync::Arc;

use plugin_core::{
    abi::AperioPlugin, manager::PluginManager, manifest::PluginManifest,
    shim::{FfiCalendarAdapter, FfiContactsAdapter, FfiTasksAdapter},
    Capability, PluginType, ABI_VERSION,
};

fn manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.cal-adapter-todoist".into(),
        name: "Aperio Todoist".into(),
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
        unsafe { cal_adapter_todoist_plugin::build_descriptor() };
    assert!(!d.is_null());
    let dx: unsafe extern "C" fn(*mut AperioPlugin) =
        cal_adapter_todoist_plugin::DESTROY_FN;
    m.register_static(manifest(), d, dx).unwrap();
    m
}

fn open_one(manager: &PluginManager, token: &str) -> Arc<plugin_core::LoadedInstance> {
    let loaded = manager.get("com.aperio.cal-adapter-todoist").unwrap();
    let cfg = serde_json::json!({ "token": token });
    manager.open_instance(loaded, &cfg.to_string()).expect("open")
}

#[test]
fn todoist_plugin_wraps_through_ffi_tasks_adapter() {
    let manager = register();
    let inst = open_one(&manager, "test-token-a");
    let _adapter: Arc<FfiTasksAdapter> = Arc::new(
        FfiTasksAdapter::new(inst).expect("tasks slot present"),
    );
}

#[test]
fn todoist_plugin_has_no_calendar_or_contacts_slots() {
    let manager = register();
    let inst = open_one(&manager, "test-token-b");
    assert!(FfiCalendarAdapter::new(inst.clone()).is_none());
    assert!(FfiContactsAdapter::new(inst).is_none());
}

#[test]
fn multiple_instances_per_library_get_distinct_handles() {
    // Two opens against the same loaded library must produce two
    // distinct handles — that's the v2 ABI promise. If the
    // singleton legacy regressed, both opens would return the
    // same pointer (or the second would fail outright).
    let manager = register();
    let a = open_one(&manager, "token-account-a");
    let b = open_one(&manager, "token-account-b");
    assert_ne!(
        a.handle() as usize,
        b.handle() as usize,
        "v2 must hand out one handle per open_instance call",
    );
    // Both should wrap independently.
    let _wrap_a: Arc<FfiTasksAdapter> = Arc::new(
        FfiTasksAdapter::new(a).expect("a"),
    );
    let _wrap_b: Arc<FfiTasksAdapter> = Arc::new(
        FfiTasksAdapter::new(b).expect("b"),
    );
}

#[test]
fn open_instance_rejects_empty_token() {
    let manager = register();
    let loaded = manager.get("com.aperio.cal-adapter-todoist").unwrap();
    let bad_cfg = serde_json::json!({ "token": "   " });
    let err = manager.open_instance(loaded, &bad_cfg.to_string()).unwrap_err();
    match err {
        plugin_core::error::PluginError::InstanceOpen { status, .. } => {
            assert_eq!(
                status,
                plugin_core::PLUGIN_ERR_INVALID_CONFIG,
                "empty token should surface as invalid_config",
            );
        }
        other => panic!("expected InstanceOpen, got {other:?}"),
    }
}
