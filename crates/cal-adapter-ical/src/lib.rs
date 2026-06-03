//! Read-only iCal feed adapter (DESIGN.md §6.2 — Phase 6c).
//!
//! Subscribes to a static `.ics` URL served over HTTP(S). Typical
//! examples: public holiday calendars, school timetables, sports
//! schedules, geteilte iCloud-Public-Links. The adapter:
//!
//!   - GETs the feed on every `get_events` call,
//!   - keeps the last body in memory + the server's `ETag` /
//!     `Last-Modified` so a follow-up GET sends conditional headers
//!     and gets a cheap `304 Not Modified` when nothing changed,
//!   - presents the feed as one synthetic `Calendar` whose
//!     `read_only` flag is `true` so the frontend knows not to offer
//!     create/edit actions,
//!   - returns `Error::Unsupported` from every write operation
//!     (RFC 5545 iCal feeds are a one-way distribution channel; to
//!     edit events the user needs a CalDAV account or the local
//!     adapter).
//!
//! Parsing reuses [`cal_adapter_caldav::mapping::parse_calendar_data`]
//! — the iCal feed body and a CalDAV REPORT's CALDATA blob are the
//! same syntax (VCALENDAR with VEVENTs), so a single parser covers
//! both.

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use cal_adapter_caldav::mapping;
use cal_core::{
    Adapter, AuthToken, Calendar, CalendarFeature, Capability, ContainerColor,
    Credentials as CoreCredentials, DateRange, Error, Event, FreeBusy, NewEvent, Result,
};
use reqwest::header::{
    HeaderValue, ACCEPT, AUTHORIZATION, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;
use url::Url;

pub mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum IcalError {
        #[error("invalid configuration: {0}")]
        Config(String),
        #[error("invalid URL: {0}")]
        Url(String),
        #[error("authentication failed (HTTP {0})")]
        Auth(u16),
        #[error("network error: {0}")]
        Network(String),
        #[error("server returned HTTP {0}")]
        Server(u16),
        #[error("parse error: {0}")]
        Parse(String),
    }

    pub type IcalResult<T> = std::result::Result<T, IcalError>;

    impl From<reqwest::Error> for IcalError {
        fn from(err: reqwest::Error) -> Self {
            IcalError::Network(err.to_string())
        }
    }
}

pub use error::{IcalError, IcalResult};

/// Account configuration persisted in `accounts.config_json`.
/// `feed_url` is the public `.ics` URL; the optional username covers
/// the rare case of a private feed behind Basic auth (the password
/// lives in the platform keychain via the secrets module).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcalAccountConfig {
    pub feed_url: String,
    pub username: Option<String>,
}

/// Runtime bundle of the persisted config + the resolved secret.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub config: IcalAccountConfig,
    pub password: Option<String>,
}

impl Credentials {
    pub fn new(config: IcalAccountConfig, password: Option<String>) -> Self {
        Self { config, password }
    }
}

/// One configured iCal feed account.
#[derive(Debug)]
pub struct IcalAdapter {
    credentials: Credentials,
    http: Client,
    /// Last successful fetch with its caching headers. Used to send
    /// conditional GETs on subsequent reads so unchanged feeds don't
    /// re-download the body — and, inside the `cache_ttl` window, to
    /// skip the network entirely.
    cache: Mutex<Option<CachedFeed>>,
    /// In-memory freshness window. Within this duration after the
    /// last successful fetch, `fetch_body` returns the cached body
    /// directly without even a conditional GET. Past the window we
    /// still re-validate with `If-None-Match`. Default 30 s — long
    /// enough to absorb a typical view-switch storm, short enough
    /// that a feed update is picked up within a coffee sip.
    cache_ttl: chrono::Duration,
    capabilities: Vec<Capability>,
}

#[derive(Debug, Clone)]
struct CachedFeed {
    etag: Option<String>,
    last_modified: Option<String>,
    body: String,
    fetched_at: chrono::DateTime<chrono::Utc>,
}

impl IcalAdapter {
    /// Construct an adapter for a single account. The `connect_timeout`
    /// (default 10 s) and request timeout (30 s) match the CalDAV
    /// adapter so a typo'd URL doesn't hang the UI.
    pub fn new(credentials: Credentials) -> IcalResult<Self> {
        if credentials.config.feed_url.trim().is_empty() {
            return Err(IcalError::Config("feed_url must not be empty".into()));
        }
        // Validate URL eagerly so the user finds out at account
        // creation, not on the first failed fetch.
        Url::parse(&credentials.config.feed_url).map_err(|e| IcalError::Url(e.to_string()))?;

        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| IcalError::Network(e.to_string()))?;

        Ok(Self {
            credentials,
            http,
            cache: Mutex::new(None),
            cache_ttl: chrono::Duration::seconds(30),
            capabilities: vec![Capability::Calendar],
        })
    }

    /// Stable, deterministic calendar id derived from the feed URL.
    /// Same URL → same id across restarts. The 8-byte SHA-256 prefix
    /// is collision-resistant in practice and keeps the id short.
    pub fn calendar_id(&self) -> String {
        Self::id_for_url(&self.credentials.config.feed_url)
    }

    fn id_for_url(url: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(url.as_bytes());
        let digest = hasher.finalize();
        format!("ical:{}", hex::encode(&digest[..8]))
    }

    /// Override the in-memory freshness window. The default 30 s is
    /// the right knob for production use; tests that need to
    /// exercise the network-revalidation path inject a zero TTL so
    /// every `fetch_body` call still hits the wire (with
    /// `If-None-Match`).
    #[doc(hidden)]
    pub fn with_cache_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.cache_ttl =
            chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::zero());
        self
    }

    /// Smoke test the feed: fetches once, returns Ok with the
    /// derived calendar name on success. Used by the account-creation
    /// flow so the user knows before persisting whether the URL is
    /// reachable. Doesn't poison the cache for the real adapter
    /// because the test instance is dropped after the call.
    pub async fn smoke_test(&self) -> IcalResult<String> {
        let body = self.fetch_body().await?;
        Ok(self.derive_calendar_name(&body))
    }

    /// Fetch the feed, honouring HTTP caching. Returns the iCal text
    /// (whether freshly fetched or read from the in-memory cache
    /// after a 304 Not Modified).
    async fn fetch_body(&self) -> IcalResult<String> {
        let url = Url::parse(&self.credentials.config.feed_url)
            .map_err(|e| IcalError::Url(e.to_string()))?;

        // Short-circuit when the cached body is still fresh. We
        // copy the body out of the mutex first (the borrow checker
        // would otherwise force us to hold the guard across the
        // `await`) and return without touching the network.
        {
            let guard = self.cache.lock().expect("cache poison");
            if let Some(c) = guard.as_ref() {
                let age = chrono::Utc::now().signed_duration_since(c.fetched_at);
                if age >= chrono::Duration::zero() && age < self.cache_ttl {
                    return Ok(c.body.clone());
                }
            }
        }

        let (cached_etag, cached_lastmod, cached_body) = {
            let guard = self.cache.lock().expect("cache poison");
            match guard.as_ref() {
                Some(c) => (
                    c.etag.clone(),
                    c.last_modified.clone(),
                    Some(c.body.clone()),
                ),
                None => (None, None, None),
            }
        };

        let mut req = self.http.get(url.clone()).header(
            ACCEPT,
            HeaderValue::from_static("text/calendar, application/calendar+xml, */*;q=0.1"),
        );
        if let Some(etag) = cached_etag.as_deref() {
            if let Ok(v) = HeaderValue::from_str(etag) {
                req = req.header(IF_NONE_MATCH, v);
            }
        }
        if let Some(lm) = cached_lastmod.as_deref() {
            if let Ok(v) = HeaderValue::from_str(lm) {
                req = req.header(IF_MODIFIED_SINCE, v);
            }
        }
        if let (Some(user), Some(pass)) = (
            self.credentials.config.username.as_ref(),
            self.credentials.password.as_ref(),
        ) {
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
            if let Ok(v) = HeaderValue::from_str(&format!("Basic {token}")) {
                req = req.header(AUTHORIZATION, v);
            }
        }

        let response = req.send().await?;
        let status = response.status();

        if status == StatusCode::NOT_MODIFIED {
            // Server confirmed the cache is fresh — return the
            // previous body without re-downloading.
            if let Some(body) = cached_body {
                return Ok(body);
            }
            // If we somehow got a 304 without ever having cached a
            // body, treat it as a server bug — fall through.
            return Err(IcalError::Server(status.as_u16()));
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(IcalError::Auth(status.as_u16()));
        }
        if !status.is_success() {
            return Err(IcalError::Server(status.as_u16()));
        }

        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());
        let last_modified = response
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());
        let body = response.text().await?;

        *self.cache.lock().expect("cache poison") = Some(CachedFeed {
            etag,
            last_modified,
            body: body.clone(),
            fetched_at: chrono::Utc::now(),
        });

        Ok(body)
    }

    /// Derive a display name. Most producers (Apple, Google, others)
    /// emit `X-WR-CALNAME:Foo` near the top of the file. The icalendar
    /// crate's typed API doesn't surface X-WR-* properties, so we
    /// scan the lines ourselves and stop at the first match.
    ///
    /// Fallback chain when X-WR-CALNAME is missing:
    ///   1. last path segment of the URL, sans `.ics` extension
    ///      — `/.../schulferien-sachsen-anhalt.ics` becomes
    ///      `schulferien-sachsen-anhalt`. Beats the bare hostname
    ///      because two feeds from the same provider would otherwise
    ///      share a name in the sidebar.
    ///   2. URL host (`feiertage-deutschland.de`).
    ///   3. The literal "iCal feed" if URL parsing fails entirely.
    fn derive_calendar_name(&self, body: &str) -> String {
        for raw in body.lines() {
            let line = raw.trim();
            if let Some(rest) = line
                .strip_prefix("X-WR-CALNAME:")
                .or_else(|| line.strip_prefix("X-WR-CALNAME;"))
            {
                let value = rest
                    .split_once(':')
                    .map(|(_params, v)| v)
                    .unwrap_or(rest)
                    .trim();
                if !value.is_empty() {
                    return value.to_string();
                }
            }
        }
        let parsed = Url::parse(&self.credentials.config.feed_url).ok();
        // Try the path's filename first — `.ics` files almost always
        // have a meaningful slug here.
        if let Some(url) = parsed.as_ref() {
            if let Some(last) = url
                .path_segments()
                .and_then(|segments| segments.rev().find(|s| !s.is_empty()))
            {
                let trimmed = last
                    .strip_suffix(".ics")
                    .or_else(|| last.strip_suffix(".ICS"))
                    .or_else(|| last.strip_suffix(".ical"))
                    .unwrap_or(last);
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
        parsed
            .and_then(|u| u.host_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "iCal feed".to_string())
    }
}

#[async_trait]
impl Adapter for IcalAdapter {
    async fn authenticate(&self, _credentials: CoreCredentials) -> Result<AuthToken> {
        // iCal feeds have no per-call token concept. The configured
        // Basic-auth credentials (if any) are applied on every fetch
        // inside `fetch_body`; this trait method exists only so the
        // registry can talk to every adapter through the same trait.
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl CalendarFeature for IcalAdapter {
    async fn list_calendars(&self) -> Result<Vec<Calendar>> {
        let body = self.fetch_body().await.map_err(to_core_error)?;
        let name = self.derive_calendar_name(&body);
        Ok(vec![Calendar {
            color_label: None,
            id: self.calendar_id(),
            name,
            color: None,
            read_only: true,
            default_sound: None,
            supports_scheduling: false,
        }])
    }

    async fn get_events(&self, calendar_id: &str, range: DateRange) -> Result<Vec<Event>> {
        if calendar_id != self.calendar_id() {
            // Routed to the wrong adapter. The registry shouldn't do
            // this, but be defensive — return an empty list rather
            // than a misleading error so the aggregating command
            // sees "no events from this account" and moves on.
            warn!(
                expected = %self.calendar_id(),
                got = %calendar_id,
                "get_events on iCal adapter with unknown calendar_id"
            );
            return Ok(Vec::new());
        }
        let body = self.fetch_body().await.map_err(to_core_error)?;
        let mut events = mapping::parse_calendar_data(&body, calendar_id)
            .map_err(|e| Error::Protocol(format!("ical parse: {e}")))?;
        // Local range filter. iCal feeds aren't queryable, so the
        // server hands us the whole calendar — we trim here. Recurring
        // events stay regardless of range because the frontend expands
        // them in the user's local timezone via rrule.js and applies
        // the visible-range clip after expansion.
        let DateRange { start, end } = range;
        events.retain(|ev| ev.recurrence.is_some() || (ev.end > start && ev.start < end));
        Ok(events)
    }

    async fn create_event(&self, _calendar_id: &str, _event: NewEvent) -> Result<Event> {
        Err(Error::Unsupported(
            "iCal feed accounts are read-only".into(),
        ))
    }

    async fn update_event(&self, _event: Event) -> Result<Event> {
        Err(Error::Unsupported(
            "iCal feed accounts are read-only".into(),
        ))
    }

    async fn delete_event(&self, _event_id: &str, _send_cancellations: bool) -> Result<()> {
        Err(Error::Unsupported(
            "iCal feed accounts are read-only".into(),
        ))
    }

    async fn get_free_busy(&self, _emails: &[&str], _range: DateRange) -> Result<Vec<FreeBusy>> {
        // iCal feeds don't carry per-attendee availability — they're
        // single-author distribution. Return empty rather than
        // Unsupported so the aggregating free/busy command can still
        // merge results across accounts.
        Ok(Vec::new())
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
        None
    }

    async fn add_event_exdate(
        &self,
        _event_id: &str,
        _occurrence: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        Err(Error::Unsupported(
            "iCal feed accounts are read-only".into(),
        ))
    }
}

fn to_core_error(err: IcalError) -> Error {
    use IcalError::*;
    match err {
        Auth(_) => Error::Authentication(err.to_string()),
        Url(_) | Config(_) => Error::InvalidInput(err.to_string()),
        Parse(_) => Error::Protocol(err.to_string()),
        Server(_) | Network(_) => Error::Network(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn cfg(url: &str) -> Credentials {
        Credentials::new(
            IcalAccountConfig {
                feed_url: url.to_string(),
                username: None,
            },
            None,
        )
    }

    #[test]
    fn calendar_id_is_stable_for_the_same_url() {
        let a = IcalAdapter::new(cfg("https://example.com/cal.ics")).unwrap();
        let b = IcalAdapter::new(cfg("https://example.com/cal.ics")).unwrap();
        assert_eq!(a.calendar_id(), b.calendar_id());
        assert!(a.calendar_id().starts_with("ical:"));
    }

    #[test]
    fn calendar_id_differs_per_url() {
        let a = IcalAdapter::new(cfg("https://example.com/one.ics")).unwrap();
        let b = IcalAdapter::new(cfg("https://example.com/two.ics")).unwrap();
        assert_ne!(a.calendar_id(), b.calendar_id());
    }

    #[test]
    fn rejects_empty_url() {
        let err = IcalAdapter::new(cfg("")).unwrap_err();
        assert!(matches!(err, IcalError::Config(_)));
    }

    #[test]
    fn rejects_unparseable_url() {
        let err = IcalAdapter::new(cfg("not a url")).unwrap_err();
        assert!(matches!(err, IcalError::Url(_)));
    }

    #[test]
    fn derive_calendar_name_reads_x_wr_calname() {
        let adapter = IcalAdapter::new(cfg("https://example.com/cal.ics")).unwrap();
        let body =
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:My Schedule\r\nEND:VCALENDAR\r\n";
        assert_eq!(adapter.derive_calendar_name(body), "My Schedule");
    }

    #[test]
    fn derive_calendar_name_falls_back_to_path_slug() {
        // `schulferien-sachsen-anhalt.ics` doesn't carry an
        // X-WR-CALNAME but the path-segment fallback yields a
        // meaningful name; two distinct feeds from the same host
        // therefore land on distinct sidebar entries.
        let adapter = IcalAdapter::new(cfg(
            "https://www.feiertage-deutschland.de/kalender-download/ics/schulferien-sachsen-anhalt.ics",
        ))
        .unwrap();
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
        assert_eq!(
            adapter.derive_calendar_name(body),
            "schulferien-sachsen-anhalt"
        );
    }

    #[test]
    fn derive_calendar_name_falls_back_to_host_when_path_is_empty() {
        // URL ends with a slash — no usable filename. The hostname
        // is the next-best identifier.
        let adapter = IcalAdapter::new(cfg("https://example.com/")).unwrap();
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
        assert_eq!(adapter.derive_calendar_name(body), "example.com");
    }

    #[test]
    fn derive_calendar_name_strips_ics_extension_case_insensitively() {
        let adapter = IcalAdapter::new(cfg("https://example.com/Holidays.ICS")).unwrap();
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
        assert_eq!(adapter.derive_calendar_name(body), "Holidays");
    }

    #[tokio::test]
    async fn fetch_body_uses_cache_on_304() {
        let mut server = mockito::Server::new_async().await;
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Test\r\nEND:VCALENDAR\r\n";

        let m1 = server
            .mock("GET", "/cal.ics")
            .with_status(200)
            .with_header("etag", "\"v1\"")
            .with_header("content-type", "text/calendar")
            .with_body(body)
            .expect(1)
            .create_async()
            .await;

        let m2 = server
            .mock("GET", "/cal.ics")
            .match_header("if-none-match", "\"v1\"")
            .with_status(304)
            .expect(1)
            .create_async()
            .await;

        // TTL=0 forces every fetch_body to hit the wire — otherwise
        // the second call would short-circuit on the in-memory body
        // and the 304 path under test would never run.
        let adapter = IcalAdapter::new(cfg(&format!("{}/cal.ics", server.url())))
            .unwrap()
            .with_cache_ttl(std::time::Duration::ZERO);

        // First call: 200 with body.
        let first = adapter.fetch_body().await.unwrap();
        assert!(first.contains("X-WR-CALNAME"));

        // Second call: server returns 304, adapter serves cached body.
        let second = adapter.fetch_body().await.unwrap();
        assert_eq!(first, second);

        m1.assert_async().await;
        m2.assert_async().await;
    }

    #[tokio::test]
    async fn list_calendars_synthesises_one_read_only_row() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/cal.ics")
            .with_status(200)
            .with_body(
                "BEGIN:VCALENDAR\r\n\
                 VERSION:2.0\r\n\
                 X-WR-CALNAME:Schulkalender\r\n\
                 END:VCALENDAR\r\n",
            )
            .create_async()
            .await;

        let adapter = IcalAdapter::new(cfg(&format!("{}/cal.ics", server.url()))).unwrap();
        let cals = adapter.list_calendars().await.unwrap();
        assert_eq!(cals.len(), 1);
        assert_eq!(cals[0].name, "Schulkalender");
        assert!(cals[0].read_only);
    }

    #[tokio::test]
    async fn get_events_parses_and_filters_to_range() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/cal.ics")
            .with_status(200)
            .with_body(
                "BEGIN:VCALENDAR\r\n\
                 VERSION:2.0\r\n\
                 BEGIN:VEVENT\r\n\
                 UID:ev-1@example.com\r\n\
                 SUMMARY:In range\r\n\
                 DTSTART:20260520T100000Z\r\n\
                 DTEND:20260520T110000Z\r\n\
                 END:VEVENT\r\n\
                 BEGIN:VEVENT\r\n\
                 UID:ev-2@example.com\r\n\
                 SUMMARY:Out of range (past)\r\n\
                 DTSTART:20250101T100000Z\r\n\
                 DTEND:20250101T110000Z\r\n\
                 END:VEVENT\r\n\
                 END:VCALENDAR\r\n",
            )
            .create_async()
            .await;

        let adapter = IcalAdapter::new(cfg(&format!("{}/cal.ics", server.url()))).unwrap();
        let cal_id = adapter.calendar_id();
        let range = DateRange::new(
            chrono::Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
            chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
        );
        let events = adapter.get_events(&cal_id, range).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "In range");
    }

    #[tokio::test]
    async fn write_operations_return_unsupported() {
        let adapter = IcalAdapter::new(cfg("https://example.com/cal.ics")).unwrap();
        let err = adapter.delete_event("anything", false).await.unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)));
    }

    #[tokio::test]
    async fn http_401_maps_to_authentication_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/cal.ics")
            .with_status(401)
            .create_async()
            .await;

        let adapter = IcalAdapter::new(cfg(&format!("{}/cal.ics", server.url()))).unwrap();
        let err = adapter.list_calendars().await.unwrap_err();
        assert!(matches!(err, Error::Authentication(_)));
    }

    #[tokio::test]
    async fn cache_ttl_skips_the_network_within_the_window() {
        let mut server = mockito::Server::new_async().await;
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";

        // Mockito expectation: exactly ONE request reaches the
        // server. A second `fetch_body` inside the TTL window should
        // serve directly from the in-memory cache.
        let m = server
            .mock("GET", "/cal.ics")
            .with_status(200)
            .with_body(body)
            .expect(1)
            .create_async()
            .await;

        // Default TTL (30 s) is well past any test runtime; the
        // second call must hit the cache regardless.
        let adapter = IcalAdapter::new(cfg(&format!("{}/cal.ics", server.url()))).unwrap();
        let first = adapter.fetch_body().await.unwrap();
        let second = adapter.fetch_body().await.unwrap();
        assert_eq!(first, second);
        m.assert_async().await;
    }
}
