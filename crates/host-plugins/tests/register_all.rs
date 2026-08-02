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
    // fix a credential no adapter wants. "zoom" is the case that proves it: no
    // adapter in this tree serves that kind at all, so the answer has to come
    // from the loaded manifests rather than from a guess. Any future name list
    // would break this assert.
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

/// Which picker an adapter belongs in is read off its capability list, not off
/// a list of names in a frontend.
///
/// The two questions are different and an adapter may answer yes to both: a
/// provider offering a calendar AND file storage is one account that also
/// happens to be somewhere to sync. The frontends must never grow a table of
/// which is which, so this checks the answers at the source, against the real
/// bundled manifests.
#[test]
fn every_bundled_adapter_answers_both_picker_questions() {
    let manager = PluginManager::new(env!("CARGO_PKG_VERSION"));
    host_plugins::register_all_static(&manager).expect("all bundled plugins register");

    let kinds = manager.adapter_kinds();
    assert!(!kinds.is_empty(), "no adapter kinds at all");

    for info in &kinds {
        assert!(
            info.holds_data || info.can_sync,
            "{} answers neither question, so it appears in no picker and              cannot be reached at all",
            info.kind,
        );
    }

    // Where a dataset can live, by name. This used to assert the list was
    // EMPTY — the sync plugins declared no `adapter_kind`, so they appeared in
    // no picker and a sync target was not an account at all.
    //
    // Spelled out rather than counted: a kind here is persisted in
    // `accounts.adapter_kind` and travels in the sync payload, so a rename is
    // not a rename, it is an orphaned account row on every device. And each
    // needs a label in both locale files before it can ship, which
    // `manifests_parse::every_declared_kind_is_named_in_both_locales` checks.
    // `adapter_kinds()` sorts by kind, so this comparison is order-stable.
    //
    // `google` and `googledrive` are ONE adapter: Drive folded into the Google
    // account, and the old kind stays listed because rows still carry it. Which
    // of the two may be created is `offered`, asserted just below.
    //
    // No `local_folder`, and no `local`: folder sync folded into the BUILT-IN
    // store, which is not a plugin and is therefore not in this list at all. It
    // is declared by `host_core::builtin_adapters` and asserted at the bottom
    // of this test — leaving it out here would have quietly dropped the one
    // storage backend that needs no account of its own.
    let syncable: Vec<&str> = kinds
        .iter()
        .filter(|i| i.can_sync)
        .map(|i| i.kind.as_str())
        .collect();
    assert_eq!(
        syncable,
        ["dropbox", "ftp", "google", "googledrive", "sftp", "webdav"],
        "these are the bundled kinds a dataset can live on",
    );

    // …and only one Google entry may be CREATED. `googledrive` is listed so the
    // rows that still carry it stay visible and groupable, but the adapter that
    // minted them is gone, so offering it would put an entry in the Add-account
    // picker for something that no longer exists on its own.
    let offered: Vec<&str> = kinds
        .iter()
        .filter(|i| i.can_sync && i.offered)
        .map(|i| i.kind.as_str())
        .collect();
    assert_eq!(
        offered,
        ["dropbox", "ftp", "google", "sftp", "webdav"],
        "an adopted kind resolves but is never offered",
    );

    // No PLUGIN may claim the built-in store's kind. `AdapterKind::is_host_internal`
    // answers true for `local`, so such a plugin would be unregisterable while
    // both frontends disagreed about what one of its rows meant.
    assert!(
        !kinds.iter().any(|i| i.kind == "local"),
        "no plugin may claim the built-in store's kind",
    );

    // The built-in store, from the other list. Folder sync folded into it, so
    // it is the one place a dataset can live without an account of its own —
    // and the only entry that answers both questions while being creatable by
    // nobody, because it is already there.
    let builtin = host_core::builtin_adapters::builtin_adapter_kinds();
    let local = builtin
        .iter()
        .find(|k| k.kind == "local")
        .expect("the built-in store declares itself");
    assert!(local.can_sync, "folder sync folded into it");
    assert!(local.holds_data, "and it still holds the calendars");
    assert!(!local.offered, "there is one, and it exists from bootstrap");

    // A sync backend that holds no data keeps its account row OFF the wire
    // (`host_core::accounts::travels_between_devices`): the row's `config_json`
    // names the very server the log is written to, and often a path that means
    // nothing on another machine.
    //
    // Google answers both questions, and that is a decision rather than a side
    // effect. Drive folded into the Google account, so one row is a calendar
    // source AND a place the dataset can live — it travelled before the merge
    // because of its calendars and goes on travelling. What makes that safe is
    // that nothing in a Drive config is machine-specific: a client id, and the
    // name of a folder that is the same folder seen from every device.
    // Contrast `local_folder` and `sftp`, whose paths are not.
    //
    // Spelled out, so a SEVENTH backend that grows a data family has to come
    // through here and answer the same question rather than start travelling
    // quietly.
    let both: Vec<&str> = kinds
        .iter()
        .filter(|i| i.can_sync && i.holds_data)
        .map(|i| i.kind.as_str())
        .collect();
    assert_eq!(
        both,
        ["google", "googledrive"],
        "a sync backend that also holds data starts travelling between devices; \
         check its config carries nothing machine-specific first",
    );
}
