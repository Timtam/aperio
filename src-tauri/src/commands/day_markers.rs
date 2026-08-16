//! Day-marker commands.
//!
//! Like colour labels next door, day markers live exclusively in local SQLite
//! — no external provider models "how was Tuesday" — so every mutation is by
//! definition a local one and unconditionally emits a sync event. No account
//! routing, no LOCAL_ID guard.

use std::sync::Arc;

use adapter_local::LocalAdapter;
use cal_core::{ColorLabelId, DayLog, DayMarker};
use chrono::NaiveDate;
use serde::Deserialize;
use sync_core::{EventPayload, IdPayload, SyncEvent};
use tauri::State;

use super::CommandResult;
use crate::event_log::EventLogWriter;

/// Parse a `YYYY-MM-DD` coming from the frontend.
///
/// The frontends speak local day keys everywhere (see `shared/dateKey.ts`), so
/// this is the one place the string becomes a date. A malformed one is the
/// caller's bug, not a state to render around.
fn parse_day(raw: &str) -> CommandResult<NaiveDate> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|_| cal_core::Error::InvalidInput(format!("not a YYYY-MM-DD day: {raw}")).into())
}

#[tauri::command]
pub async fn list_day_markers(adapter: State<'_, LocalAdapter>) -> CommandResult<Vec<DayMarker>> {
    Ok(adapter.list_day_markers()?)
}

#[derive(Debug, Deserialize)]
pub struct CreateDayMarkerRequest {
    pub name: String,
    pub symbol: Option<String>,
    pub color_label: Option<String>,
}

#[tauri::command]
pub async fn create_day_marker(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    request: CreateDayMarkerRequest,
) -> CommandResult<DayMarker> {
    let color = request.color_label.map(ColorLabelId::new);
    let marker =
        adapter.create_day_marker(&request.name, request.symbol.as_deref(), color.as_ref())?;
    if let Ok(fields) = serde_json::to_value(&marker) {
        event_log.append(SyncEvent::DayMarkerWritten(EventPayload {
            id: marker.id.clone(),
            fields,
        }));
    }
    Ok(marker)
}

/// Write a marker back whole — rename, re-symbol, recolour, reorder.
///
/// The caller sends the marker it already has, so a reorder is the same call
/// as a rename and the frontend needs one code path for both.
#[tauri::command]
pub async fn update_day_marker(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    marker: DayMarker,
) -> CommandResult<DayMarker> {
    let stamped = DayMarker {
        updated_at: chrono::Utc::now(),
        ..marker
    };
    adapter.write_day_marker(&stamped)?;
    if let Ok(fields) = serde_json::to_value(&stamped) {
        event_log.append(SyncEvent::DayMarkerWritten(EventPayload {
            id: stamped.id.clone(),
            fields,
        }));
    }
    Ok(stamped)
}

#[tauri::command]
pub async fn delete_day_marker(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: String,
) -> CommandResult<()> {
    adapter.delete_day_marker(&id)?;
    event_log.append(SyncEvent::DayMarkerDeleted(IdPayload { id }));
    Ok(())
}

/// What one day was marked with. An untouched day comes back as an empty log,
/// never as an error or a null — the callers render the same thing either way.
#[tauri::command]
pub async fn day_log(adapter: State<'_, LocalAdapter>, day: String) -> CommandResult<DayLog> {
    Ok(adapter.day_log(parse_day(&day)?)?)
}

/// Every logged day in an inclusive range — what a week or month view asks for
/// once, instead of one call per day.
#[tauri::command]
pub async fn day_logs_in_range(
    adapter: State<'_, LocalAdapter>,
    from: String,
    to: String,
) -> CommandResult<Vec<DayLog>> {
    Ok(adapter.day_logs_in_range(parse_day(&from)?, parse_day(&to)?)?)
}

/// Set a day's log. Emitted even when it empties the day: the receiving side's
/// `set_day_log` deletes the row for an empty log, which is how "I unticked
/// the last one" reaches the other device.
#[tauri::command]
pub async fn set_day_log(
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    log: DayLog,
) -> CommandResult<DayLog> {
    let stamped = DayLog {
        updated_at: chrono::Utc::now(),
        ..log
    };
    adapter.set_day_log(&stamped)?;
    if let Ok(fields) = serde_json::to_value(&stamped) {
        event_log.append(SyncEvent::DayLogSet(EventPayload {
            id: stamped.day.format("%Y-%m-%d").to_string(),
            fields,
        }));
    }
    Ok(stamped)
}
