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
        name: m.name.clone(),
        plugin_id: m.id.clone(),
        owns_containers: m.has_data_family(),
        // It needs no connect form and signs in nowhere: it is already there.
        declares_account_schema: false,
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
        assert!(!local.can_sync, "the built-in store is not a sync target");
        assert!(
            !local.offered,
            "there is exactly one, and it already exists"
        );
        assert!(!local.declares_account_schema, "it needs no connect form");
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
