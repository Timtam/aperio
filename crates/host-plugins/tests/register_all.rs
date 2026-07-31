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

/// Every bundled plugin says what it is, and every family is represented.
///
/// `register_all_static` already fails if a manifest promises a capability its
/// vtable does not have — that check lives in `check_declared_surfaces` and this
/// test being green is the proof for all 17. What it adds is the other
/// direction: that the manifests still NAME their families after the type tag
/// stopped doing it. A dropped `capabilities` entry is now an adapter that
/// loads, registers against nothing, and looks installed.
#[test]
fn every_bundled_plugin_declares_the_family_it_serves() {
    use plugin_core::Capability;

    let manager = PluginManager::new("0.1.0");
    host_plugins::register_all_static(&manager).expect("all bundled plugins register");

    let mut sync = 0;
    let mut vc = 0;
    let mut data = 0;
    for id in EXPECTED_IDS {
        let plugin = manager.get(id).expect("registered above");
        let caps = &plugin.manifest.capabilities;
        assert!(!caps.is_empty(), "{id} declares no capability");
        assert_eq!(
            plugin.manifest.plugin_type,
            plugin_core::PluginType::Adapter,
            "{id} should be a plain adapter",
        );
        if plugin.manifest.has_capability(&Capability::Sync) {
            sync += 1;
        }
        if plugin.manifest.has_capability(&Capability::Videoconference) {
            vc += 1;
        }
        if plugin.manifest.has_data_family() {
            data += 1;
        }
    }
    assert_eq!(sync, 6, "six bundled sync backends");
    assert_eq!(vc, 1, "one bundled meeting provider");
    assert_eq!(data, 7, "seven bundled calendar/task/contact adapters");
}

/// What the SHIPPED manifests say about credentials, asserted end to end.
///
/// This replaces a pair of hand-written `match kind { … }` tables — one per
/// host — that answered the same three questions and had already drifted apart:
/// the desktop's listed the four videoconference kinds and the mobile one did
/// not, so a Webex account with no refresh token was flagged for repair on one
/// platform and silently ignored on the other. Deriving the answer from the
/// manifest means there is one statement per adapter, made by the adapter.
#[test]
fn every_bundled_adapter_says_where_its_credential_lives() {
    use host_core::account_setup::{repair_slot, required_slots_for_kind, schema_for_kind};
    use sync_engine::SecretSlot;

    let manager = PluginManager::new("0.1.0");
    host_plugins::register_all_static(&manager).expect("all bundled plugins register");

    // Typed credentials: the slot is the one the schema names, and a repair can
    // replace it by pasting.
    for (kind, slot) in [
        ("caldav", SecretSlot::Password),
        ("ews", SecretSlot::Password),
        ("vikunja", SecretSlot::ApiToken),
        ("todoist", SecretSlot::ApiToken),
    ] {
        let schema = schema_for_kind(&manager, kind).unwrap_or_else(|| panic!("{kind} schema"));
        assert_eq!(repair_slot(&schema), Some(slot), "{kind} repair slot");
        assert!(
            required_slots_for_kind(&manager, kind).contains(&slot),
            "{kind} must require {slot:?}",
        );
    }

    // Sign-in adapters: a refresh token is required, and there is nothing to
    // paste — offering a credential field for these is the bug this prevents.
    for kind in ["google", "microsoft_graph", "webex"] {
        let schema = schema_for_kind(&manager, kind).unwrap_or_else(|| panic!("{kind} schema"));
        assert!(
            host_core::account_setup::signs_in_with_oauth(&schema),
            "{kind} signs in through its provider",
        );
        assert_eq!(repair_slot(&schema), None, "{kind} has nothing to paste");
        assert!(
            required_slots_for_kind(&manager, kind).contains(&SecretSlot::RefreshToken),
            "{kind} must require a refresh token",
        );
    }

    // There is no fallback left. A kind no bundled plugin claims answers
    // nothing — not a guessed `Password`, not a remembered refresh token — and
    // that is what keeps the credential-repair banner from asking the user to
    // fix a credential no adapter wants. "zoom" is the case that proves it: the
    // adapter exists in the tree but is unplugged, so this build genuinely does
    // not serve the kind. Any future name list would break this assert.
    assert!(
        schema_for_kind(&manager, "zoom").is_none(),
        "zoom is not bundled, so no schema can be found for it",
    );
    assert!(
        required_slots_for_kind(&manager, "zoom").is_empty(),
        "an unserved kind must answer nothing rather than a guess",
    );

    // An iCal feed is a URL. Requiring a secret here reported every working
    // feed as needing to be reconnected, which is how the old table failed.
    assert!(
        required_slots_for_kind(&manager, "ical").is_empty(),
        "an iCal feed needs no credential",
    );

    // Host-internal kinds have no plugin at all, and the empty answer is the
    // right one rather than a guess: their auth is an OS permission grant.
    for kind in ["local", "device_calendar"] {
        assert!(schema_for_kind(&manager, kind).is_none(), "{kind}");
        assert!(required_slots_for_kind(&manager, kind).is_empty(), "{kind}");
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
