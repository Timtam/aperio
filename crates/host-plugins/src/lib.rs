//! Static plugin registry — the no-dlopen path.
//!
//! The desktop host discovers adapters by scanning a directory and
//! `dlopen`-ing each bundled cdylib (see
//! [`plugin_core::manager::PluginManager::scan_dir`]). iOS forbids
//! `dlopen` of app-bundled code, so the mobile build instead links
//! every adapter's `-plugin` rlib straight into the app binary (now
//! possible since the `#[no_mangle]` C-ABI exports were split out
//! into the separate `*-cdylib` crates — those are never linked
//! here, so the rlibs carry only crate-mangled symbols and don't
//! collide).
//!
//! [`register_all_static`] is the static counterpart to `scan_dir`:
//! for each bundled plugin it parses the crate's own `plugin.json`
//! (`include_bytes!` — the SAME manifest the desktop ships, so the
//! two paths can't drift) and hands the crate-mangled
//! `build_descriptor()` + `DESTROY_FN` to
//! [`plugin_core::manager::PluginManager::register_static`].
//!
//! Interactive-auth / discover / probe-host-key fn-pointers are left
//! `None` by `register_static` for now; wiring the static OAuth path
//! (the typed-twin auth fns the `*-cdylib` crates already expose) is
//! deferred to the mobile OAuth phase.

#[cfg(feature = "static")]
use plugin_core::{manager::PluginManager, manifest::PluginManifest, PluginResult};

/// Register every bundled adapter plugin into `manager` via static
/// linkage instead of `dlopen`.
///
/// The manifests are embedded at compile time from each plugin
/// crate's `plugin.json`, so this list stays in lock-step with what
/// the desktop bundles. Registration is fail-fast: the first plugin
/// whose manifest is incompatible with `manager`'s app version (or
/// whose descriptor is NULL) returns its error and aborts the rest.
///
/// Only compiled when the `static` feature is enabled (the default);
/// with it off this crate links none of the `-plugin` rlibs.
#[cfg(feature = "static")]
pub fn register_all_static(manager: &PluginManager) -> PluginResult<()> {
    /// Parse one crate's embedded `plugin.json` + register its
    /// statically-linked descriptor. The optional third token wires the
    /// crate's auth hook (the crate-mangled typed twin `__aperio_*_impl`,
    /// which P0 left `pub` in each auth-capable `-plugin` crate) through
    /// `register_static_with_auth`, so OAuth / Autodiscover / TOFU adapters
    /// expose their handler when statically embedded:
    ///   `register!(crate, "path")`                    — no auth hook
    ///   `register!(crate, "path", interactive_auth)`  — OAuth (Google/MS/…)
    ///   `register!(crate, "path", discover)`          — Autodiscover (EWS)
    ///   `register!(crate, "path", probe_host_key)`    — TOFU (SFTP)
    macro_rules! register {
        ($plugin_crate:ident, $manifest_path:literal) => {{
            let manifest = PluginManifest::from_bytes(include_bytes!($manifest_path))?;
            // SAFETY: `build_descriptor` returns a freshly heap-
            // allocated descriptor; `register_static` takes ownership
            // and pairs it with `DESTROY_FN` for teardown on drop.
            let descriptor = unsafe { $plugin_crate::build_descriptor() };
            manager.register_static(manifest, descriptor, $plugin_crate::DESTROY_FN)?;
        }};
        ($plugin_crate:ident, $manifest_path:literal, interactive_auth) => {{
            let manifest = PluginManifest::from_bytes(include_bytes!($manifest_path))?;
            let descriptor = unsafe { $plugin_crate::build_descriptor() };
            manager.register_static_with_auth(
                manifest,
                descriptor,
                $plugin_crate::DESTROY_FN,
                Some($plugin_crate::__aperio_interactive_auth_impl),
                None,
                None,
            )?;
        }};
        ($plugin_crate:ident, $manifest_path:literal, discover) => {{
            let manifest = PluginManifest::from_bytes(include_bytes!($manifest_path))?;
            let descriptor = unsafe { $plugin_crate::build_descriptor() };
            manager.register_static_with_auth(
                manifest,
                descriptor,
                $plugin_crate::DESTROY_FN,
                None,
                Some($plugin_crate::__aperio_discover_impl),
                None,
            )?;
        }};
        ($plugin_crate:ident, $manifest_path:literal, probe_host_key) => {{
            let manifest = PluginManifest::from_bytes(include_bytes!($manifest_path))?;
            let descriptor = unsafe { $plugin_crate::build_descriptor() };
            manager.register_static_with_auth(
                manifest,
                descriptor,
                $plugin_crate::DESTROY_FN,
                None,
                None,
                Some($plugin_crate::__aperio_probe_host_key_impl),
            )?;
        }};
    }

    // Calendar / task adapters.
    register!(
        cal_adapter_caldav_plugin,
        "../../cal-adapter-caldav-plugin/plugin.json"
    );
    register!(
        cal_adapter_ical_plugin,
        "../../cal-adapter-ical-plugin/plugin.json"
    );
    register!(
        cal_adapter_google_plugin,
        "../../cal-adapter-google-plugin/plugin.json",
        interactive_auth
    );
    register!(
        cal_adapter_microsoft_graph_plugin,
        "../../cal-adapter-microsoft-graph-plugin/plugin.json",
        interactive_auth
    );
    register!(
        cal_adapter_ews_plugin,
        "../../cal-adapter-ews-plugin/plugin.json",
        discover
    );
    register!(
        cal_adapter_vikunja_plugin,
        "../../cal-adapter-vikunja-plugin/plugin.json"
    );
    register!(
        cal_adapter_todoist_plugin,
        "../../cal-adapter-todoist-plugin/plugin.json"
    );

    // Sync adapters.
    register!(
        sync_adapter_local_plugin,
        "../../sync-adapter-local-plugin/plugin.json"
    );
    register!(
        sync_adapter_webdav_plugin,
        "../../sync-adapter-webdav-plugin/plugin.json"
    );
    register!(
        sync_adapter_ftp_plugin,
        "../../sync-adapter-ftp-plugin/plugin.json"
    );
    register!(
        sync_adapter_sftp_plugin,
        "../../sync-adapter-sftp-plugin/plugin.json",
        probe_host_key
    );
    register!(
        sync_adapter_dropbox_plugin,
        "../../sync-adapter-dropbox-plugin/plugin.json",
        interactive_auth
    );
    register!(
        sync_adapter_googledrive_plugin,
        "../../sync-adapter-googledrive-plugin/plugin.json",
        interactive_auth
    );

    // Video-conferencing adapters.
    register!(
        vc_adapter_zoom_plugin,
        "../../vc-adapter-zoom-plugin/plugin.json"
    );
    register!(
        vc_adapter_teams_plugin,
        "../../vc-adapter-teams-plugin/plugin.json"
    );
    register!(
        vc_adapter_meet_plugin,
        "../../vc-adapter-meet-plugin/plugin.json"
    );
    register!(
        vc_adapter_webex_plugin,
        "../../vc-adapter-webex-plugin/plugin.json"
    );

    Ok(())
}

/// Number of plugins [`register_all_static`] registers. Lets callers
/// (and the integration test) assert the manager is fully populated
/// without hard-coding the count at every site.
#[cfg(feature = "static")]
pub const BUNDLED_PLUGIN_COUNT: usize = 17;
