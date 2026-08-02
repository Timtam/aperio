//! Every `plugin.json` in the tree survives the loader's own parser.
//!
//! `PluginManifest::from_bytes` is what the host runs against a manifest before
//! it will load the plugin, and it is more than a serde call: it validates the
//! declared `account` block. A manifest that serialises fine but declares, say,
//! a secret field with no slot is rejected there — at load time, on a user's
//! machine, as a plugin that silently fails to appear.
//!
//! Nothing guarded that until now. The bundled-plugin test only covers plugins
//! this build actually stages, so an unbundled one (Zoom today) could carry a
//! broken manifest indefinitely, and the six sync manifests carry `account`
//! blocks written by hand.
//!
//! Deliberately walks the directory rather than listing crates: a manifest
//! added tomorrow is covered without anyone remembering this file.

use std::fs;
use std::path::PathBuf;

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("host-plugins lives under crates/")
        .to_path_buf()
}

#[test]
fn every_plugin_manifest_in_the_tree_parses_and_validates() {
    let crates = crates_dir();
    let mut checked = Vec::new();

    for entry in fs::read_dir(&crates).expect("read crates/") {
        let path = entry.expect("dir entry").path().join("plugin.json");
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let manifest = plugin_core::PluginManifest::from_bytes(&bytes)
            .unwrap_or_else(|e| panic!("{} is not a loadable manifest: {e}", path.display()));

        // An id is what everything else keys on; an empty one would produce a
        // plugin the host can load but nothing can reference.
        assert!(
            !manifest.id.trim().is_empty(),
            "{} declares an empty id",
            path.display(),
        );
        checked.push(manifest.id);
    }

    // A path bug that found no manifests would otherwise pass silently.
    assert!(
        checked.len() >= 10,
        "only found {} manifests under {} — the walk is probably wrong",
        checked.len(),
        crates.display(),
    );

    checked.sort();
    let before = checked.len();
    checked.dedup();
    assert_eq!(
        checked.len(),
        before,
        "two plugins in the tree share an id; the loader keys on it",
    );
}

/// Every manifest in the tree, paired with the crate directory it came from.
fn manifests_in_tree() -> Vec<(String, plugin_core::PluginManifest)> {
    let mut found = Vec::new();
    for entry in fs::read_dir(crates_dir()).expect("read crates/") {
        let dir = entry.expect("dir entry").path();
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let path = dir.join("plugin.json");
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let manifest = plugin_core::PluginManifest::from_bytes(&bytes)
            .unwrap_or_else(|e| panic!("{} is not a loadable manifest: {e}", path.display()));
        found.push((name, manifest));
    }
    found
}

/// The six sync backends and the kind each one answers to, spelled out here so
/// a rename has to be made twice — once in the manifest, once in front of
/// somebody reading this list.
///
/// These strings are not internal. They are written into `accounts.adapter_kind`
/// and travel in the sync payload, so changing one orphans every account row a
/// user already has. `local_folder` in particular is not `local`: that name is
/// already taken by the built-in store, which is `AdapterKind::LOCAL` and
/// host-internal — an adapter claiming it would be unregisterable, and the two
/// frontends would each have a different idea of what the row meant.
const SYNC_ADAPTER_KINDS: &[(&str, &str)] = &[
    ("sync-adapter-local-plugin", "local_folder"),
    ("sync-adapter-webdav-plugin", "webdav"),
    ("sync-adapter-sftp-plugin", "sftp"),
    ("sync-adapter-ftp-plugin", "ftp"),
    ("sync-adapter-dropbox-plugin", "dropbox"),
    ("sync-adapter-googledrive-plugin", "googledrive"),
];

/// The sync adapters are published: each declares the kind above, and nothing
/// else in the tree answers to it.
///
/// This replaces the tripwire that used to fail the moment any of them declared
/// a kind at all. `adapter_kind` is what binds a schema to the Add-account
/// picker and to `schema_for_kind`, so declaring it is the act of publishing;
/// the two things that had to be true first are the host-key pin
/// (`AccountHostKeyPin`, which the SFTP schema now carries, so the plugin no
/// longer reads an empty pin as "trust whatever answers") and sync restoring
/// from an account rather than from `user_prefs`.
///
/// What is left to guard is the naming itself, which is why this test kept the
/// walk: a seventh sync adapter added tomorrow lands here without anyone
/// remembering this file, and it has to be given a kind and a label before it
/// can ship.
#[test]
fn the_sync_adapters_declare_the_kinds_the_hosts_resolve() {
    let manifests = manifests_in_tree();
    let mut seen = 0usize;

    for (name, manifest) in &manifests {
        if !name.starts_with("sync-adapter-") {
            continue;
        }
        seen += 1;
        assert!(manifest.account.is_some(), "{name} lost its account schema");

        let expected = SYNC_ADAPTER_KINDS
            .iter()
            .find(|(dir, _)| dir == name)
            .map(|(_, kind)| *kind)
            .unwrap_or_else(|| {
                panic!(
                    "{name} is a sync adapter this test has never heard of. Give it a \
                     kind, add it to SYNC_ADAPTER_KINDS, and give that kind a label in \
                     both locale files — otherwise it reaches a screen reader as the \
                     raw key string.",
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

    // A test that passes because it found nothing is not a test.
    assert_eq!(
        seen,
        SYNC_ADAPTER_KINDS.len(),
        "expected {} sync adapter manifests, walked {seen}",
        SYNC_ADAPTER_KINDS.len(),
    );
}

/// One kind, one adapter — across the whole tree, bundled or not.
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
fn no_two_adapters_in_the_tree_share_a_kind() {
    let mut by_kind: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (name, manifest) in manifests_in_tree() {
        if let Some(kind) = manifest.adapter_kind {
            by_kind.entry(kind).or_default().push(name);
        }
    }

    assert!(
        !by_kind.is_empty(),
        "no manifest in the tree declares a kind — the walk is probably wrong",
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

    // `local` is the built-in store and `device_calendar` the native bridge;
    // both are recognised by value in `host_core::accounts::AdapterKind` and
    // resolve to no plugin at all.
    for reserved in ["local", "device_calendar"] {
        assert!(
            !by_kind.contains_key(reserved),
            "{reserved} is host-internal — an adapter claiming it can never be \
             registered, and {:?} tried",
            by_kind.get(reserved),
        );
    }
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
fn an_adopted_kind_has_no_other_claimant_in_the_tree() {
    let manifests = manifests_in_tree();
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

    assert!(problems.is_empty(), "{}", problems.join("; "));
}

/// So it is asserted here, against the shipped locale files, rather than left to
/// be noticed.
#[test]
fn every_declared_kind_is_named_in_both_locales() {
    let repo_root = crates_dir()
        .parent()
        .expect("crates/ lives in the repo root")
        .to_path_buf();

    let mut kinds: Vec<String> = manifests_in_tree()
        .into_iter()
        .filter_map(|(_, m)| m.adapter_kind)
        .collect();
    kinds.sort();
    kinds.dedup();
    assert!(!kinds.is_empty(), "no kinds found — the walk is wrong");

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
