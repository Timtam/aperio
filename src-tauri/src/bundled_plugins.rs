//! Wire the seven bundled cal-adapter plugins into a fresh
//! [`plugin_core::PluginManager`] at host startup (DESIGN.md
//! §20.6 + §22.2).
//!
//! ## Why `register_static` instead of dlopen
//!
//! The §22.2 release pipeline says bundled plugins live in
//! `plugins/bundled/*.{dll,dylib,so}` next to the Aperio binary
//! and the manager dlopens them at startup. That requires a
//! build-time pipeline that copies each plugin's cdylib into the
//! release artifact + a runtime path resolution dance. Neither
//! exists yet — they're tracked as a separate phase.
//!
//! In the meantime the same effect is achieved by linking each
//! plugin crate directly into the host binary and calling
//! [`plugin_core::PluginManager::register_static`] with the
//! plugin's typed `build_descriptor()` accessor. The Tauri
//! command surface sees the same `Arc<PluginManager>` regardless
//! of which path produced it, so the eventual dlopen flip-over
//! is a single-file change here without touching the registry or
//! the commands.
//!
//! ## Why the typed accessors (not aperio_plugin_create)
//!
//! Each plugin's `declare_lifecycle!` macro emits the C-ABI
//! `aperio_plugin_create` / `aperio_plugin_destroy` symbols
//! behind a `cdylib-exports` cargo feature (default-on). The
//! host disables that feature on its plugin deps so the 7
//! `#[no_mangle]` symbols don't collide at link time. The macro
//! also always emits typed Rust accessors (`build_descriptor()`
//! + `DESTROY_FN` constant), which the host uses below.

use std::sync::Arc;

use plugin_core::{
    manifest::PluginManifest, Capability, PluginError, PluginManager, PluginType,
    ABI_VERSION,
};
use tracing::warn;

/// Build a [`PluginManager`] with every bundled cal-adapter
/// plugin registered statically. Errors registering any
/// individual plugin are logged + skipped — one broken plugin
/// must NEVER prevent the app from coming up.
pub fn build_manager(app_version: &str) -> Arc<PluginManager> {
    let manager = PluginManager::new(app_version);

    register_plugin(
        &manager,
        cal_adapter_caldav_manifest(),
        unsafe { cal_adapter_caldav_plugin::build_descriptor() },
        cal_adapter_caldav_plugin::DESTROY_FN,
    );
    register_plugin(
        &manager,
        cal_adapter_ical_manifest(),
        unsafe { cal_adapter_ical_plugin::build_descriptor() },
        cal_adapter_ical_plugin::DESTROY_FN,
    );
    register_plugin(
        &manager,
        cal_adapter_google_manifest(),
        unsafe { cal_adapter_google_plugin::build_descriptor() },
        cal_adapter_google_plugin::DESTROY_FN,
    );
    register_plugin(
        &manager,
        cal_adapter_microsoft_graph_manifest(),
        unsafe { cal_adapter_microsoft_graph_plugin::build_descriptor() },
        cal_adapter_microsoft_graph_plugin::DESTROY_FN,
    );
    register_plugin(
        &manager,
        cal_adapter_ews_manifest(),
        unsafe { cal_adapter_ews_plugin::build_descriptor() },
        cal_adapter_ews_plugin::DESTROY_FN,
    );
    register_plugin(
        &manager,
        cal_adapter_vikunja_manifest(),
        unsafe { cal_adapter_vikunja_plugin::build_descriptor() },
        cal_adapter_vikunja_plugin::DESTROY_FN,
    );
    register_plugin(
        &manager,
        cal_adapter_todoist_manifest(),
        unsafe { cal_adapter_todoist_plugin::build_descriptor() },
        cal_adapter_todoist_plugin::DESTROY_FN,
    );

    Arc::new(manager)
}

fn register_plugin(
    manager: &PluginManager,
    manifest: PluginManifest,
    descriptor: *mut plugin_core::AperioPlugin,
    destroy_fn: unsafe extern "C" fn(*mut plugin_core::AperioPlugin),
) {
    let id = manifest.id.clone();
    if let Err(err) = manager.register_static(manifest, descriptor, destroy_fn) {
        match err {
            PluginError::AbiMismatch { .. } | PluginError::AppTooOld { .. } => {
                warn!(plugin_id = %id, ?err, "bundled plugin rejected on register");
            }
            _ => warn!(plugin_id = %id, ?err, "bundled plugin failed to register"),
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Manifests
//
// Every bundled plugin ships its own `plugin.json` next to the
// cdylib for the dlopen path, and the values below must match
// exactly. Kept in code rather than parsing the JSON because
// the host doesn't have a stable working directory at startup
// (the manifests live inside the source tree, not next to the
// running binary).
// ─────────────────────────────────────────────────────────────

fn cal_adapter_caldav_manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.cal-adapter-caldav".into(),
        name: "Aperio CalDAV".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::CalendarAdapter,
        capabilities: vec![
            Capability::Calendar,
            Capability::Tasks,
            Capability::Contacts,
        ],
        abi_version: ABI_VERSION,
        min_app_version: "0.1.0".into(),
        author: Some("Aperio Contributors".into()),
        description: Some("Bundled".into()),
        signed: false,
    }
}

fn cal_adapter_ical_manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.cal-adapter-ical".into(),
        name: "Aperio iCal Feed".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::CalendarAdapter,
        capabilities: vec![Capability::Calendar],
        abi_version: ABI_VERSION,
        min_app_version: "0.1.0".into(),
        author: Some("Aperio Contributors".into()),
        description: Some("Bundled".into()),
        signed: false,
    }
}

fn cal_adapter_google_manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.cal-adapter-google".into(),
        name: "Aperio Google".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::CalendarAdapter,
        capabilities: vec![
            Capability::Calendar,
            Capability::Tasks,
            Capability::Contacts,
        ],
        abi_version: ABI_VERSION,
        min_app_version: "0.1.0".into(),
        author: Some("Aperio Contributors".into()),
        description: Some("Bundled".into()),
        signed: false,
    }
}

fn cal_adapter_microsoft_graph_manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.cal-adapter-microsoft-graph".into(),
        name: "Aperio Microsoft 365".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::CalendarAdapter,
        capabilities: vec![
            Capability::Calendar,
            Capability::Tasks,
            Capability::Contacts,
        ],
        abi_version: ABI_VERSION,
        min_app_version: "0.1.0".into(),
        author: Some("Aperio Contributors".into()),
        description: Some("Bundled".into()),
        signed: false,
    }
}

fn cal_adapter_ews_manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.cal-adapter-ews".into(),
        name: "Aperio Exchange (EWS)".into(),
        version: "0.1.0".into(),
        plugin_type: PluginType::CalendarAdapter,
        capabilities: vec![
            Capability::Calendar,
            Capability::Tasks,
            Capability::Contacts,
        ],
        abi_version: ABI_VERSION,
        min_app_version: "0.1.0".into(),
        author: Some("Aperio Contributors".into()),
        description: Some("Bundled".into()),
        signed: false,
    }
}

fn cal_adapter_vikunja_manifest() -> PluginManifest {
    PluginManifest {
        id: "com.aperio.cal-adapter-vikunja".into(),
        name: "Aperio Vikunja".into(),
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

fn cal_adapter_todoist_manifest() -> PluginManifest {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_manager_registers_all_seven_plugins() {
        let mgr = build_manager(env!("CARGO_PKG_VERSION"));
        // The 7 cal-adapter plugins all wired up + addressable
        // by id. A missing one would surface as None here, which
        // means the registry would fail to bootstrap that
        // account kind.
        for id in [
            "com.aperio.cal-adapter-caldav",
            "com.aperio.cal-adapter-ical",
            "com.aperio.cal-adapter-google",
            "com.aperio.cal-adapter-microsoft-graph",
            "com.aperio.cal-adapter-ews",
            "com.aperio.cal-adapter-vikunja",
            "com.aperio.cal-adapter-todoist",
        ] {
            assert!(mgr.get(id).is_some(), "plugin {id} not registered");
        }
        assert_eq!(mgr.len(), 7, "exactly 7 cal-adapter plugins expected");
    }
}
