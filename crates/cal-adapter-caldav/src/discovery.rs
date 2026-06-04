//! CalDAV server discovery (RFC 6764).
//!
//! Given a base URL the user typed, find the URL of the user's
//! calendar-home collection in three hops:
//!
//! 1. **Well-known**. Try `<base>/.well-known/caldav`. Most servers
//!    answer with a redirect (3xx, `Location:`) to the actual DAV
//!    root. Some answer with a 200 directly. If well-known is not
//!    served (404), we keep using the base URL.
//! 2. **Current user principal**. PROPFIND the DAV root with depth 0
//!    + `<current-user-principal>`. The server returns the URL that
//!    identifies the authenticated user.
//! 3. **Calendar home set**. PROPFIND the principal URL with depth 0
//!    + `<calendar-home-set>` from the CalDAV namespace. The
//!    returned URL points at the collection that contains the
//!    user's calendars.
//!
//! The final URL is what the next phase (calendar listing) uses as
//! its PROPFIND target. We keep the absolute URL on this side so
//! relative `<href>` values from the server are joined against the
//! right base.

use reqwest::header::{HeaderName, HeaderValue, CONTENT_TYPE};
use reqwest::{Client, Method, Response, StatusCode};
use tracing::debug;
use url::Url;

use crate::auth::auth_header;
use crate::config::Credentials;
use crate::error::{CaldavError, CaldavResult};
use crate::xml::extract_first_nested_href;

const PROPFIND: &str = "PROPFIND";

const PRINCIPAL_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop>
    <d:current-user-principal/>
  </d:prop>
</d:propfind>"#;

const HOME_SET_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <c:calendar-home-set/>
  </d:prop>
</d:propfind>"#;

/// Same PROPFIND shape as above, but for CardDAV's
/// `addressbook-home-set`. Asked separately rather than packed into
/// the single PROPFIND so servers without CardDAV support (a
/// plain calendar host) don't reject the whole request — most
/// servers respond with a 404 for unknown properties on the same
/// principal, but a separate request lets us read the addressbook
/// status independently and treat it as "no addressbooks
/// available" rather than "discovery failed".
const ADDRESSBOOK_HOME_SET_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:cr="urn:ietf:params:xml:ns:carddav">
  <d:prop>
    <cr:addressbook-home-set/>
  </d:prop>
</d:propfind>"#;

/// RFC 6638 scheduling probe: the principal's `schedule-outbox-URL` (its
/// presence means the server auto-schedules) plus `calendar-user-address-set`
/// (the user's `mailto:` for `ORGANIZER`).
const SCHEDULING_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <c:calendar-user-address-set/>
    <c:schedule-outbox-URL/>
  </d:prop>
</d:propfind>"#;

/// Result of a full discovery pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    /// The base URL we settled on after well-known. All other URLs
    /// are absolute and joined against this.
    pub dav_root: Url,
    /// Absolute URL of the authenticated user's principal resource.
    pub principal_url: Url,
    /// CalDAV `calendar-home-set` — the collection that contains the
    /// user's calendars. Optional: a CardDAV-only server (e.g. Synology
    /// Contacts) advertises no calendar home. Absent ⇒ the adapter
    /// returns empty calendar/task listings rather than failing.
    pub calendar_home_url: Option<Url>,
    /// CardDAV `addressbook-home-set`. Optional: a server may
    /// advertise CalDAV only (Apple Calendar server in
    /// calendar-only mode, some Radicale set-ups). Absent ⇒ the
    /// adapter returns empty contact listings. Discovery fails only
    /// when BOTH homes are absent (see `run`).
    pub addressbook_home_url: Option<Url>,
    /// True when the server advertises RFC 6638 calendar auto-scheduling
    /// (it exposes a `schedule-outbox-URL` on the principal) AND we found a
    /// usable `mailto:` organizer address. When false the adapter never
    /// writes `ORGANIZER`/`ATTENDEE`, so the server never emails attendees.
    pub supports_scheduling: bool,
    /// The user's `mailto:` calendar-user-address (from
    /// `calendar-user-address-set`), used as `ORGANIZER` when writing a
    /// scheduled event. `None` when the server advertised none.
    pub calendar_user_address: Option<String>,
    /// Absolute URL of the principal's `schedule-outbox-URL` (RFC 6638).
    /// We POST an iTIP `VFREEBUSY` request here to query attendees'
    /// availability. `None` when the server doesn't advertise an outbox
    /// (free-busy then degrades to "unknown").
    pub schedule_outbox_url: Option<Url>,
}

pub async fn run(client: &Client, credentials: &Credentials) -> CaldavResult<Discovery> {
    let base = parse_base_url(&credentials.config.server_url)?;
    let dav_root = resolve_well_known(client, &base, credentials).await?;
    let principal_url = find_principal(client, &dav_root, credentials).await?;
    // Both homes are best-effort: a server may surface only CalDAV
    // (calendars/tasks) or only CardDAV (contacts — e.g. Synology
    // Contacts advertises an addressbook-home-set but no
    // calendar-home-set). A missing/404 probe just means "this server
    // doesn't surface that side"; the trait methods then return empty
    // listings for it. We only tear discovery down when NEITHER home is
    // present (below), which is the real "wrong URL / not a DAV server"
    // signal.
    let calendar_home_url = match find_calendar_home(client, &principal_url, credentials).await {
        Ok(url) => Some(url),
        Err(err) => {
            debug!(?err, "calendar-home-set discovery skipped");
            None
        }
    };
    let addressbook_home_url =
        match find_addressbook_home(client, &principal_url, credentials).await {
            Ok(url) => Some(url),
            Err(err) => {
                debug!(?err, "addressbook-home-set discovery skipped");
                None
            }
        };
    if calendar_home_url.is_none() && addressbook_home_url.is_none() {
        return Err(CaldavError::Discovery(
            "server advertised neither a calendar-home-set nor an addressbook-home-set".into(),
        ));
    }
    // RFC 6638 scheduling support + organizer address. Best-effort: a
    // server without the properties (or an odd response) just means "no
    // scheduling", which hides the notify toggle rather than failing.
    let (supports_scheduling, calendar_user_address, schedule_outbox_url) =
        find_scheduling(client, &principal_url, credentials).await;
    Ok(Discovery {
        dav_root,
        principal_url,
        calendar_home_url,
        addressbook_home_url,
        supports_scheduling,
        calendar_user_address,
        schedule_outbox_url,
    })
}

/// Best-effort RFC 6638 probe on the principal. Returns
/// `(supports_scheduling, organizer_mailto, schedule_outbox_url)`. Never
/// errors — any network / parse failure, or a server that doesn't expose
/// the properties, degrades to `(false, None, None)` so the rest of
/// discovery still succeeds and the UI simply hides the "notify attendees"
/// toggle for this account.
async fn find_scheduling(
    client: &Client,
    principal_url: &Url,
    credentials: &Credentials,
) -> (bool, Option<String>, Option<Url>) {
    let body = match propfind(client, principal_url, SCHEDULING_BODY, credentials, 0).await {
        Ok(resp) => match expect_207(resp).await {
            Ok(b) => b,
            Err(_) => return (false, None, None),
        },
        Err(_) => return (false, None, None),
    };
    // Resolve the outbox href against the principal so it's absolute and
    // ready to POST to.
    let outbox_url = extract_first_nested_href(&body, b"schedule-outbox-URL")
        .ok()
        .flatten()
        .and_then(|href| principal_url.join(&href).ok());
    let organizer = first_mailto(&body);
    // Need BOTH server auto-scheduling AND a usable organizer address —
    // without the latter we can't write a valid ORGANIZER.
    (
        outbox_url.is_some() && organizer.is_some(),
        organizer,
        outbox_url,
    )
}

/// Pull the first `mailto:` calendar-user-address out of a PROPFIND body.
/// `calendar-user-address-set` lists several addresses (principal paths and
/// mailtos); `ORGANIZER`/`ATTENDEE` need the `mailto:` form.
fn first_mailto(body: &str) -> Option<String> {
    let start = body.find("mailto:")?;
    let rest = &body[start..];
    let end = rest
        .find(|c: char| c == '<' || c == '"' || c.is_whitespace())
        .unwrap_or(rest.len());
    let addr = rest[..end].trim();
    (addr.len() > "mailto:".len()).then(|| addr.to_string())
}

fn parse_base_url(raw: &str) -> CaldavResult<Url> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CaldavError::Config("server URL is empty".into()));
    }
    // Accept "example.com" without scheme — default to https://, the
    // only scheme any real CalDAV server publishes on. We never
    // silently fall back to plain HTTP.
    if !trimmed.contains("://") {
        return Ok(Url::parse(&format!("https://{trimmed}"))?);
    }
    let url = Url::parse(trimmed)?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(CaldavError::Config(format!(
            "unsupported URL scheme: {}",
            url.scheme()
        )));
    }
    Ok(url)
}

async fn resolve_well_known(
    client: &Client,
    base: &Url,
    credentials: &Credentials,
) -> CaldavResult<Url> {
    // RFC 6764 places the well-known URIs at the HOST ROOT regardless of
    // the path the user pasted. Try CalDAV first, then CardDAV: a
    // contacts-only server (e.g. Synology Contacts) only answers the
    // `carddav` one. This is purely additive — for servers that already
    // resolved via `caldav` (redirect or 200) nothing changes; only the
    // previous "caldav 404 → base URL" path now gets a `carddav` probe
    // before falling back to the pasted URL.
    for path in ["/.well-known/caldav", "/.well-known/carddav"] {
        if let Some(resolved) = probe_well_known(client, base, path, credentials).await? {
            return Ok(resolved);
        }
    }
    Ok(base.clone())
}

/// Probe one well-known path on the host root. Returns:
///   - `Some(url)` on a usable redirect (Location resolved) or a 200,
///   - `None` on 404 / unusable redirect / transient error, so the
///     caller tries the next candidate or falls back to the base URL.
async fn probe_well_known(
    client: &Client,
    base: &Url,
    path: &str,
    credentials: &Credentials,
) -> CaldavResult<Option<Url>> {
    let mut wk = base.clone();
    wk.set_path(path);
    wk.set_query(None);
    wk.set_fragment(None);

    let response = match client
        .request(Method::GET, wk.clone())
        .headers(auth_header(credentials)?)
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => {
            debug!(path, "well-known request errored");
            return Ok(None);
        }
    };

    let status = response.status();
    if status.is_redirection() {
        if let Some(loc) = response.headers().get(reqwest::header::LOCATION) {
            if let Ok(s) = loc.to_str() {
                if let Ok(joined) = wk.join(s) {
                    return Ok(Some(joined));
                }
            }
        }
        debug!(
            path,
            "well-known redirected but Location header was unusable"
        );
        return Ok(None);
    }
    if status.is_success() {
        // 200 on well-known: the server serves DAV from there.
        return Ok(Some(wk));
    }
    if status == StatusCode::NOT_FOUND {
        // Common case — many servers don't bother. Try the next probe.
        return Ok(None);
    }
    // Anything else (5xx, 401) we treat as soft — let the next probe or
    // the base-URL fallback surface a real error downstream.
    debug!(path, ?status, "well-known returned unexpected status");
    Ok(None)
}

async fn find_principal(
    client: &Client,
    dav_root: &Url,
    credentials: &Credentials,
) -> CaldavResult<Url> {
    let response = propfind(client, dav_root, PRINCIPAL_BODY, credentials, 0).await?;
    let body = expect_207(response).await?;
    let href = extract_first_nested_href(&body, b"current-user-principal")?.ok_or_else(|| {
        CaldavError::Discovery("server did not return a current-user-principal".into())
    })?;
    dav_root.join(&href).map_err(Into::into)
}

async fn find_calendar_home(
    client: &Client,
    principal_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Url> {
    let response = propfind(client, principal_url, HOME_SET_BODY, credentials, 0).await?;
    let body = expect_207(response).await?;
    let href = extract_first_nested_href(&body, b"calendar-home-set")?.ok_or_else(|| {
        CaldavError::Discovery("server did not return a calendar-home-set".into())
    })?;
    principal_url.join(&href).map_err(Into::into)
}

async fn find_addressbook_home(
    client: &Client,
    principal_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Url> {
    let response = propfind(
        client,
        principal_url,
        ADDRESSBOOK_HOME_SET_BODY,
        credentials,
        0,
    )
    .await?;
    let body = expect_207(response).await?;
    let href = extract_first_nested_href(&body, b"addressbook-home-set")?.ok_or_else(|| {
        CaldavError::Discovery("server did not return an addressbook-home-set".into())
    })?;
    principal_url.join(&href).map_err(Into::into)
}

async fn propfind(
    client: &Client,
    url: &Url,
    body: &'static str,
    credentials: &Credentials,
    depth: u8,
) -> CaldavResult<Response> {
    let method = Method::from_bytes(PROPFIND.as_bytes()).expect("PROPFIND is valid");
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    headers.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_str(&depth.to_string()).expect("digit"),
    );
    let resp = client
        .request(method, url.clone())
        .headers(headers)
        .body(body)
        .send()
        .await?;
    Ok(resp)
}

async fn expect_207(response: Response) -> CaldavResult<String> {
    let status = response.status();
    if status != StatusCode::from_u16(207).unwrap() && !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(CaldavError::Http {
            status: status.as_u16(),
            message: if body.is_empty() {
                status.canonical_reason().unwrap_or("").to_string()
            } else {
                body.chars().take(200).collect()
            },
        });
    }
    response.text().await.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthKind, CaldavAccountConfig};
    use mockito::Server;

    fn creds(server_url: &str) -> Credentials {
        Credentials::new(
            CaldavAccountConfig {
                server_url: server_url.into(),
                username: "alice".into(),
                auth_kind: AuthKind::Basic,
            },
            "hunter2".into(),
        )
    }

    /// Test client mirrors the production setup in `CaldavAdapter::new`:
    /// reqwest's auto-redirect must be off so the well-known step
    /// can read the Location header itself.
    fn test_client() -> Client {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

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

    /// A CardDAV-only principal: advertises an `addressbook-home-set`
    /// but NO `calendar-home-set` (e.g. Synology Contacts). The same
    /// body answers the calendar-home, addressbook-home and scheduling
    /// PROPFINDs — only the addressbook probe finds its element.
    const ADDRESSBOOK_ONLY_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <d:response>
    <d:href>/principals/users/alice/</d:href>
    <d:propstat>
      <d:prop>
        <card:addressbook-home-set>
          <d:href>/addressbooks/alice/</d:href>
        </card:addressbook-home-set>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    /// A principal that advertises neither home — a wrong URL or a
    /// non-DAV endpoint. Discovery must reject this.
    const NO_HOME_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/principals/users/alice/</d:href>
    <d:propstat>
      <d:prop/>
      <d:status>HTTP/1.1 404 Not Found</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    #[tokio::test]
    async fn full_discovery_via_well_known_404_fallback() {
        let mut server = Server::new_async().await;
        let _wk = server
            .mock("GET", "/.well-known/caldav")
            .with_status(404)
            .create_async()
            .await;
        let _principal = server
            .mock("PROPFIND", "/")
            .match_header("depth", "0")
            .with_status(207)
            .with_body(PRINCIPAL_RESPONSE)
            .create_async()
            .await;
        let _home = server
            .mock("PROPFIND", "/principals/users/alice/")
            .match_header("depth", "0")
            .with_status(207)
            .with_body(HOME_SET_RESPONSE)
            .create_async()
            .await;

        let client = test_client();
        let discovery = run(&client, &creds(&server.url())).await.unwrap();
        assert_eq!(discovery.principal_url.path(), "/principals/users/alice/");
        assert_eq!(
            discovery.calendar_home_url.as_ref().unwrap().path(),
            "/calendars/alice/",
        );
    }

    #[tokio::test]
    async fn contacts_only_server_discovers_without_calendar_home() {
        // The Synology Contacts case: the principal advertises an
        // addressbook-home-set but no calendar-home-set. Discovery must
        // succeed with calendar_home_url=None (so calendar/task listings
        // come back empty) rather than erroring "not found".
        let mut server = Server::new_async().await;
        let _wk_caldav = server
            .mock("GET", "/.well-known/caldav")
            .with_status(404)
            .create_async()
            .await;
        let _wk_carddav = server
            .mock("GET", "/.well-known/carddav")
            .with_status(404)
            .create_async()
            .await;
        let _principal = server
            .mock("PROPFIND", "/")
            .match_header("depth", "0")
            .with_status(207)
            .with_body(PRINCIPAL_RESPONSE)
            .create_async()
            .await;
        let _home = server
            .mock("PROPFIND", "/principals/users/alice/")
            .match_header("depth", "0")
            .with_status(207)
            .with_body(ADDRESSBOOK_ONLY_RESPONSE)
            .create_async()
            .await;

        let client = test_client();
        let discovery = run(&client, &creds(&server.url())).await.unwrap();
        assert!(
            discovery.calendar_home_url.is_none(),
            "contacts-only server should have no calendar home",
        );
        assert_eq!(
            discovery.addressbook_home_url.as_ref().unwrap().path(),
            "/addressbooks/alice/",
        );
    }

    #[tokio::test]
    async fn neither_home_returns_discovery_error() {
        // A principal that advertises neither home — wrong URL / not a
        // DAV server. Discovery rejects it (the real "not found" case).
        let mut server = Server::new_async().await;
        let _wk_caldav = server
            .mock("GET", "/.well-known/caldav")
            .with_status(404)
            .create_async()
            .await;
        let _wk_carddav = server
            .mock("GET", "/.well-known/carddav")
            .with_status(404)
            .create_async()
            .await;
        let _principal = server
            .mock("PROPFIND", "/")
            .match_header("depth", "0")
            .with_status(207)
            .with_body(PRINCIPAL_RESPONSE)
            .create_async()
            .await;
        let _home = server
            .mock("PROPFIND", "/principals/users/alice/")
            .match_header("depth", "0")
            .with_status(207)
            .with_body(NO_HOME_RESPONSE)
            .create_async()
            .await;

        let client = test_client();
        let result = run(&client, &creds(&server.url())).await;
        assert!(
            matches!(result, Err(CaldavError::Discovery(_))),
            "expected a Discovery error, got {result:?}",
        );
    }

    #[tokio::test]
    async fn well_known_redirect_is_followed() {
        let mut server = Server::new_async().await;
        let new_root = format!("{}/dav/", server.url());
        let _wk = server
            .mock("GET", "/.well-known/caldav")
            .with_status(301)
            .with_header("location", &new_root)
            .create_async()
            .await;
        let _principal = server
            .mock("PROPFIND", "/dav/")
            .with_status(207)
            .with_body(PRINCIPAL_RESPONSE)
            .create_async()
            .await;
        let _home = server
            .mock("PROPFIND", "/principals/users/alice/")
            .with_status(207)
            .with_body(HOME_SET_RESPONSE)
            .create_async()
            .await;

        let client = test_client();
        let discovery = run(&client, &creds(&server.url())).await.unwrap();
        assert!(discovery.dav_root.path().ends_with("/dav/"));
    }

    #[tokio::test]
    async fn missing_principal_returns_discovery_error() {
        let mut server = Server::new_async().await;
        let _wk = server
            .mock("GET", "/.well-known/caldav")
            .with_status(404)
            .create_async()
            .await;
        let empty = r#"<d:multistatus xmlns:d="DAV:"></d:multistatus>"#;
        let _principal = server
            .mock("PROPFIND", "/")
            .with_status(207)
            .with_body(empty)
            .create_async()
            .await;

        let client = test_client();
        let err = run(&client, &creds(&server.url())).await.unwrap_err();
        assert!(matches!(err, CaldavError::Discovery(_)));
    }

    #[tokio::test]
    async fn unauthorized_surfaces_http_401() {
        let mut server = Server::new_async().await;
        let _wk = server
            .mock("GET", "/.well-known/caldav")
            .with_status(404)
            .create_async()
            .await;
        let _principal = server
            .mock("PROPFIND", "/")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;

        let client = test_client();
        let err = run(&client, &creds(&server.url())).await.unwrap_err();
        match err {
            CaldavError::Http { status, .. } => assert_eq!(status, 401),
            other => panic!("expected HTTP 401, got {other:?}"),
        }
    }

    #[test]
    fn parse_base_url_defaults_to_https() {
        let url = parse_base_url("example.com").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("example.com"));
    }

    #[test]
    fn parse_base_url_rejects_unsupported_scheme() {
        let err = parse_base_url("ftp://example.com").unwrap_err();
        assert!(matches!(err, CaldavError::Config(_)));
    }

    #[test]
    fn first_mailto_picks_the_mailto_address() {
        let body = r#"<c:calendar-user-address-set>
            <d:href>/principals/users/alice/</d:href>
            <d:href>mailto:alice@example.com</d:href>
          </c:calendar-user-address-set>"#;
        assert_eq!(
            first_mailto(body).as_deref(),
            Some("mailto:alice@example.com")
        );
        assert_eq!(first_mailto("<d:href>/no/mailto/here/</d:href>"), None);
    }

    const SCHEDULING_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/users/alice/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-user-address-set>
          <d:href>mailto:alice@example.com</d:href>
          <d:href>/principals/users/alice/</d:href>
        </c:calendar-user-address-set>
        <c:schedule-outbox-URL>
          <d:href>/calendars/alice/outbox/</d:href>
        </c:schedule-outbox-URL>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    #[tokio::test]
    async fn discovery_detects_rfc6638_scheduling() {
        let mut server = Server::new_async().await;
        let _wk = server
            .mock("GET", "/.well-known/caldav")
            .with_status(404)
            .create_async()
            .await;
        let _principal = server
            .mock("PROPFIND", "/")
            .with_status(207)
            .with_body(PRINCIPAL_RESPONSE)
            .create_async()
            .await;
        // Two PROPFINDs hit the principal — route them by requested property.
        let _home = server
            .mock("PROPFIND", "/principals/users/alice/")
            .match_body(mockito::Matcher::Regex("calendar-home-set".into()))
            .with_status(207)
            .with_body(HOME_SET_RESPONSE)
            .create_async()
            .await;
        let _sched = server
            .mock("PROPFIND", "/principals/users/alice/")
            .match_body(mockito::Matcher::Regex("schedule-outbox-URL".into()))
            .with_status(207)
            .with_body(SCHEDULING_RESPONSE)
            .create_async()
            .await;

        let client = test_client();
        let d = run(&client, &creds(&server.url())).await.unwrap();
        assert!(d.supports_scheduling);
        assert_eq!(
            d.calendar_user_address.as_deref(),
            Some("mailto:alice@example.com")
        );
        // The outbox href is resolved to an absolute URL ready to POST to.
        let expected_outbox = format!("{}/calendars/alice/outbox/", server.url());
        assert_eq!(
            d.schedule_outbox_url.as_ref().map(Url::as_str),
            Some(expected_outbox.as_str())
        );
    }
}
