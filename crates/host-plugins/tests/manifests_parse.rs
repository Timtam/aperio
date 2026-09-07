//! What every adapter this build declares promises about itself.
//!
//! These tests used to walk `crates/*/plugin.json`. That answered "which crates
//! happen to sit next to this one" — precisely the fact that changes the day an
//! adapter moves into its own repository, and every test here would have failed
//! at once on a tree that was perfectly correct.
//!
//! The source is now the REGISTRY: a `PluginManager` that `register_all_static`
//! has populated, exactly as the app populates it, plus the two adapters the
//! host links instead of loading. That set does not change when a crate moves;
//! it changes when the build ships a different adapter, which is when these
//! tests SHOULD have something new to say.
//!
//! Two questions genuinely are about the tree, and they stay there, in
//! [`no_manifest_in_this_repo_is_unaccounted_for`]. A `plugin.json` that no
//! build declares reaches no user, so parsing it proves nothing — but it is
//! also a crate somebody meant to wire up and did not. And two crates claiming
//! one plugin id is a thing the registry structurally cannot see: the manager
//! refuses the second registration, so the pair never arrives as a pair.
//!
//! Each question below is asked of whatever this build links, so a build that
//! ships three adapters gets three answers rather than a failure. The one
//! exception is the tree question, which only has an answer when every adapter
//! is linked; it carries the condition for that on itself.

use plugin_core::manager::PluginManager;
use plugin_core::manifest::PluginManifest;
use std::fs;
use std::path::PathBuf;

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("host-plugins lives under crates/")
        .to_path_buf()
}

/// Every adapter this build declares, paired with the plugin id that owns it.
///
/// Two sources, and together they are the whole set: the bundled plugins, read
/// out of a manager `register_all_static` has filled; and the ones the host
/// implements itself, from `host_core::builtin_adapters`. Neither list is
/// maintained here — asking the two places that already know is what keeps this
/// file from being a third thing that can drift.
///
/// Registration is part of the question, not a cost of asking it: a manifest
/// the loader would reject is not one this build declares, whatever the file
/// says.
fn declared_manifests() -> Vec<(String, PluginManifest)> {
    let manager = PluginManager::new("0.1.0");
    host_plugins::register_all_static(&manager).expect("every bundled plugin registers");

    let mut found: Vec<(String, PluginManifest)> = manager
        .all()
        .into_iter()
        .map(|p| (p.manifest.id.clone(), p.manifest.clone()))
        .collect();
    found.extend(
        host_core::builtin_adapters::builtin_manifests()
            .into_iter()
            .map(|m| (m.id.clone(), m.clone())),
    );
    found
}

/// The plugin ids of the two adapters the host links rather than loads. Named
/// here because they are the two that stay in this repository however many
/// plugin crates move out — so a check that cannot find them is broken, and one
/// that finds only them is right.
const HOST_LINKED_IDS: [&str; 2] = [
    "com.aperio.cal-adapter-local",
    "com.aperio.adapter-device-calendar",
];

#[test]
fn every_manifest_this_build_declares_has_an_id_of_its_own() {
    let declared = declared_manifests();

    for (id, _) in &declared {
        assert!(!id.trim().is_empty(), "a declared manifest has an empty id");
    }

    // The count is derived, not written down: whatever the feature set links
    // plus the host's own. A build that drops an adapter changes both sides.
    assert_eq!(
        declared.len(),
        host_plugins::BUNDLED_PLUGIN_COUNT + host_core::builtin_adapters::builtin_manifests().len(),
        "declared manifests: {:?}",
        declared.iter().map(|(id, _)| id).collect::<Vec<_>>(),
    );
    for id in HOST_LINKED_IDS {
        assert!(
            declared.iter().any(|(seen, _)| seen == id),
            "{id} is not among the declared manifests: {:?}",
            declared.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        );
    }

    let mut ids: Vec<&str> = declared.iter().map(|(id, _)| id.as_str()).collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        before,
        "two adapters share a plugin id; the loader keys on it",
    );
}

/// Nothing in this repository ships a manifest that no build declares, and no
/// two crates claim one plugin id.
///
/// The one question here that really is about the directory tree, and the
/// reason it survives the extraction unchanged: after the adapters move out it
/// finds two manifests, both declared, and passes. What it catches is a crate
/// added with a `plugin.json` and never wired into `register_all_static` — a
/// manifest that parses, validates, and reaches nobody.
///
/// The id check has to live here rather than over the registry. `PluginManager`
/// refuses a second registration of an id it already holds, so two crates
/// claiming one id never reach `declared_manifests()` as two — and the crate
/// that gets refused is the one a copy-and-edit produces: everything renamed
/// except the id it was copied from, which then looks accounted for because the
/// crate it was copied from is registered under exactly that id.
///
/// Only compiled when every bundled adapter is linked, and keyed on that
/// CONDITION rather than on the `static` feature that happens to turn them all
/// on: `cal-ffi` enables the twelve one by one and never names `static`, so
/// keying on the name would compile this away for the mobile host's own feature
/// set. A build that links three adapters legitimately leaves eleven crates
/// unwired and has no answer to give here. Every other test in this file does
/// have one, which is why this is the only one gated.
#[cfg(all(
    feature = "caldav",
    feature = "ical",
    feature = "google",
    feature = "microsoft-graph",
    feature = "ews",
    feature = "vikunja",
    feature = "todoist",
    feature = "webdav",
    feature = "ftp",
    feature = "sftp",
    feature = "dropbox",
    feature = "webex",
))]
#[test]
fn no_manifest_in_this_repo_is_unaccounted_for() {
    let crates = crates_dir();

    // The walk first, and only then the registry. A manifest that does not
    // parse fails here naming the file; `declared_manifests()` would hit the
    // same bytes through `register_all_static`, whose error carries no path.
    let mut in_tree: Vec<(String, String)> = Vec::new();
    for entry in fs::read_dir(&crates).expect("read crates/") {
        let dir = entry.expect("dir entry").path();
        let path = dir.join("plugin.json");
        if !path.is_file() {
            continue;
        }
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let manifest = PluginManifest::from_bytes(&bytes)
            .unwrap_or_else(|e| panic!("{} is not a loadable manifest: {e}", path.display()));
        in_tree.push((name, manifest.id));
    }

    let mut by_id: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for (name, id) in &in_tree {
        by_id.entry(id.as_str()).or_default().push(name.as_str());
    }
    let clashes: Vec<_> = by_id
        .iter()
        .filter(|(_, dirs)| dirs.len() > 1)
        .map(|(id, dirs)| format!("{id}: {dirs:?}"))
        .collect();
    assert!(
        clashes.is_empty(),
        "these crates claim one plugin id between them, and the loader keys on \
         it — the second to register is refused, so one of the two adapters is \
         simply absent, and which one depends on load order: {clashes:?}",
    );

    let declared = declared_manifests();
    let stray: Vec<_> = in_tree
        .iter()
        .filter(|(_, id)| !declared.iter().any(|(seen, _)| seen == id))
        .map(|(name, id)| format!("{name} ({id})"))
        .collect();
    assert!(
        stray.is_empty(),
        "these crates ship a manifest no build declares, so nothing loads them \
         and no test above says anything about them — wire them into \
         `register_all_static` or delete the manifest: {stray:?}",
    );

    // Named rather than counted: the number of crates is exactly what the
    // extraction changes, so a floor would fail on a correct tree.
    for dir in ["adapter-local", "adapter-device-calendar"] {
        assert!(
            in_tree.iter().any(|(name, _)| name == dir),
            "{dir} was not found under {} — the walk is probably wrong (found: {:?})",
            crates.display(),
            in_tree.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        );
    }
}

/// Every adapter that can hold a dataset, and the kind it answers to, spelled
/// out here so a rename has to be made twice — once in the manifest, once in
/// front of somebody reading this list.
///
/// These strings are not internal. They are written into `accounts.adapter_kind`
/// and travel in the sync payload, so changing one orphans every account row a
/// user already has.
///
/// Two of the six are not storage backends in the old sense, and that is the
/// point of the consolidations: `google` holds a dataset in Drive with the same
/// account that serves its calendars, and `local` is the built-in store
/// mirroring into a folder. Both got here by folding a separate adapter in.
///
/// Keyed by plugin id rather than by crate directory. The id is what the
/// manifest declares and what the loader keys on; the directory name is an
/// accident of where the crate happens to live, which is the thing extraction
/// takes away.
const SYNC_ADAPTER_KINDS: &[(&str, &str)] = &[
    ("com.aperio.sync-adapter-webdav", "webdav"),
    ("com.aperio.sync-adapter-sftp", "sftp"),
    ("com.aperio.sync-adapter-ftp", "ftp"),
    ("com.aperio.sync-adapter-dropbox", "dropbox"),
    ("com.aperio.cal-adapter-google", "google"),
    ("com.aperio.cal-adapter-local", "local"),
];

/// Every adapter that declares the sync capability declares a kind with it, and
/// that kind is the one the hosts resolve.
///
/// The pass is by CAPABILITY, not by name. It used to filter on a
/// `sync-adapter-` prefix, which stopped meaning anything the moment the
/// prefixes came off — and a test that silently walks nothing is worse than no
/// test, so the guard at the bottom names an adapter that is always there.
///
/// What this guards is the naming: a seventh backend added tomorrow lands here
/// without anyone remembering this file, and it has to be given a kind and a
/// label before it can ship.
#[test]
fn the_sync_adapters_declare_the_kinds_the_hosts_resolve() {
    let manifests = declared_manifests();
    let mut seen = 0usize;

    for (name, manifest) in &manifests {
        if !manifest.has_capability(&plugin_core::Capability::Sync) {
            continue;
        }
        seen += 1;
        assert!(
            manifest.account.is_some(),
            "{name} can hold a dataset but declares no account schema, so nothing \
             can ask the user where to put it",
        );

        let expected = SYNC_ADAPTER_KINDS
            .iter()
            .find(|(dir, _)| dir == name)
            .map(|(_, kind)| *kind)
            .unwrap_or_else(|| {
                panic!(
                    "{name} can hold a dataset and this test has never heard of it. \
                     Add its plugin id to SYNC_ADAPTER_KINDS and give its kind a label \
                     in both locale files — otherwise it reaches a screen reader as \
                     the raw key string.",
                )
            });
        assert_eq!(
            manifest.adapter_kind.as_deref(),
            Some(expected),
            "{name} must declare `{expected}` — the string is persisted in \
             accounts.adapter_kind and travels in the sync payload, so changing it \
             orphans the rows users already have",
        );
    }

    // A test that passes because it found nothing is not a test — and this one
    // matched nothing for exactly one commit, when the prefix it filtered on
    // stopped existing.
    //
    // The guard is a name, not the table's length: a build that links three
    // adapters is a legitimate build with three answers to give, but there is no
    // build without the folder mirror, because it is part of the store the host
    // links in. If that one is missing, nothing was walked.
    assert!(
        seen > 0
            && manifests
                .iter()
                .any(|(id, _)| id == "com.aperio.cal-adapter-local"),
        "the built-in store did not turn up among the {} adapters that can hold a \
         dataset — nothing was walked",
        seen,
    );
}

/// One kind, one adapter — across everything this build declares.
///
/// The host resolves kind → plugin by asking the loaded plugins for the first
/// match (`PluginManager::plugin_for_kind`), and account rows carry nothing but
/// the kind. Two adapters answering to one name means an account silently binds
/// to whichever registered first, which differs between the desktop's dlopen
/// order and the mobile static registry — the same row, two adapters, two
/// machines.
///
/// The two host-internal names are checked in the same breath: they have no
/// manifest and never will, so a plugin claiming one is not a duplicate the
/// loader could ever notice.
#[test]
fn no_two_adapters_share_a_kind() {
    let mut by_kind: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (name, manifest) in declared_manifests() {
        if let Some(kind) = manifest.adapter_kind {
            by_kind.entry(kind).or_default().push(name);
        }
    }

    assert!(
        !by_kind.is_empty(),
        "no declared manifest names a kind — the registry is probably empty",
    );

    let clashes: Vec<_> = by_kind
        .iter()
        .filter(|(_, dirs)| dirs.len() > 1)
        .map(|(kind, dirs)| format!("{kind}: {dirs:?}"))
        .collect();
    assert!(
        clashes.is_empty(),
        "these kinds are claimed by more than one adapter, so an account row \
         binds to whichever plugin registered first: {clashes:?}",
    );

    // `local` is the built-in store. It DOES declare itself now — a manifest in
    // its own crate, read by `host_core::builtin_adapters` so every surface can
    // describe it like any other adapter — but it is not loaded as a plugin,
    // and `AdapterKind::is_host_internal` short-circuits its registration. So
    // exactly one adapter may claim it, and it is that one; a second claimant
    // would be a plugin that could never be registered.
    assert_eq!(
        by_kind.get("local").map(Vec::as_slice),
        Some(&["com.aperio.cal-adapter-local".to_string()][..]),
        "`local` belongs to the built-in store's own manifest and nothing else",
    );

    // `device_calendar` is the phone's own store, reached through a native
    // bridge the mobile host injects. Same shape as `local`: it declares itself
    // in a manifest and is CALLED directly rather than through the ABI, so
    // exactly one adapter may claim the kind.
    //
    // It used to declare nothing at all, and the string lived as a literal in
    // four places in `cal-ffi`. That is what this pins now — one manifest, one
    // claimant — rather than the old "nobody may claim it", which was only ever
    // true because nothing had been written down.
    assert_eq!(
        by_kind.get("device_calendar").map(Vec::as_slice),
        Some(&["com.aperio.adapter-device-calendar".to_string()][..]),
        "`device_calendar` belongs to the device adapter's own manifest and nothing else",
    );
}

/// An adopted kind belongs to exactly one adapter too — and never to a plugin
/// that is still shipping.
///
/// Adoption exists so a merged adapter can take over the rows of one it
/// replaced ([`PluginManifest::adopts_adapter_kinds`]). Shipping both at once
/// is not a state this tree should ever be in: the resolver prefers the plugin
/// that owns the kind, so the adopting half would sit there serving nothing,
/// and which of them a user's accounts bind to would depend on which is
/// installed — a difference between two machines running the same version.
///
/// When you adopt, delete what you adopted from.
#[test]
fn an_adopted_kind_has_no_other_claimant() {
    let manifests = declared_manifests();
    let mut own: std::collections::BTreeMap<String, String> = Default::default();
    for (name, manifest) in &manifests {
        if let Some(kind) = &manifest.adapter_kind {
            own.insert(kind.clone(), name.clone());
        }
    }

    let mut problems = Vec::new();
    let mut adopted: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (name, manifest) in &manifests {
        for kind in &manifest.adopts_adapter_kinds {
            if let Some(owner) = own.get(kind) {
                problems.push(format!(
                    "{name} adopts `{kind}`, which {owner} still declares as its own",
                ));
            }
            adopted.entry(kind.clone()).or_default().push(name.clone());
        }
    }
    for (kind, names) in &adopted {
        if names.len() > 1 {
            problems.push(format!(
                "`{kind}` is adopted by more than one adapter: {names:?}"
            ));
        }
    }

    assert!(
        !manifests.is_empty(),
        "this build declares no adapters at all — the registry is probably empty",
    );
    assert!(problems.is_empty(), "{}", problems.join("; "));
}

/// Every kind an adapter declares has a name a person can hear.
///
/// Both account-row label sites — the desktop's `AccountsPanel`/`Sidebar` and
/// the mobile `AccountsScreen` — call
/// `t('dialogs.accounts.kindName.' + kind)` with no `defaultValue`, and the
/// reconnect dialog does the same with `syncAccountsConnect.kind.`. i18next
/// returns the key itself when it misses, so the failure mode is not a blank or
/// a fallback: it is a row that a screen reader reads out as
/// "dialogs dot accounts dot kind name dot googledrive", once per account,
/// forever, with nothing on screen looking wrong to a sighted reviewer.
///
/// So it is asserted here, against the shipped locale files, rather than left to
/// be noticed.
///
/// ADOPTED kinds count. They are listed by `PluginManager::adapter_kinds()`
/// (only `offered` is false), so an account carrying one is drawn, grouped and
/// labelled through exactly the same `t(...)` call as any other. A kind that
/// stops being anybody's `adapter_kind` because it was adopted must not fall
/// out of this guard on the way.
#[test]
fn every_declared_kind_is_named_in_both_locales() {
    let repo_root = crates_dir()
        .parent()
        .expect("crates/ lives in the repo root")
        .to_path_buf();

    let mut kinds: Vec<String> = declared_manifests()
        .into_iter()
        .flat_map(|(_, m)| {
            m.adapter_kind
                .into_iter()
                .chain(m.adopts_adapter_kinds)
                .collect::<Vec<_>>()
        })
        .collect();
    kinds.sort();
    kinds.dedup();
    assert!(
        !kinds.is_empty(),
        "no kinds found — the registry is probably empty"
    );

    let mut missing = Vec::new();
    for lang in ["en", "de"] {
        let path = repo_root
            .join("locales")
            .join(lang)
            .join("translation.json");
        let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let root: serde_json::Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()));

        for (block, value) in [
            (
                "dialogs.accounts.kindName",
                root.pointer("/dialogs/accounts/kindName"),
            ),
            (
                "syncAccountsConnect.kind",
                root.pointer("/syncAccountsConnect/kind"),
            ),
        ] {
            let table = value
                .and_then(serde_json::Value::as_object)
                .unwrap_or_else(|| panic!("{lang}: {block} is not an object"));
            for kind in &kinds {
                let named = table
                    .get(kind)
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty());
                if !named {
                    missing.push(format!("{lang}: {block}.{kind}"));
                }
            }
        }
    }

    assert!(
        missing.is_empty(),
        "these kind labels are missing, and i18next renders a missing key as the \
         key itself — a screen reader reads the literal dotted string out loud: \
         {missing:#?}",
    );
}

/// What each adapter that can assign a task to somebody declares it can hold
/// (DESIGN §9.7). Spelled out here so a change has to be made twice — once in
/// the manifest, once in front of somebody reading this list.
///
/// The limit is invisible from the outside: Todoist's REST task carries a
/// single `assignee_id`, so its adapter takes `assignees[0]` and warns about
/// the rest. Before this capability existed the editor happily offered a
/// second person, the save reported success, and the name was gone at the next
/// refresh. Anything not listed here declares nothing and is read as `none`,
/// which hides the picker rather than crediting an adapter with an ability
/// whose failure is silent.
///
/// Keyed by plugin id, for the same reason as [`SYNC_ADAPTER_KINDS`].
const TASK_ASSIGNMENT: &[(&str, plugin_core::TaskAssignment)] = &[
    (
        "com.aperio.cal-adapter-todoist",
        plugin_core::TaskAssignment::Single,
    ),
    (
        "com.aperio.cal-adapter-vikunja",
        plugin_core::TaskAssignment::Multiple,
    ),
];

#[test]
fn the_adapters_that_can_assign_say_how_many_people_they_hold() {
    let manifests = declared_manifests();
    // Whichever of them this build links. A build that ships neither has
    // nothing to say here and is not wrong for it — the question is what an
    // adapter promises, not which adapters exist.
    for (id, expected) in TASK_ASSIGNMENT {
        let Some((_, manifest)) = manifests.iter().find(|(seen, _)| seen == id) else {
            continue;
        };
        assert_eq!(
            manifest.tasks.task_assignment, *expected,
            "{id} no longer declares the assignment limit the editor gates on",
        );
    }
}

#[test]
fn an_adapter_that_says_nothing_cannot_assign() {
    // The default has to be the CAUTIOUS one. Crediting a silent manifest with
    // multi-assignment is how the picker came to offer a choice the source
    // could not keep.
    let manifests = declared_manifests();
    let listed: Vec<&str> = TASK_ASSIGNMENT.iter().map(|(id, _)| *id).collect();
    let mut checked = 0;
    for (id, manifest) in &manifests {
        if listed.contains(&id.as_str()) {
            continue;
        }
        assert_eq!(
            manifest.tasks.task_assignment,
            plugin_core::TaskAssignment::None,
            "{id} declares an assignment mode but is not listed in TASK_ASSIGNMENT",
        );
        checked += 1;
    }
    // Exact, not a floor: every declared adapter is either one this build links
    // from the list above or one that says nothing, so the two have to account
    // for all of them. Counting the listed ones that are actually DECLARED, so
    // a build that links neither still adds up.
    let listed_and_linked = manifests
        .iter()
        .filter(|(id, _)| listed.contains(&id.as_str()))
        .count();
    assert_eq!(
        checked + listed_and_linked,
        manifests.len(),
        "{checked} silent + {listed_and_linked} listed does not account for all {} \
         declared adapters",
        manifests.len(),
    );
}
