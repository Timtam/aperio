//! Color-label commands.
//!
//! Color labels live exclusively in local SQLite (§8) — no external
//! provider has an equivalent concept, so every mutation is by
//! definition a LOCAL one and unconditionally emits a sync event.
//! That's the simplest case in the event-log integration: no
//! account routing, no LOCAL_ID guard, just always-on emission.

use std::sync::Arc;

use cal_adapter_local::LocalAdapter;
use cal_core::ColorLabel;
use serde::Deserialize;
use sync_core::{EventPayload, IdPayload, SyncEvent};
use tauri::State;

use super::CommandResult;
use crate::event_log::EventLogWriter;

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
    event_log: State<'_, Arc<EventLogWriter>>,
    request: CreateColorLabelRequest,
) -> CommandResult<ColorLabel> {
    let label = adapter.create_color_label(&request.name, &request.hex)?;
    if let Ok(fields) = serde_json::to_value(&label) {
        event_log.append(SyncEvent::ColorLabelCreated(EventPayload {
            id: label.id.as_str().to_string(),
            fields,
        }));
    }
    Ok(label)
}

#[tauri::command]
pub async fn update_color_label(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    label: ColorLabel,
) -> CommandResult<ColorLabel> {
    let updated = adapter.update_color_label(label)?;
    if let Ok(fields) = serde_json::to_value(&updated) {
        event_log.append(SyncEvent::ColorLabelUpdated(EventPayload {
            id: updated.id.as_str().to_string(),
            fields,
        }));
    }
    Ok(updated)
}

#[tauri::command]
pub async fn delete_color_label(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
) -> CommandResult<()> {
    adapter.delete_color_label(&id)?;
    event_log.append(SyncEvent::ColorLabelDeleted(IdPayload { id: id.clone() }));
    Ok(())
}
