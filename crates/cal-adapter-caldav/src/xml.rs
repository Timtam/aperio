//! Tiny XML helpers for the CalDAV PROPFIND traffic we send.
//!
//! CalDAV's wire format is `multistatus` documents — a flat
//! `<response>` list, each with one `<href>` and one or more
//! `<propstat>` blocks. We never need the full DOM, just the
//! `<href>` plus a handful of named property values out of the first
//! successful propstat. The helpers below pull those out without
//! pulling in a full DOM library.

use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::reader::Reader;

use crate::error::{CaldavError, CaldavResult};

/// Local-name match against an XML start/end event regardless of the
/// namespace prefix the server used. CalDAV servers vary on whether
/// they declare `DAV:` as the default namespace or use a `d:` prefix
/// — `local_name()` strips both.
pub fn local_name_eq(name: QName<'_>, want: &[u8]) -> bool {
    name.local_name().as_ref() == want
}

/// Iterate over every `<response><href>…</href></response>` pair in a
/// CalDAV multistatus document and return the inner text of each
/// `<href>`. The order of the returned list matches the server's
/// response order.
///
/// Used by the calendar-listing layer in 6b.2; declared here in 6b.1
/// because it lives next to the rest of the multistatus parsing code.
#[allow(dead_code)]
pub fn extract_response_hrefs(body: &str) -> CaldavResult<Vec<String>> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut hrefs = Vec::new();
    let mut buf = Vec::new();
    let mut inside_response = false;
    let mut capture_href = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name_eq(e.name(), b"response") {
                    inside_response = true;
                } else if inside_response && local_name_eq(e.name(), b"href") {
                    capture_href = true;
                }
            }
            Ok(Event::End(e)) => {
                if local_name_eq(e.name(), b"response") {
                    inside_response = false;
                } else if local_name_eq(e.name(), b"href") {
                    capture_href = false;
                }
            }
            Ok(Event::Text(t)) => {
                if capture_href {
                    let text = t
                        .unescape()
                        .map_err(|e| CaldavError::Protocol(e.to_string()))?
                        .to_string();
                    if !text.is_empty() {
                        hrefs.push(text);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(CaldavError::Protocol(format!("xml parse: {err}")));
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(hrefs)
}

/// Extract the *first* `<href>` nested inside any element with local
/// name `prop_local_name` (e.g. `current-user-principal`,
/// `calendar-home-set`). Returns `None` when no such pair is found —
/// the caller decides whether that is fatal.
pub fn extract_first_nested_href(
    body: &str,
    prop_local_name: &[u8],
) -> CaldavResult<Option<String>> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth_inside_prop: u32 = 0;
    let mut capture_href = false;
    let mut found: Option<String> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name_eq(e.name(), prop_local_name) {
                    depth_inside_prop = depth_inside_prop.saturating_add(1);
                } else if depth_inside_prop > 0
                    && local_name_eq(e.name(), b"href")
                    && found.is_none()
                {
                    capture_href = true;
                }
            }
            Ok(Event::End(e)) => {
                if local_name_eq(e.name(), prop_local_name) {
                    depth_inside_prop = depth_inside_prop.saturating_sub(1);
                } else if local_name_eq(e.name(), b"href") {
                    capture_href = false;
                }
            }
            Ok(Event::Text(t)) => {
                if capture_href && found.is_none() {
                    let text = t
                        .unescape()
                        .map_err(|e| CaldavError::Protocol(e.to_string()))?
                        .to_string();
                    if !text.is_empty() {
                        found = Some(text);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(CaldavError::Protocol(format!("xml parse: {err}")));
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(found)
}

/// One parsed `<response>` block. Captures the bits the calendar
/// listing and event-range read both need without picking up the
/// whole tree.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResponseEntry {
    pub href: String,
    pub displayname: Option<String>,
    pub etag: Option<String>,
    /// `true` when the `<resourcetype>` block contains a
    /// `<C:calendar/>` element from the CalDAV namespace.
    pub is_calendar: bool,
    /// `true` when the `<resourcetype>` block contains a
    /// `<CR:addressbook/>` element from the CardDAV namespace.
    /// Used by the contacts listing path to filter the
    /// addressbook-home-set's children.
    pub is_addressbook: bool,
    /// `<ical:calendar-color>` or `<C:calendar-color>` value when
    /// present (servers vary on the namespace).
    pub calendar_color: Option<String>,
    /// `name` attribute of every `<comp/>` inside the
    /// `<supported-calendar-component-set>` block. Empty Vec when the
    /// property is absent.
    pub supported_components: Vec<String>,
    /// Inline iCal body returned by `<calendar-data>` on an
    /// `addressbook-query` / `calendar-query` REPORT. Empty in plain
    /// PROPFIND responses.
    pub calendar_data: Option<String>,
    /// Inline vCard body returned by `<address-data>` on an
    /// `addressbook-query` / `addressbook-multiget` REPORT, or by a
    /// PROPFIND that asks for it directly. Same CDATA / multi-chunk
    /// caveats as `calendar_data` — `capture_text` appends rather
    /// than overwrites.
    pub address_data: Option<String>,
}

/// Walk a multistatus document and emit one [`ResponseEntry`] per
/// `<response>` block. State-machine style — quick-xml is fast but
/// has no DOM, so we collect the bits we care about as the parser
/// streams through.
pub fn parse_multistatus(body: &str) -> CaldavResult<Vec<ResponseEntry>> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut out: Vec<ResponseEntry> = Vec::new();
    let mut current: Option<ResponseEntry> = None;

    // Track nesting so we don't pick up properties from outside the
    // currently-open <response>.
    let mut in_response = false;
    let mut in_resourcetype = false;
    let mut in_supported_set = false;
    // Where the next text event should land.
    let mut text_target: TextTarget = TextTarget::None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = e.name();

                if local_name_eq(name, b"response") {
                    in_response = true;
                    current = Some(ResponseEntry::default());
                    continue;
                }
                if !in_response {
                    continue;
                }
                let Some(entry) = current.as_mut() else {
                    continue;
                };

                if local_name_eq(name, b"resourcetype") {
                    in_resourcetype = true;
                } else if local_name_eq(name, b"supported-calendar-component-set") {
                    in_supported_set = true;
                } else if in_resourcetype && local_name_eq(name, b"calendar") {
                    entry.is_calendar = true;
                } else if in_resourcetype && local_name_eq(name, b"addressbook") {
                    entry.is_addressbook = true;
                } else if in_supported_set && local_name_eq(name, b"comp") {
                    // `<comp name="VEVENT"/>` — the value lives in the
                    // `name` attribute, not in the element text.
                    for attr in e.attributes().with_checks(false).flatten() {
                        if attr.key.local_name().as_ref() == b"name" {
                            if let Ok(v) = attr.unescape_value() {
                                entry.supported_components.push(v.into_owned());
                            }
                        }
                    }
                } else if local_name_eq(name, b"href") {
                    // Only the *direct* <href> child of <response>
                    // becomes the entry href; nested hrefs (inside
                    // resourcetype etc.) are ignored.
                    if !in_resourcetype && entry.href.is_empty() {
                        text_target = TextTarget::Href;
                    }
                } else if local_name_eq(name, b"displayname") {
                    text_target = TextTarget::Displayname;
                } else if local_name_eq(name, b"getetag") {
                    text_target = TextTarget::Etag;
                } else if local_name_eq(name, b"calendar-color") {
                    text_target = TextTarget::Color;
                } else if local_name_eq(name, b"calendar-data") {
                    text_target = TextTarget::CalendarData;
                } else if local_name_eq(name, b"address-data") {
                    text_target = TextTarget::AddressData;
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name();
                if local_name_eq(name, b"response") {
                    if let Some(entry) = current.take() {
                        if !entry.href.is_empty() {
                            out.push(entry);
                        }
                    }
                    in_response = false;
                    in_resourcetype = false;
                    in_supported_set = false;
                    text_target = TextTarget::None;
                    continue;
                }
                if local_name_eq(name, b"resourcetype") {
                    in_resourcetype = false;
                } else if local_name_eq(name, b"supported-calendar-component-set") {
                    in_supported_set = false;
                } else if !matches!(text_target, TextTarget::None) {
                    text_target = TextTarget::None;
                }
            }
            Ok(Event::Text(t)) => {
                if let Ok(text) = t.unescape() {
                    capture_text(&mut current, text_target, &text);
                }
            }
            Ok(Event::CData(t)) => {
                // CDATA shows up in CalDAV occasionally (Nextcloud for
                // XML-unsafe display names, iCloud for some
                // `<c:calendar-data>` payloads). The bytes are already
                // unescaped. `capture_text`'s CalendarData branch
                // appends, so a body served as
                // `Text + CData + Text` round-trips intact.
                if let Ok(text) = std::str::from_utf8(t.as_ref()) {
                    capture_text(&mut current, text_target, text);
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(CaldavError::Protocol(format!("xml parse: {err}")));
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy)]
enum TextTarget {
    None,
    Href,
    Displayname,
    Etag,
    Color,
    CalendarData,
    AddressData,
}

fn capture_text(current: &mut Option<ResponseEntry>, target: TextTarget, text: &str) {
    if matches!(target, TextTarget::None) {
        return;
    }
    let Some(entry) = current.as_mut() else {
        return;
    };
    match target {
        TextTarget::Href => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                entry.href = trimmed.to_string();
            }
        }
        TextTarget::Displayname => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                entry.displayname = Some(trimmed.to_string());
            }
        }
        TextTarget::Etag => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                entry.etag = Some(trimmed.to_string());
            }
        }
        TextTarget::Color => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                entry.calendar_color = Some(trimmed.to_string());
            }
        }
        TextTarget::CalendarData => {
            // iCal needs CRLF/LF preserved as-is; only collapse the
            // leading/trailing whitespace the XML pretty-printer
            // adds around the element body.
            //
            // **Append, never overwrite.** quick_xml can deliver the
            // body in several pieces — e.g. when the server wraps the
            // iCal payload in a `<![CDATA[…]]>` block we see
            // `Text("...whitespace...") + CData(body) +
            //  Text("...whitespace...")`, and when the body contains
            // an XML-escaped char like `&amp;` quick_xml emits
            // `Text(before) + GeneralRef + Text(after)`. iCloud /
            // Nextcloud are both fond of CDATA; without this every
            // chunk after the first would wipe the previous one and
            // we'd end up with a truncated VCALENDAR — typical visible
            // symptom: events still appear (the SUMMARY survives in
            // the last chunk) but the VALARM block from the first
            // chunk is gone, so `parse_valarms` silently returns an
            // empty list.
            let stripped = text.trim_matches(|c: char| c == ' ' || c == '\t');
            if !stripped.is_empty() {
                match &mut entry.calendar_data {
                    Some(buf) => buf.push_str(stripped),
                    None => entry.calendar_data = Some(stripped.to_string()),
                }
            }
        }
        TextTarget::AddressData => {
            // Same shape as CalendarData: vCard bodies have CRLF /
            // LF semantics, can ship CDATA-wrapped from iCloud,
            // can split across chunks when the XML escaping kicks
            // in. Append-not-overwrite or we truncate the vCard
            // on first sub-chunk.
            let stripped = text.trim_matches(|c: char| c == ' ' || c == '\t');
            if !stripped.is_empty() {
                match &mut entry.address_data {
                    Some(buf) => buf.push_str(stripped),
                    None => entry.address_data = Some(stripped.to_string()),
                }
            }
        }
        TextTarget::None => {}
    }
}

/// Walk a 207 Multi-Status body and return the first `<status>` element
/// whose HTTP code is **not** in the 2xx range. None means the body
/// contained only successful statuses (or no status at all — caller
/// decides whether that's tolerable).
///
/// PROPPATCH responses use this: a "207 with a 403 inside on the
/// displayname propstat" must surface as a real failure, not a silent
/// success. Some CalDAV servers (notably read-only iCal views from
/// vendors that advertise PROPPATCH but reject it) take exactly that
/// shape.
///
/// The walker scans every `<status>` regardless of whether it sits
/// directly inside `<response>` (response-wide status) or inside a
/// `<propstat>` (per-property-group status). RFC 4918 §9.2.1 allows
/// both shapes; either form indicates the operation's verdict.
pub fn first_failed_status_code(body: &str) -> Option<u16> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut capture_status = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name_eq(e.name(), b"status") {
                    capture_status = true;
                }
            }
            Ok(Event::End(e)) => {
                if local_name_eq(e.name(), b"status") {
                    capture_status = false;
                }
            }
            Ok(Event::Text(t)) if capture_status => {
                if let Ok(text) = t.unescape() {
                    if let Some(code) = parse_http_status_line(&text) {
                        if !(200..300).contains(&code) {
                            return Some(code);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
    None
}

/// Pull the integer status code out of a status-line like
/// `HTTP/1.1 200 OK` or `HTTP/1.1 403 Forbidden`. Tolerant of stray
/// leading / trailing whitespace.
fn parse_http_status_line(s: &str) -> Option<u16> {
    let s = s.trim();
    let mut parts = s.split_whitespace();
    parts.next()?; // "HTTP/1.1"
    parts.next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_principal_href() {
        let body = r#"<?xml version="1.0"?>
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
        let href =
            extract_first_nested_href(body, b"current-user-principal").unwrap();
        assert_eq!(href.as_deref(), Some("/principals/users/alice/"));
    }

    #[test]
    fn extracts_calendar_home_set_with_caldav_prefix() {
        // A more realistic body where the server uses the CalDAV
        // namespace + a separate prefix from the DAV: namespace.
        let body = r#"<?xml version="1.0"?>
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
        let href =
            extract_first_nested_href(body, b"calendar-home-set").unwrap();
        assert_eq!(href.as_deref(), Some("/calendars/alice/"));
    }

    #[test]
    fn collects_response_hrefs_in_order() {
        let body = r#"<?xml version="1.0"?>
            <d:multistatus xmlns:d="DAV:">
              <d:response><d:href>/calendars/alice/work/</d:href></d:response>
              <d:response><d:href>/calendars/alice/home/</d:href></d:response>
            </d:multistatus>"#;
        let hrefs = extract_response_hrefs(body).unwrap();
        assert_eq!(
            hrefs,
            vec!["/calendars/alice/work/", "/calendars/alice/home/"],
        );
    }

    #[test]
    fn missing_property_returns_none() {
        let body = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/</d:href></d:response></d:multistatus>"#;
        let href = extract_first_nested_href(body, b"calendar-home-set").unwrap();
        assert!(href.is_none());
    }

    #[test]
    fn multistatus_parses_calendar_listing() {
        // PROPFIND on the calendar-home-set, depth 1. First entry
        // is the home set itself (not a calendar), the rest are
        // individual calendars with mixed properties.
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"
              xmlns:ical="http://apple.com/ns/ical/">
  <d:response>
    <d:href>/calendars/alice/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>alice</d:displayname>
        <d:resourcetype><d:collection/></d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Work</d:displayname>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
        <ical:calendar-color>#1e88e5</ical:calendar-color>
        <c:supported-calendar-component-set>
          <c:comp name="VEVENT"/>
        </c:supported-calendar-component-set>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/alice/tasks/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Tasks</d:displayname>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
        <c:supported-calendar-component-set>
          <c:comp name="VTODO"/>
        </c:supported-calendar-component-set>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let entries = parse_multistatus(body).unwrap();
        assert_eq!(entries.len(), 3);

        // Home-set entry: collection but not a calendar.
        assert_eq!(entries[0].href, "/calendars/alice/");
        assert!(!entries[0].is_calendar);
        assert_eq!(entries[0].displayname.as_deref(), Some("alice"));

        let work = &entries[1];
        assert_eq!(work.href, "/calendars/alice/work/");
        assert!(work.is_calendar);
        assert_eq!(work.displayname.as_deref(), Some("Work"));
        assert_eq!(work.calendar_color.as_deref(), Some("#1e88e5"));
        assert_eq!(work.supported_components, vec!["VEVENT".to_string()]);

        let tasks = &entries[2];
        assert!(tasks.is_calendar);
        assert_eq!(tasks.supported_components, vec!["VTODO".to_string()]);
    }

    #[test]
    fn multistatus_parses_calendar_data_for_event_query() {
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/work/event-1.ics</d:href>
    <d:propstat>
      <d:prop>
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
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let entries = parse_multistatus(body).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].etag.as_deref(), Some("\"abc-123\""));
        let data = entries[0].calendar_data.as_deref().unwrap();
        assert!(data.contains("UID:event-1@aperio"));
        assert!(data.contains("DTSTART:20260520T080000Z"));
    }

    #[test]
    fn calendar_data_survives_cdata_wrapping() {
        // Regression: iCloud (and Nextcloud, sometimes) wrap the iCal
        // body inside `<c:calendar-data><![CDATA[…]]></c:calendar-data>`.
        // quick_xml emits that as a `Text(whitespace) + CData(body) +
        // Text(whitespace)` triple. The old `capture_text`
        // implementation **overwrote** `entry.calendar_data` on every
        // text event, so the CData body got clipped — typically the
        // last Text chunk (just whitespace) won and the iCal body was
        // empty, which silently dropped every VALARM along with the
        // rest of the VEVENT contents. The user-visible symptom: the
        // calendar still showed events (because the SUMMARY survived
        // in whatever chunk happened to land last), but every iCloud
        // event came in with no reminders.
        //
        // Append-not-overwrite is the fix; this test pins it.
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/work/event-1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"abc-123"</d:getetag>
        <c:calendar-data>
          <![CDATA[BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Apple Inc.//iCloud Calendar//EN
BEGIN:VEVENT
UID:icloud-event@example.com
SUMMARY:Standup
DTSTART:20260520T080000Z
DTEND:20260520T083000Z
BEGIN:VALARM
ACTION:DISPLAY
DESCRIPTION:Event reminder
TRIGGER:-PT15M
END:VALARM
END:VEVENT
END:VCALENDAR]]>
        </c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let entries = parse_multistatus(body).unwrap();
        assert_eq!(entries.len(), 1);
        let data = entries[0].calendar_data.as_deref().unwrap();
        // Spot-check every section of the body — overwrite-bug would
        // strip out at least one of these.
        assert!(data.contains("BEGIN:VCALENDAR"), "missing VCALENDAR open");
        assert!(data.contains("UID:icloud-event@example.com"), "missing UID");
        assert!(data.contains("BEGIN:VALARM"), "missing VALARM open");
        assert!(data.contains("TRIGGER:-PT15M"), "missing TRIGGER");
        assert!(data.contains("END:VALARM"), "missing VALARM close");
        assert!(data.contains("END:VCALENDAR"), "missing VCALENDAR close");
    }

    #[test]
    fn calendar_data_survives_entity_reference_split() {
        // Another way quick_xml splits text events: an inline entity
        // reference (`&amp;` etc.) breaks the surrounding text into
        // two `Event::Text` calls with a `GeneralRef` in between.
        // Aperio's `capture_text` re-unescapes the surrounding chunks
        // via `t.unescape()`, so the `&` is restored, and the append
        // semantics keep the order intact.
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/alice/work/event-2.ics</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:amp@example.com
SUMMARY:Tom &amp; Jerry
DTSTART:20260520T080000Z
DTEND:20260520T083000Z
BEGIN:VALARM
ACTION:DISPLAY
TRIGGER:-PT10M
END:VALARM
END:VEVENT
END:VCALENDAR</c:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let entries = parse_multistatus(body).unwrap();
        let data = entries[0].calendar_data.as_deref().unwrap();
        assert!(data.contains("Tom & Jerry"), "entity reference dropped or split: {data}");
        assert!(data.contains("BEGIN:VALARM"));
        assert!(data.contains("TRIGGER:-PT10M"));
    }

    #[test]
    fn first_failed_status_finds_inner_403() {
        // PROPPATCH-shaped response: the outer status is 207, but
        // the displayname propstat reports 403 Forbidden.
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:propstat>
      <d:prop><d:displayname/></d:prop>
      <d:status>HTTP/1.1 403 Forbidden</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        assert_eq!(first_failed_status_code(body), Some(403));
    }

    #[test]
    fn first_failed_status_ignores_all_2xx() {
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:propstat>
      <d:prop><d:displayname/></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        assert_eq!(first_failed_status_code(body), None);
    }

    #[test]
    fn first_failed_status_catches_response_level_failure() {
        // The `<status>` may live directly inside `<response>` (the
        // whole resource got the same verdict for every property).
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/calendars/alice/work/</d:href>
    <d:status>HTTP/1.1 423 Locked</d:status>
  </d:response>
</d:multistatus>"#;
        assert_eq!(first_failed_status_code(body), Some(423));
    }

    #[test]
    fn first_failed_status_returns_none_on_empty_body() {
        assert_eq!(first_failed_status_code(""), None);
    }

    #[test]
    fn first_failed_status_returns_first_when_mixed() {
        // 200 first, 403 second — caller wants the 403.
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/x/</d:href>
    <d:propstat>
      <d:prop><d:displayname/></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
    <d:propstat>
      <d:prop><d:resourcetype/></d:prop>
      <d:status>HTTP/1.1 403 Forbidden</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        assert_eq!(first_failed_status_code(body), Some(403));
    }
}
