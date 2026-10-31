//! File import (DESIGN-file-import.md): the formats the plugins read, and a
//! file read through the plugin that reads its format. The rules live in
//! `host_core::file_import`; the phone's host calls the same two.

use std::path::PathBuf;
use std::sync::Arc;

use host_core::file_import::ImportFormat;
use plugin_core::PluginManager;
use tauri::State;

use super::CommandResult;

/// The formats the enabled plugins read, for the open dialog's filter, named
/// in `lang`.
#[tauri::command]
pub async fn file_import_formats(
    plugin_manager: State<'_, Arc<PluginManager>>,
    lang: String,
) -> CommandResult<Vec<ImportFormat>> {
    Ok(host_core::file_import::import_formats(
        &plugin_manager,
        &lang,
    ))
}

/// Read a file dropped on the window for an import: a drop brings the file's
/// name and bytes (`content`, base64), never its path.
#[tauri::command]
pub async fn read_import_content(
    plugin_manager: State<'_, Arc<PluginManager>>,
    file_name: String,
    content: String,
) -> CommandResult<cal_core::FileImport> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content.as_bytes())
        .map_err(|err| {
            cal_core::Error::InvalidInput(format!("the dropped file did not arrive: {err}"))
        })?;
    Ok(host_core::file_import::import_content(&plugin_manager, &file_name, &bytes).await?)
}

/// Read the file at `path` (the open dialog's choice) for an import. Nothing
/// is written: the surface fills the event editor from the answer, or lists
/// it.
#[tauri::command]
pub async fn read_import_file(
    plugin_manager: State<'_, Arc<PluginManager>>,
    path: PathBuf,
) -> CommandResult<cal_core::FileImport> {
    Ok(host_core::file_import::import_file(&plugin_manager, &path).await?)
}
