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

use std::path::PathBuf;
use std::sync::Arc;

use plugin_core::PluginManager;
use serde::Serialize;
use sync_core::{IdPayload, PluginPayload, SyncEvent};
use tauri::State;
use tracing::warn;

use super::{CommandError, CommandResult};
use crate::accounts::{AccountsRepo, AdapterKind};
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;
use crate::registry::AdapterRegistry;
use crate::remote_plugins::RemotePluginsRepo;
use crate::user_prefs::UserPrefsRepo;

/// Newtype Tauri state carrying the resolved
/// `<data_dir>/plugins/user/` path. Wrapped so the State
/// lookup doesn't collide with other PathBuf state.
#[derive(Clone)]
pub struct UserPluginsDir(pub PathBuf);

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

/// Map a sync-adapter plugin id to the `sync.adapter.kind`
/// wire string the SyncPanel persists. Returns `None` for
/// plugins that aren't sync adapters — the active-sync guard
/// only fires when there's a real match risk.
fn sync_kind_for_plugin(plugin_id: &str) -> Option<&'static str> {
    match plugin_id {
        "com.aperio.sync-adapter-local" => Some("local"),
        "com.aperio.sync-adapter-webdav" => Some("webdav"),
        "com.aperio.sync-adapter-ftp" => Some("ftp"),
        "com.aperio.sync-adapter-sftp" => Some("sftp"),
        "com.aperio.sync-adapter-dropbox" => Some("dropbox"),
        "com.aperio.sync-adapter-googledrive" => Some("googledrive"),
        _ => None,
    }
}

/// `user_prefs` key naming the currently-configured sync
/// adapter family. Duplicated from `commands/sync.rs`'s
/// `PREF_ADAPTER_KIND` constant on purpose — keeping the
/// guard self-contained avoids dragging the sync module into
/// this command's dependency surface.
const PREF_ADAPTER_KIND: &str = "sync.adapter.kind";

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
    /// Where the plugin lives on disk: `"bundled"` ships with
    /// the app under `<binary>/plugins/bundled/` and CANNOT
    /// be uninstalled; `"user"` was installed via the §20.7
    /// `.aperio` flow under `<data_dir>/plugins/user/` and can
    /// be removed via the Settings → Plugins panel.
    pub source: PluginSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginSource {
    Bundled,
    User,
}

/// Return metadata for every plugin currently loaded into the
/// host's [`PluginManager`]. Sorted by id so the React side
/// renders a stable order across re-fetches.
#[tauri::command]
pub async fn list_plugins(
    plugin_manager: State<'_, Arc<PluginManager>>,
    user_plugins_dir: State<'_, UserPluginsDir>,
) -> CommandResult<Vec<PluginInfo>> {
    let user_dir = &user_plugins_dir.0;
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
                source: plugin_source(&m.id, user_dir),
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Decide whether a plugin id maps to a user-installed
/// directory under `<user_plugins_dir>/<id>/` (Source::User)
/// or the read-only bundled tree (Source::Bundled). Used by
/// both the list payload (so the panel can hide the Uninstall
/// button for bundled plugins) and the uninstall path (so we
/// refuse to scrub a path we don't own).
fn plugin_source(plugin_id: &str, user_plugins_dir: &PathBuf) -> PluginSource {
    if user_plugins_dir.join(plugin_id).is_dir() {
        PluginSource::User
    } else {
        PluginSource::Bundled
    }
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

    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);

    // Refuse to disable the sync adapter the user is currently
    // using — the orchestrator would start failing every round
    // with "plugin missing" the moment the gate flips. The
    // frontend surfaces this as a "switch sync first" hint
    // pointing the user at the Sync tab.
    if !request.enabled {
        if let Some(plugin_sync_kind) = sync_kind_for_plugin(&request.plugin_id) {
            let active_kind = prefs
                .get(PREF_ADAPTER_KIND)
                .map_err(|e| CommandError {
                    code: "internal",
                    message: format!("read sync.adapter.kind: {e}"),
                })?
                .filter(|s| !s.is_empty());
            if active_kind.as_deref() == Some(plugin_sync_kind) {
                return Err(CommandError {
                    code: "active_sync_conflict",
                    message: format!(
                        "{} is the sync adapter you're currently using; \
                         switch to a different one in Settings → Sync \
                         before disabling it.",
                        request.plugin_id,
                    ),
                });
            }
        }
    }

    // 1) Persist first. If a later step fails, the user's
    //    intent is at least recorded and the next app start
    //    will honour it.
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

// ─────────────────────────────────────────────────────────────
// §20.7 — .aperio community-plugin installer
// ─────────────────────────────────────────────────────────────

/// Preview the manifest of a `.aperio` archive without writing
/// anything to disk. The Settings → Plugins install dialog
/// renders this before asking the user to confirm.
///
/// Also reports whether the plugin id is already loaded so the
/// frontend can render the dialog as "install" vs "update"
/// without a second round-trip.
#[derive(Debug, Clone, Serialize)]
pub struct PluginArchivePreview {
    /// Parsed manifest fields. Same shape as [`PluginInfo`]
    /// but without the runtime flags (the manifest doesn't
    /// know about hooks; signed is forward-compat only).
    pub id: String,
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub capabilities: Vec<String>,
    pub abi_version: u32,
    pub min_app_version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub signed: bool,
    /// `true` when a plugin with the same id is already
    /// loaded. The frontend uses this to phrase the dialog as
    /// an update + (eventually) refuse downgrades.
    pub already_installed: bool,
    /// Currently-installed version (manifest.version), or
    /// `None` when [`Self::already_installed`] is false.
    pub installed_version: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct InspectPluginArchiveRequest {
    /// Absolute path to the `.aperio` archive the user picked
    /// in the file dialog.
    pub archive_path: String,
}

#[tauri::command]
pub async fn inspect_plugin_archive(
    plugin_manager: State<'_, Arc<PluginManager>>,
    request: InspectPluginArchiveRequest,
) -> CommandResult<PluginArchivePreview> {
    let manifest = plugin_core::inspect_archive(&request.archive_path)
        .map_err(plugin_error_to_command)?;
    let existing = plugin_manager.get_including_disabled(&manifest.id);
    let installed_version = existing.as_ref().map(|p| p.manifest.version.clone());
    Ok(PluginArchivePreview {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        plugin_type: manifest.plugin_type.as_str().to_string(),
        capabilities: manifest
            .capabilities
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
        abi_version: manifest.abi_version,
        min_app_version: manifest.min_app_version.clone(),
        author: manifest.author.clone(),
        description: manifest.description.clone(),
        signed: manifest.signed,
        already_installed: existing.is_some(),
        installed_version,
    })
}

#[derive(Debug, serde::Deserialize)]
pub struct InstallPluginArchiveRequest {
    pub archive_path: String,
}

/// Extract a `.aperio` archive into `<data_dir>/plugins/user/
/// <plugin_id>/`, load it via [`PluginManager::load_from_dir`],
/// and re-register any account whose adapter_kind maps to the
/// freshly-installed plugin (a previous bootstrap attempt may
/// have failed with PluginMissing because the plugin wasn't
/// yet installed). Returns the populated [`PluginInfo`] so the
/// frontend can splice it straight into the panel's list
/// without a follow-up `list_plugins`.
///
/// Per DESIGN §20.7 every community plugin is treated as
/// unsigned in this phase — the dialog that calls this fn has
/// already shown the "install from trusted sources only"
/// warning + got an explicit confirmation, so the command
/// itself doesn't second-guess that.
#[tauri::command]
pub async fn install_plugin_archive(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    user_plugins_dir: State<'_, UserPluginsDir>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: InstallPluginArchiveRequest,
) -> CommandResult<PluginInfo> {
    // Pre-flight: read the manifest WITHOUT extracting. Lets
    // us decide between "fresh install" and "upgrade" paths
    // before install_archive wipes the existing plugin dir.
    let preflight_manifest =
        plugin_core::inspect_archive(&request.archive_path)
            .map_err(plugin_error_to_command)?;
    let plugin_id = preflight_manifest.id.clone();
    let shared = db.shared();

    // If this is an in-place upgrade, try to tear the old
    // copy down first. The active-sync guard (parallel to
    // iteration 14's disable guard) refuses upfront — a
    // mid-sync upgrade would be disruptive even if the unload
    // succeeded, and the user can fix it by switching sync
    // adapters first.
    let is_upgrade = plugin_manager
        .get_including_disabled(&plugin_id)
        .is_some();
    if is_upgrade {
        if let Some(plugin_sync_kind) = sync_kind_for_plugin(&plugin_id) {
            let prefs = UserPrefsRepo::new(&shared);
            let active_kind = prefs
                .get(PREF_ADAPTER_KIND)
                .map_err(|e| CommandError {
                    code: "internal",
                    message: format!("read sync.adapter.kind: {e}"),
                })?
                .filter(|s| !s.is_empty());
            if active_kind.as_deref() == Some(plugin_sync_kind) {
                return Err(CommandError {
                    code: "active_sync_conflict",
                    message: format!(
                        "{} is the sync adapter you're currently using; \
                         switch to a different one in Settings → Sync \
                         before upgrading it.",
                        plugin_id,
                    ),
                });
            }
        }
        try_unload_for_upgrade(&plugin_manager, &registry, &shared, &plugin_id)
            .await?;
    }

    // Safe to extract. install_archive wipes any stale
    // directory under the same id (an upgrade that just got
    // its in-memory copy unloaded, or a leftover from a
    // previous install whose plugin then got unloaded between
    // restarts).
    let installed = plugin_core::install_archive(
        &request.archive_path,
        &user_plugins_dir.0,
    )
    .map_err(plugin_error_to_command)?;

    // Load + insert. Errors here leave the freshly-extracted
    // files in place — the user can retry without re-picking
    // the archive.
    plugin_manager
        .load_from_dir(&installed.plugin_dir)
        .map_err(plugin_error_to_command)?;

    // The plugin landed; now re-register any account whose
    // adapter_kind maps to it. Accounts whose plugin was
    // missing at bootstrap will have stayed unregistered;
    // installing the plugin should bring them back online
    // without an app restart.
    let plugin_id = installed.manifest.id.clone();
    if let Some(kind) = adapter_kind_for_plugin(&plugin_id) {
        let shared = db.shared();
        let accounts_repo = AccountsRepo::new(&shared);
        let accounts = accounts_repo.list().map_err(|e| CommandError {
            code: "internal",
            message: format!("list accounts for post-install register: {e}"),
        })?;
        for account in accounts {
            if account.adapter_kind != kind {
                continue;
            }
            if let Err(err) = registry.register(&account) {
                warn!(
                    account_id = %account.id,
                    plugin_id = %plugin_id,
                    ?err,
                    "post-install register failed",
                );
            }
        }
    }

    // Build the PluginInfo from the just-loaded LoadedPlugin
    // so the response shape matches the panel's list payload.
    let loaded = plugin_manager
        .get_including_disabled(&plugin_id)
        .ok_or_else(|| CommandError {
            code: "internal",
            message: format!(
                "plugin {} extracted but not in manager after load",
                plugin_id,
            ),
        })?;
    let m = &loaded.manifest;
    // §20.8: announce the install to other devices via the
    // event log. Only User-source plugins get announced —
    // bundled plugins are guaranteed present on every install
    // and would be noise. install_archive only ever lands
    // under user_plugins_dir, so this is trivially always
    // User; we name the variant for clarity rather than
    // skip the check.
    if matches!(PluginSource::User, PluginSource::User) {
        event_log.append(SyncEvent::PluginInstalled(PluginPayload {
            id: m.id.clone(),
            version: m.version.clone(),
            source: None,
            name: Some(m.name.clone()),
            plugin_type: Some(m.plugin_type.as_str().to_string()),
        }));
    }

    // Local mirror: if the remote_plugins table carried an
    // announcement for this id from another device, drop it
    // — the announcement is no longer "pending" once we
    // have the binary. Errors are non-fatal; the
    // announcement just stays in the UI until next restart.
    let prefs_db = db.shared();
    if let Err(err) = RemotePluginsRepo::new(&prefs_db).delete(&plugin_id) {
        warn!(
            plugin_id = %plugin_id,
            ?err,
            "couldn't drop remote_plugins row after local install",
        );
    }

    Ok(PluginInfo {
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
        has_interactive_auth: loaded.has_interactive_auth(),
        has_discover: loaded.has_discover(),
        has_probe_host_key: loaded.has_probe_host_key(),
        enabled: plugin_manager.is_enabled(&plugin_id),
        // Just installed via the §20.7 archive flow, so this
        // is always Source::User. (The bundled scan happens
        // at startup against a different directory; nothing
        // we do at runtime can promote a user plugin to
        // bundled.)
        source: PluginSource::User,
    })
}

/// Tear down the in-memory copy of `plugin_id` so the
/// install path can re-extract + re-load the new version.
/// The sequence mirrors the disable path: unregister every
/// account using the plugin, then flip the runtime gate, then
/// poll `PluginManager::unload_plugin` until the in-flight
/// counter drains to 0.
///
/// Determinism: once the registry has unregistered the shim
/// Arcs and the disabled flag is set, no NEW FFI calls can
/// start against this plugin. Existing in-flight calls hold
/// their guards across `.await`; the bounded retry loop waits
/// for those guards to drop. If the deadline expires we roll
/// back (re-enable + re-register the accounts) and surface
/// `restart_required` — that case is reserved for OAuth dances
/// or sync rounds that legitimately take longer than the
/// upgrade is willing to wait.
///
/// Only called from the upgrade branch of
/// [`install_plugin_archive`].
async fn try_unload_for_upgrade(
    plugin_manager: &PluginManager,
    registry: &AdapterRegistry,
    db: &crate::db::SharedConn,
    plugin_id: &str,
) -> CommandResult<()> {
    /// Maximum time we wait for in-flight calls to drain
    /// before declaring an upgrade impossible without a
    /// restart. 1.5s comfortably covers typical sub-second
    /// CalDAV / Graph reads + leaves headroom for slow
    /// networks without making the user stare at a frozen
    /// dialog.
    const DRAIN_TIMEOUT: std::time::Duration =
        std::time::Duration::from_millis(1500);
    /// Poll interval between retry attempts. Short enough
    /// that a typical FFI call completes within 1-2 polls;
    /// long enough that we don't burn a CPU.
    const POLL_INTERVAL: std::time::Duration =
        std::time::Duration::from_millis(50);

    // 1) Walk accounts whose adapter_kind matches the plugin
    //    we're about to unload. For each, unregister + remember
    //    the account so we can re-register on rollback or
    //    after the new version loads.
    let mut affected_accounts = Vec::new();
    if let Some(kind) = adapter_kind_for_plugin(plugin_id) {
        let accounts_repo = AccountsRepo::new(db);
        let accounts = accounts_repo.list().map_err(|e| CommandError {
            code: "internal",
            message: format!("list accounts for upgrade unload: {e}"),
        })?;
        for account in accounts {
            if account.adapter_kind != kind {
                continue;
            }
            registry.unregister(&account.id);
            affected_accounts.push(account);
        }
    }

    // 2) Gate further get() lookups. This is the same flag
    //    the toggle uses; we'll clear it on success after
    //    load (the registry's register() reads enabled
    //    state) or on rollback.
    let previously_enabled = plugin_manager.is_enabled(plugin_id);
    plugin_manager.set_enabled(plugin_id, false);

    // 3) Bounded poll: wait for in_flight to drain. The
    //    counter is monotonically non-increasing now that the
    //    gate is set + the registry's shims dropped, so this
    //    loop terminates deterministically.
    let deadline = std::time::Instant::now() + DRAIN_TIMEOUT;
    loop {
        match plugin_manager.unload_plugin(plugin_id) {
            Ok(()) => return Ok(()),
            Err(plugin_core::UnloadError::NotLoaded(_)) => {
                // Race: someone else dropped the plugin
                // between the install command's pre-flight
                // check and now. Treat as success.
                return Ok(());
            }
            Err(plugin_core::UnloadError::StillReferenced {
                id,
                in_flight,
            }) => {
                if std::time::Instant::now() >= deadline {
                    // Out of patience — roll back + report.
                    // `in_flight` from the most recent attempt
                    // is what the user sees in the message.
                    if previously_enabled {
                        plugin_manager.set_enabled(&id, true);
                    }
                    for account in affected_accounts {
                        if let Err(err) = registry.register(&account) {
                            warn!(
                                account_id = %account.id,
                                plugin_id = %id,
                                ?err,
                                "rollback re-register failed after drain timeout",
                            );
                        }
                    }
                    return Err(CommandError {
                        code: "restart_required",
                        message: format!(
                            "{id} still has {in_flight} active call(s) \
                             after {}ms; restart Aperio to install the new version",
                            DRAIN_TIMEOUT.as_millis(),
                        ),
                    });
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────
// §20.8 — remote plugin announcements (other devices' plugins)
// ─────────────────────────────────────────────────────────────

/// List every plugin OTHER devices have installed that this
/// device doesn't have loaded locally. The Settings → Plugins
/// panel renders these as the "Plugin benötigt" section so the
/// user can manually fetch the matching `.aperio` archive +
/// install it via the existing flow.
///
/// Sorted by announced_at DESC (most recent first) — the
/// remote_plugins table's index covers this so the read stays
/// trivial. We exclude announcements for plugins that ARE
/// loaded locally because they'd be noise; the install command
/// also drops the row on success, but a race or a stale
/// announcement from a prior install/uninstall cycle could
/// leave one around.
#[tauri::command]
pub async fn list_remote_plugins(
    db: State<'_, DbHandle>,
    plugin_manager: State<'_, Arc<PluginManager>>,
) -> CommandResult<Vec<crate::remote_plugins::RemotePluginAnnouncement>> {
    let shared = db.shared();
    let repo = crate::remote_plugins::RemotePluginsRepo::new(&shared);
    let all = repo.list().map_err(|err| CommandError {
        code: "internal",
        message: format!("list remote_plugins: {err}"),
    })?;
    let out = all
        .into_iter()
        .filter(|row| plugin_manager.get_including_disabled(&row.id).is_none())
        .collect();
    Ok(out)
}

// ─────────────────────────────────────────────────────────────
// §20.10 — uninstall verb (community plugins only)
// ─────────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct UninstallPluginRequest {
    pub plugin_id: String,
}

/// Drop a community plugin: drain in-flight calls, unload
/// from the manager, scrub `<user_plugins_dir>/<plugin_id>/`,
/// clear the user_prefs disabled flag for the id.
///
/// Refuses bundled plugins (DESIGN.md §20.10 explicitly notes
/// these are not user-removable) and the active sync plugin
/// (parallel to the disable + upgrade guards). Accounts that
/// reference the uninstalled plugin are unregistered from
/// the registry but NOT deleted from the accounts table —
/// the §20.8 "Plugin fehlt" path is what surfaces them in
/// the UI as needing attention, and the user might re-install
/// the plugin later. Wiping the rows on uninstall would be a
/// destructive surprise.
#[tauri::command]
pub async fn uninstall_plugin(
    db: State<'_, DbHandle>,
    registry: State<'_, Arc<AdapterRegistry>>,
    plugin_manager: State<'_, Arc<PluginManager>>,
    user_plugins_dir: State<'_, UserPluginsDir>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: UninstallPluginRequest,
) -> CommandResult<()> {
    let plugin_id = request.plugin_id;

    // 1) Bundled plugins are read-only. Refuse before we
    //    touch any state.
    if plugin_source(&plugin_id, &user_plugins_dir.0) == PluginSource::Bundled {
        return Err(CommandError {
            code: "unsupported",
            message: format!(
                "{plugin_id} is a bundled plugin; bundled plugins can't be uninstalled.",
            ),
        });
    }

    // 2) Active-sync guard. Uninstalling the sync adapter
    //    the user is actively syncing with would break every
    //    subsequent sync round; same posture as iterations
    //    14 + 17.
    let shared = db.shared();
    if let Some(plugin_sync_kind) = sync_kind_for_plugin(&plugin_id) {
        let prefs = UserPrefsRepo::new(&shared);
        let active_kind = prefs
            .get(PREF_ADAPTER_KIND)
            .map_err(|e| CommandError {
                code: "internal",
                message: format!("read sync.adapter.kind: {e}"),
            })?
            .filter(|s| !s.is_empty());
        if active_kind.as_deref() == Some(plugin_sync_kind) {
            return Err(CommandError {
                code: "active_sync_conflict",
                message: format!(
                    "{plugin_id} is the sync adapter you're currently using; \
                     switch to a different one in Settings → Sync before uninstalling it.",
                ),
            });
        }
    }

    // 3) If the plugin is currently loaded, tear it down via
    //    the same drain-and-unload helper that the upgrade
    //    flow uses. On `restart_required` rollback the helper
    //    re-registers the affected accounts so the user keeps
    //    a functional setup until they retry / restart.
    if plugin_manager.get_including_disabled(&plugin_id).is_some() {
        try_unload_for_upgrade(&plugin_manager, &registry, &shared, &plugin_id)
            .await?;
    }

    // 4) Scrub the plugin directory. Best-effort: if the dir
    //    is somehow already gone, treat as success (matches
    //    user intent — "I want this plugin gone, full stop").
    let plugin_dir = user_plugins_dir.0.join(&plugin_id);
    if plugin_dir.is_dir() {
        std::fs::remove_dir_all(&plugin_dir).map_err(|e| CommandError {
            code: "internal",
            message: format!(
                "remove plugin directory {}: {e}",
                plugin_dir.display(),
            ),
        })?;
    }

    // 5) Housekeeping: drop the user_prefs disabled flag so
    //    a re-install starts from a clean enabled state.
    //    Deletion errors are non-fatal — the install path
    //    will overwrite the row if it ever exists again.
    let prefs = UserPrefsRepo::new(&shared);
    let key = pref_key_for_disabled(&plugin_id);
    if let Err(err) = prefs.delete(&key) {
        warn!(
            plugin_id = %plugin_id,
            ?err,
            "uninstall: couldn't drop plugin.disabled flag from user_prefs",
        );
    }

    // 6) §20.8: announce the uninstall to other devices. Same
    //    user-source-only contract as the install path —
    //    bundled plugins are already filtered out above by
    //    the source check; community plugins always emit.
    event_log.append(SyncEvent::PluginUninstalled(IdPayload {
        id: plugin_id.clone(),
    }));

    // 7) Drop the local remote_plugins mirror row if this
    //    plugin was originally announced by another device.
    //    Non-fatal — the UI shows it as "missing" until next
    //    restart otherwise.
    if let Err(err) = RemotePluginsRepo::new(&shared).delete(&plugin_id) {
        warn!(
            plugin_id = %plugin_id,
            ?err,
            "uninstall: couldn't drop remote_plugins row",
        );
    }

    Ok(())
}

/// Map plugin-core's error type onto the frontend-friendly
/// envelope. The PluginError variants are narrower than the
/// generic CommandError code list so we collapse onto the
/// closest match (`Io` / `Manifest` / `Version` mostly mean
/// "bad input" from the user's perspective).
fn plugin_error_to_command(err: plugin_core::error::PluginError) -> CommandError {
    use plugin_core::error::PluginError::*;
    let (code, message) = match err {
        Io(m) => ("invalid_input", format!("io error: {m}")),
        Manifest(m) => ("invalid_input", m),
        Semver { value, reason } => (
            "invalid_input",
            format!("malformed version {value:?}: {reason}"),
        ),
        AbiMismatch { host, plugin } => (
            "invalid_input",
            format!(
                "plugin ABI version {plugin} doesn't match host's {host}",
            ),
        ),
        AppTooOld { required, running } => (
            "invalid_input",
            format!(
                "plugin requires Aperio {required} or newer; this build is {running}",
            ),
        ),
        InstanceOpen { status, message } => (
            "internal",
            format!("open_instance(status {status}): {message}"),
        ),
    };
    CommandError { code, message }
}
