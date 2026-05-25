//! Plugin-management Tauri commands (DESIGN.md §20.10).
//!
//! v1.1 covers list + enable/disable. The remaining verbs
//! land in future iterations — uninstall needs the
//! `plugin.uninstalled` event-log surface (§20.10) plus a way
//! to scrub the cdylib + plugin.json from `plugins/user/`;
//! install needs the `.aperio` archive extractor (§20.7).
//!
//! ## Disable semantics
//!
//! Disabling a plugin doesn't unload the cdylib — the library
//! stays mapped, but the host's [`PluginManager::get`] returns
//! `None` for the id so the rest of the host treats it as if
//! the id were never installed. The flag persists across
//! restarts via user_prefs key `plugin.disabled.<id>`. Every
//! account whose adapter_kind maps to the affected plugin is
//! unregistered from the [`AdapterRegistry`] so calendar /
//! tasks / contacts reads start failing with "no adapter"
//! until the user flips the toggle back. Re-enabling
//! re-registers the same accounts in the same gesture.

use std::sync::Arc;

use plugin_core::PluginManager;
use serde::Serialize;
use tauri::State;
use tracing::warn;

use super::{CommandError, CommandResult};
use crate::accounts::{AccountsRepo, AdapterKind};
use crate::db::DbHandle;
use crate::registry::AdapterRegistry;
use crate::user_prefs::UserPrefsRepo;

/// `user_prefs` key prefix carrying the disabled flag for each
/// plugin. The full key is `plugin.disabled.<plugin_id>`; the
/// value is the literal string `"true"` (any other value is
/// treated as enabled).
pub const PREF_PREFIX_PLUGIN_DISABLED: &str = "plugin.disabled.";

/// Build the user_prefs key for a plugin's disabled flag.
pub fn pref_key_for_disabled(plugin_id: &str) -> String {
    format!("{PREF_PREFIX_PLUGIN_DISABLED}{plugin_id}")
}

/// Map a plugin id to the [`AdapterKind`] used to find
/// matching account rows. Returns `None` for plugin types that
/// aren't account-scoped (sync adapters live in user_prefs,
/// not the accounts table; notification plugins have no
/// per-account state yet).
fn adapter_kind_for_plugin(plugin_id: &str) -> Option<AdapterKind> {
    match plugin_id {
        "com.aperio.cal-adapter-caldav" => Some(AdapterKind::Caldav),
        "com.aperio.cal-adapter-ical" => Some(AdapterKind::Ical),
        "com.aperio.cal-adapter-google" => Some(AdapterKind::Google),
        "com.aperio.cal-adapter-microsoft-graph" => Some(AdapterKind::MicrosoftGraph),
        "com.aperio.cal-adapter-ews" => Some(AdapterKind::Ews),
        "com.aperio.cal-adapter-vikunja" => Some(AdapterKind::Vikunja),
        "com.aperio.cal-adapter-todoist" => Some(AdapterKind::Todoist),
        "com.aperio.vc-adapter-zoom" => Some(AdapterKind::Zoom),
        "com.aperio.vc-adapter-teams" => Some(AdapterKind::Teams),
        "com.aperio.vc-adapter-meet" => Some(AdapterKind::Meet),
        "com.aperio.vc-adapter-webex" => Some(AdapterKind::Webex),
        _ => None,
    }
}

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
    /// `true` when the plugin is currently enabled (the host's
    /// [`PluginManager`] routes calls to it). `false` when the
    /// user has flipped the Settings → Plugins toggle off; the
    /// cdylib stays loaded but the host treats the id as
    /// uninstalled until the toggle goes back on.
    pub enabled: bool,
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
                enabled: plugin_manager.is_enabled(&m.id),
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

#[derive(Debug, serde::Deserialize)]
pub struct SetPluginEnabledRequest {
    pub plugin_id: String,
    pub enabled: bool,
}

/// Flip a plugin's enabled/disabled flag. Persists the new
/// state in `user_prefs` (so it survives a restart) and
/// re-syncs the [`AdapterRegistry`] for any account whose
/// adapter_kind maps to the affected plugin id — disabled
/// plugins get their accounts unregistered so the next read
/// fails with "no adapter"; re-enabled plugins get their
/// accounts re-registered so reads start working again
/// without an app restart.
///
/// Bundled vs community: no distinction at this layer. The
/// frontend renders the toggle for every loaded plugin
/// (DESIGN.md §20.10's table doesn't gate disable on
/// bundled-ness; only uninstall is community-only). A future
/// follow-up could refuse to disable plugins the app
/// fundamentally depends on (e.g. the local sync adapter
/// when the user is on the implicit-local path), but v1
/// trusts the user.
#[tauri::command]
pub async fn set_plugin_enabled(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    request: SetPluginEnabledRequest,
) -> CommandResult<()> {
    // Refuse to act on plugins the host doesn't actually have
    // loaded — the persistence layer would happily write the
    // flag, but the UI gesture would never round-trip back to
    // a visible plugin row. Better to surface this as an
    // explicit error than silently no-op.
    if plugin_manager
        .get_including_disabled(&request.plugin_id)
        .is_none()
    {
        return Err(CommandError {
            code: "plugin_missing",
            message: format!("plugin {} is not loaded", request.plugin_id),
        });
    }

    // 1) Persist first. If a later step fails, the user's
    //    intent is at least recorded and the next app start
    //    will honour it.
    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);
    let key = pref_key_for_disabled(&request.plugin_id);
    let persist_result = if request.enabled {
        prefs.delete(&key)
    } else {
        prefs.set(&key, "true")
    };
    persist_result.map_err(|e| CommandError {
        code: "internal",
        message: format!("persist plugin-disabled flag: {e}"),
    })?;

    // 2) Flip the runtime gate. `set_enabled` reports whether
    //    the state actually changed — when it didn't, no need
    //    to walk the accounts table.
    let changed = plugin_manager.set_enabled(&request.plugin_id, request.enabled);

    // 3) Re-sync the registry if the gate flipped + the plugin
    //    is account-scoped (calendar / tasks / contacts / vc).
    //    Sync adapters live in user_prefs not the accounts
    //    table; their next sync round will just hit a
    //    plugin-missing error and surface it through the
    //    SyncPanel.
    if changed {
        if let Some(kind) = adapter_kind_for_plugin(&request.plugin_id) {
            let accounts_repo = AccountsRepo::new(&shared);
            let accounts = accounts_repo.list().map_err(|e| CommandError {
                code: "internal",
                message: format!("list accounts for re-sync: {e}"),
            })?;
            for account in accounts {
                if account.adapter_kind != kind {
                    continue;
                }
                if request.enabled {
                    if let Err(err) = registry.register(&account) {
                        warn!(
                            account_id = %account.id,
                            plugin_id = %request.plugin_id,
                            ?err,
                            "re-register after plugin enable failed",
                        );
                    }
                } else {
                    registry.unregister(&account.id);
                }
            }
        }
    }

    Ok(())
}
