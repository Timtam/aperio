//! An account row can always be told what its adapter is called.
//!
//! The name of an adapter kind moved out of the app's locale files and into the
//! manifest of the adapter that owns the kind. That fixed who OWNS the name and
//! broke, briefly, how AVAILABLE it is: the app's translations are compiled in
//! and unconditional, while a host lookup can be late, can fail, and — the case
//! this file exists for — hides plugins the user has disabled.
//!
//! `kind_name_for` is the answer for a ROW, as distinct from the adapter-kind
//! LISTING a picker is built from. A row is drawn whether or not its plugin is
//! currently loadable, and it is drawn for a reader who hears it rather than
//! glancing at it, so "microsoft_graph" is not an acceptable answer while the
//! manifest that says "Microsoft (Outlook / To Do)" is still in memory.

use plugin_core::manager::PluginManager;

fn manager() -> PluginManager {
    let manager = PluginManager::new("0.1.0");
    host_plugins::register_all_static(&manager).expect("every bundled plugin registers");
    manager
}

#[test]
fn every_kind_this_build_serves_has_a_name_that_is_not_the_kind() {
    let manager = manager();
    let mut checked = Vec::new();

    for info in host_core::builtin_adapters::all_adapter_kinds(&manager, "en") {
        let (name, short) = host_core::builtin_adapters::kind_name_for(&manager, &info.kind, "en");
        assert_ne!(
            name, info.kind,
            "{} fell through to its own kind string, so a row would read it out",
            info.kind,
        );
        assert!(!short.trim().is_empty(), "{} has no short name", info.kind);
        checked.push(info.kind);
    }

    // The device adapter is not in that list — it is offered on the phone's own
    // terms — but it labels account rows exactly like the rest.
    let (device, _) = host_core::builtin_adapters::kind_name_for(&manager, "device_calendar", "en");
    assert_ne!(device, "device_calendar");

    for expected in ["local", "caldav", "webdav"] {
        assert!(
            checked.iter().any(|k| k == expected),
            "{expected} was not among the kinds walked: {checked:?}",
        );
    }
}

#[test]
fn a_disabled_plugins_accounts_keep_their_name() {
    // The regression this whole arrangement exists to prevent. Disabling a
    // plugin leaves its accounts on screen — that is deliberate, they are the
    // rows that say "plugin missing" — and `adapter_kinds()` stops reporting
    // its kind, because a picker must not offer an adapter that cannot run.
    // Naming is a different question with a different answer: the manager still
    // holds the manifest, so the row keeps its word.
    let manager = manager();
    let before = host_core::builtin_adapters::kind_name_for(&manager, "caldav", "en");
    assert_ne!(before.0, "caldav");

    assert!(
        manager.set_enabled("com.aperio.cal-adapter-caldav", false),
        "the CalDAV plugin is registered and can be disabled",
    );
    assert!(
        !manager
            .adapter_kinds("en")
            .iter()
            .any(|k| k.kind == "caldav"),
        "a disabled plugin is correctly absent from the picker's listing",
    );

    let after = host_core::builtin_adapters::kind_name_for(&manager, "caldav", "en");
    assert_eq!(
        after, before,
        "the row's name must not depend on whether the plugin is enabled",
    );
}

#[test]
fn an_adopted_kind_is_named_by_the_adapter_that_took_it_on() {
    // `local_folder` is the folder sync's retired kind, adopted by the built-in
    // store. It is deliberately NOT in the adapter-kind listing — putting it
    // there would hand it the store's capability flags and sprout an empty
    // sidebar branch — so this is the only path that can name it.
    let manager = manager();
    let (name, _) = host_core::builtin_adapters::kind_name_for(&manager, "local_folder", "en");
    assert_ne!(name, "local_folder");
    assert!(
        !host_core::builtin_adapters::all_adapter_kinds(&manager, "en")
            .iter()
            .any(|k| k.kind == "local_folder"),
        "an adopted kind is nameable without being listed",
    );
}

#[test]
fn a_kind_no_adapter_serves_is_called_by_its_kind() {
    // The last stop, and the only honest one: the plugin is gone and its
    // manifest with it, so nothing anywhere knows what it called itself. The
    // row that carries the kind says "plugin not installed" in its own words.
    let manager = manager();
    let (name, short) = host_core::builtin_adapters::kind_name_for(&manager, "acme-gone", "en");
    assert_eq!(name, "acme-gone");
    assert_eq!(short, "acme-gone");
}

#[test]
fn the_language_asked_for_is_the_language_answered_in() {
    let manager = manager();
    let (en, _) = host_core::builtin_adapters::kind_name_for(&manager, "local", "en");
    let (de, _) = host_core::builtin_adapters::kind_name_for(&manager, "local", "de");
    assert_eq!(en, "Local");
    assert_eq!(de, "Lokal");
}
