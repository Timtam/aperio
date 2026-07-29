//! Google Meet adapter via the Google Calendar API's
//! `conferenceData` field (DESIGN.md §11.3).
//!
//! The current implementation is a thin stub: every
//! [`VcAdapter`] method returns
//! [`VcError::Unsupported`] with an actionable message. The
//! adapter type + account config shape are real so the rest of
//! the plugin pipeline (vc-adapter-meet-plugin, host registry,
//! Tauri commands) can wire against them; the actual Calendar
//! API calls land in a follow-up iteration.
//!
//! ## Auth model
//!
//! Meet shares the OAuth 2.0 refresh token of the
//! `cal-adapter-google` adapter — no separate sign-in needed.
//! The plugin reaches into the same keychain slot the Google
//! calendar adapter uses; Meet links are minted via the
//! Calendar API's `conferenceData` field rather than a dedicated
//! Meet REST endpoint.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use vc_core::{Meeting, MeetingId, MeetingRemoval, NewMeeting, VcAdapter, VcError, VcResult};

/// Non-secret half of the account config — what the user types
/// into the AccountsDialog. Meet shares the cal-adapter-google
/// config exactly because the API endpoint is the same.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetAccountConfig {
    /// OAuth client id from the user's Google Cloud OAuth client
    /// (Google calls it the "Client ID").
    pub client_id: String,
    /// OAuth client secret (Google's "Client secret"). Required
    /// for the token-exchange step.
    pub client_secret: String,
}

/// Concrete adapter. Holds the configured credentials + the
/// refresh token borrowed from the Google calendar adapter so a
/// future `api.rs` module can mint access tokens on demand; the
/// stub trait impl below doesn't reach for them.
pub struct MeetAdapter {
    _config: MeetAccountConfig,
    _shared_refresh_token: String,
}

impl MeetAdapter {
    /// Build an adapter from validated config + a refresh token
    /// the host pulled from the shared Google keychain slot.
    pub fn new(config: MeetAccountConfig, shared_refresh_token: String) -> Self {
        Self {
            _config: config,
            _shared_refresh_token: shared_refresh_token,
        }
    }
}

#[async_trait]
impl VcAdapter for MeetAdapter {
    async fn test_connection(&self) -> VcResult<()> {
        Err(VcError::Unsupported(
            "Meet adapter stub — REST calls not yet implemented".to_string(),
        ))
    }

    async fn create_meeting(&self, _spec: NewMeeting) -> VcResult<Meeting> {
        Err(VcError::Unsupported(
            "Meet adapter stub — create_meeting not yet implemented".to_string(),
        ))
    }

    async fn get_meeting(&self, _id: &MeetingId) -> VcResult<Option<Meeting>> {
        Err(VcError::Unsupported(
            "Meet adapter stub — get_meeting not yet implemented".to_string(),
        ))
    }

    async fn delete_meeting(&self, _removal: MeetingRemoval) -> VcResult<()> {
        Err(VcError::Unsupported(
            "Meet adapter stub — delete_meeting not yet implemented".to_string(),
        ))
    }
}
