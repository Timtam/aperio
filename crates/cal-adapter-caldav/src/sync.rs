//! RFC 6578 `sync-collection` per-resource delta (CACHE-9).
//!
//! The CTag gate ([`crate::ctag`]) makes an *idle* collection nearly
//! free, but a single change still forces a full windowed re-list. This
//! module adds the per-resource leg: a `sync-collection` REPORT returns
//! just the hrefs that changed (and a fresh sync-token) since the last
//! token, and a follow-up `*-multiget` REPORT fetches the bodies of only
//! those resources.
//!
//! ## The deletion caveat (why a delete still triggers a full re-list)
//!
//! `sync-collection` reports a removed resource by **href** with a 404
//! status — it can't carry the resource's UID, because the resource is
//! gone. Aperio's cache keys CalDAV items by their **UID** (see
//! `mapping::map_event`), so a removed href can't be resolved back to a
//! cache id. Rather than maintain a lossy href→UID side-map, the delta
//! read uses `sync-collection` only for the common create/modify case
//! (fetch just the changed resources) and falls back to a clean windowed
//! full re-list whenever a sync batch contains ANY deletion — the full
//! replace drops the removed rows correctly. Idle and edit-heavy
//! collections get the per-resource win; the rarer delete pays one full
//! re-list, still gated behind an actual change.

use quick_xml::events::Event as XmlEvent;
use quick_xml::reader::Reader;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use reqwest::{Client, Method, StatusCode};
use url::Url;

use crate::config::Credentials;
use crate::error::{CaldavError, CaldavResult};
use crate::http::SendRetrying;
use crate::xml::{parse_multistatus, ResponseEntry};

/// Parsed result of a `sync-collection` REPORT (also reused to pluck the
/// token out of a `DAV:sync-token` PROPFIND response).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncCollection {
    /// Hrefs of created/modified resources — the ones carrying a
    /// `getetag` (a live 2xx propstat).
    pub changed: Vec<String>,
    /// Hrefs of removed resources — a `<response>` with an href but no
    /// `getetag` (a 404 status). We only act on the count, but keep the
    /// hrefs for diagnostics.
    pub deleted: Vec<String>,
    /// The collection's fresh sync-token, to pass back next round.
    pub sync_token: Option<String>,
}

/// Run a `sync-collection` REPORT against `collection_url`. An empty
/// `sync_token` requests an initial sync (every resource reported as
/// changed); a prior token requests only the delta since it.
pub async fn sync_collection(
    client: &Client,
    collection_url: &Url,
    sync_token: &str,
    credentials: &Credentials,
) -> CaldavResult<SyncCollection> {
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:sync-collection xmlns:d="DAV:">
  <d:sync-token>{}</d:sync-token>
  <d:sync-level>1</d:sync-level>
  <d:prop><d:getetag/></d:prop>
</d:sync-collection>"#,
        escape_xml(sync_token),
    );
    let xml = send_report(client, collection_url, body, credentials).await?;
    parse_sync_collection(&xml)
}

/// Read a collection's current `DAV:sync-token` via a cheap PROPFIND
/// depth 0. `None` means the server doesn't support sync-collection (the
/// property is absent) — the caller then stays on the CTag path.
pub async fn read_sync_token(
    client: &Client,
    collection_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Option<String>> {
    const BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:sync-token/></d:prop></d:propfind>"#;
    let method = Method::from_bytes(b"PROPFIND").expect("PROPFIND");
    let mut headers = base_headers(credentials)?;
    headers.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_static("0"),
    );
    let response = client
        .request(method, collection_url.clone())
        .headers(headers)
        .body(BODY)
        .send_retrying()
        .await?;
    let status = response.status();
    if status != StatusCode::from_u16(207).unwrap() && !status.is_success() {
        // A 4xx here just means "no sync-token support" — degrade to the
        // CTag path rather than failing the whole refresh.
        return Ok(None);
    }
    let xml = response.text().await?;
    Ok(parse_sync_collection(&xml)?.sync_token)
}

/// Enumerate every member resource of `collection_url` via a PROPFIND
/// Depth 1 (href + getetag), returning the resource hrefs only — the
/// collection's own (trailing-slash) href is dropped by
/// [`parse_sync_collection`].
///
/// This is the bootstrap enumeration. Two reasons it beats the
/// alternatives on iCloud: an empty-token `sync-collection` answers with
/// only a partial set (so the initial sync would miss most of a large
/// calendar), and a windowed `calendar-query` REPORT makes the server
/// scan the whole collection to apply the time-range filter, which times
/// out on large calendars. A Depth-1 PROPFIND returns the COMPLETE list
/// and, being metadata-only (no bodies, no filter), stays fast.
pub async fn list_resource_hrefs(
    client: &Client,
    collection_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Vec<String>> {
    const BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:getetag/></d:prop></d:propfind>"#;
    let method = Method::from_bytes(b"PROPFIND").expect("PROPFIND");
    let mut headers = base_headers(credentials)?;
    headers.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_static("1"),
    );
    let response = client
        .request(method, collection_url.clone())
        .headers(headers)
        .body(BODY)
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
    let xml = response.text().await?;
    Ok(parse_sync_collection(&xml)?.changed)
}

/// `calendar-multiget` the bodies of `hrefs` (VEVENT or VTODO). Returns
/// the raw response entries — the caller maps `calendar_data` into
/// events/tasks. Empty `hrefs` short-circuits to an empty Vec.
pub async fn calendar_multiget(
    client: &Client,
    collection_url: &Url,
    hrefs: &[String],
    credentials: &Credentials,
) -> CaldavResult<Vec<ResponseEntry>> {
    if hrefs.is_empty() {
        return Ok(Vec::new());
    }
    let body = multiget_body(
        "c:calendar-multiget",
        r#"xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav""#,
        "<c:calendar-data/>",
        hrefs,
    );
    let xml = send_report(client, collection_url, body, credentials).await?;
    parse_multistatus(&xml)
}

/// `addressbook-multiget` the bodies of `hrefs` (VCARD). Returns the raw
/// response entries — the caller maps `address_data` into contacts.
pub async fn addressbook_multiget(
    client: &Client,
    collection_url: &Url,
    hrefs: &[String],
    credentials: &Credentials,
) -> CaldavResult<Vec<ResponseEntry>> {
    if hrefs.is_empty() {
        return Ok(Vec::new());
    }
    let body = multiget_body(
        "cr:addressbook-multiget",
        r#"xmlns:d="DAV:" xmlns:cr="urn:ietf:params:xml:ns:carddav""#,
        "<cr:address-data/>",
        hrefs,
    );
    let xml = send_report(client, collection_url, body, credentials).await?;
    parse_multistatus(&xml)
}

fn multiget_body(elem: &str, ns: &str, data_prop: &str, hrefs: &[String]) -> String {
    let mut out = String::with_capacity(256 + hrefs.len() * 64);
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str(&format!("<{elem} {ns}>\n"));
    out.push_str(&format!("  <d:prop><d:getetag/>{data_prop}</d:prop>\n"));
    for href in hrefs {
        out.push_str(&format!("  <d:href>{}</d:href>\n", escape_xml(href)));
    }
    out.push_str(&format!("</{elem}>"));
    out
}

fn base_headers(credentials: &Credentials) -> CaldavResult<HeaderMap> {
    let mut headers = crate::auth::auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    Ok(headers)
}

async fn send_report(
    client: &Client,
    url: &Url,
    body: String,
    credentials: &Credentials,
) -> CaldavResult<String> {
    let method = Method::from_bytes(b"REPORT").expect("REPORT");
    let mut headers = base_headers(credentials)?;
    // RFC 6578 §3.2: sync-collection takes Depth 0; multiget is keyed off
    // the explicit href list and is happy with 0 too.
    headers.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_static("0"),
    );
    let response = client
        .request(method, url.clone())
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
    Ok(response.text().await?)
}

/// Walk a `sync-collection` (or sync-token PROPFIND) multistatus body.
///
/// Each `<d:response>` is classified by whether it carries a `getetag`:
/// present ⇒ the resource is live (changed/created); absent ⇒ it was
/// removed (404). The collection's `<d:sync-token>` appears once as a
/// direct child of `<d:multistatus>` (REPORT) or inside a propstat
/// (PROPFIND) — we grab it wherever it lands.
pub fn parse_sync_collection(body: &str) -> CaldavResult<SyncCollection> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut out = SyncCollection::default();
    let mut in_response = false;
    let mut cur_href = String::new();
    let mut cur_has_etag = false;
    // Where the next Text event lands: 1 = href, 2 = sync-token. (A small
    // int beats a borrow against `buf`.)
    let mut text_target = 0u8;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                match local.as_slice() {
                    b"response" => {
                        in_response = true;
                        cur_href.clear();
                        cur_has_etag = false;
                    }
                    b"href" if in_response && cur_href.is_empty() => text_target = 1,
                    b"getetag" if in_response => cur_has_etag = true,
                    b"sync-token" => text_target = 2,
                    _ => {}
                }
            }
            Ok(XmlEvent::End(e)) => {
                let local = e.local_name().as_ref().to_ascii_lowercase();
                if local.as_slice() == b"response" {
                    // A trailing-slash href is the collection itself (or a
                    // sub-collection), which servers echo back in a Depth-1
                    // listing / sync-collection report. It is never a
                    // syncable resource — and multi-getting it asks the
                    // server for the WHOLE collection's calendar-data, which
                    // times out on large calendars. Drop it.
                    if !cur_href.is_empty() && !cur_href.ends_with('/') {
                        if cur_has_etag {
                            out.changed.push(std::mem::take(&mut cur_href));
                        } else {
                            out.deleted.push(std::mem::take(&mut cur_href));
                        }
                    } else {
                        cur_href.clear();
                    }
                    in_response = false;
                }
                text_target = 0;
            }
            Ok(XmlEvent::Text(t)) if text_target != 0 => {
                let raw = match t.unescape() {
                    Ok(c) => c.to_string(),
                    Err(_) => continue,
                };
                let s = raw.trim();
                if s.is_empty() {
                    continue;
                }
                match text_target {
                    1 => cur_href.push_str(s),
                    2 => out.sync_token = Some(s.to_string()),
                    _ => {}
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(err) => {
                return Err(CaldavError::Protocol(format!(
                    "sync-collection xml parse: {err}"
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(out)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sync_collection_splits_changed_deleted_and_token() {
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/cal/e1.ics</d:href>
    <d:propstat>
      <d:prop><d:getetag>"v2"</d:getetag></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/cal/gone.ics</d:href>
    <d:status>HTTP/1.1 404 Not Found</d:status>
  </d:response>
  <d:sync-token>http://sabre.io/ns/sync/42</d:sync-token>
</d:multistatus>"#;
        let r = parse_sync_collection(body).unwrap();
        assert_eq!(r.changed, vec!["/cal/e1.ics".to_string()]);
        assert_eq!(r.deleted, vec!["/cal/gone.ics".to_string()]);
        assert_eq!(r.sync_token.as_deref(), Some("http://sabre.io/ns/sync/42"));
    }

    #[test]
    fn parse_sync_collection_drops_the_collection_self_href() {
        // iCloud (and a Depth-1 PROPFIND) echo the collection's own
        // trailing-slash href back. It is NOT a syncable resource —
        // multi-getting it asks for the whole calendar and times out — so
        // it must never land in `changed`/`deleted`. Only the real
        // resource survives.
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/calendars/home/</d:href>
    <d:propstat>
      <d:prop><d:getetag>"collection-ctag"</d:getetag></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/home/evt%40aperio.ics</d:href>
    <d:propstat>
      <d:prop><d:getetag>"v1"</d:getetag></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let r = parse_sync_collection(body).unwrap();
        assert_eq!(
            r.changed,
            vec!["/calendars/home/evt%40aperio.ics".to_string()]
        );
        assert!(r.deleted.is_empty());
    }

    #[test]
    fn parse_sync_collection_reads_propfind_token() {
        // A DAV:sync-token PROPFIND nests the token inside a propstat; we
        // still pluck it out (and ignore the collection's own href, which
        // has no getetag).
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/cal/</d:href>
    <d:propstat>
      <d:prop><d:sync-token>tok-1</d:sync-token></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let r = parse_sync_collection(body).unwrap();
        assert_eq!(r.sync_token.as_deref(), Some("tok-1"));
    }

    #[test]
    fn multiget_body_lists_every_href() {
        let body = multiget_body(
            "c:calendar-multiget",
            r#"xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav""#,
            "<c:calendar-data/>",
            &["/cal/a.ics".into(), "/cal/b.ics".into()],
        );
        assert!(body.contains("<d:href>/cal/a.ics</d:href>"));
        assert!(body.contains("<d:href>/cal/b.ics</d:href>"));
        assert!(body.contains("<c:calendar-data/>"));
    }
}
