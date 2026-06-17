//! Integration test for the static plugin registry.
//!
//! Proves the mobile/iOS no-dlopen path: linking the 17 bundled
//! `-plugin` rlibs into one binary and registering every one into a
//! fresh [`PluginManager`] succeeds — no duplicate-symbol link
//! failure, every manifest parses + is ABI/version compatible, and
//! each expected plugin id ends up loaded.

use plugin_core::manager::PluginManager;

/// The reverse-DNS ids of all bundled plugins, mirroring the
/// manifests `register_all_static` embeds. Kept here independently so
/// the test fails loudly if a plugin silently drops out of the
/// registry or its id changes.
const EXPECTED_IDS: &[&str] = &[
    "com.aperio.cal-adapter-caldav",
    "com.aperio.cal-adapter-ical",
    "com.aperio.cal-adapter-google",
    "com.aperio.cal-adapter-microsoft-graph",
    "com.aperio.cal-adapter-ews",
    "com.aperio.cal-adapter-vikunja",
    "com.aperio.cal-adapter-todoist",
    "com.aperio.sync-adapter-local",
    "com.aperio.sync-adapter-webdav",
    "com.aperio.sync-adapter-ftp",
    "com.aperio.sync-adapter-sftp",
    "com.aperio.sync-adapter-dropbox",
    "com.aperio.sync-adapter-googledrive",
    "com.aperio.vc-adapter-zoom",
    "com.aperio.vc-adapter-teams",
    "com.aperio.vc-adapter-meet",
    "com.aperio.vc-adapter-webex",
];

#[test]
fn register_all_static_loads_every_bundled_plugin() {
    let manager = PluginManager::new("0.1.0");
    host_plugins::register_all_static(&manager).expect("all bundled plugins register");

    assert_eq!(
        manager.len(),
        host_plugins::BUNDLED_PLUGIN_COUNT,
        "registered plugin count should match BUNDLED_PLUGIN_COUNT",
    );
    assert_eq!(EXPECTED_IDS.len(), host_plugins::BUNDLED_PLUGIN_COUNT);

    for id in EXPECTED_IDS {
        assert!(
            manager.get(id).is_some(),
            "expected bundled plugin {id} to be registered",
        );
    }
}

#[test]
fn register_all_static_is_idempotent_safe_to_inspect() {
    // A second manager from scratch must register cleanly too —
    // catches any accidental process-global state in the descriptors
    // (there is none; build_descriptor() boxes a fresh descriptor
    // each call).
    let a = PluginManager::new("0.1.0");
    host_plugins::register_all_static(&a).expect("first manager");
    let b = PluginManager::new("0.1.0");
    host_plugins::register_all_static(&b).expect("second manager");

    assert_eq!(a.len(), b.len());
    assert_eq!(a.len(), host_plugins::BUNDLED_PLUGIN_COUNT);
}
