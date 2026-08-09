//! Event-group commands (`DESIGN-event-groups.md`).
//!
//! A group is Aperio's statement that several events mean the same
//! appointment. It touches no provider: grouping two events changes neither of
//! them, and ungrouping leaves both exactly as they were. So there is no
//! account routing and no LOCAL_ID guard here — like colour labels, every
//! mutation is by definition a local one and always emits a sync event.
//!
//! Ids are the SERIES MASTER's, matching meetings and colour overrides: a
//! recurring appointment is grouped as a series, not one row per occurrence.

use std::sync::Arc;

use host_core::event_groups::{EventGroupsRepo, NewMember, Ungrouped};
use serde::Deserialize;
use sync_core::{EventPayload, IdPayload, SyncEvent};
use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;

/// One event, as the frontend names it.
#[derive(Debug, Deserialize)]
pub struct EventRef {
    pub calendar_id: String,
    /// Series master id.
    pub event_id: String,
}

/// A member on the way in: the reference plus the signature it had when the
/// user grouped it, so it can be found again after the provider re-mints its
/// id.
#[derive(Debug, Deserialize)]
pub struct NewMemberRequest {
    pub calendar_id: String,
    pub event_id: String,
    pub title: String,
    pub starts_at: String,
}

fn map_group_err(err: host_core::event_groups::EventGroupsError) -> CommandError {
    use host_core::event_groups::EventGroupsError as E;
    // The two refusals are answers, not failures: the user has to be told
    // which one it was, so each keeps its own code rather than collapsing into
    // a generic "invalid input" the frontend cannot phrase.
    let (code, message) = match err {
        E::TooFewMembers => ("event_group_too_few", err.to_string()),
        E::ConflictingGroups => ("event_group_conflict", err.to_string()),
        E::Sqlite(err) => ("internal", err.to_string()),
    };
    CommandError {
        code,
        message: message.to_string(),
    }
}

/// Declare that these events mean the same appointment.
///
/// Joins an existing group when exactly one of the named events is already in
/// one — the natural "and this one too".
#[tauri::command]
pub async fn group_events(
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    members: Vec<NewMemberRequest>,
) -> CommandResult<cal_core::EventGroup> {
    let shared = db.shared();
    let members: Vec<NewMember> = members
        .into_iter()
        .map(|m| NewMember {
            calendar_id: m.calendar_id,
            event_id: m.event_id,
            title: m.title,
            starts_at: m.starts_at,
        })
        .collect();
    let group = EventGroupsRepo::new(&shared)
        .group(&members)
        .map_err(map_group_err)?;
    emit_group(&event_log, &group);
    Ok(group)
}

/// Take one event out of its group, dissolving the group if that leaves fewer
/// than two members.
#[tauri::command]
pub async fn ungroup_event(
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    calendar_id: String,
    event_id: String,
) -> CommandResult<Option<cal_core::EventGroup>> {
    let shared = db.shared();
    let outcome = EventGroupsRepo::new(&shared)
        .ungroup(&calendar_id, &event_id)
        .map_err(map_group_err)?;
    Ok(match outcome {
        Some(Ungrouped::Remains(group)) => {
            emit_group(&event_log, &group);
            Some(group)
        }
        Some(Ungrouped::Dissolved { group_id }) => {
            event_log.append(SyncEvent::EventGroupDissolved(IdPayload { id: group_id }));
            None
        }
        // The event was not grouped. Nothing happened, so nothing is told.
        None => None,
    })
}

/// Dissolve a whole group. The events themselves are untouched.
#[tauri::command]
pub async fn dissolve_event_group(
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    group_id: String,
) -> CommandResult<()> {
    let shared = db.shared();
    if EventGroupsRepo::new(&shared)
        .dissolve(&group_id)
        .map_err(map_group_err)?
    {
        event_log.append(SyncEvent::EventGroupDissolved(IdPayload { id: group_id }));
    }
    Ok(())
}

/// Every group any of these events belongs to — whole, including members
/// outside the rendered range.
#[tauri::command]
pub async fn event_groups_for_events(
    db: State<'_, DbHandle>,
    events: Vec<EventRef>,
) -> CommandResult<Vec<cal_core::EventGroup>> {
    let shared = db.shared();
    let refs: Vec<(String, String)> = events
        .into_iter()
        .map(|e| (e.calendar_id, e.event_id))
        .collect();
    EventGroupsRepo::new(&shared)
        .groups_for_events(&refs)
        .map_err(map_group_err)
}

/// A group change travels as the whole membership — see `SyncEvent`'s own note
/// on why a diff would let two devices interleave into a set neither meant.
fn emit_group(event_log: &EventLogWriter, group: &cal_core::EventGroup) {
    if let Ok(fields) = serde_json::to_value(group) {
        event_log.append(SyncEvent::EventGroupUpdated(EventPayload {
            id: group.id.clone(),
            fields,
        }));
    }
}
