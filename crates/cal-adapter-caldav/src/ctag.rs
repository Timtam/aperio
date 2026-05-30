//! CTag-gated incremental refresh (CACHE-7).
//!
//! CalDAV's Calendar Server extension exposes a collection-level change
//! tag, `<CS:getctag>` (`http://calendarserver.org/ns/`), that bumps
//! whenever ANY resource in the collection changes. Reading it is a
//! single cheap PROPFIND depth 0 — far cheaper than the full
//! `calendar-query` / `addressbook-query` REPORT.
//!
//! The host's delta path uses it as a gate: if the CTag matches the one
//! stored from the last refresh, nothing changed and we return an empty
//! `ChangeSet` without re-listing. Only when it differs (or the server
//! doesn't advertise a CTag at all) do we fall back to a full fetch and
//! hand the new CTag back as the token. This makes the periodic
//! background warm nearly free when a calendar is idle, which is the
//! common case.
//!
//! Per-resource `sync-collection` (RFC 6578) deltas would additionally
//! avoid re-fetching unchanged resources on a *change*, but CalDAV
//! identifies resources by href whereas Aperio's cache keys events by a
//! composite `href|uid` id — mapping a sync-report's removed hrefs back
//! to cache ids is lossy. The CTag gate sidesteps that entirely (a
//! change triggers a clean full replace), so it's the robust first cut.

use reqwest::header::{HeaderName, HeaderValue, CONTENT_TYPE};
use reqwest::{Client, Method, StatusCode};
use url::Url;

use crate::config::Credentials;
use crate::error::{CaldavError, CaldavResult};
use crate::xml::parse_multistatus;

const CTAG_PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/">
  <d:prop>
    <cs:getctag/>
  </d:prop>
</d:propfind>"#;

/// Read a collection's CTag via PROPFIND depth 0. Returns `None` when
/// the server doesn't advertise `getctag` (older / minimal CalDAV
/// servers) — the caller then treats the collection as "always changed"
/// and falls back to a full fetch, i.e. no regression, just no delta
/// benefit.
pub async fn read_ctag(
    client: &Client,
    collection_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Option<String>> {
    let method = Method::from_bytes(b"PROPFIND").expect("PROPFIND");
    let mut headers = crate::auth::auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    headers.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_static("0"),
    );
    let response = client
        .request(method, collection_url.clone())
        .headers(headers)
        .body(CTAG_PROPFIND_BODY)
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
    let entries = parse_multistatus(&body)?;
    Ok(entries.into_iter().find_map(|e| e.getctag))
}
