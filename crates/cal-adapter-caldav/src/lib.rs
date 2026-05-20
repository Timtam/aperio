//! CalDAV calendar + task adapter (DESIGN.md §6.2).
//!
//! Phase 6b lands in three internal increments:
//!
//!  - **6b.1** (this iteration) — server discovery + auth. The
//!    adapter can take a hostname / URL + credentials and find the
//!    URL of the user's calendar-home collection. That is the
//!    foundation every following operation builds on.
//!  - **6b.2** — calendar listing + range read of events.
//!  - **6b.3** — create / update / delete events + VTODO tasks.
//!
//! The shape is intentionally small for now: one `CaldavAdapter`
//! struct, constructed from `Credentials`, holding a shared
//! `reqwest::Client` and the discovered URLs after the first call to
//! [`CaldavAdapter::discover`]. Implementing the full `cal-core`
//! `Adapter` / `CalendarFeature` / `TasksFeature` trait surface is
//! left to the later iterations — at this point the registry layer
//! that would route through those traits doesn't exist yet either.

mod auth;
pub mod calendars;
pub mod config;
pub mod discovery;
pub mod error;
pub mod events;
pub mod mapping;
pub mod tasks;
mod xml;

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use cal_core::{
    Adapter, AuthToken, Calendar, CalendarFeature, Capability, ContainerColor,
    Credentials as CoreCredentials, DateRange, Error as CoreError, Event, FreeBusy,
    NewEvent, NewTask, Result as CoreResult, Task, TaskList, TasksFeature,
};
use reqwest::Client;
use url::Url;

pub use config::{AuthKind, CaldavAccountConfig, Credentials};
pub use discovery::Discovery;
pub use error::{CaldavError, CaldavResult};

/// One configured CalDAV account. Cheap to clone — the `Client`
/// is reference-counted by reqwest and the `Mutex` only protects
/// the lazily-cached discovery result.
pub struct CaldavAdapter {
    credentials: Credentials,
    http: Client,
    /// Filled on first `discover()` call so subsequent reads don't
    /// re-walk the well-known chain. Cleared when the user changes
    /// credentials (currently by constructing a fresh adapter).
    discovery: Mutex<Option<Discovery>>,
    /// Capabilities the adapter declares to the registry. Always
    /// `[Calendar]` at this point — `TasksFeature` joins once
    /// VTODO read/write lands in 6b.3.
    capabilities: Vec<Capability>,
}

impl CaldavAdapter {
    /// Construct an adapter for a single account.
    ///
    /// `connect_timeout` defaults to 10s when `None` — most CalDAV
    /// servers respond within that window and the user should not
    /// be left waiting on a typo'd URL for the full system default.
    pub fn new(
        credentials: Credentials,
        connect_timeout: Option<Duration>,
    ) -> CaldavResult<Self> {
        // Disable automatic redirect following — the well-known
        // discovery step in `discovery::resolve_well_known` *needs*
        // to see the 301/302 directly to read the Location header.
        // Letting reqwest swallow the redirect would land us on the
        // final endpoint with a GET, which most CalDAV servers
        // answer with 405 / 501, masking the actual flow.
        // PROPFIND traffic isn't supposed to redirect; if it ever
        // does, the discovery layer handles it the same way.
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(connect_timeout.unwrap_or(Duration::from_secs(10)))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            credentials,
            http,
            discovery: Mutex::new(None),
            capabilities: vec![Capability::Calendar, Capability::Tasks],
        })
    }

    /// Returns the discovered calendar-home URL, running the
    /// discovery chain once and caching the result. Subsequent calls
    /// return the cached value without any HTTP traffic.
    pub async fn discover(&self) -> CaldavResult<Discovery> {
        {
            if let Some(d) = self.discovery.lock().expect("poison").as_ref() {
                return Ok(d.clone());
            }
        }
        let fresh = discovery::run(&self.http, &self.credentials).await?;
        *self.discovery.lock().expect("poison") = Some(fresh.clone());
        Ok(fresh)
    }

    /// Re-run discovery from scratch. Used when the user changes
    /// their server URL or after they re-enter credentials.
    pub async fn refresh_discovery(&self) -> CaldavResult<Discovery> {
        *self.discovery.lock().expect("poison") = None;
        self.discover().await
    }

    /// Test-only: peek at the cached result without going to the wire.
    #[cfg(test)]
    fn cached_calendar_home(&self) -> Option<url::Url> {
        self.discovery
            .lock()
            .expect("poison")
            .as_ref()
            .map(|d| d.calendar_home_url.clone())
    }
}

/// Translate a CalDAV-specific error into the shared `cal_core::Error`
/// shape so the rest of the app can pattern-match it uniformly.
fn to_core_error(err: CaldavError) -> CoreError {
    match err {
        CaldavError::Network(msg) => CoreError::Network(msg),
        CaldavError::Http { status, message } => match status {
            401 | 403 => CoreError::Authentication(message),
            404 => CoreError::NotFound(message),
            409 | 412 => CoreError::Conflict(message),
            _ => CoreError::Protocol(format!("HTTP {status}: {message}")),
        },
        CaldavError::Protocol(msg) => CoreError::Protocol(msg),
        CaldavError::Discovery(msg) => CoreError::Protocol(format!("discovery: {msg}")),
        CaldavError::Config(msg) => CoreError::InvalidInput(msg),
    }
}

#[async_trait]
impl Adapter for CaldavAdapter {
    async fn authenticate(&self, _credentials: CoreCredentials) -> CoreResult<AuthToken> {
        // CalDAV auth happens per request via the Authorization
        // header — there's no separate token to fetch up front.
        // Triggering a discovery here doubles as a connection +
        // credential smoke test so the registry can decide whether
        // to keep the account active.
        self.discover().await.map_err(to_core_error)?;
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl CalendarFeature for CaldavAdapter {
    async fn list_calendars(&self) -> CoreResult<Vec<Calendar>> {
        let discovery = self.discover().await.map_err(to_core_error)?;
        calendars::list_calendars(
            &self.http,
            &discovery.calendar_home_url,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)
    }

    async fn get_events(
        &self,
        calendar_id: &str,
        range: DateRange,
    ) -> CoreResult<Vec<Event>> {
        // The calendar id is the absolute collection URL produced
        // by `list_calendars`. Re-parse it so the request lands at
        // the exact path the server told us about; falling back to
        // a join against the discovered home would be too lax.
        let cal_url = Url::parse(calendar_id)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        events::get_events(&self.http, &cal_url, range, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn create_event(
        &self,
        calendar_id: &str,
        event: NewEvent,
    ) -> CoreResult<Event> {
        let cal_url = Url::parse(calendar_id)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        events::create_event(&self.http, &cal_url, event, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn update_event(&self, event: Event) -> CoreResult<Event> {
        events::update_event(&self.http, event, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn add_event_exdate(
        &self,
        event_id: &str,
        occurrence: chrono::DateTime<chrono::Utc>,
    ) -> CoreResult<()> {
        // Same walk-the-home-set workaround as delete_event below —
        // the trait signature loses the parent calendar id, so we
        // try every calendar in the home set until one accepts the
        // EXDATE update. The aperio command layer routes via the
        // registry's calendar→account map, so production hits this
        // path with the right adapter already; the walk is the
        // fallback if a caller forgot to thread the calendar_id
        // through.
        let discovery = self.discover().await.map_err(to_core_error)?;
        let cals = calendars::list_calendars(
            &self.http,
            &discovery.calendar_home_url,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)?;
        let mut last_err: Option<CoreError> = None;
        for cal in cals {
            let cal_url = match Url::parse(&cal.id) {
                Ok(u) => u,
                Err(_) => continue,
            };
            match events::add_event_exdate(
                &self.http,
                &cal_url,
                event_id,
                occurrence,
                &self.credentials,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(err) => {
                    // 404 just means "this event lives in a different
                    // calendar" — keep walking. Anything else we
                    // remember in case nothing else works.
                    if matches!(err, CaldavError::Http { status: 404, .. }) {
                        continue;
                    }
                    last_err = Some(to_core_error(err));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            CoreError::NotFound(format!(
                "event '{event_id}' not found in any calendar"
            ))
        }))
    }

    async fn delete_event(&self, event_id: &str) -> CoreResult<()> {
        // The trait signature only gives us the event id. CalDAV
        // needs the calendar collection URL too — we recover it
        // by re-reading the discovery cache; callers that know
        // the calendar URL up front can hit `events::delete_event`
        // directly. The current API is good enough for the
        // registry layer where the calling code picked up the
        // event from `get_events` first (so we know which calendar
        // it lives on).
        //
        // Without the calendar id we fall back to a best-effort:
        // walk every calendar in the home set and try to DELETE on
        // each. That's the lazy path; the registry will refactor
        // the trait signature in Phase 6b.4 to thread the calendar
        // id through delete as well.
        let discovery = self.discover().await.map_err(to_core_error)?;
        let cals = calendars::list_calendars(
            &self.http,
            &discovery.calendar_home_url,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)?;
        for cal in cals {
            let cal_url = match Url::parse(&cal.id) {
                Ok(u) => u,
                Err(_) => continue,
            };
            // Without an ETag we don't bother with If-Match — the
            // user explicitly chose to delete this row, so a
            // concurrent modification is informational at best.
            if let Ok(()) = events::delete_event(
                &self.http,
                &cal_url,
                event_id,
                None,
                &self.credentials,
            )
            .await
            {
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
        // CalDAV exposes free-busy at the principal level via a
        // separate REPORT. Out of scope for the calendar-first
        // iteration; returning an empty list keeps consumers calm.
        Ok(Vec::new())
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
        // The color is fetched together with the listing and lives
        // on the Calendar struct already. A second per-id round-trip
        // would just duplicate work; consumers should use the value
        // off the Calendar they got from `list_calendars`.
        None
    }

    async fn rename_calendar(
        &self,
        calendar_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        // Calendar id == collection URL in CalDAV (see `to_calendar`).
        let url = Url::parse(calendar_id).map_err(|e| {
            CoreError::InvalidInput(format!("calendar id is not a URL: {e}"))
        })?;
        calendars::proppatch_displayname(&self.http, &url, new_name, &self.credentials)
            .await
            .map_err(to_core_error)
    }
}

#[async_trait]
impl TasksFeature for CaldavAdapter {
    async fn list_task_lists(&self) -> CoreResult<Vec<TaskList>> {
        let discovery = self.discover().await.map_err(to_core_error)?;
        tasks::list_task_lists(
            &self.http,
            &discovery.calendar_home_url,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)
    }

    async fn get_tasks(&self, list_id: &str) -> CoreResult<Vec<Task>> {
        let list_url = Url::parse(list_id)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        tasks::get_tasks(&self.http, &list_url, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn create_task(
        &self,
        list_id: &str,
        task: NewTask,
    ) -> CoreResult<Task> {
        let list_url = Url::parse(list_id)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        tasks::create_task(&self.http, &list_url, task, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn update_task(&self, task: Task) -> CoreResult<Task> {
        tasks::update_task(&self.http, task, &self.credentials)
            .await
            .map_err(to_core_error)
    }

    async fn delete_task(&self, task_id: &str) -> CoreResult<()> {
        // Same shape as delete_event: the trait signature loses the
        // list id so we walk the home set and try each candidate
        // collection. 6b.4 will refactor the trait to carry the
        // list/calendar id alongside the row id.
        let discovery = self.discover().await.map_err(to_core_error)?;
        let lists = tasks::list_task_lists(
            &self.http,
            &discovery.calendar_home_url,
            &self.credentials,
        )
        .await
        .map_err(to_core_error)?;
        for list in lists {
            let url = match Url::parse(&list.id) {
                Ok(u) => u,
                Err(_) => continue,
            };
            if let Ok(()) = tasks::delete_task(
                &self.http,
                &url,
                task_id,
                None,
                &self.credentials,
            )
            .await
            {
                return Ok(());
            }
        }
        Err(CoreError::NotFound(format!(
            "task '{task_id}' not found in any list"
        )))
    }

    async fn rename_task_list(
        &self,
        list_id: &str,
        new_name: &str,
    ) -> CoreResult<()> {
        // Same as calendars — VTODO collections are renamed via the
        // same PROPPATCH on the collection URL.
        let url = Url::parse(list_id).map_err(|e| {
            CoreError::InvalidInput(format!("task list id is not a URL: {e}"))
        })?;
        calendars::proppatch_displayname(&self.http, &url, new_name, &self.credentials)
            .await
            .map_err(to_core_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthKind, CaldavAccountConfig};
    use mockito::Server;

    const PRINCIPAL_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal>
          <d:href>/principals/users/alice/</d:href>
        </d:current-user-principal>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    const HOME_SET_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/users/alice/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-home-set>
          <d:href>/calendars/alice/</d:href>
        </c:calendar-home-set>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    #[tokio::test]
    async fn discover_caches_after_first_call() {
        let mut server = Server::new_async().await;
        let _wk = server
            .mock("GET", "/.well-known/caldav")
            .with_status(404)
            .create_async()
            .await;
        // Expect exactly one PROPFIND per phase — second discover()
        // call must hit the cache, not the wire.
        let _principal = server
            .mock("PROPFIND", "/")
            .with_status(207)
            .with_body(PRINCIPAL_RESPONSE)
            .expect(1)
            .create_async()
            .await;
        let _home = server
            .mock("PROPFIND", "/principals/users/alice/")
            .with_status(207)
            .with_body(HOME_SET_RESPONSE)
            .expect(1)
            .create_async()
            .await;

        let adapter = CaldavAdapter::new(
            Credentials::new(
                CaldavAccountConfig {
                    server_url: server.url(),
                    username: "alice".into(),
                    auth_kind: AuthKind::Basic,
                },
                "hunter2".into(),
            ),
            Some(Duration::from_secs(3)),
        )
        .unwrap();

        let first = adapter.discover().await.unwrap();
        let second = adapter.discover().await.unwrap();
        assert_eq!(first.calendar_home_url, second.calendar_home_url);
        assert!(adapter.cached_calendar_home().is_some());
    }
}
