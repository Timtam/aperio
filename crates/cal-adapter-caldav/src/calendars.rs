//! Calendar listing — PROPFIND on the discovered calendar-home-set
//! collection with depth 1 to enumerate every individual calendar
//! the user owns.
//!
//! What we ask for:
//!   - `displayname` → the user-visible name shown in the sidebar
//!   - `resourcetype` → distinguishes plain WebDAV collections from
//!     real calendars (`<calendar/>` inside)
//!   - `supported-calendar-component-set` → tells us whether the
//!     calendar accepts VEVENT (calendar features), VTODO (task
//!     features), or both
//!   - `calendar-color` → optional Apple/Nextcloud extension; when
//!     present we pre-fill the container colour so Aperio shows the
//!     same shade as the source app
//!
//! Tasks are deliberately filtered to "is a calendar AND supports
//! VEVENT": Aperio's calendar feature in this iteration only knows
//! how to render events. The VTODO half lives behind the
//! `TasksFeature` trait and arrives in 6b.3.

use cal_core::{Calendar, ColorSource, ContainerColor};
use reqwest::{
    header::{HeaderName, HeaderValue, CONTENT_TYPE},
    Client, Method, StatusCode,
};
use url::Url;

use crate::config::Credentials;
use crate::error::{CaldavError, CaldavResult};
use crate::xml::{parse_multistatus, ResponseEntry};

const CALENDAR_PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"
           xmlns:ical="http://apple.com/ns/ical/">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
    <c:supported-calendar-component-set/>
    <ical:calendar-color/>
  </d:prop>
</d:propfind>"#;

/// Walk the calendar-home-set with PROPFIND depth 1 and return one
/// entry per child collection that the server flags as a calendar.
/// The home-set itself shows up in the response too and is filtered
/// out by the `is_calendar` check.
/// Return one [`Calendar`] per calendar collection under
/// `home_url`. The persistence layer is responsible for stamping
/// `account_id` onto the corresponding SQLite row — the adapter
/// itself stays oblivious to which account it is serving.
pub async fn list_calendars(
    client: &Client,
    home_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Vec<Calendar>> {
    let entries = propfind_calendars(client, home_url, credentials).await?;
    Ok(entries
        .into_iter()
        .filter(|e| e.is_calendar)
        .filter(|e| supports_vevent(e))
        .map(|e| to_calendar(home_url, e))
        .collect())
}

async fn propfind_calendars(
    client: &Client,
    url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Vec<ResponseEntry>> {
    let method = Method::from_bytes(b"PROPFIND").expect("PROPFIND");
    let mut headers = crate::auth::auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    headers.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_static("1"),
    );
    let response = client
        .request(method, url.clone())
        .headers(headers)
        .body(CALENDAR_PROPFIND_BODY)
        .send()
        .await?;
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
    let body = response.text().await?;
    parse_multistatus(&body)
}

fn supports_vevent(entry: &ResponseEntry) -> bool {
    // Servers vary on whether they include the property when the
    // calendar accepts everything; an empty list is treated as
    // "VEVENT works". Apple iCloud for example skips the property
    // for default calendars.
    entry.supported_components.is_empty()
        || entry
            .supported_components
            .iter()
            .any(|c| c.eq_ignore_ascii_case("VEVENT"))
}

fn to_calendar(home_url: &Url, entry: ResponseEntry) -> Calendar {
    // Joined absolute URL for the calendar collection — used as the
    // calendar id so subsequent operations can address it directly
    // without re-walking the listing.
    let id = home_url
        .join(&entry.href)
        .map(|u| u.to_string())
        .unwrap_or(entry.href.clone());

    let color = entry.calendar_color.and_then(|raw| {
        // Some servers (Apple) emit "#RRGGBBAA". Strip the alpha so
        // it fits the Calendar.color hex format which is 6-digit.
        let trimmed = raw.trim();
        if trimmed.starts_with('#') && (trimmed.len() == 7 || trimmed.len() == 9) {
            let hex6 = &trimmed[..7]; // "#" + 6 hex digits
            Some(ContainerColor {
                hex: hex6.to_string(),
                source: ColorSource::Native,
            })
        } else {
            None
        }
    });

    Calendar {
        id,
        name: entry
            .displayname
            .unwrap_or_else(|| "Unnamed calendar".into()),
        color,
        read_only: false,
        default_sound: None,
    }
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

    fn client() -> Client {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    const LISTING_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"
              xmlns:ical="http://apple.com/ns/ical/">
  <d:response>
    <d:href>/calendars/alice/</d:href>
    <d:propstat><d:prop>
      <d:displayname>alice</d:displayname>
      <d:resourcetype><d:collection/></d:resourcetype>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Work</d:displayname>
      <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      <ical:calendar-color>#1e88e5ff</ical:calendar-color>
      <c:supported-calendar-component-set>
        <c:comp name="VEVENT"/>
      </c:supported-calendar-component-set>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/alice/tasks/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Tasks</d:displayname>
      <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      <c:supported-calendar-component-set>
        <c:comp name="VTODO"/>
      </c:supported-calendar-component-set>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;

    #[tokio::test]
    async fn lists_calendars_filtering_out_home_set_and_task_collections() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("PROPFIND", "/calendars/alice/")
            .match_header("depth", "1")
            .with_status(207)
            .with_body(LISTING_RESPONSE)
            .create_async()
            .await;

        let home = Url::parse(&format!("{}/calendars/alice/", server.url())).unwrap();
        let cals = list_calendars(&client(), &home, &creds(&server.url()))
            .await
            .unwrap();
        // Only the VEVENT calendar survives — the home-set itself is
        // a plain collection, the tasks calendar only declares VTODO.
        assert_eq!(cals.len(), 1);
        assert_eq!(cals[0].name, "Work");
        assert!(cals[0].id.ends_with("/calendars/alice/work/"));
        let color = cals[0].color.as_ref().unwrap();
        assert_eq!(color.hex, "#1e88e5");
        assert_eq!(color.source, ColorSource::Native);
    }

    #[tokio::test]
    async fn empty_supported_components_treated_as_vevent_capable() {
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/personal/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Personal</d:displayname>
      <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;
        let mut server = Server::new_async().await;
        let _m = server
            .mock("PROPFIND", "/calendars/alice/")
            .with_status(207)
            .with_body(body)
            .create_async()
            .await;
        let home = Url::parse(&format!("{}/calendars/alice/", server.url())).unwrap();
        let cals = list_calendars(&client(), &home, &creds(&server.url()))
            .await
            .unwrap();
        assert_eq!(cals.len(), 1);
        assert_eq!(cals[0].name, "Personal");
    }
}
