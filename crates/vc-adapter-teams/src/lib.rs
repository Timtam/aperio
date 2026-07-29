//! Microsoft Teams Online Meetings adapter via Microsoft Graph
//! (DESIGN.md §11.3).
//!
//! The current implementation is a thin stub: every
//! [`VcAdapter`] method returns
//! [`VcError::Unsupported`] with an actionable message. The
//! adapter type + account config shape are real so the rest of
//! the plugin pipeline (vc-adapter-teams-plugin, host registry,
//! Tauri commands) can wire against them; the actual Graph REST
//! calls land in a follow-up iteration.
//!
//! ## Auth model
//!
//! Teams shares the OAuth 2.0 access token of the
//! `cal-adapter-microsoft-graph` adapter — there's no separate
//! sign-in. The plugin retrieves the token from the same
//! keychain slot the Graph calendar adapter uses, so the user
//! only authenticates once.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use vc_core::{Meeting, MeetingId, MeetingRemoval, NewMeeting, VcAdapter, VcError, VcResult};

/// Non-secret half of the account config — what the user types
/// into the AccountsDialog. Teams uses PKCE-only public-client
/// OAuth via the Graph endpoint, the same shape as the
/// cal-adapter-microsoft-graph adapter, so only the client id
/// is configured here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamsAccountConfig {
    /// OAuth client id from the user's Azure AD app registration
    /// (Microsoft calls it the "Application (client) ID").
    pub client_id: String,
}

/// Concrete adapter. Holds the configured client id + the access
/// token borrowed from the Graph calendar adapter so a future
/// `api.rs` module can make Graph calls on demand; the stub
/// trait impl below doesn't reach for them.
pub struct TeamsAdapter {
    _config: TeamsAccountConfig,
    _shared_access_token: String,
}

impl TeamsAdapter {
    /// Build an adapter from validated config + an access token
    /// the host pulled from the shared Graph keychain slot.
    pub fn new(config: TeamsAccountConfig, shared_access_token: String) -> Self {
        Self {
            _config: config,
            _shared_access_token: shared_access_token,
        }
    }
}

#[async_trait]
impl VcAdapter for TeamsAdapter {
    async fn test_connection(&self) -> VcResult<()> {
        Err(VcError::Unsupported(
            "Teams adapter stub — REST calls not yet implemented".to_string(),
        ))
    }

    async fn create_meeting(&self, _spec: NewMeeting) -> VcResult<Meeting> {
        Err(VcError::Unsupported(
            "Teams adapter stub — create_meeting not yet implemented".to_string(),
        ))
    }

    async fn get_meeting(&self, _id: &MeetingId) -> VcResult<Option<Meeting>> {
        Err(VcError::Unsupported(
            "Teams adapter stub — get_meeting not yet implemented".to_string(),
        ))
    }

    async fn delete_meeting(&self, _removal: MeetingRemoval) -> VcResult<()> {
        Err(VcError::Unsupported(
            "Teams adapter stub — delete_meeting not yet implemented".to_string(),
        ))
    }
}
