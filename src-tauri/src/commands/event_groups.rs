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
        E::Vanished => ("internal", err.to_string()),
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

/// Record that two events are NOT the same appointment, so Aperio stops
/// offering to group them.
#[tauri::command]
pub async fn decline_group_suggestion(
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    first: EventRef,
    second: EventRef,
) -> CommandResult<()> {
    let shared = db.shared();
    let decline = EventGroupsRepo::new(&shared)
        .decline_suggestion(
            (&first.calendar_id, &first.event_id),
            (&second.calendar_id, &second.event_id),
        )
        .map_err(map_group_err)?;
    if let Ok(fields) = serde_json::to_value(&decline) {
        event_log.append(SyncEvent::EventGroupSuggestionDeclined(EventPayload {
            // The pair IS the identity; there is no id of its own to mint.
            id: format!(
                "{} {} {} {}",
                decline.calendar_a, decline.event_a, decline.calendar_b, decline.event_b
            ),
            fields,
        }));
    }
    Ok(())
}

/// Every pair the user has said is not one appointment.
#[tauri::command]
pub async fn group_suggestion_declines(
    db: State<'_, DbHandle>,
) -> CommandResult<Vec<cal_core::SuggestionDecline>> {
    let shared = db.shared();
    EventGroupsRepo::new(&shared)
        .declined_suggestions()
        .map_err(map_group_err)
}

/// One member, found again under the id its event carries now.
///
/// The frontend spots this while folding a range it has in hand: a member
/// whose stored start falls inside the range, whose id resolves to nothing,
/// and whose signature matches exactly one event there. Silent on purpose —
/// it repairs Aperio's own bookkeeping and changes nothing about which events
/// mean the same appointment.
#[tauri::command]
pub async fn heal_event_group_member(
    db: State<'_, DbHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    group_id: String,
    calendar_id: String,
    old_event_id: String,
    new_event_id: String,
) -> CommandResult<()> {
    let shared = db.shared();
    let healed = EventGroupsRepo::new(&shared)
        .heal_member(&group_id, &calendar_id, &old_event_id, &new_event_id)
        .map_err(map_group_err)?;
    if let Some(group) = healed {
        emit_group(&event_log, &group);
    }
    Ok(())
}

/// Take a deleted event out of whatever group it was in, and tell the other
/// devices.
///
/// A deleted event cannot go on meaning the same appointment as anything, and
/// a membership row pointing at nothing is worse than none: the group still
/// counts it and still names it. Passing the id through as-is is deliberate —
/// memberships store the SERIES master id, so deleting a single occurrence
/// finds no row and correctly changes nothing.
///
/// Best-effort by design: the event IS deleted by the time this runs, and
/// failing the whole command over the bookkeeping beside it would report a
/// delete that actually happened as a failure.
pub(crate) fn forget_event_grouping(
    db: &DbHandle,
    event_log: &EventLogWriter,
    calendar_id: &str,
    event_id: &str,
) {
    let shared = db.shared();
    match EventGroupsRepo::new(&shared).ungroup(calendar_id, event_id) {
        Ok(Some(Ungrouped::Remains(group))) => emit_group(event_log, &group),
        Ok(Some(Ungrouped::Dissolved { group_id })) => {
            event_log.append(SyncEvent::EventGroupDissolved(IdPayload { id: group_id }));
        }
        Ok(None) => {}
        Err(err) => tracing::warn!(?err, "could not clear the deleted event's grouping"),
    }
}

/// Follow a moved event, and tell the other devices.
///
/// Best-effort like `forget_event_grouping`: the move has already happened.
pub(crate) fn relocate_event_grouping(
    db: &DbHandle,
    event_log: &EventLogWriter,
    old_calendar_id: &str,
    old_event_id: &str,
    new_calendar_id: &str,
    new_event_id: &str,
) {
    let shared = db.shared();
    match EventGroupsRepo::new(&shared).relocate(
        old_calendar_id,
        old_event_id,
        new_calendar_id,
        new_event_id,
    ) {
        Ok(Some(group)) => emit_group(event_log, &group),
        Ok(None) => {}
        Err(err) => tracing::warn!(?err, "could not follow the moved event's grouping"),
    }
}

/// Take a deleted calendar's events out of their groups, and tell the other
/// devices.
pub(crate) fn forget_calendar_groupings(
    db: &DbHandle,
    event_log: &EventLogWriter,
    calendar_id: &str,
) {
    let shared = db.shared();
    match EventGroupsRepo::new(&shared).forget_calendar(calendar_id) {
        Ok(groups) => {
            for group in &groups {
                emit_group(event_log, group);
            }
        }
        Err(err) => tracing::warn!(?err, "could not clear the calendar's groupings"),
    }
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
