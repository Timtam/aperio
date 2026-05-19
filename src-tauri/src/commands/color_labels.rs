//! Color-label commands.

use cal_adapter_local::LocalAdapter;
use cal_core::ColorLabel;
use serde::Deserialize;
use tauri::State;

use super::CommandResult;

#[tauri::command]
pub async fn list_color_labels(adapter: State<'_, LocalAdapter>) -> CommandResult<Vec<ColorLabel>> {
    Ok(adapter.list_color_labels()?)
}

#[derive(Debug, Deserialize)]
pub struct CreateColorLabelRequest {
    pub name: String,
    pub hex: String,
}

#[tauri::command]
pub async fn create_color_label(
    adapter: State<'_, LocalAdapter>,
    request: CreateColorLabelRequest,
) -> CommandResult<ColorLabel> {
    Ok(adapter.create_color_label(&request.name, &request.hex)?)
}

#[tauri::command]
pub async fn update_color_label(
    adapter: State<'_, LocalAdapter>,
    label: ColorLabel,
) -> CommandResult<ColorLabel> {
    Ok(adapter.update_color_label(label)?)
}

#[tauri::command]
pub async fn delete_color_label(adapter: State<'_, LocalAdapter>, id: String) -> CommandResult<()> {
    Ok(adapter.delete_color_label(&id)?)
}
