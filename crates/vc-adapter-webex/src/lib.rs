//! Cisco WebEx Meetings REST API adapter (DESIGN.md §11.3).
//!
//! The current implementation is a thin stub: every
//! [`VcAdapter`] method returns
//! [`VcError::Unsupported`] with an actionable message. The
//! adapter type + account config shape are real so the rest of
//! the plugin pipeline (vc-adapter-webex-plugin, host registry,
//! Tauri commands) can wire against them; the actual WebEx REST
//! calls land in a follow-up iteration.
//!
//! ## Auth model
//!
//! WebEx uses its own OAuth 2.0 flow against
//! `https://webexapis.com/v1/authorize` — distinct from Zoom,
//! Meet, and Teams. The plugin will eventually own its own
//! `interactive_auth` entry point; the refresh token lives in
//! the OS keychain under a WebEx-specific slot.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use vc_core::{Meeting, MeetingId, NewMeeting, VcAdapter, VcError, VcResult};

/// Non-secret half of the account config — what the user types
/// into the AccountsDialog. The refresh token lives in the OS
/// keychain alongside the other OAuth adapters'.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebexAccountConfig {
    /// OAuth client id from the user's WebEx Developer integration
    /// (WebEx calls it the "Client ID").
    pub client_id: String,
    /// OAuth client secret (WebEx's "Client Secret"). Required
    /// for the token-exchange step.
    pub client_secret: String,
}

/// Concrete adapter. Holds the configured credentials so a
/// future `oauth.rs` module can mint access tokens on demand;
/// the stub trait impl below doesn't reach for them.
pub struct WebexAdapter {
    _config: WebexAccountConfig,
    _refresh_token: String,
}

impl WebexAdapter {
    /// Build an adapter from validated config + a refresh token
    /// the host pulled from the keychain.
    pub fn new(config: WebexAccountConfig, refresh_token: String) -> Self {
        Self {
            _config: config,
            _refresh_token: refresh_token,
        }
    }
}

#[async_trait]
impl VcAdapter for WebexAdapter {
    async fn test_connection(&self) -> VcResult<()> {
        Err(VcError::Unsupported(
            "WebEx adapter stub — REST calls not yet implemented".to_string(),
        ))
    }

    async fn create_meeting(&self, _spec: NewMeeting) -> VcResult<Meeting> {
        Err(VcError::Unsupported(
            "WebEx adapter stub — create_meeting not yet implemented".to_string(),
        ))
    }

    async fn get_meeting(&self, _id: &MeetingId) -> VcResult<Option<Meeting>> {
        Err(VcError::Unsupported(
            "WebEx adapter stub — get_meeting not yet implemented".to_string(),
        ))
    }

    async fn delete_meeting(&self, _id: &MeetingId) -> VcResult<()> {
        Err(VcError::Unsupported(
            "WebEx adapter stub — delete_meeting not yet implemented".to_string(),
        ))
    }
}
