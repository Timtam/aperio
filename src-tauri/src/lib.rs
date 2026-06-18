//! Aperio — Tauri backend.
//!
//! Phase 1 wires up the local SQLite store, the local calendar/task
//! adapter, and a first round of Tauri commands for CRUD. The plugin
//! manager, sync engine, and external adapters arrive in later phases.

pub mod audio;
pub mod bundled_plugins;
pub mod cache_refresh;
pub mod commands;
pub mod contact_sync;
pub mod event_log;
pub mod logging;
pub mod overrides;
mod platform;
pub mod reminders;
pub mod secrets;
pub mod sound;
pub mod tray;
mod window_state;

// `db` + `paths` were extracted into the shared, Tauri-free `host-core`
// crate (so the mobile UniFFI host reuses the same SQLite handle + portable
// path resolution). Re-exported here so existing `crate::db::*` /
// `crate::paths::*` references across the backend keep resolving unchanged.
pub use host_core::db::{DbError, DbHandle, DbResult, SharedConn};
pub use host_core::paths::{resolve_data_dir, DataDirKind, DataDirResolution};
pub use host_core::{
    accounts, cache, conflicts, credential_sync, db, device_names, paths, registry, remote_plugins,
    sftp_host_keys, sound_assets, sync_log, user_prefs,
};

use cal_adapter_local::LocalAdapter;
use contact_sync::ContactSyncScheduler;
// The sync-engine assembly (writer / applier / snapshot / compactor /
// orchestrator) is now built by host_core::sync::build_orchestrator; lib.rs
// only names the desktop-side SyncScheduler.
use event_log::SyncScheduler;
use registry::AdapterRegistry;
use reminders::ReminderScheduler;
use std::sync::Arc;
use tauri::Manager;
use tracing::{info, warn};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Resolve the data dir first: the file-logging layer lives under it, and
    // logging must be up before anything else emits a trace.
    let data_dir = resolve_data_dir();
    let log_state = logging::init(&data_dir.path);
    // With logging up, route panics into the logs so a hard crash leaves a
    // report the user can send (release builds are `panic = "abort"`, so this
    // writes the crash file synchronously).
    logging::install_panic_hook(log_state.logs_dir.clone());

    // Register the AUMID and pin it to this process before tauri starts
    // — toast notifications inherit it at process scope. On non-Windows
    // platforms this is a no-op.
    platform::setup();

    info!(
        kind = ?data_dir.kind,
        path = %data_dir.path.display(),
        "resolved data directory"
    );

    let db_path = data_dir.path.join("aperio.sqlite");
    let db = DbHandle::open(&db_path).expect("failed to open local database");
    info!(path = %db_path.display(), "opened local database");

    // Apply the user's saved log level now the DB is open. init() started at
    // RUST_LOG / the default so early startup is captured; this overrides it
    // with the persisted choice (Settings → Protokolle). Device-local pref —
    // deliberately NOT on the sync whitelist.
    {
        let shared = db.shared();
        if let Ok(Some(level)) =
            crate::user_prefs::UserPrefsRepo::new(&shared).get(commands::PREF_LOG_LEVEL)
        {
            log_state.set_filter(&level);
        }
    }

    // The Tauri backend owns the connection. Subsystems (calendar adapter,
    // sync engine, plugin manager) take an `Arc` clone of the same mutex.
    let local_adapter = LocalAdapter::new(db.shared());
    let db_for_scheduler = db.shared();

    // Build the adapter registry up-front and let it walk the
    // persisted accounts to materialise external adapters. Failures
    // per account are logged inside `bootstrap` — a single broken
    // credential mustn't keep the app from coming up.
    //
    // The registry is wrapped in `Arc` because two consumers hold it
    // concurrently: Tauri's command State (via `manage`) and the
    // reminder scheduler's background task. Both call the same
    // adapters; sharing the same instance keeps the in-adapter
    // listing caches coherent across read paths.
    // Plugin manager — every external calendar/tasks/contacts
    // adapter is registered as a static plugin before the
    // registry's bootstrap walk so the per-account `open_instance`
    // calls have a populated PluginManager to look up against.
    // The eventual dlopen pipeline (DESIGN.md §22.2) replaces
    // this with a `scan_dir("plugins/bundled/")` round.
    let plugin_manager = bundled_plugins::build_manager(env!("CARGO_PKG_VERSION"), &data_dir.path);
    info!(
        plugin_count = plugin_manager.len(),
        "registered bundled + user plugins",
    );
    // Resolve the per-user plugins dir once so the install /
    // uninstall commands can grab it from Tauri state without
    // re-deriving it from data_dir each time.
    let user_plugins_dir = bundled_plugins::user_plugins_dir(&data_dir.path);

    // §20.10: hydrate the per-plugin disabled flag from
    // user_prefs BEFORE the registry bootstraps — disabled
    // plugins must look "not installed" to register_*'s
    // PluginManager::get lookup. Failures here are logged + the
    // plugin is left enabled (the user can re-disable from the
    // Settings panel).
    {
        let shared = db.shared();
        let prefs = crate::user_prefs::UserPrefsRepo::new(&shared);
        for plugin in plugin_manager.all() {
            let key = commands::pref_key_for_disabled(&plugin.manifest.id);
            match prefs.get(&key) {
                Ok(Some(v)) if v == "true" => {
                    plugin_manager.set_enabled(&plugin.manifest.id, false);
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(
                        plugin_id = %plugin.manifest.id,
                        ?err,
                        "couldn't read plugin-disabled flag; leaving enabled",
                    );
                }
            }
        }
    }

    let registry = Arc::new(AdapterRegistry::with_data_dir(
        Arc::clone(&plugin_manager),
        Arc::new(crate::secrets::KeyringSecretStore),
        Some(data_dir.path.clone()),
    ));
    {
        let shared = db.shared();
        let repo = accounts::AccountsRepo::new(&shared);
        registry.bootstrap(&repo);
    }

    // CACHE-1: host-owned snapshot cache for external adapters + the
    // background-refresh dedup guard. Both live in Tauri State so the
    // read commands can serve cached data instantly and kick a
    // deduplicated refresh.
    let cache_store = Arc::new(cache::CacheStore::new(db.clone()));
    let refresh_coordinator = Arc::new(cache::RefreshCoordinator::new());

    // One-time heal for the EWS cursor-desync bug: older builds let the
    // reminder scan's `get_events` drain advance + persist the EWS
    // SyncFolderItems cookie independently of the host's, so a later delta
    // could skip changes the host never cached — an edited Outlook event
    // stuck at its old time. The drain is now host-token-authoritative; this
    // clears the host's EWS event cursor ONCE so the next warm pass
    // full-resyncs and recovers any already-stuck edits. Best-effort: on any
    // failure the completion flag stays unset and the heal retries next boot.
    {
        const HEAL_FLAG: &str = "cache.ewsCursorHealV1";
        let shared = db.shared();
        let prefs = crate::user_prefs::UserPrefsRepo::new(&shared);
        let already_done = matches!(prefs.get(HEAL_FLAG).ok().flatten().as_deref(), Some("done"));
        if !already_done {
            match accounts::AccountsRepo::new(&shared).list() {
                Ok(accts) => {
                    let mut healed = 0usize;
                    for acc in accts
                        .iter()
                        .filter(|a| a.adapter_kind == accounts::AdapterKind::Ews)
                    {
                        match cache_store.reset_event_sync(&acc.id) {
                            Ok(n) => healed += n,
                            Err(err) => tracing::warn!(
                                account_id = %acc.id,
                                ?err,
                                "EWS cursor heal: reset_event_sync failed",
                            ),
                        }
                    }
                    match prefs.set(HEAL_FLAG, "done") {
                        Ok(()) => tracing::info!(
                            target: "aperio::cache",
                            containers = healed,
                            "EWS cursor heal: cleared event cursors for a one-time full resync",
                        ),
                        Err(err) => tracing::warn!(
                            ?err,
                            "EWS cursor heal: couldn't persist completion flag; will retry next boot",
                        ),
                    }
                }
                Err(err) => tracing::warn!(
                    ?err,
                    "EWS cursor heal: couldn't list accounts; will retry next boot",
                ),
            }
        }
    }

    // One-time heal for EWS attendees/organizer: events synced before the
    // RSVP read-path landed were marked `detail_fetched` without ever
    // pulling `Organizer`/`RequiredAttendees` (the SyncFolderItems shape
    // omits them and the old detail fan-out neither requested nor merged
    // them), so existing meetings render with an empty attendee list. A
    // cold re-sync rebuilds each folder from a fresh state and re-enriches
    // every item — now including attendees — so we clear the EWS event
    // cursors ONCE more. Same best-effort + idempotent flag pattern as the
    // cursor heal above; a separate flag so accounts already cursor-healed
    // still get the attendee backfill.
    {
        const HEAL_FLAG: &str = "cache.ewsAttendeeHealV1";
        let shared = db.shared();
        let prefs = crate::user_prefs::UserPrefsRepo::new(&shared);
        let already_done = matches!(prefs.get(HEAL_FLAG).ok().flatten().as_deref(), Some("done"));
        if !already_done {
            match accounts::AccountsRepo::new(&shared).list() {
                Ok(accts) => {
                    let mut healed = 0usize;
                    for acc in accts
                        .iter()
                        .filter(|a| a.adapter_kind == accounts::AdapterKind::Ews)
                    {
                        match cache_store.reset_event_sync(&acc.id) {
                            Ok(n) => healed += n,
                            Err(err) => tracing::warn!(
                                account_id = %acc.id,
                                ?err,
                                "EWS attendee heal: reset_event_sync failed",
                            ),
                        }
                    }
                    match prefs.set(HEAL_FLAG, "done") {
                        Ok(()) => tracing::info!(
                            target: "aperio::cache",
                            containers = healed,
                            "EWS attendee heal: cleared event cursors so meetings re-enrich with attendees",
                        ),
                        Err(err) => tracing::warn!(
                            ?err,
                            "EWS attendee heal: couldn't persist completion flag; will retry next boot",
                        ),
                    }
                }
                Err(err) => tracing::warn!(
                    ?err,
                    "EWS attendee heal: couldn't list accounts; will retry next boot",
                ),
            }
        }
    }

    // One-time heal for CardDAV/contacts: an older CalDAV read fetched
    // a book's contacts via a non-standard inline-`address-data` PROPFIND
    // that iCloud / Synology Contacts silently ignore, so the bootstrap
    // wrote ZERO contacts yet still persisted a valid sync token. Every
    // delta since then reported "no changes" over an empty cache, leaving
    // address books permanently empty. The read now uses
    // addressbook-multiget; clearing the contacts sync tokens once forces
    // each book to re-bootstrap and recover its contacts. Best-effort +
    // idempotent, same pattern as the EWS heals above; resets every
    // account's contacts scope (a no-op for accounts with no contact
    // data, e.g. calendar-only or task-only providers).
    {
        const HEAL_FLAG: &str = "cache.contactsMultigetHealV2";
        let shared = db.shared();
        let prefs = crate::user_prefs::UserPrefsRepo::new(&shared);
        let already_done = matches!(prefs.get(HEAL_FLAG).ok().flatten().as_deref(), Some("done"));
        if !already_done {
            match accounts::AccountsRepo::new(&shared).list() {
                Ok(accts) => {
                    let mut healed = 0usize;
                    for acc in &accts {
                        match cache_store.reset_contacts_sync(&acc.id) {
                            Ok(n) => healed += n,
                            Err(err) => tracing::warn!(
                                account_id = %acc.id,
                                ?err,
                                "contacts heal: reset_contacts_sync failed",
                            ),
                        }
                    }
                    match prefs.set(HEAL_FLAG, "done") {
                        Ok(()) => tracing::info!(
                            target: "aperio::cache",
                            containers = healed,
                            "contacts heal: cleared contact sync tokens for a one-time re-bootstrap",
                        ),
                        Err(err) => tracing::warn!(
                            ?err,
                            "contacts heal: couldn't persist completion flag; will retry next boot",
                        ),
                    }
                }
                Err(err) => tracing::warn!(
                    ?err,
                    "contacts heal: couldn't list accounts; will retry next boot",
                ),
            }
        }
    }

    // CACHE-3: clones for the background warm/periodic refresher spawned
    // in `setup()` (the originals are moved into Tauri State below).
    let registry_for_cache_refresh = Arc::clone(&registry);
    let cache_for_refresh = Arc::clone(&cache_store);
    let coord_for_refresh = Arc::clone(&refresh_coordinator);
    let db_for_cache_refresh = db.shared();

    // The scheduler holds its own Arc so its background scan can
    // fan out to external adapters even while a command is awaiting
    // them via Tauri's State. Both handles point at the same
    // registry instance.
    let registry_for_scheduler = Arc::clone(&registry);
    // Phase 10j: same Arc-sharing pattern for the contact sync
    // scheduler. A second clone lives in the background task; the
    // primary Arc keeps living inside Tauri State so the
    // `sync_contacts_now` command can dispatch through the same
    // instance and benefit from the in-flight dedupe guard.
    let registry_for_contact_sync = Arc::clone(&registry);
    let db_for_contact_sync = db.shared();

    // Phase Sb (DESIGN.md §19): mint or load this install's
    // DeviceId from user_prefs and spawn the event-log writer.
    // The writer's background drain task lives in `event_log::
    // mod::drain_loop` — keeps one JSONL session file open under
    // `<data_dir>/sync/log/pending/` and appends every local
    // mutation that flows through the command layer's writer
    // hooks. Wrapped in Arc so cloning into Tauri State is free.
    // One boot instant, shared by the writer (which names its session JSONL
    // file with it) and the orchestrator (its stale-stub cleanup guard). See
    // host_core::sync::build_orchestrator + EventLogWriter::spawn_with_kick.
    let boot_at = chrono::Utc::now();
    let secret_store: Arc<dyn sync_engine::SecretStore> =
        Arc::new(crate::secrets::KeyringSecretStore);
    // The full sync graph — the SAME assembly the mobile UniFFI host builds
    // (host_core::sync::build_orchestrator), wired here with the desktop's
    // keyring SecretStore. The writer's drain task starts with `tokio::spawn`,
    // which needs an active runtime context; `run()` executes synchronously
    // before `app.run()` drives the loop, so establish the context via Tauri's
    // global runtime (the very one the app then uses).
    let sync_graph = tauri::async_runtime::block_on(async {
        host_core::sync::build_orchestrator(
            db.shared(),
            data_dir.path.clone(),
            Arc::clone(&secret_store),
            env!("CARGO_PKG_VERSION"),
            boot_at,
        )
    });
    info!(
        device_id = %sync_graph.device_id,
        "event-log writer device id",
    );
    let event_log_writer = Arc::clone(&sync_graph.writer);
    let kick_notify = Arc::clone(&sync_graph.kick);
    let onboarding = Arc::clone(&sync_graph.onboarding);
    let sync_orchestrator = Arc::clone(&sync_graph.orchestrator);

    // One-shot backfills: existing accounts (pre-dating the Account.* sync
    // events) and existing LOCAL task lists/tasks (created while the `boot_at`
    // writer race was live) never reached the event log. Replay them through
    // the writer once so the next sync round carries them. Idempotent — each
    // is gated by a pref it sets on success.
    commands::backfill_account_events(&db, &event_log_writer);
    commands::backfill_local_task_events(&db, &event_log_writer);

    // Local custom-sound store (user content, outside the sync/ subtree). §14.4:
    // the reminder scheduler + the sound import/list/preview/delete commands
    // resolve files out of this same dir. (build_orchestrator computes its own
    // copy internally for the sync-round hooks; this is the same path.)
    let sounds_dir = crate::sound_assets::sounds_dir_under(&data_dir.path);
    let sounds_dir_for_scheduler = sounds_dir.clone();
    let sounds_dir_for_commands = sounds_dir;
    // If the user had previously configured a sync adapter,
    // reconstruct it now so `sync_now` works without a
    // re-configure step. Adapter credentials are device-local
    // (per §19.2.1) and never propagate, so the user_prefs
    // lookup is the single source of truth.
    if let Some(adapter) = commands::build_adapter_from_prefs(&db.shared(), &plugin_manager) {
        info!("restoring previously-configured sync adapter");
        sync_orchestrator.configure(adapter);
    }
    // Phase Se: hold a separate clone for the app-exit hook below.
    // The `RunEvent::ExitRequested` callback is `FnMut`, not async,
    // so it captures this Arc and blocks on a final `push_now()`
    // before the process dies.
    let orchestrator_for_exit = Arc::clone(&sync_orchestrator);
    let scheduler_kick_for_setup = Arc::clone(&kick_notify);
    let scheduler_orchestrator = Arc::clone(&sync_orchestrator);
    let scheduler_db = db.shared();

    // §14.4: one process-wide audio thread owns the output stream and
    // plays custom notification sounds. Shared between the reminder
    // scheduler (which plays on fire) and the `preview_sound` command
    // (the Test button in the SoundPicker UI).
    let audio_player = crate::audio::AudioPlayer::spawn();
    let audio_for_scheduler = audio_player.clone();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(audio_player)
        .manage(commands::SoundsDir(sounds_dir_for_commands))
        .manage(local_adapter)
        .manage(registry)
        .manage(cache_store)
        .manage(refresh_coordinator)
        .manage(db)
        .manage(event_log_writer)
        .manage(sync_orchestrator)
        .manage(onboarding)
        .manage(plugin_manager)
        // §20.7 install commands need to know where to extract
        // user plugins; the newtype wrapper keeps the State
        // lookup unambiguous.
        .manage(commands::UserPluginsDir(user_plugins_dir))
        // Diagnostics: the file-logging state (writer guard + reload handle).
        // Managing it keeps the non-blocking writer's WorkerGuard alive for
        // the whole process and lets the log commands change verbosity.
        .manage(log_state)
        // Window events → tray. The custom title-bar buttons route through
        // `request_window_close` / `request_window_minimize`; this global
        // handler covers the paths that DON'T go through a button:
        //   - CloseRequested: OS-level close (Alt+F4, window-manager X).
        //   - Resized + minimized: OS-level minimize (Win+M, taskbar, the
        //     window menu) — Tauri has no dedicated minimize event, so we
        //     watch resizes and check the minimized state.
        .on_window_event(|window, event| {
            // Hide the MAIN *webview* window to the tray (the same object the
            // title-bar command hides) — hiding the bare `&Window` the event
            // hands us leaves it on screen.
            let hide_main = || {
                if let Some(w) = window.app_handle().get_webview_window("main") {
                    let _ = w.hide();
                }
            };
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    // §15.3: remember the window's size + position before it
                    // closes (or hides to the tray). Captures the last visible
                    // geometry either way.
                    window_state::flush(window);
                    let app = window.app_handle();
                    let tray = app.state::<tray::TrayHandles>();
                    if tray.available && tray::pref_is_true(app, tray::CLOSE_TO_TRAY_PREF) {
                        api.prevent_close();
                        hide_main();
                    }
                }
                tauri::WindowEvent::Moved(_) => {
                    // §15.3: track position changes into the in-memory store.
                    window_state::remember(window);
                }
                tauri::WindowEvent::Resized(_) => {
                    // §15.3: track size changes into the in-memory store.
                    window_state::remember(window);
                    // Cheap state check first; only touch the DB pref when the
                    // window actually became minimized, not every resize frame.
                    if matches!(window.is_minimized(), Ok(true)) {
                        let app = window.app_handle();
                        let tray = app.state::<tray::TrayHandles>();
                        if tray.available && tray::pref_is_true(app, tray::MINIMIZE_TO_TRAY_PREF) {
                            // Clear the minimized state first so the window
                            // isn't stuck minimized-and-hidden, then tuck it
                            // into the tray.
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.unminimize();
                            }
                            hide_main();
                        }
                    }
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            frontend_log,
            commands::get_log_level,
            commands::set_log_level,
            commands::get_recent_logs,
            commands::collect_logs,
            commands::export_logs,
            commands::clear_logs,
            commands::logs_dir_path,
            commands::open_external_url,
            commands::list_calendars,
            commands::create_calendar,
            commands::delete_calendar,
            commands::get_events,
            commands::create_event,
            commands::update_event,
            commands::delete_event,
            commands::add_event_exdate,
            commands::get_event_by_id,
            commands::query_free_busy,
            commands::calendar_current_user_email,
            commands::respond_to_event,
            commands::get_task_by_id,
            commands::list_task_lists,
            commands::create_task_list,
            commands::delete_task_list,
            commands::reparent_task_list,
            commands::get_tasks,
            commands::create_task,
            commands::update_task,
            commands::delete_task,
            commands::get_sections,
            commands::task_list_members,
            commands::task_current_user,
            commands::task_list_shares,
            commands::task_search_users,
            commands::task_add_member,
            commands::task_remove_member,
            commands::task_set_member_right,
            commands::create_section,
            commands::update_section,
            commands::delete_section,
            commands::list_color_labels,
            commands::create_color_label,
            commands::get_or_create_ad_hoc_color_label,
            commands::update_color_label,
            commands::delete_color_label,
            commands::search,
            commands::list_upcoming_reminders,
            commands::invalidate_reminders,
            // §14.4 custom notification sounds: import a user audio
            // file into the content-addressed store, list/delete the
            // stored sounds, and preview one (the SoundPicker's Test
            // button). The sound *config* itself rides the generic
            // user_prefs commands.
            commands::import_sound,
            commands::list_custom_sounds,
            commands::preview_sound,
            commands::delete_custom_sound,
            commands::list_accounts,
            commands::list_accounts_missing_credentials,
            commands::set_account_secret,
            commands::reconnect_google_account,
            commands::reconnect_microsoft_account,
            commands::create_account,
            commands::delete_account,
            commands::rename_account,
            commands::test_caldav_connection,
            commands::test_ical_feed,
            commands::test_ews_connection,
            commands::test_vikunja_connection,
            commands::test_todoist_connection,
            commands::discover_ews_endpoint,
            commands::connect_google_account,
            commands::connect_microsoft_account,
            commands::set_container_name_override,
            commands::clear_container_name_override,
            commands::set_container_color_label,
            commands::set_section_color,
            commands::set_event_color,
            commands::rename_container,
            commands::get_user_pref,
            commands::set_user_pref,
            commands::delete_user_pref,
            commands::show_context_menu,
            // Phase 10a-2: contacts. The local adapter is always
            // present; external CardDAV-class adapters wire
            // themselves in via the registry's `external_contacts`
            // slot once they land (Phase 10b onward).
            commands::list_contact_lists,
            commands::create_contact_list,
            commands::delete_contact_list,
            commands::rename_contact_list,
            commands::get_contacts,
            commands::search_contacts,
            commands::create_contact,
            commands::update_contact,
            commands::delete_contact,
            // Phase 10g: contact photo CRUD. Each command takes
            // an optional `list_id` routing hint that the
            // frontend supplies from the rendered contact row;
            // missing hints fall back to the local adapter the
            // same way `delete_contact` does.
            commands::get_contact_photo,
            commands::set_contact_photo,
            commands::delete_contact_photo,
            // Phase 10j: contact sync scheduler. The
            // ContactSyncScheduler is registered into State during
            // setup() below so these commands can fan out to every
            // external adapter on user demand.
            commands::sync_contacts_now,
            commands::get_contacts_sync_status,
            // Phase 10k: Settings → Kontakte. Cache management +
            // configurable sync interval; the privacy notice is
            // a frontend-only concern routed through user_prefs.
            commands::clear_contacts_cache,
            commands::set_contacts_sync_interval,
            commands::set_contacts_include_read_only_on_sync,
            // Phase Sd (DESIGN.md §19): cross-device sync. The
            // orchestrator is registered in setup; these commands
            // are the user-facing surface.
            commands::configure_sync_adapter,
            commands::test_sync_adapter,
            commands::sync_now,
            commands::get_sync_status,
            commands::get_sync_adapter_summary,
            commands::set_sync_interval,
            // Phase Sf (DESIGN.md §19.11): onboarding flow.
            commands::preview_sync_target,
            commands::accept_remote_dataset,
            commands::adopt_local_dataset,
            // §19.7: rotate the dataset's E2E passphrase.
            commands::change_sync_passphrase,
            // §19.7: turn off E2E encryption on the dataset (in-place).
            commands::disable_sync_encryption,
            // §19.7: turn on E2E encryption on an existing dataset
            // (in-place re-encryption of every log + snapshot).
            commands::enable_sync_encryption,
            // §19.7: adopt encryption that was activated on another
            // device — pure passphrase-unlock, no migration.
            commands::adopt_remote_encryption,
            // §19.10: stale-device resume.
            commands::resume_stale_device,
            // Phase Sg (DESIGN.md §19.10): snapshot + log
            // compaction. The auto-trigger lives inside
            // `sync_now`; this is the manual override.
            commands::compact_now,
            // Phase Sh (DESIGN.md §19.3): conflict surfacing +
            // resolution. The merge logic in the applier records
            // conflicts; these commands let the frontend read and
            // resolve them.
            commands::list_sync_conflicts,
            commands::get_sync_conflicts_count,
            commands::resolve_sync_conflict,
            // Phase Sm (DESIGN.md §19.5): SFTP host-key trust
            // dialog. Preview reads the server's fingerprint
            // without authenticating; trust commits a user-
            // confirmed pin.
            commands::preview_sftp_host_key,
            // §19.6 Dropbox OAuth dance.
            commands::connect_dropbox_oauth,
            commands::has_dropbox_refresh_token,
            // §19.6 Google Drive OAuth dance.
            commands::connect_googledrive_oauth,
            commands::has_googledrive_refresh_token,
            commands::trust_sftp_host_key,
            commands::forget_sftp_host_key,
            commands::get_pinned_sftp_host_key,
            // Phase Sm follow-up (DESIGN.md §19.9): the detailed
            // sync protocol. `list_sync_log_entries` reads the
            // history, `clear_sync_log` scrubs it.
            commands::list_sync_log_entries,
            commands::clear_sync_log,
            // §11 Videoconference adapters. Per-account CRUD over
            // the registered VcAdapter; the per-provider adapters
            // are still stubs that return "unsupported" until the
            // REST layers land.
            commands::test_vc_connection,
            commands::create_meeting,
            commands::get_meeting,
            commands::delete_meeting,
            // §20.10 Settings → Plugins panel. list_plugins
            // surfaces metadata + enabled state;
            // set_plugin_enabled persists the toggle + re-syncs
            // the affected accounts in the AdapterRegistry.
            // §20.7 install: inspect previews the .aperio
            // archive before the user confirms; install
            // extracts under plugins/user/, loads, re-registers
            // matching accounts.
            commands::list_plugins,
            commands::set_plugin_enabled,
            commands::inspect_plugin_archive,
            commands::install_plugin_archive,
            commands::uninstall_plugin,
            // §20.8 — list plugins announced by other devices
            // that aren't installed locally; drives the
            // "Plugin benötigt" section in the Settings panel.
            commands::list_remote_plugins,
            // List plugins the manager refused to load at
            // startup (ABI mismatch, dlopen failure, …).
            // Drives the "Konnten nicht geladen werden"-
            // section so stale community plugins after an
            // Aperio update don't silently disappear.
            commands::list_failed_plugins,
            // CACHE-3 — manual external-cache refresh + status for the
            // toolbar indicator.
            commands::refresh_external_cache,
            commands::get_cache_refresh_status,
            // System tray: availability gate + close/minimize routing
            // (the title-bar buttons call these so close/minimize can hide
            // to the tray when the user opted in).
            tray::tray_available,
            tray::request_window_close,
            tray::request_window_minimize,
            tray::set_tray_labels,
        ])
        .setup(move |app| {
            // Spawn the reminder scheduler on the Tauri/tokio runtime
            // and register its handle so command modules can call
            // `invalidate()` after every mutation.
            let scheduler = ReminderScheduler::spawn(
                db_for_scheduler.clone(),
                Arc::clone(&registry_for_scheduler),
                sounds_dir_for_scheduler.clone(),
                audio_for_scheduler.clone(),
                app.handle().clone(),
            );
            app.manage(scheduler);

            // Phase 10j: contact sync scheduler. Boots its own
            // periodic worker (default 60 min, configurable via
            // user_prefs) plus runs a one-shot pass shortly after
            // app start. Stored in State so the
            // `sync_contacts_now` command can drive a manual pass
            // through the same in-flight guard.
            let contact_sync = ContactSyncScheduler::spawn(
                Arc::clone(&registry_for_contact_sync),
                db_for_contact_sync.clone(),
                app.handle().clone(),
            );
            app.manage(contact_sync);

            // CACHE-3: external-cache warm/periodic refresher. Runs a
            // wide-window warm pass shortly after boot and on a
            // prefs-driven interval, deduplicated against the per-read
            // SWR path. Stored in State so `refresh_external_cache` can
            // drive a manual pass.
            let cache_refresher = cache_refresh::CacheRefresher::spawn(
                Arc::clone(&registry_for_cache_refresh),
                Arc::clone(&cache_for_refresh),
                Arc::clone(&coord_for_refresh),
                db_for_cache_refresh.clone(),
                app.handle().clone(),
            );
            app.manage(cache_refresher);

            // Phase Se: sync scheduler. Spawns the periodic worker
            // + listens on the kick `Notify` shared with the event-
            // log writer, so any local mutation triggers a debounced
            // push round. Registered into State so the
            // `configure_sync_adapter` / `set_sync_interval` commands
            // can wake the loop without an indirection through a
            // global.
            let sync_scheduler = SyncScheduler::spawn(
                Arc::clone(&scheduler_orchestrator),
                scheduler_db.clone(),
                Arc::clone(&scheduler_kick_for_setup),
                app.handle().clone(),
            );
            app.manage(sync_scheduler);

            // Shared state for the native context-menu popups. The
            // global `on_menu_event` handler below routes selections
            // from any popup back to the awaiting command via a
            // oneshot channel held in this state.
            app.manage(commands::ContextMenuState::new());
            let handle = app.handle().clone();
            app.on_menu_event(move |_app, event| {
                let state = handle.state::<commands::ContextMenuState>();
                let mut guard = state.pending.lock().expect("ctx menu poisoned");
                if let Some((_, tx)) = guard.take() {
                    let _ = tx.send(event.id().as_ref().to_string());
                }
            });

            // System tray (close/minimize to tray). Best-effort: a desktop
            // without a tray reports `available = false` and the Settings
            // toggles disable themselves. Built here, after the window
            // exists.
            let tray_handles = tray::build(app.handle());
            app.manage(tray_handles);

            // §15.3: restore the saved window size + position (device-local,
            // `app_config.json`). Register the geometry store FIRST so the
            // resize/move events that `set_size` / `maximize` emit below can
            // find it.
            app.manage(window_state::Store::default());
            if let Some(win) = app.get_webview_window("main") {
                // Restore the saved geometry when there is one; otherwise the
                // window keeps the config default (1280×800 logical).
                if let Some(geom) = window_state::load() {
                    window_state::restore(&win, &geom);
                    if let Some(store) = app.try_state::<window_state::Store>() {
                        *store.lock().expect("window-state mutex poisoned") = Some(geom);
                    }
                }
                // Either way, shrink the window if it ends up larger than the
                // monitor it's on — at high display scaling even the default
                // size exceeds the screen, leaving the toolbar's right edge
                // (the sync indicator) off-canvas. The resize it may emit is
                // picked up by the geometry store registered just above.
                window_state::fit_to_current_monitor(&win);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri app");

    // Phase Se: app-exit hook. DESIGN.md §19.8 mandates pushing
    // pending logs before the process terminates so the next device
    // doesn't see a multi-day-old view of this one. We use the
    // push-only variant (`push_now`) rather than `sync_now` because
    // fetching during shutdown is wasted work — the applied events
    // wouldn't make it into the UI before the window closes.
    //
    // `block_on` here is fine: the run callback runs on the main
    // thread after the event loop has stopped accepting new work.
    // A bounded timeout via `tokio::time::timeout` keeps a hung
    // network drive from stalling the close indefinitely.
    app.run(move |_app, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            if !orchestrator_for_exit.status().configured {
                return;
            }
            info!("running app-exit sync push");
            let orchestrator = Arc::clone(&orchestrator_for_exit);
            // 10s ceiling matches the user's patience window for
            // "I just clicked X" — Phase Sj's network adapters
            // will tune this per-adapter.
            tauri::async_runtime::block_on(async move {
                let push = orchestrator.push_now();
                match tokio::time::timeout(std::time::Duration::from_secs(10), push).await {
                    Ok(Ok(count)) => {
                        info!(pushed = count, "app-exit sync push complete",);
                    }
                    Ok(Err(err)) => {
                        warn!(?err, "app-exit sync push failed");
                    }
                    Err(_) => {
                        warn!("app-exit sync push timed out after 10s");
                    }
                }
            });
        }
    });
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        name: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Mirror a webview `console.*` call into the Rust tracing stream (target
/// `aperio::webview`) so frontend logs land in the same sinks as backend logs
/// — the dev terminal AND the persistent log file. The frontend forwards in
/// every build now (release included) so a user's exported log captures
/// frontend errors too.
#[tauri::command]
fn frontend_log(level: String, message: String) {
    // Map to INFO and above so everything is visible under the default
    // (INFO) log filter — the whole point is to surface webview output
    // without raising the global level.
    match level.as_str() {
        "error" => tracing::error!(target: "aperio::webview", "{message}"),
        "warn" => tracing::warn!(target: "aperio::webview", "{message}"),
        other => tracing::info!(target: "aperio::webview", level = %other, "{message}"),
    }
}

#[derive(serde::Serialize)]
struct AppInfo {
    name: String,
    version: String,
}
