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
    /// Absolute URL of the collection that contains the user's
    /// calendars — the PROPFIND target for the next layer (calendar
    /// listing in 6b.2).
    pub calendar_home_url: Url,
    /// CardDAV `addressbook-home-set`. Optional: a server may
    /// advertise CalDAV only (Apple Calendar server in
    /// calendar-only mode, some Radicale set-ups). Absent ⇒ the
    /// adapter declines the `Contacts` capability at the trait
    /// boundary.
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
}

pub async fn run(client: &Client, credentials: &Credentials) -> CaldavResult<Discovery> {
    let base = parse_base_url(&credentials.config.server_url)?;
    let dav_root = resolve_well_known(client, &base, credentials).await?;
    let principal_url = find_principal(client, &dav_root, credentials).await?;
    let calendar_home_url = find_calendar_home(client, &principal_url, credentials).await?;
    // Addressbook home is best-effort: a missing or 404 response
    // means "this server doesn't surface CardDAV", which is fine —
    // we just don't expose the Contacts capability. Anything else
    // (auth failure, 5xx) we swallow and log; the calendar side is
    // already authenticated by the time we get here, so a single
    // odd response on this probe shouldn't tear down the whole
    // discovery result.
    let addressbook_home_url =
        match find_addressbook_home(client, &principal_url, credentials).await {
            Ok(url) => Some(url),
            Err(err) => {
                debug!(?err, "addressbook-home-set discovery skipped");
                None
            }
        };
    // RFC 6638 scheduling support + organizer address. Best-effort: a
    // server without the properties (or an odd response) just means "no
    // scheduling", which hides the notify toggle rather than failing.
    let (supports_scheduling, calendar_user_address) =
        find_scheduling(client, &principal_url, credentials).await;
    Ok(Discovery {
        dav_root,
        principal_url,
        calendar_home_url,
        addressbook_home_url,
        supports_scheduling,
        calendar_user_address,
    })
}

/// Best-effort RFC 6638 probe on the principal. Returns
/// `(supports_scheduling, organizer_mailto)`. Never errors — any network /
/// parse failure, or a server that doesn't expose the properties, degrades
/// to `(false, None)` so the rest of discovery still succeeds and the UI
/// simply hides the "notify attendees" toggle for this account.
async fn find_scheduling(
    client: &Client,
    principal_url: &Url,
    credentials: &Credentials,
) -> (bool, Option<String>) {
    let body = match propfind(client, principal_url, SCHEDULING_BODY, credentials, 0).await {
        Ok(resp) => match expect_207(resp).await {
            Ok(b) => b,
            Err(_) => return (false, None),
        },
        Err(_) => return (false, None),
    };
    let has_outbox = extract_first_nested_href(&body, b"schedule-outbox-URL")
        .ok()
        .flatten()
        .is_some();
    let organizer = first_mailto(&body);
    // Need BOTH server auto-scheduling AND a usable organizer address —
    // without the latter we can't write a valid ORGANIZER.
    (has_outbox && organizer.is_some(), organizer)
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
    // Construct the well-known path on the host root, not at the
    // user-supplied path: RFC 6764 places the well-known URI at the
    // host root regardless of the path the user pasted.
    let mut wk = base.clone();
    wk.set_path("/.well-known/caldav");
    wk.set_query(None);
    wk.set_fragment(None);

    let response = client
        .request(Method::GET, wk.clone())
        .headers(auth_header(credentials)?)
        .send()
        .await;
    let response = match response {
        Ok(r) => r,
        Err(_) => {
            // Server doesn't answer the well-known endpoint at all —
            // fall back to whatever the user pasted.
            debug!("well-known request errored; falling back to base URL");
            return Ok(base.clone());
        }
    };

    let status = response.status();
    if status.is_redirection() {
        if let Some(loc) = response.headers().get(reqwest::header::LOCATION) {
            if let Ok(s) = loc.to_str() {
                if let Ok(joined) = wk.join(s) {
                    return Ok(joined);
                }
            }
        }
        debug!("well-known redirected but Location header was unusable");
        return Ok(base.clone());
    }
    if status.is_success() {
        // 200 on well-known: the server happily serves CalDAV from
        // there. Use the well-known URL as the new base.
        return Ok(wk);
    }
    if status == StatusCode::NOT_FOUND {
        // Common case — many servers don't bother. Caller URL is fine.
        return Ok(base.clone());
    }
    // Anything else (5xx, 401) we treat as soft — let the next step
    // surface a real error against the base URL.
    debug!(
        ?status,
        "well-known returned unexpected status; using base URL"
    );
    Ok(base.clone())
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
        assert_eq!(discovery.calendar_home_url.path(), "/calendars/alice/");
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
    }
}
