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
/// module — `client_id` and `client_secret` sit here in the DB.
///
/// "Why is the client_secret in the DB and not the keychain?"
/// Google's own Desktop-app OAuth documentation concedes that the
/// client secret in this flow "is not treated as a secret" —
/// installed apps are expected to embed it in the source code. We
/// adopt the same view: it's an identifier of the user's own
/// Google Cloud project, not a credential that would let an
/// attacker who reads it impersonate the user. Putting it next to
/// the client_id keeps the data layout coherent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleAccountConfig {
    /// OAuth 2.0 client id from the user's Google Cloud Console
    /// (Desktop app type). Treated as public information.
    pub client_id: String,
    /// The matching client_secret from the same Cloud Console
    /// entry. See the type-level doc for the "not really a secret"
    /// discussion.
    pub client_secret: String,
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
    pub fn new(client_id: String, client_secret: String, tokens: TokenSet) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            state: ApiState::new(tokens, client_id, client_secret, http),
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
        client_secret: &str,
    ) -> GoogleResult<TokenSet> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| GoogleError::Io(e.to_string()))?;
        auth::run_default(client_id, client_secret, &http).await
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
        calendar_id: &str,
        event: NewEvent,
    ) -> CoreResult<Event> {
        api::create_event(&self.state, calendar_id, event)
            .await
            .map_err(to_core_error)
    }

    async fn update_event(&self, event: Event) -> CoreResult<Event> {
        api::update_event(&self.state, &event)
            .await
            .map_err(to_core_error)
    }

    async fn delete_event(&self, event_id: &str) -> CoreResult<()> {
        // Aperio's command layer hands us the calendar_id alongside
        // the event_id when it can, but the legacy
        // `delete_event(event_id)` trait method doesn't carry it.
        // We walk every calendar in the listing cache and try the
        // delete against each — the first 2xx wins, the rest get
        // their 404s swallowed. Mirrors how the CalDAV adapter
        // copes with the same signature gap.
        let cals = self.list_calendars().await?;
        for cal in cals {
            if api::delete_event(&self.state, &cal.id, event_id).await.is_ok() {
                return Ok(());
            }
        }
        Err(CoreError::NotFound(format!(
            "event '{event_id}' not found in any calendar"
        )))
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
        calendar_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        api::rename_calendar(&self.state, calendar_id, new_name)
            .await
            .map_err(to_core_error)?;
        // The cached listing still has the old summary — drop it so
        // the next list_calendars walks Google again and surfaces
        // the new name.
        *self.calendars_cache.lock().await = None;
        Ok(())
    }

    async fn add_event_exdate(
        &self,
        event_id: &str,
        occurrence: chrono::DateTime<chrono::Utc>,
    ) -> CoreResult<()> {
        // Same calendar-id-walking pattern as delete_event — the
        // trait method doesn't carry the calendar id.
        let cals = self.list_calendars().await?;
        for cal in cals {
            if api::add_event_exdate(&self.state, &cal.id, event_id, occurrence)
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(CoreError::NotFound(format!(
            "event '{event_id}' not found in any calendar"
        )))
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
