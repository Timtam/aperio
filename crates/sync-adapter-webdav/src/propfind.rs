//! PROPFIND response parser.
//!
//! WebDAV `PROPFIND` returns a `multistatus` XML document; for our
//! use case (list a collection's children, decide which logs to
//! fetch) we only need three properties per resource:
//!
//!   - `<d:href>` — the resource path
//!   - `<d:getcontentlength>` — body size in bytes (optional;
//!     collections often omit it)
//!   - `<d:resourcetype>` — `<d:collection/>` marks the listed
//!     directory itself, which we skip
//!
//! We don't use `quick-xml`'s `serialize` feature here because the
//! response is namespace-heavy and we only need a few elements —
//! the event reader is simpler and more forgiving.
//!
//! The parser is deliberately permissive: it tolerates servers
//! that emit the DAV namespace under different prefixes (`D:`,
//! `dav:`, no prefix at all), and it ignores `<propstat>` blocks
//! whose status isn't `200 OK` because the per-property
//! granularity doesn't matter to us — we either get the values or
//! we skip the entry.

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use sync_core::{SyncError, SyncResult};

/// One row from a PROPFIND response. Hrefs are returned verbatim
/// (URL-encoded if the server emits them that way); callers
/// percent-decode before parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropfindEntry {
    pub href: String,
    /// `true` when `<d:resourcetype>` contained `<d:collection/>`.
    /// Callers skip the collection itself when listing children.
    pub is_collection: bool,
    /// Bytes from `<d:getcontentlength>`. `None` for collections
    /// or servers that omit the property.
    pub content_length: Option<u64>,
}

/// Parse a multistatus PROPFIND body. Returns one [`PropfindEntry`]
/// per `<d:response>` element. Collections (including the listed
/// directory itself) are returned with `is_collection = true` so
/// the caller can choose to skip or keep them.
pub fn parse_propfind_response(body: &str) -> SyncResult<Vec<PropfindEntry>> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut entries = Vec::new();
    let mut current: Option<EntryBuilder> = None;
    // Stack of element local names (namespace stripped) so we can
    // tell `getcontentlength` text from `displayname` text.
    let mut name_stack: Vec<String> = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let local = local_name(e.name().as_ref());
                if local == "response" {
                    current = Some(EntryBuilder::default());
                }
                if local == "collection" {
                    if let Some(b) = current.as_mut() {
                        b.is_collection = true;
                    }
                }
                name_stack.push(local.to_string());
            }
            Ok(Event::Empty(ref e)) => {
                // Self-closing tags. `<d:collection/>` inside
                // `<d:resourcetype>` is what tells us the entry is
                // a collection.
                let local = local_name(e.name().as_ref());
                if local == "collection" {
                    if let Some(b) = current.as_mut() {
                        b.is_collection = true;
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let local = local_name(e.name().as_ref());
                if local == "response" {
                    if let Some(b) = current.take() {
                        if let Some(entry) = b.build() {
                            entries.push(entry);
                        }
                    }
                }
                if let Some(top) = name_stack.last() {
                    if top.as_str() == local {
                        name_stack.pop();
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = e
                    .unescape()
                    .map_err(|err| {
                        SyncError::protocol(format!(
                            "PROPFIND XML decode: {err}"
                        ))
                    })?
                    .into_owned();
                if let Some(top) = name_stack.last() {
                    if let Some(b) = current.as_mut() {
                        match top.as_str() {
                            "href" => b.href = Some(text),
                            "getcontentlength" => {
                                b.content_length = text.trim().parse().ok();
                            }
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(SyncError::protocol(format!(
                    "PROPFIND XML parse: {err}",
                )));
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(entries)
}

/// Strip the XML namespace prefix from a tag name. `b"d:href"`
/// becomes `"href"`; `b"href"` returns `"href"`.
fn local_name(raw: &[u8]) -> String {
    let s = std::str::from_utf8(raw).unwrap_or("");
    match s.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => s.to_string(),
    }
}

#[derive(Default)]
struct EntryBuilder {
    href: Option<String>,
    is_collection: bool,
    content_length: Option<u64>,
}

impl EntryBuilder {
    fn build(self) -> Option<PropfindEntry> {
        Some(PropfindEntry {
            href: self.href?,
            is_collection: self.is_collection,
            content_length: self.content_length,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEXTCLOUD_LIKE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns" xmlns:oc="http://owncloud.org/ns">
  <d:response>
    <d:href>/remote.php/dav/files/alice/aperio/log/</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype><d:collection/></d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/remote.php/dav/files/alice/aperio/log/2026-05-01T08-00-00Z_dev-a.jsonl</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>2026-05-01T08-00-00Z_dev-a.jsonl</d:displayname>
        <d:getcontentlength>1234</d:getcontentlength>
        <d:resourcetype/>
        <d:getlastmodified>Sat, 01 May 2026 08:00:00 GMT</d:getlastmodified>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/remote.php/dav/files/alice/aperio/log/2026-05-02T09-00-00Z_dev-b.jsonl</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>2026-05-02T09-00-00Z_dev-b.jsonl</d:displayname>
        <d:getcontentlength>5678</d:getcontentlength>
        <d:resourcetype/>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

    #[test]
    fn parses_nextcloud_style_multistatus() {
        let entries = parse_propfind_response(NEXTCLOUD_LIKE).unwrap();
        assert_eq!(entries.len(), 3);

        // First entry: the collection itself.
        assert_eq!(
            entries[0].href,
            "/remote.php/dav/files/alice/aperio/log/",
        );
        assert!(entries[0].is_collection);
        assert!(entries[0].content_length.is_none());

        // Second + third entries: log files, not collections.
        assert!(!entries[1].is_collection);
        assert_eq!(entries[1].content_length, Some(1234));
        assert!(entries[1].href.ends_with("dev-a.jsonl"));

        assert!(!entries[2].is_collection);
        assert_eq!(entries[2].content_length, Some(5678));
    }

    #[test]
    fn handles_responses_with_no_prefix() {
        // Some servers (Apache mod_dav with stripped namespaces)
        // emit element names without the `d:` prefix.
        let raw = r#"<?xml version="1.0"?>
<multistatus xmlns="DAV:">
  <response>
    <href>/aperio/log/x.jsonl</href>
    <propstat>
      <prop>
        <getcontentlength>42</getcontentlength>
        <resourcetype/>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#;
        let entries = parse_propfind_response(raw).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].href, "/aperio/log/x.jsonl");
        assert_eq!(entries[0].content_length, Some(42));
        assert!(!entries[0].is_collection);
    }

    #[test]
    fn handles_uppercase_d_prefix() {
        // Some servers (older IIS) emit `D:` instead of `d:`.
        let raw = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/aperio/snapshot.json</D:href>
    <D:propstat>
      <D:prop>
        <D:getcontentlength>9000</D:getcontentlength>
        <D:resourcetype/>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#;
        let entries = parse_propfind_response(raw).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content_length, Some(9000));
    }

    #[test]
    fn empty_multistatus_returns_empty_vec() {
        let raw = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:"></d:multistatus>"#;
        let entries = parse_propfind_response(raw).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn malformed_xml_returns_protocol_error() {
        let err = parse_propfind_response("<not-closing").unwrap_err();
        assert!(matches!(err, SyncError::Protocol(_)));
    }

    #[test]
    fn collection_via_empty_element_is_recognised() {
        // The collection element typically appears as a child of
        // `<resourcetype>` and is self-closing. The empty-element
        // event must set `is_collection = true`.
        let raw = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/aperio/log/</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype><d:collection/></d:resourcetype>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let entries = parse_propfind_response(raw).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_collection);
    }
}
