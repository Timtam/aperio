//! CardDAV contacts client (RFC 6352).
//!
//! Layered on top of the same WebDAV plumbing the CalDAV calendars
//! and VTODO tasks use:
//!
//!   - `list_contact_lists` — PROPFIND Depth: 1 on the
//!     addressbook-home-set, kept to collections that advertise
//!     `<CR:addressbook/>` as a resourcetype.
//!   - `get_contacts` — PROPFIND Depth: 1 on an addressbook URL
//!     asking for `<CR:address-data/>` so each vCard body comes
//!     back inline. Saves a follow-up GET per row.
//!   - `create_contact` / `update_contact` / `delete_contact` —
//!     PUT / PUT (with `If-Match`) / DELETE on the resource URL.
//!     Resource URLs follow the same `{href}|{uid}` encoding the
//!     tasks module uses so PUT / DELETE land on the path the
//!     server actually stored under, even when iCloud (or others)
//!     renames the resource away from `{home}/{uid}.vcf`.
//!   - `rename_contact_list` — reuses `calendars::proppatch_displayname`
//!     (DAV:displayname is namespace-agnostic and the existing
//!     helper handles the 207-inside-failed-propstat case).

use cal_core::{Contact, ContactList, ContainerColor, NewContact};
use reqwest::{
    header::{HeaderName, HeaderValue, CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH},
    Client, Method, StatusCode,
};
use url::Url;
use uuid::Uuid;

use crate::auth::auth_header;
use crate::config::Credentials;
use crate::error::{CaldavError, CaldavResult};
use crate::vcard::{build_vcard, parse_vcard, rebuild_vcard};
use crate::xml::{parse_multistatus, ResponseEntry};

const PROPFIND: &str = "PROPFIND";

/// PROPFIND body for the addressbook-home listing. Asks for
/// `<displayname>`, `<resourcetype>` (to filter on
/// `<CR:addressbook/>`), and an Apple-specific colour property
/// some servers (iCloud, Nextcloud's Contacts app) attach to the
/// collection. Servers that don't know the property answer with a
/// 404-status `<propstat>` — we ignore those and keep walking.
const ADDRESSBOOK_LIST_PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:cr="urn:ietf:params:xml:ns:carddav"
            xmlns:ical="http://apple.com/ns/ical/">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
    <ical:calendar-color/>
  </d:prop>
</d:propfind>"#;

/// PROPFIND body for the vcard-listing pass on an address book.
/// Pulls the etag plus the inline `address-data` so a single
/// round-trip gives us everything we need to build `Contact`s.
const ADDRESSBOOK_QUERY_PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:cr="urn:ietf:params:xml:ns:carddav">
  <d:prop>
    <d:getetag/>
    <cr:address-data/>
  </d:prop>
</d:propfind>"#;

/// `PROPFIND Depth: 1` on the addressbook-home, keep collections
/// that declare `<CR:addressbook/>` as a resourcetype.
pub async fn list_contact_lists(
    client: &Client,
    home_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Vec<ContactList>> {
    let entries =
        propfind(client, home_url, ADDRESSBOOK_LIST_PROPFIND_BODY, credentials, 1).await?;
    Ok(entries
        .into_iter()
        .filter(|e| e.is_addressbook)
        .map(|entry| to_contact_list(home_url, entry))
        .collect())
}

fn to_contact_list(home_url: &Url, entry: ResponseEntry) -> ContactList {
    let id = home_url
        .join(&entry.href)
        .map(|u| u.to_string())
        .unwrap_or(entry.href.clone());

    let color = entry.calendar_color.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.starts_with('#') && (trimmed.len() == 7 || trimmed.len() == 9) {
            // Apple emits `#RRGGBBAA`; we keep only the visible
            // RGB and discard the alpha so the Aperio hex parser
            // (which only understands `#RRGGBB`) doesn't choke.
            Some(ContainerColor {
                hex: trimmed[..7].to_string(),
                source: cal_core::ColorSource::Native,
            })
        } else {
            None
        }
    });

    ContactList {
        id,
        name: entry
            .displayname
            .unwrap_or_else(|| "Unnamed address book".into()),
        color,
        read_only: false,
    }
}

/// `PROPFIND Depth: 1` on the address book URL, asking for the
/// inline vCard body. Each entry whose `address_data` is non-empty
/// becomes a `Contact`; the others are the address book itself
/// (typically the first response with no body) or 404'd
/// sub-resources, both skipped.
pub async fn get_contacts(
    client: &Client,
    addressbook_url: &Url,
    credentials: &Credentials,
) -> CaldavResult<Vec<Contact>> {
    let entries = propfind(
        client,
        addressbook_url,
        ADDRESSBOOK_QUERY_PROPFIND_BODY,
        credentials,
        1,
    )
    .await?;
    let list_id = addressbook_url.as_str();
    let mut out = Vec::new();
    for entry in entries {
        let Some(body) = entry.address_data.as_deref() else {
            continue;
        };
        if body.trim().is_empty() {
            continue;
        }
        // Compose the same `{href}|{uid}` id shape the tasks
        // module uses: the href is what we need on
        // update/delete, the UID is the canonical vCard
        // identifier, and the pipe is a sentinel we'd never see
        // in either component.
        let uid = extract_vcard_uid(body).unwrap_or_else(|| entry.href.clone());
        let id = format!("{}|{}", entry.href, uid);
        match parse_vcard(body, list_id, id, entry.etag.clone()) {
            Ok(contact) => out.push(contact),
            Err(err) => {
                // A single malformed vCard shouldn't sink the whole
                // listing — log it and move on. The user might
                // still need to clean up the offending entry via
                // the web UI of the server, but their other
                // contacts stay reachable in Aperio.
                tracing::warn!(
                    href = %entry.href,
                    ?err,
                    "skipping vcard with parse error",
                );
            }
        }
    }
    Ok(out)
}

pub async fn create_contact(
    client: &Client,
    addressbook_url: &Url,
    new: NewContact,
    credentials: &Credentials,
) -> CaldavResult<Contact> {
    let uid = format!("{}@aperio", Uuid::new_v4());
    let resource = resource_url(addressbook_url, &uid)?;
    let body = build_vcard(&uid, &new);
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/vcard; charset=utf-8"),
    );
    // `If-None-Match: *` makes the PUT a create-only — if a
    // resource already lives at this UID the server answers 412
    // and we don't accidentally overwrite somebody else's row.
    headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
    let response = client
        .put(resource.clone())
        .headers(headers)
        .body(body)
        .send()
        .await?;
    let etag = expect_write(&response)?;
    let now = chrono::Utc::now();
    // Mint the composite id so subsequent updates / deletes find
    // the same resource path the server stored under. We use the
    // relative resource path as the href half — that's what
    // `resource_url_for_contact` re-joins against the address
    // book URL.
    let href = resource.path().to_string();
    let id = format!("{href}|{uid}");
    Ok(Contact {
        id,
        list_id: addressbook_url.to_string(),
        display_name: new.display_name,
        given_name: new.given_name,
        family_name: new.family_name,
        organization: new.organization,
        emails: new.emails,
        phone_numbers: new.phone_numbers,
        birthday: new.birthday,
        notes: new.notes,
        members: new.members,
        created_at: now,
        updated_at: now,
        etag,
    })
}

pub async fn update_contact(
    client: &Client,
    contact: Contact,
    credentials: &Credentials,
) -> CaldavResult<Contact> {
    let list_url = Url::parse(&contact.list_id).map_err(|e| {
        CaldavError::Config(format!("contact.list_id is not a URL: {e}"))
    })?;
    let resource = resource_url_for_contact(&list_url, &contact.id)?;
    let (_, uid) = decode_id(&contact.id);
    let body = rebuild_vcard(uid, &contact);
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/vcard; charset=utf-8"),
    );
    if let Some(etag) = &contact.etag {
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
    let new_etag = expect_write(&response)?;
    Ok(Contact {
        etag: new_etag.or(contact.etag.clone()),
        updated_at: chrono::Utc::now(),
        ..contact
    })
}

pub async fn delete_contact(
    client: &Client,
    addressbook_url: &Url,
    contact_id: &str,
    etag: Option<&str>,
    credentials: &Credentials,
) -> CaldavResult<()> {
    let resource = resource_url_for_contact(addressbook_url, contact_id)?;
    let mut headers = auth_header(credentials)?;
    if let Some(etag) = etag {
        let value = HeaderValue::from_str(etag)
            .map_err(|e| CaldavError::Config(e.to_string()))?;
        headers.insert(IF_MATCH, value);
    }
    let response = client.delete(resource).headers(headers).send().await?;
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

// ── Internal helpers ───────────────────────────────────────────────────

async fn propfind(
    client: &Client,
    url: &Url,
    body: &'static str,
    credentials: &Credentials,
    depth: u8,
) -> CaldavResult<Vec<ResponseEntry>> {
    let method = Method::from_bytes(PROPFIND.as_bytes()).expect("PROPFIND");
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    headers.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_str(&depth.to_string()).expect("digit"),
    );
    let response = client
        .request(method, url.clone())
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
    parse_multistatus(&text)
}

fn resource_url(list_url: &Url, uid: &str) -> CaldavResult<Url> {
    let slug = format!("{}.vcf", urlencoding(uid));
    list_url.join(&slug).map_err(Into::into)
}

/// Splits a contact id into `(Some(href), uid)` (composite ids
/// minted by `get_contacts` and `create_contact`) or `(None, id)`
/// for plain UIDs (pre-refetch, legacy callers).
fn decode_id(contact_id: &str) -> (Option<&str>, &str) {
    match contact_id.split_once('|') {
        Some((href, uid)) if !href.is_empty() => (Some(href), uid),
        _ => (None, contact_id),
    }
}

/// Resolve the absolute URL of the vCard resource. Prefers the
/// server-provided href encoded into the id by `get_contacts`;
/// falls back to `{list}/{uid}.vcf` when only a bare UID is
/// available. Same shape the tasks module uses for VTODO
/// resources.
fn resource_url_for_contact(list_url: &Url, contact_id: &str) -> CaldavResult<Url> {
    let (href, uid) = decode_id(contact_id);
    if let Some(href) = href {
        return list_url.join(href).map_err(Into::into);
    }
    resource_url(list_url, uid)
}

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn expect_write(response: &reqwest::Response) -> CaldavResult<Option<String>> {
    let status = response.status();
    if !status.is_success() {
        return Err(CaldavError::Http {
            status: status.as_u16(),
            message: status.canonical_reason().unwrap_or("").to_string(),
        });
    }
    Ok(response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string()))
}

/// Peek at a vCard body's `UID:` line. Used to mint the composite
/// id so subsequent updates / deletes can find the same resource
/// even when the server's href doesn't follow our `.vcf`
/// convention. Returns `None` when no `UID:` is present (rare —
/// the RFC requires it — but we tolerate the absence and fall
/// back to the server href).
fn extract_vcard_uid(body: &str) -> Option<String> {
    for raw_line in body.lines() {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        // The `UID:` property has no useful parameters in
        // practice; tolerate `UID;PARAM=val:value` by splitting at
        // the first `:` after a name match.
        let upper = line.to_ascii_uppercase();
        if let Some(rest) = upper.strip_prefix("UID") {
            let after_name = rest.split(':').nth(1)?;
            // We matched on the upper-cased version — pull the
            // value out of the original line at the same offset.
            let offset = upper.len() - after_name.len();
            if let Some(value) = line.get(offset..) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_uid_pulls_canonical_value() {
        let body = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nUID:abc-123@example.org\r\nEND:VCARD\r\n";
        assert_eq!(
            extract_vcard_uid(body).as_deref(),
            Some("abc-123@example.org"),
        );
    }

    #[test]
    fn extract_uid_tolerates_missing() {
        let body = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Test\r\nEND:VCARD\r\n";
        assert!(extract_vcard_uid(body).is_none());
    }

    #[test]
    fn decode_id_splits_composite() {
        let (href, uid) = decode_id("/addressbooks/alice/main/abc.vcf|abc@aperio");
        assert_eq!(href, Some("/addressbooks/alice/main/abc.vcf"));
        assert_eq!(uid, "abc@aperio");
    }

    #[test]
    fn decode_id_falls_back_to_bare_uid() {
        let (href, uid) = decode_id("abc@aperio");
        assert!(href.is_none());
        assert_eq!(uid, "abc@aperio");
    }

    #[test]
    fn resource_url_appends_vcf_extension() {
        let base = Url::parse("https://example.org/addressbooks/alice/main/").unwrap();
        let url = resource_url(&base, "abc@aperio").unwrap();
        // URL-encoded `@` should be `%40`.
        assert_eq!(
            url.path(),
            "/addressbooks/alice/main/abc%40aperio.vcf",
        );
    }

    #[test]
    fn resource_url_for_contact_prefers_server_href() {
        let base = Url::parse("https://example.org/addressbooks/alice/main/").unwrap();
        let url = resource_url_for_contact(
            &base,
            "/addressbooks/alice/main/server-chosen.vcf|abc",
        )
        .unwrap();
        assert_eq!(
            url.path(),
            "/addressbooks/alice/main/server-chosen.vcf",
        );
    }

    #[test]
    fn to_contact_list_strips_alpha_from_hex() {
        let mut entry = ResponseEntry::default();
        entry.href = "/addressbooks/alice/main/".into();
        entry.displayname = Some("Main".into());
        entry.calendar_color = Some("#1e88e5ff".into());
        entry.is_addressbook = true;
        let home = Url::parse("https://example.org/addressbooks/alice/").unwrap();
        let list = to_contact_list(&home, entry);
        assert_eq!(list.name, "Main");
        assert_eq!(list.color.unwrap().hex, "#1e88e5");
    }

    // ── End-to-end via mockito ──────────────────────────────────────

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
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap()
    }

    const HOME_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:cr="urn:ietf:params:xml:ns:carddav"
               xmlns:ical="http://apple.com/ns/ical/">
  <d:response>
    <d:href>/addressbooks/alice/</d:href>
    <d:propstat><d:prop>
      <d:resourcetype><d:collection/></d:resourcetype>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/addressbooks/alice/main/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Main</d:displayname>
      <d:resourcetype><d:collection/><cr:addressbook/></d:resourcetype>
      <ical:calendar-color>#4286f4</ical:calendar-color>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/addressbooks/alice/work/</d:href>
    <d:propstat><d:prop>
      <d:displayname>Work</d:displayname>
      <d:resourcetype><d:collection/><cr:addressbook/></d:resourcetype>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;

    #[tokio::test]
    async fn list_contact_lists_filters_to_addressbooks_only() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("PROPFIND", "/addressbooks/alice/")
            .with_status(207)
            .with_body(HOME_RESPONSE)
            .create_async()
            .await;

        let home = Url::parse(&format!("{}/addressbooks/alice/", server.url())).unwrap();
        let lists = list_contact_lists(&client(), &home, &creds(&server.url()))
            .await
            .unwrap();
        // The root collection (no <addressbook/> resourcetype) is
        // filtered out; both books survive.
        assert_eq!(lists.len(), 2);
        let names: Vec<_> = lists.iter().map(|l| l.name.as_str()).collect();
        assert!(names.contains(&"Main"));
        assert!(names.contains(&"Work"));
        let main = lists.iter().find(|l| l.name == "Main").unwrap();
        assert_eq!(main.color.as_ref().unwrap().hex, "#4286f4");
    }

    const VCARDS_RESPONSE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:cr="urn:ietf:params:xml:ns:carddav">
  <d:response>
    <d:href>/addressbooks/alice/main/abc.vcf</d:href>
    <d:propstat><d:prop>
      <d:getetag>"etag-abc"</d:getetag>
      <cr:address-data>BEGIN:VCARD&#13;
VERSION:3.0&#13;
UID:abc@aperio&#13;
FN:Max Mustermann&#13;
N:Mustermann;Max;;;&#13;
EMAIL;TYPE=INTERNET:max@example.com&#13;
END:VCARD&#13;
</cr:address-data>
    </d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/addressbooks/alice/main/def.vcf</d:href>
    <d:propstat><d:prop>
      <d:getetag>"etag-def"</d:getetag>
      <cr:address-data>BEGIN:VCARD&#13;
VERSION:3.0&#13;
UID:def@aperio&#13;
FN:Jane Doe&#13;
END:VCARD&#13;
</cr:address-data>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;

    #[tokio::test]
    async fn get_contacts_parses_inline_vcards() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("PROPFIND", "/addressbooks/alice/main/")
            .with_status(207)
            .with_body(VCARDS_RESPONSE)
            .create_async()
            .await;

        let book =
            Url::parse(&format!("{}/addressbooks/alice/main/", server.url())).unwrap();
        let contacts = get_contacts(&client(), &book, &creds(&server.url()))
            .await
            .unwrap();
        assert_eq!(contacts.len(), 2);
        let max = contacts
            .iter()
            .find(|c| c.display_name == "Max Mustermann")
            .unwrap();
        assert_eq!(max.given_name.as_deref(), Some("Max"));
        assert_eq!(max.family_name.as_deref(), Some("Mustermann"));
        assert_eq!(max.emails, vec!["max@example.com".to_string()]);
        assert_eq!(max.etag.as_deref(), Some("\"etag-abc\""));
        // Composite id pairs href and UID so the next update/delete
        // lands on the server's chosen path.
        assert!(max.id.contains('|'));
        assert!(max.id.starts_with("/addressbooks/alice/main/abc.vcf"));
    }

    #[tokio::test]
    async fn delete_contact_against_204_succeeds() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("DELETE", "/addressbooks/alice/main/abc.vcf")
            .with_status(204)
            .create_async()
            .await;
        let book =
            Url::parse(&format!("{}/addressbooks/alice/main/", server.url())).unwrap();
        delete_contact(
            &client(),
            &book,
            "/addressbooks/alice/main/abc.vcf|abc@aperio",
            None,
            &creds(&server.url()),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_contact_swallows_404_as_success() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("DELETE", "/addressbooks/alice/main/ghost.vcf")
            .with_status(404)
            .create_async()
            .await;
        let book =
            Url::parse(&format!("{}/addressbooks/alice/main/", server.url())).unwrap();
        // 404 ⇒ "already gone" ⇒ treat as success. Same shape the
        // calendars / tasks delete paths use.
        delete_contact(
            &client(),
            &book,
            "/addressbooks/alice/main/ghost.vcf|ghost",
            None,
            &creds(&server.url()),
        )
        .await
        .unwrap();
    }
}
