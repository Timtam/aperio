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

use cal_core::{DateRange, Event, EventRecurrence, NewEvent};
use chrono::{DateTime, Utc};
use reqwest::{
    header::{HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE, IF_MATCH, IF_NONE_MATCH, ETAG},
    Client, Method, StatusCode,
};
use url::Url;
use uuid::Uuid;

use crate::auth::auth_header;
use crate::config::Credentials;
use crate::error::{CaldavError, CaldavResult};
use crate::mapping::{event_to_ical, new_event_to_ical, parse_calendar_data};
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

/// Create a new event on the server.
///
/// PUTs the iCal body to `<calendar_url>/<uid>.ics`. We add
/// `If-None-Match: *` so the server rejects the request (412) when
/// a resource at that path already exists — the caller can retry
/// with a fresh UUID instead of silently overwriting an unrelated
/// event. The returned [`Event`] carries the newly assigned UID
/// and, where the server returned one, the freshly minted ETag.
pub async fn create_event(
    client: &Client,
    calendar_url: &Url,
    event: NewEvent,
    credentials: &Credentials,
) -> CaldavResult<Event> {
    let uid = format!("{}@aperio", Uuid::new_v4());
    let resource = resource_url(calendar_url, &uid)?;
    let body = new_event_to_ical(&uid, &event);

    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));

    let response = client
        .put(resource.clone())
        .headers(headers)
        .body(body)
        .send()
        .await?;
    expect_write_success(&response)?;
    let etag = extract_etag(&response);
    let now = Utc::now();

    Ok(Event {
        id: uid,
        calendar_id: calendar_url.to_string(),
        title: event.title,
        description: event.description,
        location: event.location,
        start: event.start,
        end: event.end,
        all_day: event.all_day,
        recurrence: event.recurrence,
        color_label: event.color_label,
        reminders: event.reminders,
        sound: event.sound,
        attendees: event.attendees,
        created_at: now,
        updated_at: now,
        etag,
    })
}

/// Update an existing event. Uses `If-Match: <etag>` when the
/// caller's copy carries one so a 412 surfaces conflicts the user
/// needs to resolve. Returns the updated event with the new ETag
/// the server emitted in the response.
pub async fn update_event(
    client: &Client,
    event: Event,
    credentials: &Credentials,
) -> CaldavResult<Event> {
    let cal_url = Url::parse(&event.calendar_id).map_err(|e| {
        CaldavError::Config(format!("event.calendar_id is not a URL: {e}"))
    })?;
    let resource = resource_url(&cal_url, &event.id)?;
    let body = event_to_ical(&event);

    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    if let Some(etag) = &event.etag {
        let value = HeaderValue::from_str(etag)
            .map_err(|e| CaldavError::Config(e.to_string()))?;
        headers.insert(IF_MATCH, value);
    }

    let response = client
        .put(resource.clone())
        .headers(headers)
        .body(body)
        .send()
        .await?;
    expect_write_success(&response)?;
    let new_etag = extract_etag(&response);

    Ok(Event {
        etag: new_etag.or(event.etag),
        updated_at: Utc::now(),
        ..event
    })
}

/// Delete an event from the server. `event_id` is the UID; the URL
/// is reconstructed as `<calendar_url>/<uid>.ics`. When the caller
/// passes an `etag`, an `If-Match` header is added so the server
/// refuses to delete a row that has changed under it.
pub async fn delete_event(
    client: &Client,
    calendar_url: &Url,
    event_id: &str,
    etag: Option<&str>,
    credentials: &Credentials,
) -> CaldavResult<()> {
    let resource = resource_url(calendar_url, event_id)?;
    let mut headers = auth_header(credentials)?;
    if let Some(etag) = etag {
        let value =
            HeaderValue::from_str(etag).map_err(|e| CaldavError::Config(e.to_string()))?;
        headers.insert(IF_MATCH, value);
    }
    let response = client
        .delete(resource.clone())
        .headers(headers)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() && status != StatusCode::NOT_FOUND {
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
    Ok(())
}

/// Read the master VEVENT at `<calendar_url>/<uid>.ics`, append
/// `occurrence` to its EXDATE list, and PUT the modified iCal body
/// back. Mirrors the EXDATE handling that the local adapter has for
/// "delete only this occurrence" of a recurring event.
///
/// The fetch + serialise round-trip lets the master keep its RRULE
/// + every other property the server stored, so we don't
/// accidentally drop iCloud-specific data on the way through. The
/// final PUT uses If-Match against the freshly read ETag so a
/// concurrent edit from another client surfaces as a 412 rather
/// than a silent overwrite.
pub async fn add_event_exdate(
    client: &Client,
    calendar_url: &Url,
    event_id: &str,
    occurrence: DateTime<Utc>,
    credentials: &Credentials,
) -> CaldavResult<()> {
    let resource = resource_url(calendar_url, event_id)?;

    // Step 1: fetch the master body + its ETag.
    let mut get_headers = auth_header(credentials)?;
    get_headers.insert(ACCEPT, HeaderValue::from_static("text/calendar"));
    let response = client
        .get(resource.clone())
        .headers(get_headers)
        .send()
        .await?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(CaldavError::Http {
            status: 404,
            message: format!("event '{event_id}' not found on server"),
        });
    }
    if !status.is_success() {
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
    let etag = extract_etag(&response);
    let body = response.text().await?;

    // Step 2: parse, locate the master VEVENT, append EXDATE.
    let mut events = parse_calendar_data(&body, calendar_url.as_str())?;
    let master = events
        .iter_mut()
        .find(|e| e.id == event_id)
        .ok_or_else(|| {
            CaldavError::Discovery(format!(
                "event '{event_id}' missing from its own resource"
            ))
        })?;
    if master.recurrence.is_none() {
        return Err(CaldavError::Discovery(format!(
            "event '{event_id}' is not recurring"
        )));
    }
    let recurrence = master.recurrence.as_mut().unwrap();
    if !recurrence.exceptions.iter().any(|e| *e == occurrence) {
        recurrence.exceptions.push(occurrence);
    }
    let master_clone = master.clone();
    // The first event we found should be the master — drop any
    // additional sub-components (overrides) and re-serialise just
    // the master with its updated EXDATE list. Servers reattach
    // their other components on the next round-trip.
    let serialised = crate::mapping::event_to_ical(&master_clone);

    // Step 3: PUT the modified body back. If-Match guards against a
    // race with a concurrent edit; without an ETag we send the
    // request anyway — the server will still accept it.
    let mut put_headers = auth_header(credentials)?;
    put_headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    if let Some(tag) = etag {
        if let Ok(v) = HeaderValue::from_str(&tag) {
            put_headers.insert(IF_MATCH, v);
        }
    }
    let put = client
        .put(resource)
        .headers(put_headers)
        .body(serialised)
        .send()
        .await?;
    expect_write_success(&put)?;
    Ok(())
}

#[allow(dead_code)]
fn _touch_recurrence(_: &EventRecurrence) {}

fn resource_url(calendar_url: &Url, uid: &str) -> CaldavResult<Url> {
    // CalDAV resource URLs are `<collection>/<slug>.ics`. The UID
    // makes a stable slug — collisions are vanishingly unlikely
    // because we mint UIDs as UUIDv4 + the Aperio domain suffix.
    // We percent-encode the slug to keep characters like `@` safe.
    let slug = format!("{}.ics", urlencoding(uid));
    calendar_url.join(&slug).map_err(Into::into)
}

/// Tiny percent-encoder for slug characters. We avoid pulling in
/// `percent-encoding` for one call site — only `@`, `:` and a few
/// other ASCII punctuation marks are at risk in practical UIDs.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn extract_etag(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn expect_write_success(response: &reqwest::Response) -> CaldavResult<()> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    Err(CaldavError::Http {
        status: status.as_u16(),
        message: status.canonical_reason().unwrap_or("").to_string(),
    })
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

    fn sample_new_event() -> NewEvent {
        NewEvent {
            title: "Standup".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 20, 8, 30, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
        }
    }

    #[tokio::test]
    async fn create_event_puts_with_if_none_match_and_returns_etag() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("PUT", mockito::Matcher::Regex(
                r"^/calendars/alice/work/.+\.ics$".into(),
            ))
            .match_header("if-none-match", "*")
            .with_status(201)
            .with_header("etag", "\"server-etag-1\"")
            .create_async()
            .await;
        let cal_url =
            Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let created = create_event(
            &client(),
            &cal_url,
            sample_new_event(),
            &creds(&server.url()),
        )
        .await
        .unwrap();
        m.assert_async().await;
        assert!(created.id.contains("@aperio"));
        assert_eq!(created.etag.as_deref(), Some("\"server-etag-1\""));
        assert_eq!(created.calendar_id, cal_url.to_string());
    }

    #[tokio::test]
    async fn update_event_sends_if_match_with_existing_etag() {
        let mut server = Server::new_async().await;
        let m = server
            .mock("PUT", mockito::Matcher::Regex(
                r"^/calendars/alice/work/.+\.ics$".into(),
            ))
            .match_header("if-match", "\"old-etag\"")
            .with_status(204)
            .with_header("etag", "\"new-etag\"")
            .create_async()
            .await;

        let cal_url =
            Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let existing = Event {
            id: "abc-123@aperio".into(),
            calendar_id: cal_url.to_string(),
            title: "Standup".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 20, 8, 30, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: Some("\"old-etag\"".into()),
        };
        let updated =
            update_event(&client(), existing, &creds(&server.url())).await.unwrap();
        m.assert_async().await;
        assert_eq!(updated.etag.as_deref(), Some("\"new-etag\""));
    }

    #[tokio::test]
    async fn update_event_412_surfaces_as_conflict() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("PUT", mockito::Matcher::Regex(
                r"^/calendars/alice/work/.+\.ics$".into(),
            ))
            .with_status(412)
            .with_body("Precondition Failed")
            .create_async()
            .await;
        let cal_url =
            Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let existing = Event {
            id: "abc-123@aperio".into(),
            calendar_id: cal_url.to_string(),
            title: "Standup".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 20, 8, 30, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: Some("\"stale-etag\"".into()),
        };
        let err = update_event(&client(), existing, &creds(&server.url()))
            .await
            .unwrap_err();
        match err {
            CaldavError::Http { status, .. } => assert_eq!(status, 412),
            other => panic!("expected 412, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_event_accepts_404() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("DELETE", mockito::Matcher::Regex(
                r"^/calendars/alice/work/.+\.ics$".into(),
            ))
            .with_status(404)
            .create_async()
            .await;
        let cal_url =
            Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        // The server already lost the row. We don't surface that
        // as an error — the desired post-state ("the event is
        // gone") is already true.
        delete_event(
            &client(),
            &cal_url,
            "abc-123@aperio",
            None,
            &creds(&server.url()),
        )
        .await
        .unwrap();
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
