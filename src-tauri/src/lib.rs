//! Aperio — Tauri backend.
//!
//! Phase 1 wires up the local SQLite store, the local calendar/task
//! adapter, and a first round of Tauri commands for CRUD. The plugin
//! manager, sync engine, and external adapters arrive in later phases.

pub mod accounts;
pub mod commands;
pub mod contact_sync;
pub mod db;
pub mod event_log;
pub mod overrides;
mod paths;
mod platform;
pub mod registry;
pub mod reminders;
pub mod secrets;
pub mod user_prefs;

pub use db::{DbError, DbHandle, DbResult, SharedConn};
pub use paths::{resolve_data_dir, DataDirKind, DataDirResolution};

use cal_adapter_local::LocalAdapter;
use contact_sync::ContactSyncScheduler;
use event_log::EventLogWriter;
use registry::AdapterRegistry;
use reminders::ReminderScheduler;
use std::sync::Arc;
use tauri::Manager;
use tracing::info;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    // Register the AUMID and pin it to this process before tauri starts
    // — toast notifications inherit it at process scope. On non-Windows
    // platforms this is a no-op.
    platform::setup();

    let data_dir = resolve_data_dir();
    info!(
        kind = ?data_dir.kind,
        path = %data_dir.path.display(),
        "resolved data directory"
    );

    let db_path = data_dir.path.join("aperio.sqlite");
    let db = DbHandle::open(&db_path).expect("failed to open local database");
    info!(path = %db_path.display(), "opened local database");

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
    let registry = Arc::new(AdapterRegistry::new());
    {
        let shared = db.shared();
        let repo = accounts::AccountsRepo::new(&shared);
        registry.bootstrap(&repo);
    }

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
    let device_id =
        EventLogWriter::load_or_mint_device_id(&db.shared());
    info!(
        device_id = %device_id,
        "event-log writer device id",
    );
    let event_log_writer =
        EventLogWriter::spawn(data_dir.path.clone(), device_id);

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(local_adapter)
        .manage(registry)
        .manage(db)
        .manage(event_log_writer)
        .invoke_handler(tauri::generate_handler![
            app_info,
            commands::list_calendars,
            commands::create_calendar,
            commands::delete_calendar,
            commands::get_events,
            commands::create_event,
            commands::update_event,
            commands::delete_event,
            commands::add_event_exdate,
            commands::get_event_by_id,
            commands::get_task_by_id,
            commands::list_task_lists,
            commands::create_task_list,
            commands::delete_task_list,
            commands::get_tasks,
            commands::create_task,
            commands::update_task,
            commands::delete_task,
            commands::list_color_labels,
            commands::create_color_label,
            commands::update_color_label,
            commands::delete_color_label,
            commands::search,
            commands::list_upcoming_reminders,
            commands::invalidate_reminders,
            commands::list_accounts,
            commands::create_account,
            commands::delete_account,
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
        ])
        .setup(move |app| {
            // Spawn the reminder scheduler on the Tauri/tokio runtime
            // and register its handle so command modules can call
            // `invalidate()` after every mutation.
            let scheduler = ReminderScheduler::spawn(
                db_for_scheduler.clone(),
                Arc::clone(&registry_for_scheduler),
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

            // Shared state for the native context-menu popups. The
            // global `on_menu_event` handler below routes selections
            // from any popup back to the awaiting command via a
            // oneshot channel held in this state.
            app.manage(commands::ContextMenuState::new());
            let handle = app.handle().clone();
            app.on_menu_event(move |_app, event| {
                let state = handle.state::<commands::ContextMenuState>();
                let mut guard = state.pending.lock().expect("ctx menu poisoned");
                if let Some(tx) = guard.take() {
                    let _ = tx.send(event.id().as_ref().to_string());
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start the Tauri app");
}

fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .init();
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        name: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

#[derive(serde::Serialize)]
struct AppInfo {
    name: String,
    version: String,
}
