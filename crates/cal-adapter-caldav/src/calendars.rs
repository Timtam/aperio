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
        .filter(supports_vevent)
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

/// PROPPATCH `DAV:displayname` on a collection (RFC 4918 §15.2 +
/// §9.2). Used by both calendar rename and task-list rename — both
/// are calendar collections under the same root, only their
/// `supported-calendar-component-set` differs, and PROPPATCH on the
/// collection accepts the new name regardless.
///
/// Server response handling:
///   - 207 Multi-Status with a successful propstat ⇒ Ok
///   - 207 Multi-Status with a non-2xx propstat ⇒ map the inner
///     code to a CaldavError; the body is included for debugging
///   - Any other status ⇒ `CaldavError::Http`
pub async fn proppatch_displayname(
    client: &Client,
    collection_url: &Url,
    new_name: &str,
    credentials: &Credentials,
) -> CaldavResult<()> {
    let method = Method::from_bytes(b"PROPPATCH").expect("PROPPATCH");
    let mut headers = crate::auth::auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );

    // RFC 4918 §15.2 mandates `set` with the new property value.
    // Embedded special characters need XML escaping so a name like
    // `Stuff & things` doesn't break the request body.
    let escaped = escape_xml(new_name);
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:propertyupdate xmlns:d="DAV:">
  <d:set>
    <d:prop>
      <d:displayname>{escaped}</d:displayname>
    </d:prop>
  </d:set>
</d:propertyupdate>"#
    );

    let response = client
        .request(method, collection_url.clone())
        .headers(headers)
        .body(body)
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
    // 207 Multi-Status may still encode a per-property failure
    // inside `<propstat>`: e.g. iCloud accepts the request envelope
    // but rejects the displayname change with 403 because the
    // calendar is shared read-only. Parse the body and surface that.
    if status == StatusCode::from_u16(207).unwrap() {
        let body = response.text().await.unwrap_or_default();
        if let Some(inner) = crate::xml::first_failed_status_code(&body) {
            return Err(CaldavError::Http {
                status: inner,
                message: body.chars().take(200).collect(),
            });
        }
    }
    Ok(())
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
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

    #[tokio::test]
    async fn proppatch_displayname_sends_set_with_new_value() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("PROPPATCH", "/calendars/alice/work/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("<d:set>".into()),
                mockito::Matcher::Regex("<d:displayname>Arbeit</d:displayname>".into()),
            ]))
            .with_status(207)
            .with_body(
                r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:propstat>
      <d:prop><d:displayname/></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
            )
            .expect(1)
            .create_async()
            .await;

        let url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        proppatch_displayname(&client(), &url, "Arbeit", &creds(&server.url()))
            .await
            .unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn proppatch_displayname_escapes_xml_special_chars() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("PROPPATCH", "/calendars/alice/work/")
            .match_body(mockito::Matcher::Regex(
                "<d:displayname>Stuff &amp; things</d:displayname>".into(),
            ))
            .with_status(207)
            .with_body("")
            .expect(1)
            .create_async()
            .await;

        let url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        proppatch_displayname(&client(), &url, "Stuff & things", &creds(&server.url()))
            .await
            .unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn proppatch_displayname_surfaces_http_failure() {
        let mut server = Server::new_async().await;
        server
            .mock("PROPPATCH", "/calendars/alice/work/")
            .with_status(403)
            .with_body("Forbidden")
            .create_async()
            .await;

        let url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let err = proppatch_displayname(&client(), &url, "Whatever", &creds(&server.url()))
            .await
            .unwrap_err();
        assert!(matches!(err, CaldavError::Http { status: 403, .. }));
    }

    #[tokio::test]
    async fn proppatch_displayname_catches_inner_propstat_failure() {
        // 207 envelope says "request received", but the displayname
        // propstat inside reports 403 — exactly the shape some
        // CalDAV servers use for read-only or restricted properties.
        let multistatus_with_failure = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:propstat>
      <d:prop><d:displayname/></d:prop>
      <d:status>HTTP/1.1 403 Forbidden</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let mut server = Server::new_async().await;
        server
            .mock("PROPPATCH", "/calendars/alice/work/")
            .with_status(207)
            .with_body(multistatus_with_failure)
            .create_async()
            .await;

        let url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let err = proppatch_displayname(&client(), &url, "Whatever", &creds(&server.url()))
            .await
            .unwrap_err();
        assert!(matches!(err, CaldavError::Http { status: 403, .. }));
    }
}
