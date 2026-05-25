//! Plugin-management Tauri commands (DESIGN.md §20.10).
//!
//! v1 is read-only: surfaces the loaded plugins + their
//! manifest metadata for the Settings → Plugins panel. The
//! enable/disable, uninstall, and install verbs are future
//! iterations — disable needs a per-plugin runtime gate on
//! [`PluginManager`]; uninstall needs the `plugin.uninstalled`
//! event-log surface (§20.10) plus a way to scrub the cdylib +
//! plugin.json from `plugins/user/`; install needs the
//! `.aperio` archive extractor (§20.7).

use std::sync::Arc;

use plugin_core::PluginManager;
use serde::Serialize;
use tauri::State;

use super::CommandResult;

/// Frontend-facing snapshot of one loaded plugin. Mirrors the
/// `plugin.json` manifest fields the Settings panel renders +
/// flags any optional named-symbol entry points the plugin
/// exports (interactive_auth / discover / probe_host_key).
#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    /// Stable reverse-DNS identifier (`com.aperio.cal-adapter-caldav`,
    /// `com.example.myplugin`, …). Drives `key` on the React side
    /// and is what every other plugin command takes as the
    /// referent.
    pub id: String,
    /// Human-readable display name from the manifest.
    pub name: String,
    /// SemVer string.
    pub version: String,
    /// Plugin-type wire string (`calendar-adapter`, `sync-adapter`,
    /// `videoconference-adapter`, `notification`, or whatever
    /// forward-compat tag the manifest declared).
    pub plugin_type: String,
    /// Sub-feature surface for calendar adapters (`["calendar",
    /// "tasks", "contacts"]`). Empty for plugin types that don't
    /// carry capabilities. Strings as-wired so a forward-compat
    /// future tag round-trips intact.
    pub capabilities: Vec<String>,
    /// ABI version the plugin was built against.
    pub abi_version: u32,
    /// Minimum Aperio version the manifest demands.
    pub min_app_version: String,
    /// Optional author label.
    pub author: Option<String>,
    /// Optional one-line description.
    pub description: Option<String>,
    /// `signed` manifest flag. Forward-compat only — host
    /// doesn't verify signatures yet, so `false` for every
    /// shipped plugin today.
    pub signed: bool,
    /// `true` iff the plugin exported an
    /// `aperio_plugin_interactive_auth` symbol — OAuth-style
    /// adapters (Google, Microsoft Graph, Dropbox, Google
    /// Drive). The UI uses this to surface a "manages user
    /// sign-in" badge.
    pub has_interactive_auth: bool,
    /// `true` iff the plugin exported an
    /// `aperio_plugin_discover` symbol — service-discovery
    /// adapters (EWS Autodiscover today).
    pub has_discover: bool,
    /// `true` iff the plugin exported an
    /// `aperio_plugin_probe_host_key` symbol — TOFU-transport
    /// adapters (SFTP today).
    pub has_probe_host_key: bool,
}

/// Return metadata for every plugin currently loaded into the
/// host's [`PluginManager`]. Sorted by id so the React side
/// renders a stable order across re-fetches.
#[tauri::command]
pub async fn list_plugins(
    plugin_manager: State<'_, Arc<PluginManager>>,
) -> CommandResult<Vec<PluginInfo>> {
    let mut out: Vec<PluginInfo> = plugin_manager
        .all()
        .into_iter()
        .map(|plugin| {
            let m = &plugin.manifest;
            PluginInfo {
                id: m.id.clone(),
                name: m.name.clone(),
                version: m.version.clone(),
                plugin_type: m.plugin_type.as_str().to_string(),
                capabilities: m
                    .capabilities
                    .iter()
                    .map(|c| c.as_str().to_string())
                    .collect(),
                abi_version: m.abi_version,
                min_app_version: m.min_app_version.clone(),
                author: m.author.clone(),
                description: m.description.clone(),
                signed: m.signed,
                has_interactive_auth: plugin.has_interactive_auth(),
                has_discover: plugin.has_discover(),
                has_probe_host_key: plugin.has_probe_host_key(),
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}
