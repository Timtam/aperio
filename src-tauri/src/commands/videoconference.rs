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
//! Current iteration ships the four CRUD-ish verbs the
//! "Meeting beitreten" UX needs (§11.2). All routing,
//! permission, and error mapping is real; the per-provider
//! adapters themselves are stubs that return
//! [`vc_core::VcError::Unsupported`] until the REST layers
//! land — so calling these commands today returns a structured
//! "unsupported" envelope the frontend can surface as
//! "coming soon".

use std::sync::Arc;

use serde::Deserialize;
use tauri::State;
use vc_core::{Meeting, MeetingId, NewMeeting};

use super::{CommandError, CommandResult};
use crate::registry::AdapterRegistry;

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
