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
        PluginManifest::from_bytes(include_bytes!("../../cal-adapter-local/plugin.json"))
            .expect("the built-in store's manifest parses and validates")
    })
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
pub fn builtin_adapter_kinds() -> Vec<AdapterKindInfo> {
    let m = local_manifest();
    vec![AdapterKindInfo {
        kind: m
            .adapter_kind
            .clone()
            .expect("the built-in store's manifest declares its kind"),
        // Never. There is exactly one built-in store, it is created during
        // bootstrap, and it cannot be deleted — so an Add-account picker must
        // not offer to make a second. This is the same flag an adopted kind
        // uses, and for the same underlying reason: the entry describes an
        // account that exists, not one that can be created.
        offered: false,
        // …but choosable. It is the one storage backend that needs no account
        // created first, because the account is the one every device already
        // has. Without this the sync form would have dropped it and "a folder
        // on this device" would have stopped being an answer at onboarding.
        implicit: true,
        name: m.name.clone(),
        plugin_id: m.id.clone(),
        owns_containers: m.has_data_family(),
        // It DOES declare a schema now — one field, the folder its data is
        // mirrored into when this account is chosen as the storage. Nothing is
        // asked at creation time (there is nothing to create), so the flag is
        // read off the manifest rather than pinned false.
        declares_account_schema: m.account.is_some(),
        // It signs in nowhere: it is already there.
        declares_oauth: false,
        holds_data: m
            .capabilities
            .iter()
            .any(|c| *c != plugin_core::capability::Capability::Sync),
        can_sync: m
            .capabilities
            .contains(&plugin_core::capability::Capability::Sync),
    }]
}

/// The schema that describes a built-in kind's storage settings.
///
/// The built-in store declares that it can hold the dataset AND implements it:
/// [`cal_adapter_local::LocalFsSyncAdapter`] is a `SyncAdapter` like any other,
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
    Ok(std::sync::Arc::new(
        cal_adapter_local::LocalFsSyncAdapter::new(cfg.remote_root.trim()),
    ))
}

/// The plugin kinds plus the built-in ones, sorted and deduplicated the same
/// way `adapter_kinds()` does its own.
///
/// The one call both hosts make. Keeping the merge here rather than in each
/// host is the point: a third built-in adapter appears on both platforms by
/// being added above.
pub fn all_adapter_kinds(manager: &plugin_core::PluginManager) -> Vec<AdapterKindInfo> {
    let mut kinds = manager.adapter_kinds();
    for builtin in builtin_adapter_kinds() {
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

    /// The declaration says what the built-in store actually is, read off the
    /// shipped manifest rather than restated here.
    #[test]
    fn the_built_in_store_declares_itself() {
        let kinds = builtin_adapter_kinds();
        assert_eq!(kinds.len(), 1);
        let local = &kinds[0];
        assert_eq!(local.kind, "local");
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
            crate::accounts::AdapterKind::new(&builtin_adapter_kinds()[0].kind).is_host_internal()
        );
    }

    /// Merging appends without disturbing the plugin entries, and the result
    /// stays sorted — both frontends render this list in order.
    #[test]
    fn the_merged_list_is_sorted_and_contains_both_halves() {
        let manager = plugin_core::PluginManager::new("0.1.0");
        let merged = all_adapter_kinds(&manager);
        assert!(merged.iter().any(|k| k.kind == "local"));
        let mut sorted = merged.clone();
        sorted.sort_by(|a, b| a.kind.cmp(&b.kind));
        assert_eq!(
            merged.iter().map(|k| &k.kind).collect::<Vec<_>>(),
            sorted.iter().map(|k| &k.kind).collect::<Vec<_>>(),
        );
    }
}
