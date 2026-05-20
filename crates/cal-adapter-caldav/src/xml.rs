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
}
