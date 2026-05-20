//! Range-bounded event read via CalDAV `REPORT calendar-query`
//! (RFC 4791 §7.8.1).
//!
//! Given an absolute calendar URL + a UTC date range, send a
//! `calendar-query` REPORT that asks the server to return every
//! VEVENT inside the window plus its ETag. The server may include
//! recurring masters whose RRULE has occurrences in the range — we
//! pass those through as-is so the rrule.js expansion on the
//! frontend can do its job.
//!
//! Tasks (VTODO) go through a separate path in 6b.3 since they have
//! a different component name on the filter side and a different
//! ID/etag tracking concern (completed_at vs start_utc).

use cal_core::{DateRange, Event};
use chrono::{DateTime, Utc};
use reqwest::{
    header::{HeaderName, HeaderValue, CONTENT_TYPE},
    Client, Method, StatusCode,
};
use url::Url;

use crate::auth::auth_header;
use crate::config::Credentials;
use crate::error::{CaldavError, CaldavResult};
use crate::mapping::parse_calendar_data;
use crate::xml::parse_multistatus;

/// Read every event in `range` from the calendar collection at
/// `calendar_url`. Returns one [`Event`] per VEVENT the server sent
/// back, with the `calendar_id` field stamped to `calendar_url` so
/// downstream code can address the source.
pub async fn get_events(
    client: &Client,
    calendar_url: &Url,
    range: DateRange,
    credentials: &Credentials,
) -> CaldavResult<Vec<Event>> {
    let body = build_calendar_query(range.start, range.end);
    let method = Method::from_bytes(b"REPORT").expect("REPORT");
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    // Depth 1: scan the immediate children of the calendar collection.
    headers.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_static("1"),
    );
    let response = client
        .request(method, calendar_url.clone())
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
    let text = response.text().await?;
    let entries = parse_multistatus(&text)?;

    let calendar_id = calendar_url.as_str();
    let mut out = Vec::new();
    for entry in entries {
        let Some(ical) = entry.calendar_data else {
            continue;
        };
        let mut events = parse_calendar_data(&ical, calendar_id)?;
        // Stamp the ETag the server gave us so the write layer (6b.3)
        // can use If-Match for safe updates.
        if let Some(etag) = entry.etag {
            for ev in &mut events {
                ev.etag = Some(etag.clone());
            }
        }
        out.extend(events);
    }
    Ok(out)
}

fn build_calendar_query(start: DateTime<Utc>, end: DateTime<Utc>) -> String {
    // RFC 4791 §9.9 formats time-range bounds as UTC compact
    // YYYYMMDDTHHMMSSZ. icalendar's own formatter uses the same
    // pattern; we hand-format here to keep events.rs free of the
    // icalendar dependency.
    let fmt = |dt: DateTime<Utc>| dt.format("%Y%m%dT%H%M%SZ").to_string();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{}" end="{}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#,
        fmt(start),
        fmt(end),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthKind, CaldavAccountConfig};
    use chrono::TimeZone;
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

    const REPORT_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/work/event-1.ics</d:href>
    <d:propstat><d:prop>
      <d:getetag>"abc-123"</d:getetag>
      <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//test//EN
BEGIN:VEVENT
UID:event-1@aperio
SUMMARY:Standup
DTSTART:20260520T080000Z
DTEND:20260520T083000Z
END:VEVENT
END:VCALENDAR</c:calendar-data>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/alice/work/event-2.ics</d:href>
    <d:propstat><d:prop>
      <d:getetag>"def-456"</d:getetag>
      <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//test//EN
BEGIN:VEVENT
UID:event-2@aperio
SUMMARY:Lunch
DTSTART:20260520T120000Z
DTEND:20260520T130000Z
END:VEVENT
END:VCALENDAR</c:calendar-data>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;

    #[tokio::test]
    async fn get_events_returns_mapped_events_with_etags() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("REPORT", "/calendars/alice/work/")
            .match_header("depth", "1")
            .with_status(207)
            .with_body(REPORT_RESPONSE)
            .create_async()
            .await;

        let cal_url =
            Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let range = DateRange::new(
            Utc.with_ymd_and_hms(2026, 5, 20, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 5, 21, 0, 0, 0).unwrap(),
        );
        let events = get_events(&client(), &cal_url, range, &creds(&server.url()))
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].title, "Standup");
        assert_eq!(events[0].etag.as_deref(), Some("\"abc-123\""));
        assert_eq!(events[1].title, "Lunch");
        assert_eq!(events[1].etag.as_deref(), Some("\"def-456\""));
        // Each event's calendar_id is stamped to the collection URL.
        assert!(events[0].calendar_id.ends_with("/calendars/alice/work/"));
    }

    #[tokio::test]
    async fn get_events_surfaces_http_errors() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("REPORT", "/calendars/alice/work/")
            .with_status(403)
            .with_body("Forbidden")
            .create_async()
            .await;
        let cal_url =
            Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let range = DateRange::new(
            Utc.with_ymd_and_hms(2026, 5, 20, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 5, 21, 0, 0, 0).unwrap(),
        );
        let err = get_events(&client(), &cal_url, range, &creds(&server.url()))
            .await
            .unwrap_err();
        match err {
            CaldavError::Http { status, .. } => assert_eq!(status, 403),
            other => panic!("expected 403, got {other:?}"),
        }
    }
}
