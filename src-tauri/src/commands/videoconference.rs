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
//! Webex is the only meeting provider this build carries. The Zoom adapter
//! exists but is unplugged — every method still returns
//! [`vc_core::VcError::Unsupported`], so bundling it would offer a sign-in that
//! leads nowhere.

use std::sync::Arc;

use adapter_local::LocalAdapter;
use cal_core::Event;
use host_core::meetings::{
    attendee_addresses, should_provider_announce_removal, should_provider_notify, EventMeeting,
    MeetingsRepo,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use vc_core::{Meeting, MeetingId, NewMeeting};

use super::{CommandError, CommandResult};
use crate::cache::CacheStore;
use crate::db::DbHandle;
use crate::event_log::EventLogWriter;
use crate::registry::{AdapterRegistry, LOCAL_ID};
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
    /// Link the account's permanent room instead of minting a meeting. Asked
    /// per meeting, because that is what it is a property of.
    #[serde(default)]
    pub use_personal_room: bool,
    /// Which language the join block is written in.
    ///
    /// Per meeting, and not necessarily the app's: a German user inviting
    /// English colleagues writes an English invitation. It has to be decided
    /// HERE because the block is frozen into the event the moment it is
    /// written — it lands in other people's calendars, where nothing can
    /// re-render it later. Absent falls back to English, which is also what an
    /// adapter's catalogue falls back to.
    #[serde(default)]
    pub invitation_lang: Option<String>,
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
    plugin_manager: State<'_, Arc<plugin_core::manager::PluginManager>>,
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

    // Who is coming, and whether the provider is the one who has to tell them.
    // Addresses, not the display strings the event carries: a provider
    // validates this field as an email and refuses the meeting otherwise.
    let can_invite = calendar_can_invite(&registry, &cache, &request.calendar_id);
    let guests = attendee_addresses(&event.attendees);
    let notify = should_provider_notify(&guests, can_invite);
    let meeting = vc
        .create_meeting(NewMeeting {
            title: event.title.clone(),
            start_time: Some(event.start),
            end_time: Some(event.end),
            description: event.description.clone(),
            use_personal_room: request.use_personal_room,
            attendees: guests.clone(),
            notify_attendees: notify,
        })
        .await
        .map_err(CommandError::from)?;

    // Write the link where every other client reads it. The adapter named each
    // line and supplied the values; its own catalogue supplies the words, in the
    // language this request asked for.
    let mut updated = event.clone();
    let lang = request
        .invitation_lang
        .as_deref()
        .unwrap_or(plugin_core::FALLBACK_LANG);
    let catalogue = adapter_catalogue(&plugin_manager, &db, &request.account_id);
    let block = cal_core::conferencing::meeting_block(&host_core::meetings::block_lines(
        &meeting.join_details,
        &meeting.join_url,
        catalogue.as_ref(),
        lang,
    ));
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
    // The complement of the provider decision, and the half that made the rule
    // work at all: exactly ONE channel announces the meeting. When the calendar
    // is the one that can, this write has to actually carry the link to the
    // attendees — the default is `false`, which on Exchange means
    // `SendToNone`, and then the provider stayed quiet AND the calendar said
    // nothing, so the join link reached nobody.
    updated.send_invitations = can_invite && !guests.is_empty();

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
            //
            // The cancellation follows whatever the invitation did: if the
            // provider was asked to mail everyone a moment ago, those mails are
            // already out, and staying quiet now leaves people holding an
            // invitation to a meeting that no longer exists.
            if let Err(cleanup) = vc
                .delete_meeting(vc_core::MeetingRemoval::new(meeting.id.clone(), notify))
                .await
            {
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

/// What the event editor needs to know about an event's meeting, in one answer.
#[derive(Debug, Serialize)]
pub struct EventMeetingInspection {
    /// Set when Aperio created this meeting and can therefore remove it.
    pub binding: Option<EventMeeting>,
    /// The meeting as the provider currently describes it — including who it
    /// says is invited, which is often not what the calendar event says.
    /// `None` when the event carries no meeting, or when no connected account
    /// can see the one it carries.
    pub meeting: Option<Meeting>,
    /// The account that answered. Needed to adopt the meeting.
    pub account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InspectEventMeetingRequest {
    pub event_id: String,
    pub calendar_id: String,
}

/// Everything known about the meeting on an event: whether it is ours, what the
/// provider says about it, and who is really invited.
///
/// One command rather than three, because the editor asks all three questions
/// at the same moment and each answer changes what the others mean.
///
/// The lookup goes through the JOIN LINK, which is the only identifier that
/// reaches a calendar event — the provider's meeting id travels nowhere. That
/// is what lets this work for a meeting Aperio did not create: one made in the
/// provider's web UI, one made on another device, one an invitation brought in.
///
/// Every connected videoconference account is asked in turn, and the first that
/// recognises the link answers. A provider that does not know a link says so
/// cheaply; a provider that cannot look up by link at all is skipped.
#[tauri::command]
pub async fn inspect_event_meeting(
    adapter: State<'_, LocalAdapter>,
    registry: State<'_, Arc<AdapterRegistry>>,
    cache: State<'_, Arc<CacheStore>>,
    db: State<'_, DbHandle>,
    request: InspectEventMeetingRequest,
) -> CommandResult<EventMeetingInspection> {
    let shared = db.shared();
    let binding = MeetingsRepo::new(&shared)
        .get(&request.event_id)
        .map_err(meetings_error)?;

    // Ours: ask the account that made it, by id. Exact and one request.
    if let Some(binding) = &binding {
        if let Some(vc) = registry.vc_adapter(&binding.account_id) {
            let meeting = vc.get_meeting(&binding.meeting_id).await.unwrap_or(None);
            return Ok(EventMeetingInspection {
                account_id: Some(binding.account_id.clone()),
                meeting,
                binding: Some(binding.clone()),
            });
        }
    }

    // Not ours (or its account is gone): go by the link in the event.
    let Some(event) = super::calendars::read_event_for_meeting(
        &adapter,
        &registry,
        &cache,
        &db,
        &request.event_id,
        &request.calendar_id,
    )
    .await?
    else {
        return Ok(EventMeetingInspection {
            binding,
            meeting: None,
            account_id: None,
        });
    };
    let Some(conference) =
        cal_core::conferencing::detect_conference(&cal_core::conferencing::ConferenceSources {
            location: event.location.as_deref(),
            description: event.description.as_deref(),
            ..Default::default()
        })
    else {
        return Ok(EventMeetingInspection {
            binding,
            meeting: None,
            account_id: None,
        });
    };

    for (account_id, vc) in registry.snapshot_vc_adapters() {
        match vc.resolve_meeting(&conference.join_url).await {
            Ok(Some(meeting)) => {
                return Ok(EventMeetingInspection {
                    binding,
                    meeting: Some(meeting),
                    account_id: Some(account_id),
                })
            }
            // Not this account's meeting, or this provider cannot look up by
            // link. Either way the next account gets its turn.
            Ok(None) | Err(_) => continue,
        }
    }
    Ok(EventMeetingInspection {
        binding,
        meeting: None,
        account_id: None,
    })
}

#[derive(Debug, Deserialize)]
pub struct AdoptMeetingRequest {
    pub event_id: String,
    pub calendar_id: String,
    /// The account that recognised the link — from [`inspect_event_meeting`].
    pub account_id: String,
    pub meeting_id: MeetingId,
    pub join_url: String,
}

/// Take responsibility for a meeting Aperio did not create.
///
/// The event already carries the link and already offers Join; what adopting
/// adds is the ability to REMOVE it — which needs the provider's own meeting
/// id, and that is what the link lookup recovered.
///
/// Nothing is written to the event: it already says everything it needs to.
/// This only records that this device now knows which meeting belongs here.
#[tauri::command]
pub fn adopt_meeting(
    db: State<'_, DbHandle>,
    request: AdoptMeetingRequest,
) -> CommandResult<EventMeeting> {
    let shared = db.shared();
    MeetingsRepo::new(&shared)
        .bind(
            &request.event_id,
            &request.account_id,
            &request.meeting_id,
            &request.join_url,
        )
        .map_err(meetings_error)
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

    // Whether the provider announces the cancellation. Deliberately not asked
    // of the event's attendee list: an adopted meeting has invitees the event
    // never knew, and by now the event may be gone or unreadable — where
    // "nobody" and "could not read" look identical. The provider mails only the
    // invitees it holds, so asking costs nothing when it holds none.
    let can_invite = calendar_can_invite(&registry, &cache, &request.calendar_id);
    let notify = should_provider_announce_removal(can_invite);

    // A permanent room cannot be deleted — it belongs to the account, not to
    // this event — and the adapter says so with `Unsupported`. Taking the link
    // out of the event is then the whole of what "remove" can mean, and doing
    // it is better than refusing an operation the user can reasonably expect.
    // Any other failure still aborts: forgetting the id of a meeting that DOES
    // exist would strand it for good.
    match vc
        .delete_meeting(vc_core::MeetingRemoval::new(
            binding.meeting_id.clone(),
            notify,
        ))
        .await
    {
        Ok(()) => {}
        Err(vc_core::VcError::Unsupported(reason)) => {
            tracing::info!(%reason, "unlinking a meeting the provider will not delete");
        }
        Err(err) => return Err(CommandError::from(err)),
    }
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
    // The complement again: when the calendar is the channel that announces,
    // it has to announce that the link is gone too. Otherwise the attendees
    // keep a join URL that now leads nowhere and nobody has said so.
    updated.send_invitations = can_invite && !attendee_addresses(&updated.attendees).is_empty();
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

/// The string catalogue of the plugin backing `account_id`, if it ships one.
///
/// `None` is an ordinary answer: a plugin with no catalogue renders its
/// verbatim labels, which is what a third-party adapter with no translations
/// does by design.
fn adapter_catalogue(
    plugin_manager: &plugin_core::manager::PluginManager,
    db: &DbHandle,
    account_id: &str,
) -> Option<plugin_core::StringCatalogue> {
    let shared = db.shared();
    let account = host_core::accounts::AccountsRepo::new(&shared)
        .get(account_id)
        .ok()
        .flatten()?;
    let plugin = plugin_manager.plugin_for_adapter_kind(account.adapter_kind.as_str())?;
    let catalogue = plugin.manifest.strings.clone();
    (!catalogue.is_empty()).then_some(catalogue)
}

/// Whether the calendar holding an event can invite its attendees itself —
/// send them an invitation server-side and collect their replies as RSVPs.
///
/// Read from the cached calendar listing, which the sidebar has populated long
/// before anyone opens an event editor. An unknown or uncached id degrades to
/// `false`: the worst that follows is one invitation too many, whereas the
/// other direction is a meeting nobody was told about.
fn calendar_can_invite(registry: &AdapterRegistry, cache: &CacheStore, calendar_id: &str) -> bool {
    let Some(account) = registry.account_for_calendar(calendar_id) else {
        // No route means a local calendar, which invites nobody.
        return false;
    };
    if account == LOCAL_ID {
        return false;
    }
    cache
        .read_calendars(&account)
        .ok()
        .into_iter()
        .flatten()
        .find(|c| c.id == calendar_id)
        .is_some_and(|c| c.supports_scheduling)
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
    /// Whether the provider should tell the attendees the meeting is off.
    ///
    /// Defaults to silence, because this thin verb knows only a meeting id —
    /// no event, so no attendees and no calendar to weigh. `detach_meeting`
    /// is the path that has all three and answers the question properly.
    #[serde(default)]
    pub notify_attendees: bool,
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
        .delete_meeting(vc_core::MeetingRemoval::new(
            request.meeting_id,
            request.notify_attendees,
        ))
        .await
        .map_err(CommandError::from)
}
