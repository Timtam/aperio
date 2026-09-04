//! Reminders Aperio keeps for one event and tells no provider about
//! (migration `0043_event_local_reminders.sql`).
//!
//! A reminder normally rides ON the event, where the provider stores it and
//! every other client of the calendar rings too. These stay here: they fire in
//! Aperio, reach the user's other devices through Aperio's own sync, and reach
//! nobody else on a shared calendar.
//!
//! Like the event-group commands next door there is no account routing and no
//! LOCAL_ID guard: writing one changes no event, so every mutation is by
//! definition a local one and always emits a sync event. Ids are the SERIES
//! MASTER's, matching groups, meetings and colour overrides.

use std::sync::Arc;

use host_core::event_reminders::EventRemindersRepo;
use sync_core::{EventPayload, SyncEvent};
use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;
use crate::reminders::SchedulerHandle;

fn map_err(err: host_core::event_reminders::EventRemindersError) -> CommandError {
    CommandError {
        code: "internal",
        message: err.to_string(),
    }
}

/// Every private-reminder row. Small by nature — one per event the user gave a
/// private reminder — and the editor needs to know which events have one
/// before it can show them, so it reads the set rather than asking per event.
#[tauri::command]
pub async fn list_event_local_reminders(
    db: State<'_, DbHandle>,
) -> CommandResult<Vec<cal_core::EventLocalReminders>> {
    EventRemindersRepo::new(&db.shared())
        .list()
        .map_err(map_err)
}

/// Write one event's private reminders and tell the other devices.
///
/// `title` and `starts_at` are the event's CURRENT signature, written down so
/// the row can find its event again after the provider remints the id (see the
/// migration). An empty list is stored, not deleted: it is the record of a
/// decision, and the last writer needs something to win against.
#[tauri::command]
pub async fn set_event_local_reminders(
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    scheduler: State<'_, SchedulerHandle>,
    calendar_id: String,
    event_id: String,
    reminders: Vec<cal_core::Reminder>,
    title: String,
    starts_at: String,
) -> CommandResult<cal_core::EventLocalReminders> {
    let now = chrono::Utc::now().to_rfc3339();
    let row = EventRemindersRepo::new(&db.shared())
        .set(
            &calendar_id,
            &event_id,
            &reminders,
            &title,
            &starts_at,
            &now,
        )
        .map_err(map_err)?;
    if let Ok(fields) = serde_json::to_value(&row) {
        event_log.append(SyncEvent::EventLocalRemindersSet(EventPayload {
            // The event IS the identity; there is no id of its own to mint.
            id: format!("{} {}", row.calendar_id, row.event_id),
            fields,
        }));
    }
    // The scheduler answers external events from a cached snapshot, so without
    // this the reminder just set would not fire until the cache aged out.
    scheduler.invalidate_external_cache();
    scheduler.invalidate();
    Ok(row)
}

/// Point a row at the id its event carries now.
///
/// Deliberately NOT emitted and the row's timestamp deliberately not moved —
/// see `EventRemindersRepo::heal` for why broadcasting a repair was harmful.
/// Every device sees the same events and repairs its own copy.
#[tauri::command]
pub async fn heal_event_local_reminders(
    db: State<'_, DbHandle>,
    scheduler: State<'_, SchedulerHandle>,
    calendar_id: String,
    old_event_id: String,
    new_event_id: String,
) -> CommandResult<bool> {
    let moved = EventRemindersRepo::new(&db.shared())
        .heal(&calendar_id, &old_event_id, &new_event_id)
        .map_err(map_err)?;
    if moved {
        // The row now names an event the scan can find, so what it asks for
        // can fire again.
        scheduler.invalidate_external_cache();
        scheduler.invalidate();
    }
    Ok(moved)
}

/// Write down what the event looks like now, so the signature keeps matching
/// after the user renames or moves the appointment. Local and silent, like the
/// repair above.
#[tauri::command]
pub async fn refresh_event_local_reminder_signature(
    db: State<'_, DbHandle>,
    calendar_id: String,
    event_id: String,
    title: String,
    starts_at: String,
) -> CommandResult<()> {
    EventRemindersRepo::new(&db.shared())
        .refresh_signature(&calendar_id, &event_id, &title, &starts_at)
        .map_err(map_err)
}

/// Carry an event's PRIVATE reminders (migration 0043) across a move.
///
/// Keyed by (calendar, event), so a move has to take them along — and the
/// repair in the reminder scan cannot do it, because it never looks outside
/// one calendar. Unlike that repair this is not derivable from evidence every
/// device has, so it travels: the moved row, and the emptied old one so a peer
/// still holding it stops firing.
pub(crate) fn relocate_event_local_reminders(
    db: &DbHandle,
    event_log: &EventLogWriter,
    old_calendar_id: &str,
    old_event_id: &str,
    new_calendar_id: &str,
    new_event_id: &str,
) {
    let now = chrono::Utc::now().to_rfc3339();
    match host_core::event_reminders::EventRemindersRepo::new(&db.shared()).relocate(
        old_calendar_id,
        old_event_id,
        new_calendar_id,
        new_event_id,
        &now,
    ) {
        Ok(Some((moved, emptied))) => {
            for row in [moved, emptied] {
                if let Ok(fields) = serde_json::to_value(&row) {
                    event_log.append(SyncEvent::EventLocalRemindersSet(EventPayload {
                        id: format!("{} {}", row.calendar_id, row.event_id),
                        fields,
                    }));
                }
            }
        }
        Ok(None) => {}
        Err(err) => tracing::warn!(?err, "couldn't carry the private reminders across the move",),
    }
}
