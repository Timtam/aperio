//! Conflict-resolution Tauri commands (DESIGN.md §19.3, Phase Sh).
//!
//! Three verbs:
//!
//!   - `list_sync_conflicts()` — pending conflicts for the
//!     Conflicts dialog. Read-only.
//!   - `get_sync_conflicts_count()` — fast badge counter for the
//!     status bar (no row payload).
//!   - `resolve_sync_conflict(id, choice)` — applies the user's
//!     decision. `KeepLocal` is a no-op data-wise; `TakeRemote`
//!     writes the remote value into the row + emits an updated
//!     event so other devices converge; `SaveBoth` is not yet
//!     implemented (returns `unsupported`) — the "fork into two
//!     rows" path needs UI-side glue that's part of Phase Si.
//!
//! After every resolution the command emits a
//! `sync-conflicts-changed` Tauri event so the frontend can
//! re-fetch the list without polling.

use std::sync::Arc;

use cal_adapter_local::LocalAdapter;
use serde::Deserialize;
use sync_core::{EventPayload, SyncEvent};
use tauri::{AppHandle, Emitter, Runtime, State};

use super::{CommandError, CommandResult};
use crate::conflicts::{
    ConflictKind, ConflictRecord, ConflictsError, ConflictsRepo,
    ResolutionChoice,
};
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;

/// Mapping of [`ConflictsError`] → frontend error envelope.
impl From<ConflictsError> for CommandError {
    fn from(err: ConflictsError) -> Self {
        let code: &'static str = match &err {
            ConflictsError::NotFound(_) => "not_found",
            ConflictsError::InvalidKind(_)
            | ConflictsError::InvalidResolution(_) => "invalid_input",
            ConflictsError::Sqlite(_) => "internal",
        };
        CommandError {
            code,
            message: err.to_string(),
        }
    }
}

/// Read every unresolved conflict for the dialog.
#[tauri::command]
pub async fn list_sync_conflicts(
    db: State<'_, DbHandle>,
) -> CommandResult<Vec<ConflictRecord>> {
    let shared = db.shared();
    let repo = ConflictsRepo::new(&shared);
    Ok(repo.list_unresolved()?)
}

/// Pending count for the status badge. Cheap (single COUNT(*)
/// query) so the indicator can poll on a short cadence without
/// pulling the full list.
#[tauri::command]
pub async fn get_sync_conflicts_count(
    db: State<'_, DbHandle>,
) -> CommandResult<usize> {
    let shared = db.shared();
    let repo = ConflictsRepo::new(&shared);
    Ok(repo.unresolved_count()?)
}

/// Request body for [`resolve_sync_conflict`]. The frontend dialog
/// picks one of the three choices on a button click; the
/// `Deserialize` boundary is what validates that a stray choice
/// string doesn't slip through.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionRequest {
    KeepLocal,
    TakeRemote,
    SaveBoth,
}

impl From<ResolutionRequest> for ResolutionChoice {
    fn from(r: ResolutionRequest) -> Self {
        match r {
            ResolutionRequest::KeepLocal => ResolutionChoice::KeepLocal,
            ResolutionRequest::TakeRemote => ResolutionChoice::TakeRemote,
            ResolutionRequest::SaveBoth => ResolutionChoice::SaveBoth,
        }
    }
}

/// Apply the user's resolution and broadcast a refresh.
///
/// `KeepLocal`: pure bookkeeping — flip `resolved = 1`. No data
/// change; the next sync round will emit an updated event for
/// other devices with our local value so they converge to ours.
///
/// `TakeRemote`: write the stored `remote_value` into the row +
/// emit a fresh `*Updated` SyncEvent so the change ripples out.
/// The new updated_at is `now`, which beats the original conflict
/// timestamp — so a follow-up incoming patch wouldn't immediately
/// re-flag the same field.
///
/// `SaveBoth`: deferred. The "fork into two rows" flow needs the
/// frontend to gather a new id for the copy + handle all
/// downstream references; that lands with Phase Si's full
/// conflict dialog.
#[tauri::command]
pub async fn resolve_sync_conflict<R: Runtime>(
    app: AppHandle<R>,
    db: State<'_, DbHandle>,
    adapter: State<'_, LocalAdapter>,
    event_log: State<'_, Arc<EventLogWriter>>,
    id: i64,
    choice: ResolutionRequest,
) -> CommandResult<()> {
    let shared = db.shared();
    let repo = ConflictsRepo::new(&shared);
    let record = repo.get(id)?;
    let choice: ResolutionChoice = choice.into();

    match choice {
        ResolutionChoice::KeepLocal => {
            // No data side-effect. The merge already kept the local
            // value; resolution just marks the conflict closed.
            repo.mark_resolved(id, choice)?;
        }
        ResolutionChoice::TakeRemote => {
            apply_take_remote(&adapter, &record, &event_log)?;
            repo.mark_resolved(id, choice)?;
        }
        ResolutionChoice::SaveBoth => {
            return Err(CommandError {
                code: "unsupported",
                message: "save_both is not yet implemented".into(),
            });
        }
    }

    // Tell the frontend the list moved so the dialog can refresh
    // without polling. Payload is intentionally empty — listeners
    // re-query via `list_sync_conflicts`.
    let _ = app.emit("sync-conflicts-changed", ());
    Ok(())
}

/// Apply the `TakeRemote` resolution: write `remote_value` into
/// the local row + emit a SyncEvent so other devices converge.
///
/// We do this by reading the current row, patching the one field,
/// upserting via the existing `*_from_sync` helpers, and emitting
/// the corresponding `*Updated` envelope through the writer.
fn apply_take_remote(
    adapter: &LocalAdapter,
    record: &ConflictRecord,
    event_log: &EventLogWriter,
) -> CommandResult<()> {
    // `remote_value` is JSON-encoded — null when the remote
    // cleared the field. Parse it back to a `serde_json::Value`.
    let remote_value: serde_json::Value = match &record.remote_value {
        Some(raw) => serde_json::from_str(raw).map_err(|err| CommandError {
            code: "internal",
            message: format!("decode remote value: {err}"),
        })?,
        None => serde_json::Value::Null,
    };

    // Branch on row_kind to load the typed row, patch, re-upsert,
    // and emit. The full-row patch in the event payload is the
    // same shape the writer would have emitted from a regular
    // edit — receivers reuse the same merge logic.
    match record.row_kind {
        ConflictKind::Event => {
            let mut row = adapter
                .get_event_by_id(&record.row_id)
                .map_err(|err| CommandError {
                    code: "internal",
                    message: err.to_string(),
                })?
                .ok_or(CommandError {
                    code: "not_found",
                    message: "event for conflict no longer exists".into(),
                })?;
            patch_field(&mut row, &record.field, &remote_value)?;
            row.updated_at = chrono::Utc::now();
            adapter
                .upsert_event_from_sync(&row)
                .map_err(|err| CommandError {
                    code: "internal",
                    message: err.to_string(),
                })?;
            event_log.append(SyncEvent::EventUpdated(EventPayload {
                id: row.id.clone(),
                fields: serde_json::to_value(&row).unwrap_or_default(),
            }));
        }
        ConflictKind::Task => {
            let mut row = adapter
                .get_task_by_id(&record.row_id)
                .map_err(|err| CommandError {
                    code: "internal",
                    message: err.to_string(),
                })?
                .ok_or(CommandError {
                    code: "not_found",
                    message: "task for conflict no longer exists".into(),
                })?;
            patch_field(&mut row, &record.field, &remote_value)?;
            row.updated_at = chrono::Utc::now();
            adapter
                .upsert_task_from_sync(&row)
                .map_err(|err| CommandError {
                    code: "internal",
                    message: err.to_string(),
                })?;
            event_log.append(SyncEvent::TaskUpdated(EventPayload {
                id: row.id.clone(),
                fields: serde_json::to_value(&row).unwrap_or_default(),
            }));
        }
        ConflictKind::TaskList => {
            let mut row = adapter
                .get_task_list_by_id(&record.row_id)
                .map_err(|err| CommandError {
                    code: "internal",
                    message: err.to_string(),
                })?
                .ok_or(CommandError {
                    code: "not_found",
                    message: "task list for conflict no longer exists".into(),
                })?;
            patch_field(&mut row, &record.field, &remote_value)?;
            adapter
                .upsert_task_list_from_sync(&row)
                .map_err(|err| CommandError {
                    code: "internal",
                    message: err.to_string(),
                })?;
            event_log.append(SyncEvent::TaskListUpdated(EventPayload {
                id: row.id.clone(),
                fields: serde_json::to_value(&row).unwrap_or_default(),
            }));
        }
        ConflictKind::Calendar => {
            let mut row = adapter
                .get_calendar_by_id(&record.row_id)
                .map_err(|err| CommandError {
                    code: "internal",
                    message: err.to_string(),
                })?
                .ok_or(CommandError {
                    code: "not_found",
                    message: "calendar for conflict no longer exists".into(),
                })?;
            patch_field(&mut row, &record.field, &remote_value)?;
            adapter
                .upsert_calendar_from_sync(&row)
                .map_err(|err| CommandError {
                    code: "internal",
                    message: err.to_string(),
                })?;
            event_log.append(SyncEvent::CalendarUpdated(EventPayload {
                id: row.id.clone(),
                fields: serde_json::to_value(&row).unwrap_or_default(),
            }));
        }
        ConflictKind::ColorLabel => {
            let mut row = adapter
                .get_color_label_by_id(&record.row_id)
                .map_err(|err| CommandError {
                    code: "internal",
                    message: err.to_string(),
                })?
                .ok_or(CommandError {
                    code: "not_found",
                    message: "color label for conflict no longer exists".into(),
                })?;
            patch_field(&mut row, &record.field, &remote_value)?;
            adapter
                .upsert_color_label_from_sync(&row)
                .map_err(|err| CommandError {
                    code: "internal",
                    message: err.to_string(),
                })?;
            event_log.append(SyncEvent::ColorLabelUpdated(EventPayload {
                id: row.id.0.clone(),
                fields: serde_json::to_value(&row).unwrap_or_default(),
            }));
        }
    }
    Ok(())
}

/// Generic single-field patch via serde — round-trips the row
/// through `Value`, writes the field, deserialises back. Used by
/// every `apply_take_remote` branch so the per-type code stays
/// boilerplate-free.
fn patch_field<T>(
    row: &mut T,
    field: &str,
    value: &serde_json::Value,
) -> CommandResult<()>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let mut serialised = serde_json::to_value(&*row).map_err(|err| {
        CommandError {
            code: "internal",
            message: format!("serialise row for patch: {err}"),
        }
    })?;
    if let Some(obj) = serialised.as_object_mut() {
        obj.insert(field.to_string(), value.clone());
    }
    *row = serde_json::from_value(serialised).map_err(|err| CommandError {
        code: "internal",
        message: format!("deserialise patched row: {err}"),
    })?;
    Ok(())
}
