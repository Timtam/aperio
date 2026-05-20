//! Google Calendar adapter (DESIGN.md §6.2, Phase 6d).
//!
//! Implements OAuth 2.0 PKCE for the desktop flow plus the read half
//! of Google's Calendar API v3. Write paths (`create_event` /
//! `update_event` / `delete_event` / `rename_calendar`) return
//! `Unsupported` in 6d.1 and land in 6d.2.
//!
//! **Setup requirement.** The user provides their own OAuth client id
//! out of the Google Cloud Console (Desktop app type). Aperio's
//! production registration / verification is a release-phase concern
//! and not bundled here. See the AccountsDialog help text for the
//! step-by-step.

pub mod api;
pub mod auth;
pub mod error;
pub mod mapping;

use std::time::Duration;

use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Calendar, CalendarFeature, Capability, ContainerColor,
    Credentials as CoreCredentials, DateRange, Error as CoreError, Event, FreeBusy,
    NewEvent, Result as CoreResult,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

pub use auth::TokenSet;
pub use error::{GoogleError, GoogleResult};

use crate::api::ApiState;

/// Persisted-in-the-DB part of a Google account's configuration. The
/// access + refresh tokens live in the keychain via the `secrets`
/// module — only the non-secret client_id and the user-visible email
/// live here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleAccountConfig {
    /// OAuth 2.0 client id from the user's Google Cloud Console
    /// (Desktop app type). Treated as public information.
    pub client_id: String,
    /// User-visible email / display name the consent screen showed.
    /// Optional — populated after the first authentication and
    /// helpful when the user has multiple Google accounts.
    #[serde(default)]
    pub account_label: Option<String>,
}

#[derive(Debug)]
pub struct GoogleAdapter {
    state: ApiState,
    capabilities: Vec<Capability>,
    /// Listing cache mirrors the CalDAV adapter — list_calendars is
    /// expensive (~300 ms over HTTPS to Google) and the sidebar
    /// calls it from several refresh paths.
    calendars_cache: Mutex<Option<(Vec<Calendar>, chrono::DateTime<chrono::Utc>)>>,
    listing_ttl: chrono::Duration,
}

impl GoogleAdapter {
    /// Construct an adapter from a previously-obtained [`TokenSet`].
    /// Use [`Self::authenticate_interactive`] to run the OAuth dance
    /// first when creating a fresh account.
    pub fn new(client_id: String, tokens: TokenSet) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            state: ApiState::new(tokens, client_id, http),
            capabilities: vec![Capability::Calendar],
            calendars_cache: Mutex::new(None),
            listing_ttl: chrono::Duration::minutes(5),
        }
    }

    /// Override the listing-cache freshness window. Production
    /// callers stick with the 5-minute default; tests pin it to
    /// zero to keep the network-fetch path reachable.
    #[doc(hidden)]
    pub fn with_listing_ttl(mut self, ttl: Duration) -> Self {
        self.listing_ttl = chrono::Duration::from_std(ttl)
            .unwrap_or_else(|_| chrono::Duration::zero());
        self
    }

    /// Run the interactive OAuth dance: localhost listener, browser
    /// open, code exchange. Returns the resulting [`TokenSet`] so
    /// the caller can persist `refresh_token` to keychain.
    pub async fn authenticate_interactive(
        client_id: &str,
    ) -> GoogleResult<TokenSet> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| GoogleError::Io(e.to_string()))?;
        auth::run_default(client_id, &http).await
    }

    /// Expose the current in-memory tokens. Used after a refresh so
    /// the wrapping code (Tauri command, account-creation flow) can
    /// persist the updated access / refresh token back to keychain.
    pub async fn current_tokens(&self) -> TokenSet {
        self.state.tokens.lock().await.clone()
    }

    async fn cached_calendars(&self) -> Option<Vec<Calendar>> {
        let guard = self.calendars_cache.lock().await;
        let (items, ts) = guard.as_ref()?;
        let age = chrono::Utc::now().signed_duration_since(*ts);
        if age >= chrono::Duration::zero() && age < self.listing_ttl {
            Some(items.clone())
        } else {
            None
        }
    }
}

#[async_trait]
impl Adapter for GoogleAdapter {
    async fn authenticate(
        &self,
        _credentials: CoreCredentials,
    ) -> CoreResult<AuthToken> {
        // Google's auth happens before adapter construction (the
        // OAuth dance runs in `authenticate_interactive` and stores
        // tokens in keychain). Once the adapter exists, tokens are
        // already in hand and the trait method is a no-op stub —
        // refreshing happens lazily inside the API client on 401.
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl CalendarFeature for GoogleAdapter {
    async fn list_calendars(&self) -> CoreResult<Vec<Calendar>> {
        if let Some(cached) = self.cached_calendars().await {
            return Ok(cached);
        }
        let fresh = api::list_calendars(&self.state)
            .await
            .map_err(to_core_error)?;
        *self.calendars_cache.lock().await =
            Some((fresh.clone(), chrono::Utc::now()));
        Ok(fresh)
    }

    async fn get_events(
        &self,
        calendar_id: &str,
        range: DateRange,
    ) -> CoreResult<Vec<Event>> {
        api::get_events(&self.state, calendar_id, range.start, range.end)
            .await
            .map_err(to_core_error)
    }

    async fn create_event(
        &self,
        _calendar_id: &str,
        _event: NewEvent,
    ) -> CoreResult<Event> {
        Err(CoreError::Unsupported(
            "Google create_event lands in Phase 6d.2".into(),
        ))
    }

    async fn update_event(&self, _event: Event) -> CoreResult<Event> {
        Err(CoreError::Unsupported(
            "Google update_event lands in Phase 6d.2".into(),
        ))
    }

    async fn delete_event(&self, _event_id: &str) -> CoreResult<()> {
        Err(CoreError::Unsupported(
            "Google delete_event lands in Phase 6d.2".into(),
        ))
    }

    async fn get_free_busy(
        &self,
        _emails: &[&str],
        _range: DateRange,
    ) -> CoreResult<Vec<FreeBusy>> {
        Ok(Vec::new())
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
        // Colour is read together with the listing; consumers should
        // use the value on the Calendar struct rather than asking
        // the adapter again.
        None
    }

    async fn rename_calendar(
        &self,
        _calendar_id: &str,
        _new_name: &str,
    ) -> CoreResult<()> {
        Err(CoreError::Unsupported(
            "Google rename_calendar lands in Phase 6d.2".into(),
        ))
    }
}

fn to_core_error(err: GoogleError) -> CoreError {
    use GoogleError::*;
    match err {
        Network(m) => CoreError::Network(m),
        Http { status, message } => match status {
            401 | 403 => CoreError::Authentication(message),
            404 => CoreError::NotFound(message),
            409 | 412 => CoreError::Conflict(message),
            _ => CoreError::Protocol(format!("Google HTTP {status}: {message}")),
        },
        Protocol(m) => CoreError::Protocol(m),
        AuthDenied(m) => CoreError::Authentication(format!("denied: {m}")),
        AuthTimeout => CoreError::Authentication(
            "consent screen timed out".into(),
        ),
        Csrf => CoreError::Protocol(
            "CSRF state mismatch on OAuth callback".into(),
        ),
        Io(m) => CoreError::Internal(m),
        Config(m) => CoreError::InvalidInput(m),
    }
}
