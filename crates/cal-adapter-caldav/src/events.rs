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

use cal_core::{rrule_until_instant, AttendeeStatus, DateRange, Event, EventRecurrence, NewEvent};
use chrono::{DateTime, Utc};
use reqwest::{
    header::{HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH},
    Client, Method, StatusCode,
};
use url::Url;
use uuid::Uuid;

use crate::auth::auth_header;
use crate::config::Credentials;
use crate::error::{CaldavError, CaldavResult};
use crate::http::{is_transient_send_error, SendRetrying};
use crate::mapping::{
    decode_event_id, event_to_ical, new_event_to_ical, override_recurrence_id, parse_calendar_data,
    parse_calendar_data_with_href,
};
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
        .send_retrying()
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
        let mut events = parse_calendar_data_with_href(&ical, calendar_id, Some(&entry.href))?;
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
    organizer: Option<&str>,
) -> CaldavResult<Event> {
    let uid = format!("{}@aperio", Uuid::new_v4());
    let resource = resource_url(calendar_url, &uid)?;
    let body = new_event_to_ical(&uid, &event, organizer);

    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));

    // Hand-rolled retry (instead of `send_retrying`) so the replay reuses
    // the SAME freshly minted UID: if the first PUT secretly landed before
    // the connection died, the replay answers 412 (`If-None-Match: *` on an
    // existing resource) — and since nobody else can own this UUID, a 412
    // *after a retry* simply means "our first attempt succeeded". That turns
    // the worst-case duplicate into a clean success.
    let request = client.put(resource.clone()).headers(headers).body(body);
    let retry = request.try_clone();
    let (response, retried) = match request.send().await {
        Ok(response) => (response, false),
        Err(err) if is_transient_send_error(&err) => match retry {
            Some(builder) => (builder.send().await?, true),
            None => return Err(err.into()),
        },
        Err(err) => return Err(err.into()),
    };
    let etag = if retried && response.status() == StatusCode::PRECONDITION_FAILED {
        // First PUT landed; a 412 carries no ETag — the next read refreshes it.
        None
    } else {
        expect_write_success(&response)?;
        extract_etag(&response)
    };
    let now = Utc::now();

    Ok(Event {
        send_invitations: false,
        truncate_tail_overrides: false,
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
        // Echo the native color we just wrote back to the caller so the
        // post-create event carries it without a refetch.
        color_hex: event.color_hex,
        reminders: event.reminders,
        sound: event.sound,
        attendees: event.attendees,
        created_at: now,
        updated_at: now,
        etag,
        // Write path: organizer/RSVP metadata is read-only, populated
        // only when reading the event back from the server.
        organizer: None,
        attendee_responses: Vec::new(),
        // Freshly created by us — never a cancellation.
        cancelled: false,
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
    organizer: Option<&str>,
) -> CaldavResult<Event> {
    // "This and all following" truncation: the master's rule now ends earlier, so
    // any RECURRENCE-ID override in the dropped tail must go too. The plain
    // master-only PUT below leaves them (the server reattaches its other
    // components), so a provider-modified occurrence past the cutoff would ghost.
    // Take the GET-merge path only when the caller asked for it AND the rule
    // carries an UNTIL to bound the tail; otherwise fall through unchanged.
    if event.truncate_tail_overrides {
        if let Some(until) = event
            .recurrence
            .as_ref()
            .and_then(|r| rrule_until_instant(&r.rrule))
        {
            return update_event_dropping_tail_overrides(
                client,
                event,
                until,
                credentials,
                organizer,
            )
            .await;
        }
    }
    put_master_only(client, event, credentials, organizer).await
}

/// The plain update: serialise just the master and PUT it, letting the server
/// keep the resource's other components (overrides) on the next round-trip.
async fn put_master_only(
    client: &Client,
    event: Event,
    credentials: &Credentials,
    organizer: Option<&str>,
) -> CaldavResult<Event> {
    let cal_url = Url::parse(&event.calendar_id)
        .map_err(|e| CaldavError::Config(format!("event.calendar_id is not a URL: {e}")))?;
    let resource = resource_url_for_event(&cal_url, &event.id)?;
    let body = event_to_ical(&event, organizer);

    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    if let Some(etag) = &event.etag {
        let value = HeaderValue::from_str(etag).map_err(|e| CaldavError::Config(e.to_string()))?;
        headers.insert(IF_MATCH, value);
    }

    let response = client
        .put(resource.clone())
        .headers(headers)
        .body(body)
        .send_retrying()
        .await?;
    expect_write_success(&response)?;
    let new_etag = extract_etag(&response);

    Ok(Event {
        etag: new_etag.or(event.etag),
        updated_at: Utc::now(),
        ..event
    })
}

/// "This and all following" write: GET the resource, replace the master with the
/// truncated `event`, KEEP every RECURRENCE-ID override at/before `until` verbatim
/// (so per-instance edits/VALARMs/X-props survive byte-for-byte), and DROP the
/// overrides after `until` (the deleted tail). Falls back to the plain master-only
/// PUT on any parse ambiguity, so it is never worse than today.
async fn update_event_dropping_tail_overrides(
    client: &Client,
    event: Event,
    until: DateTime<Utc>,
    credentials: &Credentials,
    organizer: Option<&str>,
) -> CaldavResult<Event> {
    let cal_url = Url::parse(&event.calendar_id)
        .map_err(|e| CaldavError::Config(format!("event.calendar_id is not a URL: {e}")))?;
    let resource = resource_url_for_event(&cal_url, &event.id)?;

    // GET the current resource (raw body + ETag) to recover the override VEVENTs.
    let mut get_headers = auth_header(credentials)?;
    get_headers.insert(ACCEPT, HeaderValue::from_static("text/calendar"));
    let get = client
        .get(resource.clone())
        .headers(get_headers)
        .send_retrying()
        .await?;
    if !get.status().is_success() {
        // Can't read it back — fall through to the plain PUT (no worse than today).
        return put_master_only(client, event, credentials, organizer).await;
    }
    let server_etag = extract_etag(&get);
    let body = get.text().await?;

    // Master via event_to_ical (new RRULE/UNTIL + VTIMEZONE); merge keeps the
    // in-range override blocks verbatim and drops the tail. A parse mismatch →
    // bail to the plain PUT (never worse than today).
    let master_vcal = event_to_ical(&event, organizer);
    let new_body = match merge_dropping_tail_overrides(&body, &master_vcal, until, cal_url.as_str())
    {
        Some(b) => b,
        None => return put_master_only(client, event, credentials, organizer).await,
    };

    let mut put_headers = auth_header(credentials)?;
    put_headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    // If-Match against the FRESH server ETag (we just read the truth) so a
    // concurrent edit surfaces as 412 rather than a silent clobber.
    if let Some(tag) = &server_etag {
        if let Ok(v) = HeaderValue::from_str(tag) {
            put_headers.insert(IF_MATCH, v);
        }
    }
    let put = client
        .put(resource)
        .headers(put_headers)
        .body(new_body)
        .send_retrying()
        .await?;
    expect_write_success(&put)?;
    let new_etag = extract_etag(&put);

    Ok(Event {
        etag: new_etag.or(server_etag).or(event.etag.clone()),
        updated_at: Utc::now(),
        ..event
    })
}

/// Rebuild the resource body for a "this and all following" truncation: the
/// `master_vcal` (from `event_to_ical`, a full VCALENDAR holding just the new
/// master) with the in-range RECURRENCE-ID overrides from `body` spliced back in
/// and the tail overrides (RECURRENCE-ID after `until`) dropped.
///
/// Overrides are identified from the MAPPED events (so a `TZID`/all-day
/// RECURRENCE-ID is zone-resolved correctly) but their raw VEVENT text is kept
/// byte-for-byte, so per-instance edits / VALARMs / X-props survive intact.
/// Returns `None` when the parsed events and raw blocks don't correspond 1:1 (an
/// unmappable VEVENT, an odd shape) so the caller can fall back safely.
fn merge_dropping_tail_overrides(
    body: &str,
    master_vcal: &str,
    until: DateTime<Utc>,
    calendar_id: &str,
) -> Option<String> {
    let parsed = parse_calendar_data(body, calendar_id).ok()?;
    let blocks = split_vevent_blocks(body);
    if parsed.len() != blocks.len() {
        return None;
    }
    let kept: Vec<&str> = parsed
        .iter()
        .zip(blocks.iter())
        .filter_map(|(ev, block)| match override_recurrence_id(&ev.id) {
            // Master VEVENT → replaced by the truncated master in `master_vcal`.
            None => None,
            // In-range override → keep verbatim; tail override → drop.
            Some(rid) if rid <= until => Some(block.as_str()),
            Some(_) => None,
        })
        .collect();
    Some(splice_overrides_before_end(master_vcal, &kept))
}

/// Split a raw VCALENDAR body into its top-level `VEVENT` blocks (each retaining
/// its own line endings), in document order. VALARM / VTIMEZONE sub-components
/// stay inside their VEVENT block (they don't start with `BEGIN:VEVENT`).
fn split_vevent_blocks(body: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed == "BEGIN:VEVENT" {
            current = Some(String::new());
        }
        if let Some(buf) = current.as_mut() {
            buf.push_str(line);
        }
        if trimmed == "END:VEVENT" {
            if let Some(buf) = current.take() {
                blocks.push(buf);
            }
        }
    }
    blocks
}

/// Insert the `overrides` VEVENT blocks just before the final `END:VCALENDAR` of
/// `vcal` (the master-only calendar from `event_to_ical`). Each block already
/// ends with a newline; a missing one is added so the result stays well-formed.
fn splice_overrides_before_end(vcal: &str, overrides: &[&str]) -> String {
    if overrides.is_empty() {
        return vcal.to_string();
    }
    let Some(pos) = vcal.rfind("END:VCALENDAR") else {
        return vcal.to_string();
    };
    let line_start = vcal[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let mut out =
        String::with_capacity(vcal.len() + overrides.iter().map(|o| o.len()).sum::<usize>());
    out.push_str(&vcal[..line_start]);
    for o in overrides {
        out.push_str(o);
        if !o.ends_with('\n') {
            out.push_str("\r\n");
        }
    }
    out.push_str(&vcal[line_start..]);
    out
}

/// Outcome of a DELETE attempt. Distinguishes "we just removed
/// the row" from "the row wasn't here in the first place" so the
/// home-set walkers in `lib.rs` know whether they've actually
/// done the work or should keep looking in the next calendar.
///
/// The direct-API delete (single-calendar caller already knows
/// the URL) treats both as success — idempotent semantics for
/// "make sure this is gone" are the right contract there. The
/// walkers cannot, because 404 from the *wrong* calendar must
/// not short-circuit the search for the *right* one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// Server returned 2xx — we actually removed the resource.
    Deleted,
    /// Server returned 404 — the resource didn't exist at the
    /// URL we computed. Idempotent success for direct callers,
    /// "keep walking" for the home-set search.
    NotFound,
}

/// Delete an event from the server. `event_id` is the UID; the URL
/// is reconstructed as `<calendar_url>/<uid>.ics`. When the caller
/// passes an `etag`, an `If-Match` header is added so the server
/// refuses to delete a row that has changed under it.
///
/// 404 is treated as a non-error outcome (`DeleteOutcome::NotFound`)
/// — idempotent semantics for "make sure this row is gone". The
/// home-set walker uses the typed outcome to keep searching past
/// 404s for the calendar that actually owns the resource.
pub async fn delete_event(
    client: &Client,
    calendar_url: &Url,
    event_id: &str,
    etag: Option<&str>,
    credentials: &Credentials,
) -> CaldavResult<DeleteOutcome> {
    let resource = resource_url_for_event(calendar_url, event_id)?;
    let mut headers = auth_header(credentials)?;
    if let Some(etag) = etag {
        let value = HeaderValue::from_str(etag).map_err(|e| CaldavError::Config(e.to_string()))?;
        headers.insert(IF_MATCH, value);
    }
    let response = client
        .delete(resource.clone())
        .headers(headers)
        .send_retrying()
        .await?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(DeleteOutcome::NotFound);
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
    Ok(DeleteOutcome::Deleted)
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
    let resource = resource_url_for_event(calendar_url, event_id)?;

    // Step 1: fetch the master body + its ETag.
    let mut get_headers = auth_header(credentials)?;
    get_headers.insert(ACCEPT, HeaderValue::from_static("text/calendar"));
    let response = client
        .get(resource.clone())
        .headers(get_headers)
        .send_retrying()
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
    // Match on the UID component — `event_id` may be the composite
    // `{href}|{uid}` while the freshly-parsed bodies carry bare UIDs.
    let (_, want_uid) = decode_event_id(event_id);
    let master = events
        .iter_mut()
        .find(|e| decode_event_id(&e.id).1 == want_uid)
        .ok_or_else(|| {
            CaldavError::Discovery(format!("event '{event_id}' missing from its own resource"))
        })?;
    if master.recurrence.is_none() {
        return Err(CaldavError::Discovery(format!(
            "event '{event_id}' is not recurring"
        )));
    }
    let recurrence = master.recurrence.as_mut().unwrap();
    if !recurrence.exceptions.contains(&occurrence) {
        recurrence.exceptions.push(occurrence);
    }
    let master_clone = master.clone();
    // The first event we found should be the master — drop any
    // additional sub-components (overrides) and re-serialise just
    // the master with its updated EXDATE list. Servers reattach
    // their other components on the next round-trip.
    // Re-serialising the master after an EXDATE skip — never a scheduling
    // write, so no organizer.
    let serialised = crate::mapping::event_to_ical(&master_clone, None);

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
        .send_retrying()
        .await?;
    expect_write_success(&put)?;
    Ok(())
}

#[allow(dead_code)]
fn _touch_recurrence(_: &EventRecurrence) {}

/// RSVP to a meeting by surgically updating the connected user's
/// `ATTENDEE;PARTSTAT` in the stored `.ics` and PUTting it back. On an
/// RFC 6638 auto-scheduling server (iCloud) the PUT triggers the iTIP
/// `REPLY` to the organizer automatically; `Schedule-Reply: F`
/// suppresses that when `send_response` is false.
///
/// We edit the raw body rather than re-serialising via `event_to_ical`
/// so every server-side property (RRULE, the other attendees, X-props)
/// is preserved untouched — only the matching `PARTSTAT` parameter
/// changes. `base_url` only needs the right scheme+host; the event id's
/// encoded href (absolute path) supplies the resource path.
pub async fn respond_to_event(
    client: &Client,
    base_url: &Url,
    event_id: &str,
    self_email: &str,
    status: AttendeeStatus,
    send_response: bool,
    credentials: &Credentials,
) -> CaldavResult<()> {
    let partstat = match status {
        AttendeeStatus::Accepted => "ACCEPTED",
        AttendeeStatus::Declined => "DECLINED",
        AttendeeStatus::Tentative => "TENTATIVE",
        AttendeeStatus::NeedsAction => {
            return Err(CaldavError::Protocol(
                "cannot RSVP with status needs-action".into(),
            ));
        }
    };
    let resource = resource_url_for_event(base_url, event_id)?;

    // Fetch the current body + ETag.
    let mut get_headers = auth_header(credentials)?;
    get_headers.insert(ACCEPT, HeaderValue::from_static("text/calendar"));
    let response = client
        .get(resource.clone())
        .headers(get_headers)
        .send_retrying()
        .await?;
    let status_code = response.status();
    if !status_code.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(CaldavError::Http {
            status: status_code.as_u16(),
            message: if text.is_empty() {
                status_code.canonical_reason().unwrap_or("").to_string()
            } else {
                text.chars().take(200).collect()
            },
        });
    }
    let etag = extract_etag(&response);
    let body = response.text().await?;

    let new_body = set_self_partstat(&body, self_email, partstat).ok_or_else(|| {
        CaldavError::Protocol(format!(
            "'{self_email}' is not an attendee of event '{event_id}'"
        ))
    })?;

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
    if !send_response {
        // RFC 6638 §8.1: suppress the auto-generated scheduling reply.
        put_headers.insert(
            HeaderName::from_static("schedule-reply"),
            HeaderValue::from_static("F"),
        );
    }
    let put = client
        .put(resource)
        .headers(put_headers)
        .body(new_body)
        .send_retrying()
        .await?;
    expect_write_success(&put)?;
    Ok(())
}

/// Surgically set `PARTSTAT` on the `ATTENDEE` line whose value matches
/// `email`, leaving every other line (and its folding) untouched.
/// Returns `None` when no matching attendee is present. Only the edited
/// ATTENDEE line is unfolded — typical attendee lines fit one physical
/// line, so we emit it unfolded and pass everything else through
/// verbatim.
fn set_self_partstat(body: &str, email: &str, partstat: &str) -> Option<String> {
    let needle = email.trim().to_ascii_lowercase();
    let mut out = String::with_capacity(body.len() + 16);
    let mut changed = false;
    let mut lines = body.split_inclusive('\n').peekable();
    while let Some(phys) = lines.next() {
        let trimmed = phys.trim_end_matches(['\r', '\n']);
        if !trimmed.to_ascii_uppercase().starts_with("ATTENDEE") {
            out.push_str(phys);
            continue;
        }
        let ending = if phys.ends_with("\r\n") { "\r\n" } else { "\n" };
        // Gather any continuation lines into one logical ATTENDEE line.
        let mut logical = trimmed.to_string();
        while let Some(next) = lines.peek() {
            if next.starts_with(' ') || next.starts_with('\t') {
                let cont = lines.next().unwrap();
                logical.push_str(cont.trim_end_matches(['\r', '\n']).get(1..).unwrap_or(""));
            } else {
                break;
            }
        }
        if logical.to_ascii_lowercase().contains(&needle) {
            logical = replace_partstat(&logical, partstat);
            changed = true;
        }
        out.push_str(&logical);
        out.push_str(ending);
    }
    changed.then_some(out)
}

/// Replace (or insert) the `PARTSTAT` parameter on a single logical
/// content line. The property value (after the first unquoted `:`) is
/// left intact, including its `mailto:` colon.
fn replace_partstat(line: &str, partstat: &str) -> String {
    let Some(colon) = line.find(':') else {
        return line.to_string();
    };
    let (head, value) = line.split_at(colon); // value starts at ':'
    let mut found = false;
    let rebuilt: Vec<String> = head
        .split(';')
        .enumerate()
        .map(|(i, p)| {
            if i > 0 && p.to_ascii_uppercase().starts_with("PARTSTAT=") {
                found = true;
                format!("PARTSTAT={partstat}")
            } else {
                p.to_string()
            }
        })
        .collect();
    let mut head = rebuilt.join(";");
    if !found {
        head.push_str(&format!(";PARTSTAT={partstat}"));
    }
    format!("{head}{value}")
}

fn resource_url(calendar_url: &Url, uid: &str) -> CaldavResult<Url> {
    // CalDAV resource URLs are `<collection>/<slug>.ics`. The UID
    // makes a stable slug — collisions are vanishingly unlikely
    // because we mint UIDs as UUIDv4 + the Aperio domain suffix.
    // We percent-encode the slug to keep characters like `@` safe.
    let slug = format!("{}.ics", urlencoding(uid));
    calendar_url.join(&slug).map_err(Into::into)
}

/// Resolve the absolute URL of an event resource. Prefers the
/// server-provided href encoded into the id by `map_event`
/// (`{href}|{uid}`); falls back to the legacy `{collection}/{uid}.ics`
/// shape for freshly-created or older bare-UID ids. Mirrors
/// `tasks::resource_url_for_task`.
fn resource_url_for_event(calendar_url: &Url, event_id: &str) -> CaldavResult<Url> {
    let (href, uid) = decode_event_id(event_id);
    if let Some(href) = href {
        // `Url::join` resolves both absolute-path ("/calendars/…") and
        // absolute-URL hrefs against the collection base.
        return calendar_url.join(href).map_err(Into::into);
    }
    resource_url(calendar_url, uid)
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

    #[test]
    fn set_self_partstat_updates_only_the_matching_attendee() {
        let body = "BEGIN:VCALENDAR\r
BEGIN:VEVENT\r
UID:mtg-1\r
SUMMARY:Planning\r
ORGANIZER;CN=Boss:mailto:boss@example.com\r
ATTENDEE;CN=Boss;PARTSTAT=ACCEPTED:mailto:boss@example.com\r
ATTENDEE;CN=Me;PARTSTAT=NEEDS-ACTION:mailto:me@example.com\r
END:VEVENT\r
END:VCALENDAR\r
";
        let out = set_self_partstat(body, "me@example.com", "DECLINED").unwrap();
        // Our row flipped to DECLINED…
        assert!(out.contains("ATTENDEE;CN=Me;PARTSTAT=DECLINED:mailto:me@example.com"));
        // …the organizer's row is untouched…
        assert!(out.contains("ATTENDEE;CN=Boss;PARTSTAT=ACCEPTED:mailto:boss@example.com"));
        // …and the rest of the body is intact.
        assert!(out.contains("SUMMARY:Planning"));
        assert!(out.contains("ORGANIZER;CN=Boss:mailto:boss@example.com"));
    }

    #[test]
    fn set_self_partstat_inserts_when_absent_and_reports_no_match() {
        let body = "BEGIN:VEVENT\r\nATTENDEE;CN=Me:mailto:me@example.com\r\nEND:VEVENT\r\n";
        let out = set_self_partstat(body, "me@example.com", "TENTATIVE").unwrap();
        assert!(out.contains("ATTENDEE;CN=Me;PARTSTAT=TENTATIVE:mailto:me@example.com"));
        // A non-attendee yields None (so the caller can surface a clear error).
        assert!(set_self_partstat(body, "stranger@example.com", "ACCEPTED").is_none());
    }

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

        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
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
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
        }
    }

    #[tokio::test]
    async fn create_event_puts_with_if_none_match_and_returns_etag() {
        let mut server = Server::new_async().await;
        let m = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .match_header("if-none-match", "*")
            .with_status(201)
            .with_header("etag", "\"server-etag-1\"")
            .create_async()
            .await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let created = create_event(
            &client(),
            &cal_url,
            sample_new_event(),
            &creds(&server.url()),
            None,
        )
        .await
        .unwrap();
        m.assert_async().await;
        assert!(created.id.contains("@aperio"));
        assert_eq!(created.etag.as_deref(), Some("\"server-etag-1\""));
        assert_eq!(created.calendar_id, cal_url.to_string());
    }

    /// The iCloud stale-keep-alive shape: the first PUT's connection dies
    /// before any response, the replay reuses the SAME UID, and the server
    /// answers 412 (`If-None-Match: *` on the resource the first PUT
    /// actually created). `create_event` must report success — not a
    /// network error, and never a duplicate.
    #[tokio::test]
    async fn create_event_treats_412_after_connection_retry_as_success() {
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        fn request_path(buf: &[u8]) -> String {
            let head = String::from_utf8_lossy(buf);
            head.lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or_default()
                .to_string()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let paths_srv = Arc::clone(&paths);
        tokio::spawn(async move {
            // 1st connection: read the request, record its path, then die
            // WITHOUT a response.
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                paths_srv.lock().unwrap().push(request_path(&buf));
                drop(sock);
            }
            // 2nd connection: the replay. Record the path, answer 412.
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                paths_srv.lock().unwrap().push(request_path(&buf));
                let _ = sock
                    .write_all(b"HTTP/1.1 412 Precondition Failed\r\ncontent-length: 0\r\n\r\n")
                    .await;
                let _ = sock.shutdown().await;
            }
        });

        let cal_url = Url::parse(&format!("{base}/calendars/alice/work/")).unwrap();
        let created = create_event(&client(), &cal_url, sample_new_event(), &creds(&base), None)
            .await
            .expect("412 after a retried send means the first PUT landed");

        assert!(created.id.contains("@aperio"));
        assert!(created.etag.is_none(), "a 412 carries no ETag");
        let seen = paths.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "exactly one replay");
        assert_eq!(seen[0], seen[1], "the replay must reuse the SAME UID");
    }

    #[tokio::test]
    async fn update_event_sends_if_match_with_existing_etag() {
        let mut server = Server::new_async().await;
        let m = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .match_header("if-match", "\"old-etag\"")
            .with_status(204)
            .with_header("etag", "\"new-etag\"")
            .create_async()
            .await;

        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
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
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: Some("\"old-etag\"".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
        };
        let updated = update_event(&client(), existing, &creds(&server.url()), None)
            .await
            .unwrap();
        m.assert_async().await;
        assert_eq!(updated.etag.as_deref(), Some("\"new-etag\""));
    }

    #[tokio::test]
    async fn update_event_412_surfaces_as_conflict() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .with_status(412)
            .with_body("Precondition Failed")
            .create_async()
            .await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
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
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: Some("\"stale-etag\"".into()),
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
        };
        let err = update_event(&client(), existing, &creds(&server.url()), None)
            .await
            .unwrap_err();
        match err {
            CaldavError::Http { status, .. } => assert_eq!(status, 412),
            other => panic!("expected 412, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_event_reports_404_as_not_found_outcome() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock(
                "DELETE",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .with_status(404)
            .create_async()
            .await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        // The server already lost the row. The direct-API contract
        // still treats this as a non-error (idempotent), but the
        // outcome distinguishes "actually deleted" from "wasn't
        // there" so the home-set walker doesn't short-circuit on
        // the first 404 from a calendar that doesn't own the event.
        let outcome = delete_event(
            &client(),
            &cal_url,
            "abc-123@aperio",
            None,
            &creds(&server.url()),
        )
        .await
        .unwrap();
        assert_eq!(outcome, DeleteOutcome::NotFound);
    }

    #[tokio::test]
    async fn delete_event_reports_2xx_as_deleted_outcome() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock(
                "DELETE",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .with_status(204)
            .create_async()
            .await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let outcome = delete_event(
            &client(),
            &cal_url,
            "abc-123@aperio",
            None,
            &creds(&server.url()),
        )
        .await
        .unwrap();
        assert_eq!(outcome, DeleteOutcome::Deleted);
    }

    #[test]
    fn rrule_until_instant_parses_datetime_and_date_only() {
        assert_eq!(
            rrule_until_instant("FREQ=WEEKLY;BYDAY=MO;UNTIL=20260810T085959Z"),
            Some(Utc.with_ymd_and_hms(2026, 8, 10, 8, 59, 59).unwrap()),
        );
        // Date-only (all-day series) → that day's last instant (inclusive).
        assert_eq!(
            rrule_until_instant("FREQ=WEEKLY;BYDAY=MO;UNTIL=20260614"),
            Some(Utc.with_ymd_and_hms(2026, 6, 14, 23, 59, 59).unwrap()),
        );
        assert_eq!(rrule_until_instant("FREQ=WEEKLY;BYDAY=MO"), None);
    }

    #[test]
    fn override_recurrence_id_reads_the_rid_suffix() {
        assert_eq!(
            override_recurrence_id("href|uid::rid::2026-08-17T09:00:00Z"),
            Some(Utc.with_ymd_and_hms(2026, 8, 17, 9, 0, 0).unwrap()),
        );
        // A master / plain id has no ::rid:: suffix.
        assert_eq!(override_recurrence_id("href|uid"), None);
    }

    #[test]
    fn merge_drops_tail_overrides_keeps_in_range_ones() {
        // Master + two RECURRENCE-ID overrides: one before the cutoff (kept), one
        // after (the deleted tail → dropped).
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
BEGIN:VEVENT\r\nUID:series-1@aperio\r\nDTSTART:20260803T090000Z\r\nDTEND:20260803T093000Z\r\n\
SUMMARY:Weekly sync\r\nRRULE:FREQ=WEEKLY;BYDAY=MO\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:series-1@aperio\r\nRECURRENCE-ID:20260803T090000Z\r\nDTSTART:20260803T100000Z\r\n\
DTEND:20260803T103000Z\r\nSUMMARY:Moved head\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:series-1@aperio\r\nRECURRENCE-ID:20260817T090000Z\r\nDTSTART:20260817T100000Z\r\n\
DTEND:20260817T103000Z\r\nSUMMARY:Moved tail\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        // The truncated master, serialised the same way update_event would.
        let master = Event {
            id: "series-1@aperio".into(),
            calendar_id: "https://example.com/cal/".into(),
            title: "Weekly sync".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 8, 3, 9, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 8, 3, 9, 30, 0).unwrap(),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=MO;UNTIL=20260810T085959Z".into(),
                exceptions: Vec::new(),
                tzid: None,
            }),
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: None,
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
        };
        let master_vcal = event_to_ical(&master, None);
        let until = rrule_until_instant(&master.recurrence.as_ref().unwrap().rrule).unwrap();

        let merged =
            merge_dropping_tail_overrides(body, &master_vcal, until, "https://example.com/cal/")
                .expect("body parses cleanly");

        // The in-range override is kept verbatim; the tail override is gone.
        assert!(
            merged.contains("SUMMARY:Moved head"),
            "in-range override kept: {merged}"
        );
        assert!(
            !merged.contains("Moved tail") && !merged.contains("20260817"),
            "tail override dropped: {merged}"
        );
        // The master carries the new UNTIL, and there is exactly one kept override.
        assert!(merged.contains("UNTIL=20260810T085959Z"));
        assert_eq!(
            merged.matches("RECURRENCE-ID").count(),
            1,
            "only the in-range override remains: {merged}"
        );
    }

    #[test]
    fn merge_bails_on_block_count_mismatch() {
        // A VEVENT without a UID won't map, so parsed.len() != blocks.len() → None
        // (caller falls back to the plain master-only PUT).
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nDTSTART:20260803T090000Z\r\nSUMMARY:No UID\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let until = Utc.with_ymd_and_hms(2026, 8, 10, 0, 0, 0).unwrap();
        assert_eq!(
            merge_dropping_tail_overrides(body, "MASTER", until, "https://example.com/cal/"),
            None,
        );
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
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
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
