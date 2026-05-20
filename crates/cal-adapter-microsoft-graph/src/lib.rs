//! Microsoft Graph adapter (DESIGN.md §6.2, Phase 6e).
//!
//! OAuth 2.0 PKCE against the Microsoft Identity Platform v2.0 endpoint
//! + the Graph Calendar API. Same architectural shape as the Google
//! adapter; the differences (no client_secret, structured recurrence,
//! `calendarView` for range expansion, `nextLink` pagination) live in
//! the `auth`, `api` and `mapping` modules.
//!
//! Setup requirement: the user registers an app of type "Public
//! client" in Azure Portal → Entra ID → App registrations, sets the
//! redirect URI to `http://localhost` (loopback IP pattern), and
//! pastes the client id into Aperio. No client_secret is needed —
//! Microsoft honours the PKCE-public-client model the spec
//! describes.

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

pub use auth::{TokenSet, DEFAULT_AUTHORITY};
pub use error::{GraphError, GraphResult};

use crate::api::ApiState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphAccountConfig {
    pub client_id: String,
    /// `common` | `consumers` | `organizations` | tenant GUID. Most
    /// users want `common` so both personal Microsoft accounts and
    /// work/school accounts can authenticate; admins who want to
    /// pin a tenant can override.
    #[serde(default = "default_authority")]
    pub authority: String,
    #[serde(default)]
    pub account_label: Option<String>,
}

fn default_authority() -> String {
    DEFAULT_AUTHORITY.to_string()
}

#[derive(Debug)]
pub struct MicrosoftGraphAdapter {
    state: ApiState,
    capabilities: Vec<Capability>,
    calendars_cache: Mutex<Option<(Vec<Calendar>, chrono::DateTime<chrono::Utc>)>>,
    listing_ttl: chrono::Duration,
}

impl MicrosoftGraphAdapter {
    pub fn new(client_id: String, authority: String, tokens: TokenSet) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            state: ApiState::new(tokens, client_id, auth::token_url(&authority), http),
            capabilities: vec![Capability::Calendar],
            calendars_cache: Mutex::new(None),
            listing_ttl: chrono::Duration::minutes(5),
        }
    }

    #[doc(hidden)]
    pub fn with_listing_ttl(mut self, ttl: Duration) -> Self {
        self.listing_ttl = chrono::Duration::from_std(ttl)
            .unwrap_or_else(|_| chrono::Duration::zero());
        self
    }

    pub async fn authenticate_interactive(
        client_id: &str,
        authority: &str,
    ) -> GraphResult<TokenSet> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| GraphError::Io(e.to_string()))?;
        auth::run_default(client_id, authority, &http).await
    }

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
impl Adapter for MicrosoftGraphAdapter {
    async fn authenticate(
        &self,
        _credentials: CoreCredentials,
    ) -> CoreResult<AuthToken> {
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl CalendarFeature for MicrosoftGraphAdapter {
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
        // Graph's event-id is mailbox-wide unique — no calendar
        // walk required, unlike Google.
        api::delete_event(&self.state, event_id)
            .await
            .map_err(to_core_error)
    }

    async fn get_free_busy(
        &self,
        _emails: &[&str],
        _range: DateRange,
    ) -> CoreResult<Vec<FreeBusy>> {
        Ok(Vec::new())
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
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
        *self.calendars_cache.lock().await = None;
        Ok(())
    }

    async fn add_event_exdate(
        &self,
        _event_id: &str,
        _occurrence: chrono::DateTime<chrono::Utc>,
    ) -> CoreResult<()> {
        // Graph exposes per-occurrence cancellation through its
        // "event series exception" endpoint, which works against
        // an instance id of an expanded series. Since we read via
        // `/calendarView` (server-side expansion) the instance ids
        // are stable per occurrence — but the API also accepts a
        // PATCH on the master event with a structured cancellation
        // shape that's distinct from RRULE EXDATE. Keep it
        // `Unsupported` until the per-occurrence cancellation
        // endpoint is wired up properly in a follow-up; the
        // frontend's "delete only this occurrence" still works for
        // CalDAV / iCal / Google / local.
        Err(CoreError::Unsupported(
            "Microsoft Graph per-occurrence delete lands in a follow-up commit".into(),
        ))
    }
}

fn to_core_error(err: GraphError) -> CoreError {
    use GraphError::*;
    match err {
        Network(m) => CoreError::Network(m),
        Http { status, message } => match status {
            401 | 403 => CoreError::Authentication(message),
            404 => CoreError::NotFound(message),
            409 | 412 => CoreError::Conflict(message),
            _ => CoreError::Protocol(format!("Graph HTTP {status}: {message}")),
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
