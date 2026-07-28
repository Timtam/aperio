//! Cisco Webex Meetings adapter (DESIGN.md §11.3).
//!
//! Gives a calendar event a Webex join link, and takes it away again. Two
//! modes, chosen per account:
//!
//!  - **a meeting per event** (the default) — its own link, password and
//!    dial-in details, created and deleted with the event;
//!  - **the Personal Meeting Room** — the one permanent link the account
//!    already owns, which needs no write scope and has no per-day cap.
//!
//! ## Auth
//!
//! Webex runs its own OAuth 2.0 against `webexapis.com`, shared with none of
//! Aperio's calendar adapters. It requires a client secret even under PKCE —
//! verified against a live account rather than assumed; see [`oauth`]. The host
//! hands the adapter a client id, a secret and a refresh token, and the adapter
//! mints access tokens as it needs them.
//!
//! ## What this adapter deliberately does not do
//!
//! It never touches a calendar. Webex has no calendar API at all, and its
//! server-side Hybrid Calendar Service is admin-provisioned and watches
//! Exchange or Google mailboxes — outside a third-party client's reach. So the
//! shape is the one every other client uses: create the meeting here, and let
//! the host write the join link into its own event.

pub mod api;
pub mod meetings;
pub mod oauth;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use vc_core::{Meeting, MeetingId, NewMeeting, VcAdapter, VcError, VcResult};

use crate::api::ApiState;
use crate::oauth::TokenSet;

/// The non-secret half of the account configuration — what the user sees and
/// can change.
///
/// The client secret and the refresh token are credentials and live in the
/// platform keychain, never here: this half is persisted in the account row,
/// which the sync engine appends to its event log unencrypted whenever
/// end-to-end encryption is off.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebexAccountConfig {
    /// OAuth client id. Not a secret — it travels in every authorization URL
    /// the user's own browser visits — but it identifies which registration an
    /// account was linked to, which matters when that registration changes.
    pub client_id: String,
    /// Which Webex site to create meetings on. Filled from the account's
    /// default site at first use; an override matters only for someone whose
    /// organisation has several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_url: Option<String>,
    /// Use the account's permanent Personal Meeting Room instead of creating a
    /// meeting per event. Costs the per-event link and password, but needs no
    /// scheduling licence and has no daily cap.
    #[serde(default)]
    pub use_personal_room: bool,
    /// Let Webex email its own invitations on top of Aperio's.
    ///
    /// Off by default, and that default is load-bearing rather than tidy:
    /// Webex's own default is ON and its mails carry an iCalendar attachment,
    /// so leaving it alone puts a second invitation and a duplicate entry in
    /// every attendee's calendar.
    #[serde(default)]
    pub send_webex_emails: bool,
}

/// One configured Webex account.
pub struct WebexAdapter {
    config: WebexAccountConfig,
    client_secret: String,
    refresh_token: String,
    http: reqwest::Client,
    /// Built on first use, then reused. Construction cannot mint an access
    /// token because that needs a network round trip, and an adapter that did
    /// I/O in its constructor would make opening an account block on Webex
    /// being reachable.
    state: OnceCell<ApiState>,
    /// The site resolved at first use, so the per-event create does not re-ask
    /// for something that does not change.
    site: OnceCell<Option<String>>,
    /// Where a rotated credential is reported, and the capability token that
    /// names this account when it is. Both `None` in an embedding that does not
    /// persist credentials — tests, and any host without the channel.
    scope_token: Option<String>,
    credential_sink: std::sync::Mutex<Option<Box<dyn api::CredentialSink>>>,
}

impl WebexAdapter {
    pub fn new(config: WebexAccountConfig, client_secret: String, refresh_token: String) -> Self {
        Self {
            config,
            client_secret,
            refresh_token,
            // Timeouts, matching every other HTTP adapter in this workspace —
            // and mattering more here than anywhere else. The API layer
            // deliberately refuses to sleep through a rate limit because "a
            // plugin call has no cancellation"; that same argument makes an
            // untimed request a hang with no way out. `expect` rather than a
            // fallback to `Client::new()`: the builder only fails when the TLS
            // backend cannot initialise, and falling back would silently
            // reinstate the untimed client this exists to delete.
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            state: OnceCell::new(),
            site: OnceCell::new(),
            scope_token: None,
            credential_sink: std::sync::Mutex::new(None),
        }
    }

    /// Attach the host channel. Called by the plugin wrapper, which is the only
    /// layer that knows about FFI; the adapter itself stays a plain library.
    pub fn with_credential_sink(
        mut self,
        scope_token: Option<String>,
        sink: Box<dyn api::CredentialSink>,
    ) -> Self {
        self.scope_token = scope_token;
        self.credential_sink = std::sync::Mutex::new(Some(sink));
        self
    }

    /// The shared authenticated state, built on first use.
    ///
    /// It starts with an access token that is already expired, so the very
    /// first call refreshes. One extra round trip, in exchange for never
    /// holding an access token the host would have to keep fresh.
    async fn state(&self) -> VcResult<&ApiState> {
        self.state
            .get_or_try_init(|| async {
                if self.config.client_id.trim().is_empty() {
                    return Err(VcError::InvalidInput(
                        "this Webex account has no client id".into(),
                    ));
                }
                if self.refresh_token.trim().is_empty() {
                    return Err(VcError::Authentication(
                        "this Webex account has no refresh token — sign in to Webex again".into(),
                    ));
                }
                let state = ApiState::new(
                    TokenSet {
                        access_token: String::new(),
                        refresh_token: Some(self.refresh_token.clone()),
                        // Already expired: the first call mints a real one.
                        expires_at: chrono::Utc::now() - chrono::Duration::seconds(1),
                        refresh_expires_at: None,
                        scope: None,
                    },
                    self.config.client_id.clone(),
                    Some(self.client_secret.clone()),
                    self.http.clone(),
                );
                // Hand the sink over once, when the state is built. Taken out
                // of the mutex rather than cloned because a boxed trait object
                // is not Clone and there is exactly one state per adapter.
                let sink = self
                    .credential_sink
                    .lock()
                    .expect("credential sink poisoned")
                    .take();
                Ok(match sink {
                    Some(sink) => state.with_credential_sink(self.scope_token.clone(), sink),
                    None => state,
                })
            })
            .await
    }

    /// The site to create on: the configured override, else the account's
    /// default, resolved once.
    async fn site_url(&self) -> VcResult<Option<String>> {
        if self.config.site_url.is_some() {
            return Ok(self.config.site_url.clone());
        }
        let state = self.state().await?;
        let resolved = self
            .site
            .get_or_try_init(|| async { meetings::test_connection(state).await })
            .await?;
        Ok(resolved.clone())
    }

    /// The always-on room, described as a [`Meeting`].
    async fn personal_room(&self) -> VcResult<Meeting> {
        let state = self.state().await?;
        meetings::create_meeting(
            state,
            &NewMeeting {
                title: String::new(),
                start_time: None,
                end_time: None,
                description: None,
            },
            None,
            true,
            false,
            None,
        )
        .await
    }
}

#[async_trait]
impl VcAdapter for WebexAdapter {
    async fn test_connection(&self) -> VcResult<()> {
        let state = self.state().await?;
        meetings::test_connection(state).await?;
        Ok(())
    }

    async fn create_meeting(&self, spec: NewMeeting) -> VcResult<Meeting> {
        if self.config.use_personal_room {
            return self.personal_room().await;
        }
        let site = self.site_url().await?;
        let state = self.state().await?;
        meetings::create_meeting(
            state,
            &spec,
            site.as_deref(),
            false,
            self.config.send_webex_emails,
            // The host threads its event id through here once the
            // event-to-meeting binding lands; until then a meeting carries no
            // back reference and can only be found by its stored id.
            None,
        )
        .await
    }

    async fn get_meeting(&self, id: &MeetingId) -> VcResult<Option<Meeting>> {
        // The personal room is not a meeting Webex knows by id — it is a
        // property of the account, so re-reading it means re-reading that.
        if meetings::is_personal_room(id) {
            return self.personal_room().await.map(Some);
        }
        let state = self.state().await?;
        meetings::get_meeting(state, id).await
    }

    async fn delete_meeting(&self, id: &MeetingId) -> VcResult<()> {
        // Deleting the personal room is not something that can happen: it
        // belongs to the account, not to any event. Silently succeeding would
        // be a lie the caller acts on, so say what is true.
        if meetings::is_personal_room(id) {
            return Err(VcError::Unsupported(
                "The Personal Meeting Room belongs to the Webex account, not to this event, so \
                 it cannot be deleted. Remove the link from the event instead."
                    .into(),
            ));
        }
        let state = self.state().await?;
        meetings::delete_meeting(state, id, self.config.send_webex_emails).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(config: WebexAccountConfig, refresh: &str) -> WebexAdapter {
        WebexAdapter::new(config, "secret".into(), refresh.into())
    }

    #[tokio::test]
    async fn an_account_without_a_refresh_token_says_so_before_any_network_call() {
        let err = adapter(
            WebexAccountConfig {
                client_id: "c".into(),
                ..Default::default()
            },
            "   ",
        )
        .test_connection()
        .await
        .expect_err("must fail");
        assert!(matches!(err, VcError::Authentication(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn an_account_without_a_client_id_is_a_configuration_error() {
        let err = adapter(WebexAccountConfig::default(), "RT")
            .test_connection()
            .await
            .expect_err("must fail");
        assert!(matches!(err, VcError::InvalidInput(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn the_personal_room_refuses_deletion_instead_of_pretending() {
        // It belongs to the account, not the event. Succeeding silently would
        // leave the caller believing something was removed.
        let err = adapter(
            WebexAccountConfig {
                client_id: "c".into(),
                use_personal_room: true,
                ..Default::default()
            },
            "RT",
        )
        .delete_meeting(&format!(
            "{}https://x.webex.com/meet/toni",
            meetings::PERSONAL_ROOM_ID_PREFIX
        ))
        .await
        .expect_err("must refuse");
        assert!(matches!(err, VcError::Unsupported(_)), "got {err:?}");
    }

    #[test]
    fn webex_emails_are_off_by_default() {
        // Webex's own default is ON and its mails carry an .ics attachment, so
        // this default is the difference between one invitation and two.
        assert!(!WebexAccountConfig::default().send_webex_emails);
    }

    #[test]
    fn the_account_config_never_carries_a_credential() {
        // config_json is persisted in the account row and appended to the sync
        // event log unencrypted whenever E2E is off. A secret in this struct
        // would travel to the user's sync target in the clear.
        let json = serde_json::to_string(&WebexAccountConfig {
            client_id: "the-id".into(),
            site_url: Some("site.webex.com".into()),
            use_personal_room: true,
            send_webex_emails: false,
        })
        .unwrap();
        assert!(!json.contains("secret"), "got {json}");
        assert!(!json.contains("token"), "got {json}");
    }
}
