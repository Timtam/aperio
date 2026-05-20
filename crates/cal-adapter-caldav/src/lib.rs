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

pub mod config;
pub mod discovery;
pub mod error;
mod xml;

use std::sync::Mutex;
use std::time::Duration;

use reqwest::Client;

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
