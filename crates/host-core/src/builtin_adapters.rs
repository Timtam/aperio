//! The adapters the host implements itself, declared the way every other
//! adapter declares itself.
//!
//! ## Why a manifest for something that is not a plugin
//!
//! The built-in store is written as an adapter — it implements the same
//! `cal_core` traits as CalDAV or Google — but it is linked in rather than
//! loaded, because it is the hot path: every event, task and contact read goes
//! through it, and routing those over the plugin ABI would put a JSON
//! round-trip on each one.
//!
//! That decision is about HOW IT IS CALLED. It said nothing about how it should
//! be DESCRIBED, and the two got conflated: because no manifest declared it,
//! `PluginManager::adapter_kinds()` did not list it, and every surface that
//! reads that list had to know about the built-in store by name. Both
//! frontends carried a `HOST_INTERNAL_KINDS` set; the desktop sidebar kept an
//! `account.id === 'local' ||` beside its capability filter, because the
//! capability filter could not answer for the one account every user has.
//!
//! So the declaration is separated from the calling convention. The manifest is
//! a real `plugin.json`, in the crate, parsed and validated by the same code as
//! every other — the tree tests walk it — and this module turns it into the
//! same [`AdapterKindInfo`] the plugin manager produces. What it does not have
//! is a vtable, and the host goes on calling the typed adapter directly.
//!
//! Promoting it to a full plugin later is then adding a vtable, not inventing a
//! description.

use plugin_core::manifest::{AdapterKindInfo, PluginManifest};
use std::sync::OnceLock;

/// The built-in store's declaration, parsed once.
///
/// `expect` rather than a fallible return: the bytes are compiled in, so a
/// malformed manifest is a build-time mistake that every test in this crate
/// would hit. There is no runtime path where it can be absent.
fn local_manifest() -> &'static PluginManifest {
    static MANIFEST: OnceLock<PluginManifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        PluginManifest::from_bytes(adapter_local::MANIFEST)
            .expect("the built-in store's manifest parses and validates")
    })
}

/// The device adapter's declaration, parsed once.
///
/// Same arrangement as the built-in store, for the same reason: it is written
/// as an adapter and CALLED directly — the mobile host injects a native
/// provider and holds the typed adapter — but until it had a manifest, its kind
/// existed only as the string `"device_calendar"` repeated in four places in
/// `cal-ffi`. Declaring it puts the name in one file that the tree tests walk.
fn device_manifest() -> &'static PluginManifest {
    static MANIFEST: OnceLock<PluginManifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        PluginManifest::from_bytes(adapter_device_calendar::MANIFEST)
            .expect("the device adapter's manifest parses and validates")
    })
}

/// Every adapter the host implements itself, as it declares itself.
///
/// The counterpart to `PluginManager::all()` for the adapters that are linked
/// rather than loaded. Between the two, a caller has the whole set of adapters
/// a build ships without having to know which came from a shared library — and,
/// more to the point, without walking `crates/` to find out. A directory walk
/// answers "which crates happen to sit next to this one"; this answers "which
/// adapters does this build declare", which is the question every caller
/// actually has.
///
/// Whole manifests rather than [`AdapterKindInfo`]: a caller checking what an
/// adapter promises — its capabilities, its account schema, what it says about
/// assigning a task — needs the declaration, not the summary the pickers use.
pub fn builtin_manifests() -> Vec<&'static PluginManifest> {
    vec![local_manifest(), device_manifest()]
}

/// The adapter kind the phone's own calendars and reminders answer to.
///
/// Read from the manifest rather than written here. It is persisted in
/// `accounts.adapter_kind`, so it is not an internal name that can be changed
/// on a whim — which is exactly why it should have one home.
pub fn device_calendar_kind() -> &'static str {
    device_manifest()
        .adapter_kind
        .as_deref()
        .expect("the device adapter's manifest declares its kind")
}

/// The device adapter as a kind the frontends can describe.
///
/// Deliberately NOT part of [`all_adapter_kinds`] — see [`builtin_adapter_kinds`].
/// The mobile accounts screen asks for this one by name, because whether it can
/// be offered at all is a question about the operating system and a permission,
/// not about which plugins are loaded.
pub fn device_adapter_kind_info(lang: &str) -> AdapterKindInfo {
    let m = device_manifest();
    let kind = device_calendar_kind().to_string();
    let (name, short_name) = plugin_core::resolve_kind_name(m, &m.strings, &kind, lang);
    AdapterKindInfo {
        kind,
        name,
        short_name,
        plugin_id: m.id.clone(),
        // It holds no dataset: the phone's own store is not somewhere Aperio
        // can put its sync payload.
        can_sync: false,
        // Offered, but only where it exists and only once the OS has said yes.
        // The mobile host gates both; nothing here can.
        offered: true,
        // Not implicit: unlike the built-in store there is no such account
        // until the user grants access, and on a desktop there can be none.
        implicit: false,
        owns_containers: m.has_data_family(),
        // Its account carries no fields at all: the "connect form" is an OS
        // permission prompt. So there is no schema to drive one from.
        declares_account_schema: m.account.is_some(),
        // It signs in nowhere — the OS decides.
        declares_oauth: false,
        holds_data: m
            .capabilities
            .iter()
            .any(|c| *c != plugin_core::capability::Capability::Sync),
    }
}

/// One [`AdapterKindInfo`] per adapter the host implements itself.
///
/// Appended to `PluginManager::adapter_kinds()` by both hosts, so a caller sees
/// one list and does not have to know which entries came from a shared library.
///
/// `device_calendar` is deliberately NOT here. It is not built in the same
/// sense — it is a bridge to whatever the OS provides, it exists only on the
/// phone platforms, and it is ADDED by granting a permission rather than
/// existing from the first launch. The mobile accounts screen offers it on its
/// own terms, which is a different question from the one this list answers.
pub fn builtin_adapter_kinds(lang: &str) -> Vec<AdapterKindInfo> {
    let m = local_manifest();
    let own = m
        .adapter_kind
        .clone()
        .expect("the built-in store's manifest declares its kind");
    // Its adopted kinds deliberately do NOT ride along. `local_folder` is the
    // folder sync's old name, and a row written under it still resolves — but
    // NAMING such a row is `kind_name_for`'s job now, and it reads the manifest
    // directly. Putting the kind in this list instead would hand it the store's
    // own capability flags, and `holds_data` is what the sidebar filters
    // account branches on: a retired sync kind would sprout a permanently empty
    // branch that can never fill.
    let info = |kind: String, offered: bool, implicit: bool| {
        let (name, short_name) = plugin_core::resolve_kind_name(m, &m.strings, &kind, lang);
        AdapterKindInfo {
            kind,
            offered,
            implicit,
            name,
            short_name,
            plugin_id: m.id.clone(),
            owns_containers: m.has_data_family(),
            declares_account_schema: m.account.is_some(),
            declares_oauth: false,
            holds_data: m
                .capabilities
                .iter()
                .any(|c| *c != plugin_core::capability::Capability::Sync),
            can_sync: m
                .capabilities
                .contains(&plugin_core::capability::Capability::Sync),
        }
    };
    vec![info(
        own,
        // Never offered. There is exactly one built-in store, it is created
        // during bootstrap, and it cannot be deleted — so an Add-account picker
        // must not offer to make a second. This is the same flag an adopted
        // kind uses, and for the same underlying reason: the entry describes an
        // account that exists, not one that can be created.
        false,
        // …but choosable, which is what `implicit` says. It is the one storage
        // backend that needs no account created first, because the account is
        // the one every device already has. Without this the sync form would
        // have dropped it and "a folder on this device" would have stopped
        // being an answer at onboarding.
        true,
    )]
}

/// The schema that describes a built-in kind's storage settings.
///
/// The built-in store declares that it can hold the dataset AND implements it:
/// [`adapter_local::LocalFsSyncAdapter`] is a `SyncAdapter` like any other,
/// so nothing here goes through the plugin ABI. It used to name a plugin id and
/// let the manager open it — a seam that existed only because the folder mirror
/// was still a separate crate behind a separate plugin. It is neither now.
///
/// `local_folder` answers too. It is the kind the folder sync carried as its
/// own before the merge, adopted by the built-in store's manifest, so a row
/// written back then still resolves — and, like every adoption, without
/// anything persisted having to change.
///
/// The returned id is what [`open_sync`] recognises. It is deliberately not a
/// plugin id: no plugin serves it, and a caller that took it to the plugin
/// manager would find nothing.
pub fn sync_plugin_for(
    adapter_kind: &str,
) -> Option<(String, plugin_core::account_schema::AccountSchema)> {
    let m = local_manifest();
    if !m.serves_kind(adapter_kind) {
        return None;
    }
    Some((BUILTIN_SYNC_ID.to_string(), m.account.clone()?))
}

/// The id [`sync_plugin_for`] hands back, and the only value [`open_sync`]
/// answers to.
///
/// Kept as the built-in store's own plugin id rather than the retired folder
/// plugin's: it names the adapter that DECLARES the capability, which is the
/// one a log line or an error should mention.
pub const BUILTIN_SYNC_ID: &str = "com.aperio.cal-adapter-local";

/// Open the built-in store's sync half, or `None` when the id is not its own.
///
/// Both hosts call this from their `SyncPlugins::open` before consulting the
/// plugin manager. Linked in, so there is no vtable, no cdylib, and no
/// serialisation on the path — the same arrangement the store's calendar half
/// has always had.
pub fn open_sync(
    plugin_id: &str,
    config_json: &str,
) -> Option<Result<std::sync::Arc<dyn sync_core::SyncAdapter>, String>> {
    if plugin_id != BUILTIN_SYNC_ID {
        return None;
    }
    Some(open_sync_inner(config_json))
}

fn open_sync_inner(
    config_json: &str,
) -> Result<std::sync::Arc<dyn sync_core::SyncAdapter>, String> {
    #[derive(serde::Deserialize)]
    struct Config {
        #[serde(default)]
        remote_root: String,
    }
    let cfg: Config =
        serde_json::from_str(config_json).map_err(|e| format!("malformed init config: {e}"))?;
    // The one thing that can be missing, and the one the user can fix. It is a
    // device-local field, so an account restored from another device arrives
    // without it — saying so beats an adapter pointed at the current directory.
    if cfg.remote_root.trim().is_empty() {
        return Err(
            "this device has no folder set for the built-in store; choose one in the sync \
             settings"
                .to_string(),
        );
    }
    Ok(std::sync::Arc::new(adapter_local::LocalFsSyncAdapter::new(
        cfg.remote_root.trim(),
    )))
}

/// What to call the adapter behind one `adapter_kind`, resolved in `lang`:
/// the descriptive name and the compact one.
///
/// This is what an ACCOUNT ROW is labelled from, and it is deliberately not the
/// same lookup as [`all_adapter_kinds`]. That one answers "which adapters can
/// this build offer" — a list, fetched once, filtered to what is usable. A row
/// asks something narrower and harder: "what is THIS account's adapter called",
/// and it has to have an answer every time the row is drawn.
///
/// So this reaches further than the picker's list does, in two directions:
///
/// - **Built-in first.** The store and the device adapter are never plugins;
///   their manifests are compiled in and cannot be absent.
/// - **`any_plugin_for_adapter_kind`, not `plugin_for_adapter_kind`.** The
///   second hides DISABLED plugins, and a disabled plugin's accounts are
///   exactly the rows that stay on screen saying "plugin missing". The manager
///   still holds the manifest; declining to read the name out of it would put a
///   machine string in front of the reader at the one moment they most need a
///   word they recognise.
///
/// - **Failed loads last.** A plugin whose library would not open parsed its
///   manifest on the way, and the manager kept it. A name needs nothing else.
///
/// The last stop is the kind itself. It is reached when the plugin is genuinely
/// gone — uninstalled, its manifest with it — and then nothing anywhere knows
/// what that adapter called itself. The row says so in its own words.
pub fn kind_name_for(
    manager: &plugin_core::PluginManager,
    adapter_kind: &str,
    lang: &str,
) -> (String, String) {
    for manifest in builtin_manifests() {
        if manifest.serves_kind(adapter_kind) {
            return plugin_core::resolve_kind_name(manifest, &manifest.strings, adapter_kind, lang);
        }
    }
    if let Some(plugin) = manager.any_plugin_for_adapter_kind(adapter_kind) {
        let strings = plugin_core::PluginManager::strings_for(&plugin, lang);
        return plugin_core::resolve_kind_name(&plugin.manifest, &strings, adapter_kind, lang);
    }
    // A plugin whose LIBRARY would not load still parsed its manifest — the
    // manager keeps it on the failed-load list for the plugins panel to explain
    // — and a manifest is all a name needs. This is the desktop's ordinary
    // breakage: a quarantined DLL, an ABI refusal after an update, a bundled
    // adapter missing from the staged directory. The account rows stay on
    // screen saying "plugin missing", and they can say it about something with
    // a name.
    for failed in manager.failed_loads() {
        if let Some(manifest) = failed.manifest {
            if manifest.serves_kind(adapter_kind) {
                let strings = manifest.strings.clone();
                return plugin_core::resolve_kind_name(&manifest, &strings, adapter_kind, lang);
            }
        }
    }
    (adapter_kind.to_string(), adapter_kind.to_string())
}

/// The plugin kinds plus the built-in ones, sorted and deduplicated the same
/// way `adapter_kinds()` does its own.
///
/// The one call both hosts make. Keeping the merge here rather than in each
/// host is the point: a third built-in adapter appears on both platforms by
/// being added above.
pub fn all_adapter_kinds(manager: &plugin_core::PluginManager, lang: &str) -> Vec<AdapterKindInfo> {
    let mut kinds = manager.adapter_kinds(lang);
    for builtin in builtin_adapter_kinds(lang) {
        // A plugin claiming a built-in kind cannot be registered anyway
        // (`AdapterKind::is_host_internal` short-circuits it), so the built-in
        // declaration is the truth and a colliding entry is dropped rather
        // than left to sort against it.
        kinds.retain(|k| k.kind != builtin.kind);
        kinds.push(builtin);
    }
    kinds.sort_by(|a, b| a.kind.cmp(&b.kind));
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::AdapterKind;

    /// A plugin whose library will not open still names its own accounts.
    ///
    /// The desktop populates its manager purely by `dlopen`, so a quarantined
    /// DLL, an ABI refusal after an update, or a staging directory that was
    /// never filled all end the same way: the manifest parses, the library does
    /// not open, and the plugin is not registered. Those accounts stay on
    /// screen — they are the rows that say "plugin missing" — and this is what
    /// keeps them from saying it about `caldav` instead of about CalDAV.
    #[test]
    fn a_plugin_whose_library_would_not_open_still_names_its_accounts() {
        let dir = tempfile::tempdir().expect("temp dir");
        let plugin_dir = dir.path().join("com.aperio.cal-adapter-caldav");
        std::fs::create_dir_all(&plugin_dir).expect("plugin dir");
        // The manifest as it ships, and no library beside it.
        std::fs::write(
            plugin_dir.join("plugin.json"),
            adapter_caldav_plugin::MANIFEST,
        )
        .expect("write manifest");

        let manager = plugin_core::PluginManager::new("0.1.0");
        let errors = manager.scan_dir(dir.path());
        assert!(
            !errors.is_empty(),
            "the library is missing, so the load fails"
        );
        assert_eq!(manager.len(), 0, "nothing was registered");

        let (name, short) = kind_name_for(&manager, "caldav", "de");
        assert_ne!(name, "caldav", "a failed load still parsed its manifest");
        assert_eq!(short, "CalDAV");
    }

    /// The two host-internal kinds are persisted in `accounts.adapter_kind` and
    /// declared in a manifest, and the two spellings must be the same one.
    ///
    /// `AdapterKind`'s constants are what the rest of the core compares
    /// against; the manifests are where a rename would be made. Nothing else
    /// connects them, so this does — a manifest edited without the constant
    /// (or the reverse) orphans every account row a user already has, and
    /// would otherwise fail somewhere far away and much later.
    #[test]
    fn the_host_internal_kinds_match_their_manifests() {
        assert_eq!(
            local_manifest().adapter_kind.as_deref(),
            Some(AdapterKind::LOCAL),
            "the built-in store's manifest and AdapterKind::LOCAL disagree",
        );
        assert_eq!(
            device_calendar_kind(),
            AdapterKind::DEVICE_CALENDAR,
            "the device adapter's manifest and AdapterKind::DEVICE_CALENDAR disagree",
        );
        // And both are recognised as host-internal, which is what keeps their
        // accounts off the sync log.
        assert!(AdapterKind::new(AdapterKind::LOCAL).is_host_internal());
        assert!(AdapterKind::new(device_calendar_kind()).is_host_internal());
    }

    /// The device adapter is describable like any other, and still not in the
    /// built-in list — those are two different questions and the second one is
    /// about the operating system.
    #[test]
    fn the_device_adapter_declares_itself_without_joining_the_builtin_list() {
        let info = device_adapter_kind_info("en");
        assert_eq!(info.kind, "device_calendar");
        assert!(!info.can_sync, "the phone's own store holds no dataset");
        assert!(info.owns_containers, "it owns calendars and reminder lists");
        assert!(
            !info.implicit,
            "it exists only once access has been granted"
        );
        assert!(
            !builtin_adapter_kinds("en")
                .iter()
                .any(|k| k.kind == info.kind),
            "device_calendar must not appear in the built-in list — it is \
             platform-conditional and permission-gated, and the mobile accounts \
             screen offers it on its own terms",
        );
    }

    /// The declaration says what the built-in store actually is, read off the
    /// shipped manifest rather than restated here.
    #[test]
    fn the_built_in_store_declares_itself() {
        let kinds = builtin_adapter_kinds("en");
        let local = kinds
            .iter()
            .find(|k| k.kind == "local")
            .expect("the built-in store declares its own kind");

        // Its adopted kind deliberately stays OUT of this list. A row written
        // under the folder sync's old name is named by `kind_name_for`, which
        // reads the manifest; listing the kind here would hand it the store's
        // capability flags, and `holds_data` is what the sidebar filters
        // account branches on.
        assert!(
            !kinds.iter().any(|k| k.kind == "local_folder"),
            "an adopted kind must not inherit the store's data flags",
        );
        assert_eq!(kinds.len(), 1, "the store's own kind, and nothing else");
        assert!(local.holds_data, "it holds calendars, tasks and contacts");
        assert!(local.owns_containers);
        assert!(
            local.can_sync,
            "folder sync folded in: the built-in account can hold the dataset",
        );
        assert!(
            !local.offered,
            "there is exactly one, and it already exists"
        );
        assert!(
            local.declares_account_schema,
            "one field — the folder its data is mirrored into",
        );
    }

    /// It is the kind `AdapterKind` already treats as the host's own. The two
    /// answers coming apart would mean a row that lists as a real adapter and
    /// is skipped by every behaviour gate.
    #[test]
    fn the_declared_kind_is_the_one_the_host_reserves() {
        assert!(
            crate::accounts::AdapterKind::new(&builtin_adapter_kinds("en")[0].kind)
                .is_host_internal()
        );
    }

    /// Merging appends without disturbing the plugin entries, and the result
    /// stays sorted — both frontends render this list in order.
    #[test]
    fn the_merged_list_is_sorted_and_contains_both_halves() {
        let manager = plugin_core::PluginManager::new("0.1.0");
        let merged = all_adapter_kinds(&manager, "en");
        assert!(merged.iter().any(|k| k.kind == "local"));
        let mut sorted = merged.clone();
        sorted.sort_by(|a, b| a.kind.cmp(&b.kind));
        assert_eq!(
            merged.iter().map(|k| &k.kind).collect::<Vec<_>>(),
            sorted.iter().map(|k| &k.kind).collect::<Vec<_>>(),
        );
    }
}
