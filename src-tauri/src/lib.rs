//! Aperio — Tauri backend (Phase 0).
//!
//! At this phase the backend only carries the app skeleton and the
//! portable data-path resolver. The SQLite layer, Tauri commands, sync
//! engine, and plugin manager arrive in later phases.

mod paths;

pub use paths::{resolve_data_dir, DataDirKind, DataDirResolution};

use tracing::info;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let data_dir = resolve_data_dir();
    info!(
        kind = ?data_dir.kind,
        path = %data_dir.path.display(),
        "resolved data directory"
    );

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_info])
        .setup(move |_app| {
            // Later phases register the SQLite layer and plugin manager here.
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
