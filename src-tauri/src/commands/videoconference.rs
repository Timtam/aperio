//! Video-conference Tauri commands (DESIGN.md §11).
//!
//! Lets the frontend ask a registered VC account to mint
//! meeting links, retrieve their current state, or drop them.
//! Each command routes through [`AdapterRegistry::vc_adapter`]
//! to find the [`vc_core::VcAdapter`] instance for the given
//! account_id; the adapter itself is a plugin-side
//! [`plugin_core::shim::FfiVcAdapter`] wrapping the loaded
//! library.
//!
//! Two layers live here. The four thin verbs (`test_vc_connection`,
//! `create_meeting`, `get_meeting`, `delete_meeting`) route straight to an
//! adapter and are what a caller reaches for when it already knows a meeting
//! id. Above them sit `attach_meeting` and `detach_meeting`, which are what the
//! event editor actually calls: they mint or drop a meeting AND put its link
//! into the event, AND record which meeting belongs to which event, in one
//! step — because doing those three things separately is how an event ends up
//! carrying a link to a meeting nobody can delete any more.
//!
//! Zoom, Teams and Meet are still stubs returning
//! [`vc_core::VcError::Unsupported`]; Webex is real.

use std::sync::Arc;

use cal_adapter_local::LocalAdapter;
use cal_core::Event;
use host_core::meetings::{EventMeeting, MeetingsRepo};
use serde::{Deserialize, Serialize};
use tauri::State;
use vc_core::{Meeting, MeetingId, NewMeeting};

use super::{CommandError, CommandResult};
use crate::cache::CacheStore;
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;
use crate::registry::AdapterRegistry;
use crate::reminders::SchedulerHandle;

/// An event and the meeting now attached to it.
#[derive(Debug, Serialize)]
pub struct AttachedMeeting {
    /// The event as saved — its link already written in.
    pub event: Event,
    pub meeting: Meeting,
}

#[derive(Debug, Deserialize)]
pub struct AttachMeetingRequest {
    /// Series master id: one meeting serves the whole series, as it does on the
    /// provider side.
    pub event_id: String,
    pub calendar_id: String,
    /// Which videoconference account mints it.
    pub account_id: String,
}

/// Create a meeting for an event, write its link into the event, and remember
/// which meeting that was.
///
/// The three steps are one command on purpose. A meeting created without its
/// link reaching the event is invisible; a link written without the binding
/// recorded is a meeting nobody can ever delete. Doing them here means a
/// failure at any point leaves the previous state, and the caller gets back the
/// event exactly as it was saved.
///
/// The link goes into the event's own fields — `location` when it is free, and
/// a block appended to the description — rather than into anything
/// Aperio-specific. That is what makes the meeting reachable from Outlook, from
/// a phone, and from a colleague who has never heard of this app.
#[tauri::command]
pub async fn attach_meeting(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    db: State<'_, DbHandle>,
    request: AttachMeetingRequest,
) -> CommandResult<AttachedMeeting> {
    let vc = require_vc_adapter(&registry, &request.account_id)?;

    // The event as it stands. Everything the provider is told about the meeting
    // comes from here, so a meeting always matches the event it belongs to.
    let event = super::calendars::read_event_for_meeting(
        &adapter,
        &registry,
        &cache,
        &db,
        &request.event_id,
        &request.calendar_id,
    )
    .await?
    .ok_or(CommandError {
        code: "not_found",
        message: "the event no longer exists".into(),
    })?;

    // A second meeting for the same event would orphan the first, since the
    // binding can only hold one. Refuse rather than leak.
    let shared = db.shared();
    if MeetingsRepo::new(&shared)
        .get(&request.event_id)
        .map_err(meetings_error)?
        .is_some()
    {
        return Err(CommandError {
            code: "conflict",
            message: "this event already has a meeting — remove it first".into(),
        });
    }

    let meeting = vc
        .create_meeting(NewMeeting {
            title: event.title.clone(),
            start_time: Some(event.start),
            end_time: Some(event.end),
            description: event.description.clone(),
        })
        .await
        .map_err(CommandError::from)?;

    // Write the link where every other client reads it.
    let mut updated = event.clone();
    let block =
        cal_core::conferencing::meeting_block(&meeting.join_url, meeting.password.as_deref());
    updated.description = Some(match updated.description.as_deref().map(str::trim) {
        Some(existing) if !existing.is_empty() => format!("{existing}\n\n{block}"),
        _ => block,
    });
    // `location` only when it is free: an event that says "Room 3.14" means it,
    // and a meeting link is not a reason to overwrite where people are sitting.
    if updated
        .location
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        updated.location = Some(meeting.join_url.clone());
    }

    let saved = super::calendars::update_event(
        adapter,
        registry,
        cache,
        scheduler,
        event_log,
        db.clone(),
        updated,
        None,
    )
    .await;
    let saved = match saved {
        Ok(saved) => saved,
        Err(err) => {
            // The event could not be saved, so the meeting has nowhere to live.
            // Take it back down rather than leaving one behind on the provider
            // that nothing on this device knows about.
            if let Err(cleanup) = vc.delete_meeting(&meeting.id).await {
                tracing::warn!(
                    meeting_id = %meeting.id,
                    ?cleanup,
                    "could not roll back a meeting after the event failed to save; \
                     it has to be removed in the provider's own interface"
                );
            }
            return Err(err);
        }
    };

    MeetingsRepo::new(&shared)
        .bind(
            &request.event_id,
            &request.account_id,
            &meeting.id,
            &meeting.join_url,
        )
        .map_err(meetings_error)?;

    Ok(AttachedMeeting {
        event: saved,
        meeting,
    })
}

#[derive(Debug, Deserialize)]
pub struct DetachMeetingRequest {
    pub event_id: String,
    pub calendar_id: String,
}

/// Drop the meeting attached to an event and take its link back out.
///
/// The provider delete happens FIRST. If it fails the binding stays, so the
/// meeting is still addressable and the user can try again — the opposite order
/// would forget the id and strand the meeting for good.
///
/// Returns the event as saved, or `None` when the event had no meeting Aperio
/// created. An event carrying someone else's meeting link is untouched: it is
/// not ours to delete, and the Join affordance keeps working.
#[tauri::command]
pub async fn detach_meeting(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    scheduler: State<'_, SchedulerHandle>,
    event_log: State<'_, Arc<EventLogWriter>>,
    db: State<'_, DbHandle>,
    request: DetachMeetingRequest,
) -> CommandResult<Option<Event>> {
    let shared = db.shared();
    let Some(binding) = MeetingsRepo::new(&shared)
        .get(&request.event_id)
        .map_err(meetings_error)?
    else {
        return Ok(None);
    };

    let vc = require_vc_adapter(&registry, &binding.account_id)?;
    vc.delete_meeting(&binding.meeting_id)
        .await
        .map_err(CommandError::from)?;
    MeetingsRepo::new(&shared)
        .unbind(&request.event_id)
        .map_err(meetings_error)?;

    // Take the link out of the event. A failure here is not fatal — the meeting
    // is already gone and the binding with it, so the worst case is a stale
    // link the user can delete by hand, which beats refusing the whole
    // operation after the provider side already happened.
    let event = super::calendars::read_event_for_meeting(
        &adapter,
        &registry,
        &cache,
        &db,
        &request.event_id,
        &request.calendar_id,
    )
    .await?;
    let Some(event) = event else {
        return Ok(None);
    };
    let mut updated = event.clone();
    updated.description = updated
        .description
        .as_deref()
        .map(|text| cal_core::conferencing::without_meeting_block(text, &binding.join_url))
        .filter(|text| !text.is_empty());
    if updated.location.as_deref().map(str::trim) == Some(binding.join_url.as_str()) {
        updated.location = None;
    }
    let saved = super::calendars::update_event(
        adapter, registry, cache, scheduler, event_log, db, updated, None,
    )
    .await?;
    Ok(Some(saved))
}

/// The meeting Aperio created for an event, if any — what the editor asks to
/// decide whether to offer "create" or "remove".
#[tauri::command]
pub fn event_meeting(
    db: State<'_, DbHandle>,
    event_id: String,
) -> CommandResult<Option<EventMeeting>> {
    let shared = db.shared();
    MeetingsRepo::new(&shared)
        .get(&event_id)
        .map_err(meetings_error)
}

fn meetings_error(err: host_core::meetings::MeetingsError) -> CommandError {
    CommandError {
        code: "internal",
        message: err.to_string(),
    }
}

/// Resolve the VcAdapter for `account_id` or surface a clear
/// `not_found` envelope. Shared by every command in this module.
fn require_vc_adapter(
    registry: &AdapterRegistry,
    account_id: &str,
) -> CommandResult<Arc<dyn vc_core::VcAdapter>> {
    registry.vc_adapter(account_id).ok_or(CommandError {
        code: "not_found",
        message: format!("no videoconference adapter registered for account {account_id}"),
    })
}

/// Round-trip a credential check against the provider. Drives
/// the AccountsDialog's "Test connection" button on the vc form.
#[tauri::command]
pub async fn test_vc_connection(
    registry: State<'_, Arc<AdapterRegistry>>,
    account_id: String,
) -> CommandResult<()> {
    let adapter = require_vc_adapter(&registry, &account_id)?;
    adapter.test_connection().await.map_err(CommandError::from)
}

#[derive(Debug, Deserialize)]
pub struct CreateMeetingRequest {
    pub account_id: String,
    pub spec: NewMeeting,
}

/// Mint a new meeting on the provider side and return the
/// populated [`Meeting`]. The frontend stores the returned
/// `id` against the calendar event so a later
/// [`delete_meeting`] / [`get_meeting`] can address it.
#[tauri::command]
pub async fn create_meeting(
    registry: State<'_, Arc<AdapterRegistry>>,
    request: CreateMeetingRequest,
) -> CommandResult<Meeting> {
    let adapter = require_vc_adapter(&registry, &request.account_id)?;
    adapter
        .create_meeting(request.spec)
        .await
        .map_err(CommandError::from)
}

#[derive(Debug, Deserialize)]
pub struct GetMeetingRequest {
    pub account_id: String,
    pub meeting_id: MeetingId,
}

/// Re-fetch a previously-created meeting. `None` means the
/// provider doesn't know about it any more (soft delete on
/// their side); the frontend clears the cached id on the event.
#[tauri::command]
pub async fn get_meeting(
    registry: State<'_, Arc<AdapterRegistry>>,
    request: GetMeetingRequest,
) -> CommandResult<Option<Meeting>> {
    let adapter = require_vc_adapter(&registry, &request.account_id)?;
    adapter
        .get_meeting(&request.meeting_id)
        .await
        .map_err(CommandError::from)
}

#[derive(Debug, Deserialize)]
pub struct DeleteMeetingRequest {
    pub account_id: String,
    pub meeting_id: MeetingId,
}

/// Drop the meeting on the provider side. Called when the user
/// explicitly removes the link from an event or deletes the
/// event with "also delete the provider-side meeting"
/// confirmed.
#[tauri::command]
pub async fn delete_meeting(
    registry: State<'_, Arc<AdapterRegistry>>,
    request: DeleteMeetingRequest,
) -> CommandResult<()> {
    let adapter = require_vc_adapter(&registry, &request.account_id)?;
    adapter
        .delete_meeting(&request.meeting_id)
        .await
        .map_err(CommandError::from)
}
