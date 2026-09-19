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
    decode_event_id, event_to_ical_preserving, new_event_to_ical, override_recurrence_id,
    override_to_vevent, parse_calendar_data, parse_calendar_data_with_href, PriorAlarms,
};
use crate::xml::parse_multistatus;
use std::ops::Range;

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
        keep_attendees: false,
        clear_attendees: false,
        organized_elsewhere: false,
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
///
/// An override id (`{href}|{uid}::rid::{slot}`) writes that one occurrence
/// and nothing else, see [`update_override`].
pub async fn update_event(
    client: &Client,
    event: Event,
    credentials: &Credentials,
    organizer: Option<&str>,
) -> CaldavResult<Event> {
    // First, and never falling through: the plain PUT below would write the
    // occurrence over its whole series' resource.
    if let Some((series_id, slot)) =
        cal_core::split_override_id(&event.id).map_err(|e| CaldavError::Config(e.to_string()))?
    {
        let series_id = series_id.to_string();
        return update_override(client, event, &series_id, slot, credentials, organizer).await;
    }
    // "This and all following" truncation: the master's rule now ends earlier, so
    // any RECURRENCE-ID override in the dropped tail must go too. The plain
    // master-only PUT below leaves them (the server reattaches its other
    // components), so a provider-modified occurrence past the cutoff would ghost.
    // Take the GET-merge path only when the caller asked for it AND the rule
    // carries an UNTIL to bound the tail; otherwise fall through unchanged.
    //
    // TIMED series only: both the UNTIL and an override's RECURRENCE-ID resolve to
    // exact UTC instants, so the `rid <= until` cutoff is precise. An ALL-DAY
    // series has a DATE-only UNTIL (end-of-day UTC) while an all-day RECURRENCE-ID
    // anchors at local midnight — mixing those mis-classifies the cutoff-day
    // override off UTC. Rather than drop/keep the wrong one, all-day truncations
    // fall through to the plain PUT (their cross-client override cleanup is a rare
    // gap, unchanged from before this feature).
    if event.truncate_tail_overrides && !event.all_day {
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

/// Read the alarms already on the server's copy, so the PUT below doesn't
/// destroy what it cannot rebuild (see [`PriorAlarms`]).
///
/// Best effort in every direction: an event with no reminders can inherit
/// nothing, so it is not worth a request; a read that fails costs only the
/// preservation and never the write. The cost is one GET on a save that
/// carries reminders — a user-initiated action, one at a time.
async fn read_prior_alarms(
    client: &Client,
    resource: &Url,
    event: &Event,
    credentials: &Credentials,
) -> PriorAlarms {
    if event.reminders.is_empty() {
        return PriorAlarms::default();
    }
    let Ok(mut headers) = auth_header(credentials) else {
        return PriorAlarms::default();
    };
    headers.insert(ACCEPT, HeaderValue::from_static("text/calendar"));
    let Ok(response) = client
        .get(resource.clone())
        .headers(headers)
        .send_retrying()
        .await
    else {
        return PriorAlarms::default();
    };
    if !response.status().is_success() {
        return PriorAlarms::default();
    }
    match response.text().await {
        Ok(body) => PriorAlarms::read(&body),
        Err(_) => PriorAlarms::default(),
    }
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
    let prior = read_prior_alarms(client, &resource, &event, credentials).await;
    let body = event_to_ical_preserving(&event, organizer, prior);

    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    // If-Match still guards with the CALLER's ETag, not one from the read
    // above: the read is only there to see what the alarms looked like, and
    // conflict detection has to keep answering "did this change since the user
    // last saw it", not "since a moment ago".
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

/// Write one changed occurrence, `event`, into its series' resource.
///
/// The server keeps a changed occurrence as an override: a VEVENT of its own in
/// the series' resource, with the series' UID and a RECURRENCE-ID that names the
/// slot. A PUT replaces the whole resource, so this reads the resource, swaps
/// that one block for the rewritten occurrence, and puts everything else back
/// byte for byte: the master, the other overrides, the time zones.
///
/// Refused rather than widened. If the resource cannot be read block by block,
/// or no override stands in the slot any more, nothing is written: the only
/// other write left would be one over the whole series.
///
/// If-Match carries the caller's ETag when there is one, as for a master: the
/// question is still whether anything changed since the user last saw it.
async fn update_override(
    client: &Client,
    event: Event,
    series_id: &str,
    slot: DateTime<Utc>,
    credentials: &Credentials,
    organizer: Option<&str>,
) -> CaldavResult<Event> {
    let cal_url = Url::parse(&event.calendar_id)
        .map_err(|e| CaldavError::Config(format!("event.calendar_id is not a URL: {e}")))?;
    let resource = resource_url_for_event(&cal_url, series_id)?;
    let (body, server_etag) = get_resource(client, &resource, credentials).await?;
    let refused = |why: &str| {
        CaldavError::Protocol(format!(
            "the occurrence of {series_id} at {}: {why}; refusing to write the whole series instead",
            slot.to_rfc3339(),
        ))
    };
    let blocks = vevent_blocks(&body, cal_url.as_str())
        .ok_or_else(|| refused("its resource cannot be read block by block"))?;
    let (_, uid) = decode_event_id(series_id);
    let block = blocks
        .iter()
        .find(|b| {
            override_recurrence_id(&b.event.id) == Some(slot)
                && cal_core::series_master_id(&b.event.id) == uid
        })
        .ok_or_else(|| refused("it is no longer an exception in the series"))?;
    let recurrence_id = recurrence_id_lines(&body[block.range.clone()])
        .ok_or_else(|| refused("its block carries no RECURRENCE-ID"))?;
    let rendered = override_to_vevent(
        &event,
        series_id,
        &recurrence_id,
        organizer,
        PriorAlarms::read_occurrence(&body, slot),
    );
    let new_body = format!(
        "{}{}{}",
        &body[..block.range.start],
        rendered,
        &body[block.range.end..]
    );

    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    if let Some(tag) = event.etag.as_ref().or(server_etag.as_ref()) {
        let value = HeaderValue::from_str(tag).map_err(|e| CaldavError::Config(e.to_string()))?;
        headers.insert(IF_MATCH, value);
    }
    let response = client
        .put(resource)
        .headers(headers)
        .body(new_body)
        .send_retrying()
        .await?;
    expect_write_success(&response)?;
    let new_etag = extract_etag(&response);

    Ok(Event {
        etag: new_etag.or(event.etag.clone()),
        updated_at: Utc::now(),
        ..event
    })
}

/// GET a resource: its body and ETag. A 404 comes back as
/// `CaldavError::Http { status: 404 }`, which the home-set walkers in `lib.rs`
/// read as "not in this calendar".
async fn get_resource(
    client: &Client,
    resource: &Url,
    credentials: &Credentials,
) -> CaldavResult<(String, Option<String>)> {
    let mut headers = auth_header(credentials)?;
    headers.insert(ACCEPT, HeaderValue::from_static("text/calendar"));
    let response = client
        .get(resource.clone())
        .headers(headers)
        .send_retrying()
        .await?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(CaldavError::Http {
            status: 404,
            message: format!("'{resource}' not found on server"),
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
    Ok((response.text().await?, etag))
}

/// One VEVENT of a resource body: where its text sits in the body, and the
/// event it maps to.
struct VeventBlock {
    range: Range<usize>,
    event: Event,
}

/// Read `body` block by block: every top-level VEVENT beside the event it maps
/// to, in document order. `None` when the two don't line up one to one (a
/// VEVENT the mapper skips, an odd shape), so no caller can mistake one block
/// for another.
fn vevent_blocks(body: &str, calendar_id: &str) -> Option<Vec<VeventBlock>> {
    let parsed = parse_calendar_data(body, calendar_id).ok()?;
    let ranges = component_ranges(body, "VEVENT");
    if parsed.len() != ranges.len() {
        return None;
    }
    Some(
        ranges
            .into_iter()
            .zip(parsed)
            .map(|(range, event)| VeventBlock { range, event })
            .collect(),
    )
}

/// The RECURRENCE-ID property of one VEVENT block, as the block spells it:
/// the property line, its folded continuation lines and their line endings.
fn recurrence_id_lines(block: &str) -> Option<String> {
    let mut out = String::new();
    let mut inside = false;
    for line in block.split_inclusive('\n') {
        // A continuation line starts with a space or a tab (RFC 5545 §3.1).
        let continued = line.starts_with(' ') || line.starts_with('\t');
        if inside && continued {
            out.push_str(line);
            continue;
        }
        if inside {
            break;
        }
        let name = line
            .split([';', ':'])
            .next()
            .unwrap_or("")
            .trim_end_matches(['\r', '\n']);
        if !continued && name.eq_ignore_ascii_case("RECURRENCE-ID") {
            inside = true;
            out.push_str(line);
        }
    }
    (!out.is_empty()).then_some(out)
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
    // bail to the plain PUT (never worse than today). The body is already in
    // hand, so the master's untouched alarms keep their identity for free —
    // the overrides' alarms survive anyway, being spliced back verbatim.
    let master_vcal = event_to_ical_preserving(&event, organizer, PriorAlarms::read(&body));
    let new_body =
        match merge_with_overrides(&body, &master_vcal, cal_url.as_str(), |rid| rid <= until) {
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

/// Rebuild a resource around a rewritten master: `master_vcal` (a VCALENDAR
/// holding just the new master, from `event_to_ical`) with the RECURRENCE-ID
/// overrides of `body` that `keep` accepts spliced back in, and the others
/// dropped. A "this and all following" truncation keeps the overrides up to its
/// cutoff; skipping one occurrence keeps all but the one in its slot.
///
/// Overrides are identified from the MAPPED events (so a `TZID`/all-day
/// RECURRENCE-ID is zone-resolved correctly) but their raw VEVENT text is kept
/// byte-for-byte, so per-instance edits / VALARMs / X-props survive intact. A
/// kept override may name a zone the new master does not, so every VTIMEZONE of
/// `body` that `master_vcal` lacks comes along with them, byte-for-byte too.
/// Returns `None` when the parsed events and raw blocks don't correspond 1:1 (an
/// unmappable VEVENT, an odd shape) so the caller can fall back safely.
fn merge_with_overrides(
    body: &str,
    master_vcal: &str,
    calendar_id: &str,
    keep: impl Fn(DateTime<Utc>) -> bool,
) -> Option<String> {
    let blocks = vevent_blocks(body, calendar_id)?;
    let kept: Vec<&str> = blocks
        .iter()
        .filter_map(|b| match override_recurrence_id(&b.event.id) {
            // Master VEVENT → replaced by the new master in `master_vcal`.
            None => None,
            Some(rid) if keep(rid) => Some(&body[b.range.clone()]),
            Some(_) => None,
        })
        .collect();
    if kept.is_empty() {
        return Some(master_vcal.to_string());
    }
    let merged = splice_overrides_before_end(master_vcal, &kept);
    Some(with_missing_vtimezones(merged, body))
}

/// The byte ranges of the top-level `BEGIN:<name>` … `END:<name>` components
/// of a raw VCALENDAR body, each with its own line endings, in document order.
/// Sub-components (a VEVENT's VALARM, a VTIMEZONE's STANDARD) stay inside their
/// parent's range, because they have other names.
fn component_ranges(body: &str, name: &str) -> Vec<Range<usize>> {
    let begin = format!("BEGIN:{name}");
    let end = format!("END:{name}");
    let mut ranges = Vec::new();
    let mut start: Option<usize> = None;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        // Match the PHYSICAL line, stripping only the trailing CR/LF — NOT leading
        // whitespace. RFC-5545 folds long values onto continuation lines prefixed
        // with a space/tab, so a real component boundary never has leading
        // whitespace; `line.trim()` would misread a folded "…\r\n BEGIN:VEVENT"
        // inside a DESCRIPTION as a boundary and corrupt the block.
        let marker = line.trim_end_matches(['\r', '\n']);
        if marker == begin {
            start = Some(offset);
        }
        offset += line.len();
        if marker == end {
            if let Some(from) = start.take() {
                ranges.push(from..offset);
            }
        }
    }
    ranges
}

/// Split a raw VCALENDAR body into its top-level `VEVENT` blocks (each retaining
/// its own line endings), in document order. VALARM / VTIMEZONE sub-components
/// stay inside their VEVENT block (they don't start with `BEGIN:VEVENT`).
#[cfg(test)]
fn split_vevent_blocks(body: &str) -> Vec<String> {
    component_ranges(body, "VEVENT")
        .into_iter()
        .map(|r| body[r].to_string())
        .collect()
}

/// `vcal` with every VTIMEZONE of `body` it does not define yet, copied
/// verbatim in front of its first VEVENT, where RFC 5545 wants a zone: before
/// the components that name it.
fn with_missing_vtimezones(mut vcal: String, body: &str) -> String {
    let defined: Vec<String> = component_ranges(&vcal, "VTIMEZONE")
        .into_iter()
        .filter_map(|r| vtimezone_tzid(&vcal[r]))
        .collect();
    let missing: String = component_ranges(body, "VTIMEZONE")
        .into_iter()
        .filter(|r| vtimezone_tzid(&body[r.clone()]).is_some_and(|tzid| !defined.contains(&tzid)))
        .map(|r| body[r].to_string())
        .collect();
    if missing.is_empty() {
        return vcal;
    }
    if let Some(pos) = vcal.find("BEGIN:VEVENT") {
        vcal.insert_str(pos, &missing);
    }
    vcal
}

/// The TZID a VTIMEZONE block defines, unfolded. Only the component itself
/// carries one; its STANDARD and DAYLIGHT parts do not.
fn vtimezone_tzid(block: &str) -> Option<String> {
    let mut lines = block.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        // `get`, not slicing: a line of server text may hold a multi-byte
        // character across byte 4.
        let named_tzid = line
            .get(..4)
            .is_some_and(|n| n.eq_ignore_ascii_case("TZID"))
            && line
                .get(4..)
                .is_some_and(|rest| rest.starts_with([':', ';']));
        if !named_tzid {
            continue;
        }
        let (_, value) = line.split_once(':')?;
        let mut tzid = value.trim_end_matches(['\r', '\n']).to_string();
        while let Some(next) = lines.peek() {
            if !(next.starts_with(' ') || next.starts_with('\t')) {
                break;
            }
            tzid.push_str(next[1..].trim_end_matches(['\r', '\n']));
            lines.next();
        }
        return Some(tzid);
    }
    None
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

/// Skip one occurrence of a recurring series: append `occurrence` to the
/// master's EXDATE list, and drop the override that stands in that slot if
/// there is one. Mirrors the EXDATE handling that the local adapter has for
/// "delete only this occurrence" of a recurring event. An override moved far
/// from its slot is found all the same: it is recognised by its RECURRENCE-ID,
/// the slot, not by where it now starts.
///
/// A PUT replaces the whole resource. The master is rebuilt from core fields
/// with its new EXDATE, as for any master write: its alarms survive, properties
/// Aperio does not model do not (see [`event_to_ical_preserving`]). Every other
/// override goes back byte for byte, and so do the time zones they name. A body
/// that cannot be read block by block gets the master alone, as every body did
/// before overrides were kept.
///
/// A resource can hold overrides without their master: an invitation to one
/// occurrence of somebody else's series arrives that way. Skipping such an
/// occurrence drops its block, and the resource goes with its last block.
///
/// The final write uses If-Match against the freshly read ETag so a concurrent
/// edit from another client surfaces as a 412 rather than a silent overwrite.
pub async fn add_event_exdate(
    client: &Client,
    calendar_url: &Url,
    event_id: &str,
    occurrence: DateTime<Utc>,
    credentials: &Credentials,
) -> CaldavResult<()> {
    let resource = resource_url_for_event(calendar_url, event_id)?;
    let (body, etag) = get_resource(client, &resource, credentials).await?;

    // Match on the UID component — `event_id` may be the composite
    // `{href}|{uid}` while the freshly-parsed bodies carry bare UIDs.
    let (_, want_uid) = decode_event_id(event_id);
    let mut events = parse_calendar_data(&body, calendar_url.as_str())?;
    let new_body = match events
        .iter_mut()
        .find(|e| decode_event_id(&e.id).1 == want_uid)
    {
        Some(master) => {
            let Some(recurrence) = master.recurrence.as_mut() else {
                return Err(CaldavError::Discovery(format!(
                    "event '{event_id}' is not recurring"
                )));
            };
            if !recurrence.exceptions.contains(&occurrence) {
                recurrence.exceptions.push(occurrence);
            }
            // Re-serialising the master after an EXDATE skip — never a
            // scheduling write, so no organizer. Skipping one occurrence changes
            // nothing about the alarms, so they all come back exactly as the
            // server had them: the body we just read is right here.
            let master_vcal = event_to_ical_preserving(master, None, PriorAlarms::read(&body));
            merge_with_overrides(&body, &master_vcal, calendar_url.as_str(), |rid| {
                rid != occurrence
            })
            .unwrap_or(master_vcal)
        }
        None => {
            let blocks = vevent_blocks(&body, calendar_url.as_str()).unwrap_or_default();
            let Some(block) = blocks.iter().find(|b| {
                override_recurrence_id(&b.event.id) == Some(occurrence)
                    && cal_core::series_master_id(&b.event.id) == want_uid
            }) else {
                return Err(CaldavError::Discovery(format!(
                    "event '{event_id}' missing from its own resource"
                )));
            };
            if blocks.len() == 1 {
                return delete_resource(client, &resource, etag.as_deref(), credentials).await;
            }
            format!("{}{}", &body[..block.range.start], &body[block.range.end..])
        }
    };

    // If-Match guards against a race with a concurrent edit; without an ETag
    // we send the request anyway — the server will still accept it.
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
        .body(new_body)
        .send_retrying()
        .await?;
    expect_write_success(&put)?;
    Ok(())
}

/// DELETE a resource whose last occurrence was just skipped. A 404 means it is
/// gone already, which is what was asked for.
async fn delete_resource(
    client: &Client,
    resource: &Url,
    etag: Option<&str>,
    credentials: &Credentials,
) -> CaldavResult<()> {
    let mut headers = auth_header(credentials)?;
    if let Some(tag) = etag {
        if let Ok(v) = HeaderValue::from_str(tag) {
            headers.insert(IF_MATCH, v);
        }
    }
    let response = client
        .delete(resource.clone())
        .headers(headers)
        .send_retrying()
        .await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(());
    }
    expect_write_success(&response)
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
    // Only the tests still call the preservation-free renderer directly.
    use crate::config::{AuthKind, CaldavAccountConfig};
    use crate::mapping::event_to_ical;
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
            organized_elsewhere: false,
            organizer: None,
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
            keep_attendees: false,
            clear_attendees: false,
            organized_elsewhere: false,
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
    async fn update_reads_the_server_copy_and_puts_apples_mark_back() {
        // The wiring, end to end: a plain update has no body in hand, so it
        // reads the server's copy first and hands the alarms to the renderer.
        // Without that read the PUT would spell Apple's own default alert as a
        // hand-made alarm and the Calendar app would stop recognising it.
        let mut server = Server::new_async().await;
        let get = server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .with_status(200)
            .with_header("content-type", "text/calendar")
            .with_body(
                "BEGIN:VCALENDAR\r
VERSION:2.0\r
BEGIN:VEVENT\r
UID:abc-123@aperio\r
DTSTAMP:20260520T060000Z\r
SUMMARY:Standup\r
DTSTART:20260520T080000Z\r
DTEND:20260520T083000Z\r
X-APPLE-DEFAULT-ALARM:TRUE\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:Event reminder\r
TRIGGER:-PT1H\r
UID:alarm-1\r
X-APPLE-DEFAULT-ALARM:TRUE\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
",
            )
            .create_async()
            .await;
        let put = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            // The caller's ETag, NOT the one the read above just saw: a change
            // since the user last looked still has to surface as a conflict.
            .match_header("if-match", "\"old-etag\"")
            .match_body(mockito::Matcher::Regex("X-APPLE-DEFAULT-ALARM:TRUE".into()))
            .with_status(204)
            .with_header("etag", "\"new-etag\"")
            .create_async()
            .await;

        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let existing = Event {
            reminders: vec![cal_core::Reminder {
                kind: cal_core::ReminderKind::Relative { minutes_before: 60 },
                sound: None,
            }],
            ..sample_existing_event(&cal_url)
        };
        update_event(&client(), existing, &creds(&server.url()), None)
            .await
            .unwrap();
        get.assert_async().await;
        put.assert_async().await;
    }

    #[tokio::test]
    async fn update_without_reminders_does_not_read_first() {
        // Nothing to inherit, so the extra round-trip would buy nothing.
        // Asserted by a GET mock that must never be hit — without it this test
        // passed whether the read happened or not.
        let mut server = Server::new_async().await;
        let get = server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .expect(0)
            .with_status(200)
            .create_async()
            .await;
        let put = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .with_status(204)
            .with_header("etag", "\"new-etag\"")
            .create_async()
            .await;

        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        update_event(
            &client(),
            sample_existing_event(&cal_url),
            &creds(&server.url()),
            None,
        )
        .await
        .unwrap();
        put.assert_async().await;
        get.assert_async().await;
    }

    #[tokio::test]
    async fn a_failed_read_costs_the_mark_and_not_the_save() {
        // The preservation is a courtesy; the write is the job.
        let mut server = Server::new_async().await;
        let _get = server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .with_status(500)
            .create_async()
            .await;
        let put = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .with_status(204)
            .with_header("etag", "\"new-etag\"")
            .create_async()
            .await;

        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let existing = Event {
            reminders: vec![cal_core::Reminder {
                kind: cal_core::ReminderKind::Relative { minutes_before: 60 },
                sound: None,
            }],
            ..sample_existing_event(&cal_url)
        };
        let updated = update_event(&client(), existing, &creds(&server.url()), None)
            .await
            .unwrap();
        put.assert_async().await;
        assert_eq!(updated.etag.as_deref(), Some("\"new-etag\""));
    }

    /// The event as the caller holds it before an update: on the server
    /// already (so it carries an ETag), with no reminders of its own.
    fn sample_existing_event(cal_url: &Url) -> Event {
        Event {
            keep_attendees: false,
            clear_attendees: false,
            organized_elsewhere: false,
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
        }
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
            keep_attendees: false,
            clear_attendees: false,
            organized_elsewhere: false,
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
            keep_attendees: false,
            clear_attendees: false,
            organized_elsewhere: false,
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

        let merged = merge_with_overrides(body, &master_vcal, "https://example.com/cal/", |rid| {
            rid <= until
        })
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
    fn split_vevent_blocks_ignores_a_folded_begin_vevent_in_a_value() {
        // A DESCRIPTION whose folded continuation line reads "BEGIN:VEVENT" must
        // NOT be treated as a component boundary — folded lines carry a leading
        // space, so the master block keeps its whole value and stays one block.
        let body = "BEGIN:VCALENDAR\r\n\
BEGIN:VEVENT\r\nUID:a\r\nDESCRIPTION:hello\r\n BEGIN:VEVENT\r\n world\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:a\r\nRECURRENCE-ID:20260803T090000Z\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
        let blocks = split_vevent_blocks(body);
        assert_eq!(
            blocks.len(),
            2,
            "the folded BEGIN:VEVENT is not a boundary: {blocks:?}"
        );
        assert!(
            blocks[0].contains("DESCRIPTION:hello")
                && blocks[0].contains(" world")
                && blocks[0].contains(" BEGIN:VEVENT"),
            "master block keeps its whole folded value: {:?}",
            blocks[0]
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
            merge_with_overrides(body, "MASTER", "https://example.com/cal/", |rid| rid
                <= until),
            None,
        );
    }

    /// A zoned weekly series as an iPhone stores it: the zone, the master, and
    /// two overrides. The first stands in for the 15 June slot and was moved
    /// three days on; the second only got a new title.
    const SERIES_BODY: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
PRODID:-//Apple Inc.//iPhone OS 18//EN\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\n\
BEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nRRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\n\
TZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nTZNAME:CET\r\nEND:STANDARD\r\n\
BEGIN:DAYLIGHT\r\nDTSTART:19700329T020000\r\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\n\
TZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\nTZNAME:CEST\r\nEND:DAYLIGHT\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\n\
DTSTART;TZID=Europe/Berlin:20260608T090000\r\nDTEND;TZID=Europe/Berlin:20260608T093000\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\nSUMMARY:Weekly sync\r\n\
X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\n\
RECURRENCE-ID;TZID=Europe/Berlin:20260615T090000\r\n\
DTSTART;TZID=Europe/Berlin:20260618T140000\r\nDTEND;TZID=Europe/Berlin:20260618T143000\r\n\
SUMMARY:Moved far\r\n\
BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Event reminder\r\nTRIGGER:-PT15M\r\n\
UID:alarm-a\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\n\
RECURRENCE-ID;TZID=Europe/Berlin:20260622T090000\r\n\
DTSTART;TZID=Europe/Berlin:20260622T090000\r\nDTEND;TZID=Europe/Berlin:20260622T093000\r\n\
SUMMARY:Retitled\r\nX-CUSTOM-PROP:kept byte for byte\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    /// The 15 June slot of [`SERIES_BODY`]: 09:00 in Berlin, summer time.
    fn moved_slot() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 15, 7, 0, 0).unwrap()
    }

    /// The block of [`SERIES_BODY`] that starts with `marker`'s VEVENT.
    fn block_of<'a>(body: &'a str, marker: &str) -> &'a str {
        let at = body.find(marker).expect("marker in body");
        let from = body[..at].rfind("BEGIN:VEVENT").unwrap();
        let to = at + body[at..].find("END:VEVENT\r\n").unwrap() + "END:VEVENT\r\n".len();
        &body[from..to]
    }

    /// Mocks `GET` of the series resource with `body`, and a `PUT` of it that
    /// records what it was sent and must come `puts` times. Returns the
    /// recorder.
    async fn serve_series(
        server: &mut Server,
        body: &'static str,
        if_match: &str,
        puts: usize,
    ) -> (
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        mockito::Mock,
        mockito::Mock,
    ) {
        use std::sync::{Arc, Mutex};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let get = server
            .mock("GET", "/calendars/alice/work/series.ics")
            .with_status(200)
            .with_header("etag", "\"server-etag\"")
            .with_body(body)
            .create_async()
            .await;
        let sink = Arc::clone(&seen);
        let put = server
            .mock("PUT", "/calendars/alice/work/series.ics")
            .match_header("if-match", if_match)
            .expect(puts)
            .with_status(204)
            .with_header("etag", "\"new-etag\"")
            .with_body_from_request(move |request| {
                let body = request.utf8_lossy_body().unwrap().into_owned();
                sink.lock().unwrap().push(body);
                Vec::new()
            })
            .create_async()
            .await;
        (seen, get, put)
    }

    fn moved_override(cal_url: &Url) -> Event {
        Event {
            id: "/calendars/alice/work/series.ics|series-1::rid::2026-06-15T07:00:00Z".into(),
            title: "Moved again".into(),
            start: Utc.with_ymd_and_hms(2026, 6, 19, 12, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 6, 19, 12, 30, 0).unwrap(),
            reminders: vec![cal_core::Reminder {
                kind: cal_core::ReminderKind::Relative { minutes_before: 15 },
                sound: None,
            }],
            etag: Some("\"res-etag\"".into()),
            ..sample_existing_event(cal_url)
        }
    }

    #[tokio::test]
    async fn an_override_update_rewrites_its_own_block_and_nothing_else() {
        let mut server = Server::new_async().await;
        // The caller's ETag, as for a master: a change since the user last
        // looked still surfaces as a conflict.
        let (seen, get, put) = serve_series(&mut server, SERIES_BODY, "\"res-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();

        let updated = update_event(
            &client(),
            moved_override(&cal_url),
            &creds(&server.url()),
            None,
        )
        .await
        .unwrap();
        get.assert_async().await;
        put.assert_async().await;

        let sent = seen.lock().unwrap()[0].clone();
        let old = block_of(SERIES_BODY, "RECURRENCE-ID;TZID=Europe/Berlin:20260615");
        let from = SERIES_BODY.find(old).unwrap();
        let (before, after) = (&SERIES_BODY[..from], &SERIES_BODY[from + old.len()..]);
        // Everything but the one block goes back byte for byte: the zone, the
        // master, the other override.
        assert!(sent.starts_with(before), "{sent}");
        assert!(sent.ends_with(after), "{sent}");
        let block = &sent[before.len()..sent.len() - after.len()];
        assert!(
            block.starts_with(
                "BEGIN:VEVENT\r\nRECURRENCE-ID;TZID=Europe/Berlin:20260615T090000\r\n"
            ),
            "the slot is copied as the server spelled it: {block}"
        );
        assert!(
            block.contains("UID:series-1\r\n"),
            "the bare series UID: {block}"
        );
        assert!(block.contains("SUMMARY:Moved again"), "{block}");
        assert!(block.contains("DTSTART:20260619T120000Z"), "{block}");
        assert!(
            block.contains("UID:alarm-a"),
            "the alarm keeps its identity: {block}"
        );
        assert!(
            !block.contains("RRULE"),
            "one occurrence has no rule: {block}"
        );
        assert!(
            !sent.contains("::rid::"),
            "no row id reaches the server: {sent}"
        );
        assert_eq!(sent.matches("BEGIN:VEVENT").count(), 3, "{sent}");

        assert_eq!(
            updated.id,
            moved_override(&cal_url).id,
            "keeps its override id"
        );
        assert_eq!(updated.etag.as_deref(), Some("\"new-etag\""));
    }

    #[tokio::test]
    async fn an_override_update_is_refused_when_the_slot_holds_no_override() {
        // Someone deleted the exception since the row was read. The only write
        // left would be one over the whole series, so there is none.
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\nDTSTART:20260608T070000Z\r\nDTEND:20260608T073000Z\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\nSUMMARY:Weekly sync\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let mut server = Server::new_async().await;
        let (_seen, _get, put) = serve_series(&mut server, body, "\"res-etag\"", 0).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();

        let err = update_event(
            &client(),
            moved_override(&cal_url),
            &creds(&server.url()),
            None,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("refusing"), "{err}");
        put.assert_async().await;
    }

    #[tokio::test]
    async fn skipping_an_occurrence_drops_its_override_and_keeps_the_others() {
        let mut server = Server::new_async().await;
        // The fresh ETag: the body is merged on what was just read.
        let (seen, _get, put) = serve_series(&mut server, SERIES_BODY, "\"server-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();

        add_event_exdate(
            &client(),
            &cal_url,
            "/calendars/alice/work/series.ics|series-1",
            moved_slot(),
            &creds(&server.url()),
        )
        .await
        .unwrap();
        put.assert_async().await;

        let sent = seen.lock().unwrap()[0].clone();
        assert!(sent.contains("EXDATE:20260615T070000Z"), "{sent}");
        assert!(
            !sent.contains("Moved far") && !sent.contains("Europe/Berlin:20260615"),
            "the slot's own override goes, however far it was moved: {sent}"
        );
        let kept = block_of(SERIES_BODY, "RECURRENCE-ID;TZID=Europe/Berlin:20260622");
        assert!(
            sent.contains(kept),
            "the other override byte for byte: {sent}"
        );
        assert_eq!(
            sent.matches("TZID:Europe/Berlin\r\n").count(),
            1,
            "the zone once, not twice: {sent}"
        );
    }

    #[tokio::test]
    async fn a_kept_override_keeps_the_zone_it_names() {
        // A zone name `chrono-tz` does not know: the master goes back in UTC
        // and writes no zone of its own, but the override still names it.
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\nTZID:W. Europe Standard Time\r\n\
BEGIN:STANDARD\r\nDTSTART:16010101T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
END:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\nDTSTART:20260608T070000Z\r\nDTEND:20260608T073000Z\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\nSUMMARY:Weekly sync\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\nRECURRENCE-ID:20260622T070000Z\r\n\
DTSTART;TZID=W. Europe Standard Time:20260622T100000\r\n\
DTEND;TZID=W. Europe Standard Time:20260622T103000\r\n\
SUMMARY:Later\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let mut server = Server::new_async().await;
        let (seen, _get, put) = serve_series(&mut server, body, "\"server-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();

        add_event_exdate(
            &client(),
            &cal_url,
            "/calendars/alice/work/series.ics|series-1",
            moved_slot(),
            &creds(&server.url()),
        )
        .await
        .unwrap();
        put.assert_async().await;

        let sent = seen.lock().unwrap()[0].clone();
        let zone = &body[body.find("BEGIN:VTIMEZONE").unwrap()
            ..body.find("END:VTIMEZONE\r\n").unwrap() + "END:VTIMEZONE\r\n".len()];
        assert!(sent.contains(zone), "the zone byte for byte: {sent}");
        assert!(
            sent.find(zone).unwrap() < sent.find("BEGIN:VEVENT").unwrap(),
            "before the components that name it: {sent}"
        );
    }

    /// An invitation to one occurrence of somebody else's series: overrides,
    /// and no master.
    const ORPHAN_BODY: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\nRECURRENCE-ID:20260615T070000Z\r\n\
DTSTART:20260615T070000Z\r\nDTEND:20260615T073000Z\r\nSUMMARY:Invited once\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\nRECURRENCE-ID:20260622T070000Z\r\n\
DTSTART:20260622T070000Z\r\nDTEND:20260622T073000Z\r\nSUMMARY:Invited twice\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[tokio::test]
    async fn skipping_one_of_several_orphan_occurrences_keeps_the_rest() {
        let mut server = Server::new_async().await;
        let (seen, _get, put) = serve_series(&mut server, ORPHAN_BODY, "\"server-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();

        add_event_exdate(
            &client(),
            &cal_url,
            "/calendars/alice/work/series.ics|series-1",
            moved_slot(),
            &creds(&server.url()),
        )
        .await
        .unwrap();
        put.assert_async().await;

        let gone = block_of(ORPHAN_BODY, "SUMMARY:Invited once");
        assert_eq!(
            seen.lock().unwrap()[0],
            ORPHAN_BODY.replace(gone, ""),
            "the one block goes, the rest byte for byte"
        );
    }

    #[tokio::test]
    async fn skipping_the_last_orphan_occurrence_deletes_the_resource() {
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\nRECURRENCE-ID:20260615T070000Z\r\n\
DTSTART:20260615T070000Z\r\nDTEND:20260615T073000Z\r\nSUMMARY:Invited once\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
        let mut server = Server::new_async().await;
        let (_seen, _get, put) = serve_series(&mut server, body, "\"server-etag\"", 0).await;
        let delete = server
            .mock("DELETE", "/calendars/alice/work/series.ics")
            .match_header("if-match", "\"server-etag\"")
            .with_status(204)
            .create_async()
            .await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();

        add_event_exdate(
            &client(),
            &cal_url,
            "/calendars/alice/work/series.ics|series-1",
            moved_slot(),
            &creds(&server.url()),
        )
        .await
        .unwrap();
        delete.assert_async().await;
        put.assert_async().await;
    }

    #[test]
    fn recurrence_id_lines_keep_their_folding() {
        let block = "BEGIN:VEVENT\r\nUID:a\r\nRECURRENCE-ID;TZID=America/Argentina/\r\n \
Buenos_Aires:20260615T090000\r\nSUMMARY:x\r\nEND:VEVENT\r\n";
        assert_eq!(
            recurrence_id_lines(block).as_deref(),
            Some("RECURRENCE-ID;TZID=America/Argentina/\r\n Buenos_Aires:20260615T090000\r\n")
        );
        // A folded value that happens to read like the property is no property.
        let block = "BEGIN:VEVENT\r\nDESCRIPTION:see\r\n RECURRENCE-ID:x\r\nEND:VEVENT\r\n";
        assert_eq!(recurrence_id_lines(block), None);
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
