//! Aperio — Tauri backend.
//!
//! Phase 1 wires up the local SQLite store, the local calendar/task
//! adapter, and a first round of Tauri commands for CRUD. The plugin
//! manager, sync engine, and external adapters arrive in later phases.

pub mod accounts;
pub mod commands;
pub mod db;
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

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(local_adapter)
        .manage(registry)
        .manage(db)
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
            commands::list_accounts,
            commands::create_account,
            commands::delete_account,
            commands::test_caldav_connection,
            commands::test_ical_feed,
            commands::test_ews_connection,
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
