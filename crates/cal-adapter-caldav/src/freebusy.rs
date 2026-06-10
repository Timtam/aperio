//! RFC 6638 free/busy lookup via the principal's schedule-outbox.
//!
//! CalDAV exposes attendee availability through the *scheduling outbox*
//! (RFC 6638 §4.1, building on the older `caldav-schedule` draft that
//! Apple's CalendarServer — and therefore iCloud — implements). The
//! flow is:
//!
//!   1. POST an iTIP `METHOD:REQUEST` VFREEBUSY to the outbox URL we
//!      discovered on the principal, naming the user as `ORGANIZER` and
//!      each queried address as an `ATTENDEE`.
//!   2. The server answers with a `<C:schedule-response>` document —
//!      one `<C:response>` per recipient, each carrying a
//!      `<C:calendar-data>` VFREEBUSY whose `FREEBUSY` lines list that
//!      recipient's busy periods.
//!
//! We map each response back to the requested address (by the
//! `<C:recipient>` href) and parse the `FREEBUSY` periods into
//! [`FreeBusySlot`]s. A recipient the server couldn't resolve (or
//! returned `FBTYPE=FREE` for) simply yields an empty slot list —
//! "availability unknown" — rather than failing the whole query.
//!
//! Servers without an outbox never reach this module: the caller checks
//! [`crate::discovery::Discovery::schedule_outbox_url`] first and
//! degrades to empty.

use cal_core::{DateRange, FreeBusy, FreeBusySlot};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use reqwest::{
    header::{HeaderName, HeaderValue, CONTENT_TYPE},
    Client, Method, StatusCode,
};
use url::Url;
use uuid::Uuid;

use crate::auth::auth_header;
use crate::config::Credentials;
use crate::error::{CaldavError, CaldavResult};
use crate::http::SendRetrying;
use crate::xml::local_name_eq;

/// Query the free/busy schedule of `emails` over `range` by POSTing an
/// iTIP VFREEBUSY request to the principal's `schedule-outbox`.
///
/// Returns one [`FreeBusy`] per requested address, in request order. A
/// recipient the server can't resolve degrades to an empty slot list
/// rather than failing the whole call.
pub async fn query_free_busy(
    client: &Client,
    outbox_url: &Url,
    organizer: &str,
    emails: &[&str],
    range: DateRange,
    credentials: &Credentials,
) -> CaldavResult<Vec<FreeBusy>> {
    if emails.is_empty() {
        return Ok(Vec::new());
    }
    let uid = Uuid::new_v4().to_string();
    let body = build_freebusy_request(organizer, emails, range, &uid, Utc::now());

    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    // The legacy caldav-schedule `Originator`/`Recipient` headers that
    // Apple's CalendarServer (and iCloud) require on an outbox POST.
    // RFC 6638 servers that derive these from the iCalendar body ignore
    // the extra headers harmlessly.
    if let Ok(v) = HeaderValue::from_str(&ensure_mailto(organizer)) {
        headers.insert(HeaderName::from_static("originator"), v);
    }
    for email in emails {
        if let Ok(v) = HeaderValue::from_str(&ensure_mailto(email)) {
            headers.append(HeaderName::from_static("recipient"), v);
        }
    }

    let response = client
        .request(Method::POST, outbox_url.clone())
        .headers(headers)
        .body(body)
        .send_retrying()
        .await?;
    let status = response.status();
    if status != StatusCode::from_u16(207).unwrap() && !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(CaldavError::Http {
            status: status.as_u16(),
            message: if text.is_empty() {
                status.canonical_reason().unwrap_or("").to_string()
            } else {
                text.chars().take(200).collect()
            },
        });
    }
    let text = response.text().await?;
    Ok(map_schedule_response(&text, emails))
}

/// One [`FreeBusy`] per address with no slots — "availability unknown".
/// Used by the caller when there's no outbox / organizer, and here as
/// the fall-through for a recipient the server didn't answer for.
pub fn unknown(emails: &[&str]) -> Vec<FreeBusy> {
    emails
        .iter()
        .map(|email| FreeBusy {
            email: (*email).to_string(),
            slots: Vec::new(),
        })
        .collect()
}

/// Build the iTIP `METHOD:REQUEST` VFREEBUSY body. iCalendar mandates
/// CRLF line endings, so we join with `\r\n`.
fn build_freebusy_request(
    organizer: &str,
    emails: &[&str],
    range: DateRange,
    uid: &str,
    dtstamp: DateTime<Utc>,
) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//Aperio//Free-Busy//EN".to_string(),
        "METHOD:REQUEST".to_string(),
        "BEGIN:VFREEBUSY".to_string(),
        format!("UID:{uid}"),
        format!("DTSTAMP:{}", fmt_compact(dtstamp)),
        format!("DTSTART:{}", fmt_compact(range.start)),
        format!("DTEND:{}", fmt_compact(range.end)),
        format!("ORGANIZER:{}", ensure_mailto(organizer)),
    ];
    for email in emails {
        lines.push(format!("ATTENDEE:{}", ensure_mailto(email)));
    }
    lines.push("END:VFREEBUSY".to_string());
    lines.push("END:VCALENDAR".to_string());
    lines.join("\r\n")
}

/// Pair every requested address with the busy slots the server returned
/// for it, matching by normalized recipient email. Order follows the
/// request; an address with no matching response gets an empty list.
fn map_schedule_response(body: &str, emails: &[&str]) -> Vec<FreeBusy> {
    let responses = parse_schedule_response(body).unwrap_or_default();
    emails
        .iter()
        .map(|email| {
            let want = normalize_email(email);
            let slots = responses
                .iter()
                .find(|(recipient, _)| normalize_email(recipient) == want)
                .map(|(_, caldata)| parse_freebusy_periods(caldata))
                .unwrap_or_default();
            FreeBusy {
                email: (*email).to_string(),
                slots,
            }
        })
        .collect()
}

/// Walk a `<C:schedule-response>` document and return one
/// `(recipient, calendar_data)` pair per `<C:response>` block. The
/// recipient is the `<C:recipient><D:href>` text; the calendar-data is
/// the inline VFREEBUSY (appended across chunks/CDATA the way the
/// multistatus parser handles `calendar-data`).
fn parse_schedule_response(body: &str) -> CaldavResult<Vec<(String, String)>> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut out: Vec<(String, String)> = Vec::new();

    let mut in_response = false;
    let mut in_recipient = false;
    let mut capture_recipient = false;
    let mut capture_caldata = false;
    let mut cur_recipient = String::new();
    let mut cur_caldata = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.name();
                if local_name_eq(name, b"response") {
                    in_response = true;
                    cur_recipient.clear();
                    cur_caldata.clear();
                } else if in_response && local_name_eq(name, b"recipient") {
                    in_recipient = true;
                } else if in_recipient && local_name_eq(name, b"href") {
                    capture_recipient = true;
                } else if in_response && local_name_eq(name, b"calendar-data") {
                    capture_caldata = true;
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name();
                if local_name_eq(name, b"response") {
                    if !cur_recipient.trim().is_empty() {
                        out.push((cur_recipient.trim().to_string(), cur_caldata.clone()));
                    }
                    in_response = false;
                    in_recipient = false;
                    capture_recipient = false;
                    capture_caldata = false;
                } else if local_name_eq(name, b"recipient") {
                    in_recipient = false;
                } else if local_name_eq(name, b"href") {
                    capture_recipient = false;
                } else if local_name_eq(name, b"calendar-data") {
                    capture_caldata = false;
                }
            }
            Ok(Event::Text(t)) => {
                if capture_recipient {
                    if let Ok(s) = t.unescape() {
                        cur_recipient.push_str(&s);
                    }
                } else if capture_caldata {
                    if let Ok(s) = t.unescape() {
                        cur_caldata.push_str(&s);
                    }
                }
            }
            Ok(Event::CData(t)) if capture_caldata => {
                if let Ok(s) = std::str::from_utf8(t.as_ref()) {
                    cur_caldata.push_str(s);
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(CaldavError::Protocol(format!("xml parse: {err}"))),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// Parse every busy period out of a VFREEBUSY body. Reads each
/// `FREEBUSY` property (after unfolding iCalendar line continuations),
/// skipping any tagged `FBTYPE=FREE`, and splits its comma-separated
/// `start/end` periods into [`FreeBusySlot`]s.
fn parse_freebusy_periods(ical: &str) -> Vec<FreeBusySlot> {
    let unfolded = unfold_ical(ical);
    let mut slots = Vec::new();
    for line in unfolded.lines() {
        let line = line.trim();
        let Some(colon) = line.find(':') else {
            continue;
        };
        let (head, value) = line.split_at(colon);
        let value = &value[1..];
        // Property name is the token before the first ';' (params) — we
        // only want the FREEBUSY property, not e.g. a SUMMARY that
        // happens to contain a colon.
        let prop = head.split(';').next().unwrap_or("").trim();
        if !prop.eq_ignore_ascii_case("FREEBUSY") {
            continue;
        }
        // Explicitly-free blocks aren't conflicts; drop them. Everything
        // else (BUSY, BUSY-TENTATIVE, BUSY-UNAVAILABLE, or no FBTYPE,
        // which defaults to BUSY) counts.
        if head.to_ascii_uppercase().contains("FBTYPE=FREE") {
            continue;
        }
        for period in value.split(',') {
            if let Some(slot) = parse_period(period.trim()) {
                slots.push(slot);
            }
        }
    }
    slots
}

/// Parse one `start/end` period. The second half is either a UTC
/// datetime or an ISO-8601 duration (`start/PT1H`).
fn parse_period(s: &str) -> Option<FreeBusySlot> {
    let (start_s, end_s) = s.split_once('/')?;
    let start = parse_ical_utc(start_s.trim())?;
    let end_s = end_s.trim();
    let end = if end_s.starts_with('P') || end_s.starts_with('p') {
        start + parse_iso_duration(end_s)?
    } else {
        parse_ical_utc(end_s)?
    };
    (end > start).then_some(FreeBusySlot { start, end })
}

/// Parse an iCalendar UTC datetime (`YYYYMMDDTHHMMSSZ`), tolerating a
/// missing `Z` (treated as UTC).
fn parse_ical_utc(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
    }
    None
}

/// Minimal ISO-8601 duration parser for the period-with-duration form
/// (`PT1H30M`, `P1D`, `PT45M`). Returns `None` for forms we don't
/// model (years/months are ambiguous and never appear in a free-busy
/// period anyway).
fn parse_iso_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let mut chars = s.chars().peekable();
    if chars.next().map(|c| c.to_ascii_uppercase()) != Some('P') {
        return None;
    }
    let mut in_time = false;
    let mut total = Duration::zero();
    let mut num = String::new();
    for c in chars {
        match c.to_ascii_uppercase() {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            unit => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                let part = match (in_time, unit) {
                    (false, 'D') => Duration::days(n),
                    (false, 'W') => Duration::weeks(n),
                    (true, 'H') => Duration::hours(n),
                    (true, 'M') => Duration::minutes(n),
                    (true, 'S') => Duration::seconds(n),
                    _ => return None,
                };
                total += part;
            }
        }
    }
    Some(total)
}

/// Undo iCalendar line folding: a CRLF (or bare LF) followed by a space
/// or tab is a continuation of the previous line.
fn unfold_ical(s: &str) -> String {
    s.replace("\r\n ", "")
        .replace("\r\n\t", "")
        .replace("\n ", "")
        .replace("\n\t", "")
}

/// Lower-case, `mailto:`-stripped form of an address for matching a
/// request address against a response recipient.
fn normalize_email(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix("mailto:")
        .or_else(|| s.strip_prefix("MAILTO:"))
        .unwrap_or(s);
    s.trim().to_ascii_lowercase()
}

/// Ensure an address carries the `mailto:` scheme iTIP requires.
fn ensure_mailto(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 7 && s[..7].eq_ignore_ascii_case("mailto:") {
        s.to_string()
    } else {
        format!("mailto:{s}")
    }
}

fn fmt_compact(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%dT%H%M%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range() -> DateRange {
        DateRange::new(
            "2026-06-01T00:00:00Z".parse().unwrap(),
            "2026-06-02T00:00:00Z".parse().unwrap(),
        )
    }

    #[test]
    fn request_body_names_organizer_and_each_attendee() {
        let body = build_freebusy_request(
            "mailto:alice@example.com",
            &["bob@example.com", "mailto:carol@example.com"],
            range(),
            "UID-123",
            "2026-05-30T12:00:00Z".parse().unwrap(),
        );
        assert!(body.contains("METHOD:REQUEST"));
        assert!(body.contains("BEGIN:VFREEBUSY"));
        assert!(body.contains("UID:UID-123"));
        assert!(body.contains("DTSTART:20260601T000000Z"));
        assert!(body.contains("DTEND:20260602T000000Z"));
        assert!(body.contains("ORGANIZER:mailto:alice@example.com"));
        // Bare address gets a mailto: prefix; an already-prefixed one is
        // left as-is (not double-prefixed).
        assert!(body.contains("ATTENDEE:mailto:bob@example.com"));
        assert!(body.contains("ATTENDEE:mailto:carol@example.com"));
        assert!(!body.contains("mailto:mailto:"));
        // iCalendar mandates CRLF.
        assert!(body.contains("\r\n"));
    }

    #[test]
    fn parses_schedule_response_busy_blocks_per_recipient() {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<C:schedule-response xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <C:response>
    <C:recipient>
      <D:href>mailto:bob@example.com</D:href>
    </C:recipient>
    <C:request-status>2.0;Success</C:request-status>
    <C:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
METHOD:REPLY
BEGIN:VFREEBUSY
UID:reply-1
DTSTART:20260601T000000Z
DTEND:20260602T000000Z
FREEBUSY;FBTYPE=BUSY:20260601T090000Z/20260601T100000Z,20260601T140000Z/20260601T143000Z
FREEBUSY;FBTYPE=FREE:20260601T120000Z/20260601T130000Z
END:VFREEBUSY
END:VCALENDAR
</C:calendar-data>
  </C:response>
  <C:response>
    <C:recipient>
      <D:href>mailto:ghost@example.com</D:href>
    </C:recipient>
    <C:request-status>3.7;Invalid calendar user</C:request-status>
  </C:response>
</C:schedule-response>"#;
        let fb = map_schedule_response(body, &["bob@example.com", "ghost@example.com"]);
        assert_eq!(fb.len(), 2);
        // Bob: two BUSY periods kept, the FREE one dropped.
        assert_eq!(fb[0].email, "bob@example.com");
        assert_eq!(fb[0].slots.len(), 2);
        assert_eq!(
            fb[0].slots[0].start,
            "2026-06-01T09:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(
            fb[0].slots[0].end,
            "2026-06-01T10:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(
            fb[0].slots[1].start,
            "2026-06-01T14:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        // Ghost: server couldn't resolve it → no calendar-data → empty,
        // but still present and labelled.
        assert_eq!(fb[1].email, "ghost@example.com");
        assert!(fb[1].slots.is_empty());
    }

    #[test]
    fn recipient_match_ignores_mailto_and_case() {
        let body = r#"<C:schedule-response xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <C:response>
    <C:recipient><D:href>MAILTO:Bob@Example.COM</D:href></C:recipient>
    <C:calendar-data>BEGIN:VFREEBUSY
FREEBUSY:20260601T090000Z/20260601T100000Z
END:VFREEBUSY</C:calendar-data>
  </C:response>
</C:schedule-response>"#;
        // Request uses the bare lower-case form; still matches.
        let fb = map_schedule_response(body, &["bob@example.com"]);
        assert_eq!(fb[0].slots.len(), 1);
    }

    #[test]
    fn period_with_duration_end_resolves() {
        // A FREEBUSY value whose period uses the start/DURATION form.
        let slots = parse_freebusy_periods(
            "BEGIN:VFREEBUSY\nFREEBUSY:20260601T090000Z/PT1H30M\nEND:VFREEBUSY",
        );
        assert_eq!(slots.len(), 1);
        assert_eq!(
            slots[0].end,
            "2026-06-01T10:30:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[test]
    fn no_freebusy_lines_yields_no_slots() {
        let slots = parse_freebusy_periods(
            "BEGIN:VFREEBUSY\nUID:x\nDTSTART:20260601T000000Z\nEND:VFREEBUSY",
        );
        assert!(slots.is_empty());
    }

    #[test]
    fn unknown_labels_every_requested_address() {
        let fb = unknown(&["a@example.com", "b@example.com"]);
        assert_eq!(fb.len(), 2);
        assert_eq!(fb[0].email, "a@example.com");
        assert!(fb[0].slots.is_empty());
        assert!(fb[1].slots.is_empty());
    }
}
