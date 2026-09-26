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
use chrono::{DateTime, Local, Utc};
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
use crate::ical_raw::{
    component_lines, fold, insert_after_head, insert_before_end, line_ending, unfold,
    without_ranges,
};
use crate::identity::OwnIdentity;
use crate::mapping::{
    decode_event_id, event_to_ical_preserving, format_utc_compact, mint_recurrence_id,
    new_event_to_ical, override_recurrence_id, override_to_vevent, parse_calendar_data_as,
    same_calendar_user, strip_mailto_scheme, PriorAlarms, SlotRefusal,
};
use crate::scheduling::{
    apply_to_block, attendee_copy, attendee_line, invitees_of, plan_block, PeopleChange, WriteCtx,
};
use crate::xml::{parse_multistatus, ResponseEntry};
use cal_core::attendee::same_address;
use cal_core::event_diff::changed_fields;
use cal_core::invitation::{reply_only_verdict, ReplyOnlyVerdict};
use cal_core::{Reminder, ReminderKind, WriteRefusal};
use std::ops::Range;

/// The `calendar-query` REPORT for every event in `range` of the calendar
/// collection at `calendar_url`: one entry per resource, with its href, ETag
/// and calendar-data. [`events_from_entries`] maps them; the adapter first
/// looks whether any of them names an ORGANIZER, because only then does it
/// need the account's own addresses.
pub async fn report_events(
    client: &Client,
    calendar_url: &Url,
    range: DateRange,
    credentials: &Credentials,
) -> CaldavResult<Vec<ResponseEntry>> {
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
    parse_multistatus(&text)
}

/// The events of REPORT or multiget `entries`, for the account `own`, one
/// per VEVENT, each with the `calendar_id` stamped to `calendar_id` and the
/// server's ETag so the write layer can use If-Match.
pub fn events_from_entries(
    entries: Vec<ResponseEntry>,
    calendar_id: &str,
    own: &OwnIdentity,
) -> CaldavResult<Vec<Event>> {
    let mut out = Vec::new();
    for entry in entries {
        let Some(ical) = entry.calendar_data else {
            continue;
        };
        let mut events = parse_calendar_data_as(&ical, calendar_id, Some(&entry.href), own)?;
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
///
/// A new event becomes a meeting only on a server that schedules, when the
/// user notifies and someone other than the account is invited: then it
/// names the account as its ORGANIZER and every invitee in a row, and the
/// server sends the invitations. Otherwise it is a plain appointment. The
/// event returned names the invitees that were written, and nobody else.
pub async fn create_event(
    client: &Client,
    calendar_url: &Url,
    event: NewEvent,
    credentials: &Credentials,
    ctx: &WriteCtx,
) -> CaldavResult<Event> {
    let uid = format!("{}@aperio", Uuid::new_v4());
    let resource = resource_url(calendar_url, &uid)?;
    let (body, written, organizer) = new_event_body(&uid, &event, ctx);

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
        check_write(response).await?
    };
    let now = Utc::now();

    Ok(Event {
        keep_attendees: false,
        keep_fields: Vec::new(),
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
        attendees: written,
        created_at: now,
        updated_at: now,
        etag,
        organizer,
        attendee_responses: Vec::new(),
        // Freshly created by us — never a cancellation.
        cancelled: false,
        scheduling_silenced: false,
    })
}

/// A new event's body, the invitees it names, and its organizer as Aperio
/// shows it when it is a meeting.
fn new_event_body(
    uid: &str,
    event: &NewEvent,
    ctx: &WriteCtx,
) -> (String, Vec<String>, Option<String>) {
    let vcal = new_event_to_ical(uid, event);
    let own = ctx.identity.clone().unwrap_or_default();
    let invitees = invitees_of(&event.attendees, &own);
    match ctx.organizer_address.as_deref() {
        Some(organizer) if ctx.schedules && event.send_invitations && !invitees.is_empty() => {
            let ending = line_ending(&vcal);
            let mut lines = fold(&format!("ORGANIZER:{organizer}"), ending);
            for entry in &invitees {
                lines.push_str(&attendee_line(entry, ending));
            }
            let shown = strip_mailto_scheme(organizer)
                .unwrap_or(organizer)
                .trim()
                .to_string();
            (splice_into_vevent(&vcal, &lines), invitees, Some(shown))
        }
        _ => (vcal, Vec::new(), None),
    }
}

/// `vcal` with `lines` inserted at the head of its first VEVENT.
fn splice_into_vevent(vcal: &str, lines: &str) -> String {
    if lines.is_empty() {
        return vcal.to_string();
    }
    let Some(range) = component_ranges(vcal, "VEVENT").into_iter().next() else {
        return vcal.to_string();
    };
    format!(
        "{}{}{}",
        &vcal[..range.start],
        insert_after_head(&vcal[range.clone()], lines),
        &vcal[range.end..]
    )
}

/// Update an existing event.
///
/// Every update reads the server's copy of the resource first and writes the
/// whole resource back: the event rebuilt from core fields, with its alarms
/// kept ([`PriorAlarms`]) and its meeting lines carried through as the
/// server's own text ([`crate::scheduling`]), and every other component byte
/// for byte. A read that fails refuses the save: without the copy, the PUT
/// would drop what it cannot see, a meeting's guests above all. A save that
/// would change nothing the server stores is not sent: on a scheduling
/// server it would mail every guest.
///
/// If-Match carries the caller's ETag when there is one: the question is
/// whether anything changed since the user last saw it.
///
/// An override id (`{href}|{uid}::rid::{slot}`) writes that one occurrence
/// and nothing else, see [`update_override`]. "This and all following"
/// (`truncate_tail_overrides` on a timed series with an UNTIL) drops the
/// overrides past the cutoff. An ALL-DAY series keeps them all: its DATE-only
/// UNTIL and its local-midnight RECURRENCE-IDs cannot be compared exactly, so
/// their cross-client cleanup stays a known gap rather than a guess.
pub async fn update_event(
    client: &Client,
    event: Event,
    credentials: &Credentials,
    ctx: &WriteCtx,
) -> CaldavResult<Event> {
    // First, and never falling through: the master path below would write the
    // occurrence over its whole series' resource.
    if let Some((series_id, slot)) =
        cal_core::split_override_id(&event.id).map_err(|e| CaldavError::Config(e.to_string()))?
    {
        let series_id = series_id.to_string();
        return update_override(client, event, &series_id, slot, credentials, ctx).await;
    }
    let cutoff = if event.truncate_tail_overrides && !event.all_day {
        event
            .recurrence
            .as_ref()
            .and_then(|r| rrule_until_instant(&r.rrule))
    } else {
        None
    };
    update_master(client, event, cutoff, credentials, ctx).await
}

/// Write a single event or a series master back into its resource; see
/// [`update_event`]. `cutoff` drops the overrides after it.
async fn update_master(
    client: &Client,
    event: Event,
    cutoff: Option<DateTime<Utc>>,
    credentials: &Credentials,
    ctx: &WriteCtx,
) -> CaldavResult<Event> {
    let cal_url = Url::parse(&event.calendar_id)
        .map_err(|e| CaldavError::Config(format!("event.calendar_id is not a URL: {e}")))?;
    let resource = resource_url_for_event(&cal_url, &event.id)?;
    let (body, server_etag) = get_resource(client, &resource, credentials).await?;
    // Nothing is written, and the caller must know it for certain: splitting a
    // series takes its new part back only after a refusal like this one
    // (decision 144), so it travels as one, not as a protocol error.
    let refused = |token: &str, why: &str| {
        tracing::warn!(event_id = %event.id, why, "refusing to write the event; nothing was saved");
        CaldavError::Forbidden(WriteRefusal::UnsafeToWrite.message(token))
    };
    let own = ctx.identity.clone().unwrap_or_default();
    let blocks = vevent_blocks(&body, cal_url.as_str(), &own).ok_or_else(|| {
        refused(
            "unreadable-blocks",
            "its resource cannot be read block by block",
        )
    })?;
    let (_, uid) = decode_event_id(&event.id);
    let master = blocks
        .iter()
        .find(|b| {
            override_recurrence_id(&b.event.id).is_none() && decode_event_id(&b.event.id).1 == uid
        })
        .ok_or_else(|| refused("no-such-event", "its resource holds no such event"))?;
    if !one_organizer(&body, &blocks) {
        return Err(refused(
            "mixed-organizers",
            "its components name different organizers",
        ));
    }
    if ctx.schedules && attendee_copy(&body[master.range.clone()], ctx)? {
        return write_attendee_copy(
            client,
            &resource,
            &body,
            master.range.clone(),
            &master.event,
            event,
            server_etag,
            credentials,
            ctx,
        )
        .await;
    }
    let plan = plan_block(&body[master.range.clone()], &event, ctx)?;
    if cutoff.is_none()
        && plan.change == PeopleChange::Keep
        && changed_fields(&event, &master.event).is_empty()
    {
        return Ok(Event {
            etag: server_etag.or(event.etag.clone()),
            ..event
        });
    }
    let master_vcal = splice_into_vevent(
        &event_to_ical_preserving(&event, PriorAlarms::read(&body)),
        &plan.lines,
    );
    let new_body = merge_with_overrides(
        &body,
        &master_vcal,
        cal_url.as_str(),
        |rid| cutoff.is_none_or(|until| rid <= until),
        |block| apply_to_block(block, &plan.change),
    )
    .ok_or_else(|| {
        refused(
            "unreadable-blocks",
            "its resource cannot be read block by block",
        )
    })?;
    let if_match = event.etag.as_deref().or(server_etag.as_deref());
    let new_etag = put_resource(client, &resource, new_body, if_match, credentials).await?;

    Ok(Event {
        etag: new_etag.or(server_etag).or(event.etag.clone()),
        updated_at: Utc::now(),
        ..event
    })
}

/// Write the account's own copy of someone else's meeting (decision 77a).
///
/// On a scheduling server an attendee may change its reply and its own alarms
/// and nothing else (RFC 6638 §3.2.2.1). So this writes the server's own block
/// back byte for byte with only its VALARMs replaced: the organizer's lines,
/// the zone spelling, `SEQUENCE`, `STATUS`, Apple's own properties and every
/// line Aperio does not model stay exactly as the organizer wrote them.
///
/// A change to anything else is refused before any PUT — unless the caller
/// edited a copy this account never saw, which makes it the organizer's change
/// and a conflict (see [`cal_core::invitation::reply_only_verdict`]).
#[allow(clippy::too_many_arguments)]
async fn write_attendee_copy(
    client: &Client,
    resource: &Url,
    body: &str,
    block: Range<usize>,
    fresh: &Event,
    mut edit: Event,
    server_etag: Option<String>,
    credentials: &Credentials,
    ctx: &WriteCtx,
) -> CaldavResult<Event> {
    // A colour the server sent back is not a change the account made when the
    // account cannot store one there anyway (iCloud keeps it on the device).
    if !ctx.writes_color {
        edit.color_hex = fresh.color_hex.clone();
    }
    let same_version = edit.etag.is_some() && edit.etag == server_etag;
    match reply_only_verdict(&edit, fresh, same_version) {
        ReplyOnlyVerdict::Refused(field) => {
            return Err(CaldavError::Forbidden(
                WriteRefusal::ReplyOnlyInvitation.message(field.token()),
            ));
        }
        ReplyOnlyVerdict::Stale => {
            // The organizer changed the meeting between the read and this
            // save. Telling the user they may not change it would name the
            // wrong person and the wrong act.
            return Err(CaldavError::Http {
                status: 412,
                message: "the meeting changed on the server since it was read".into(),
            });
        }
        ReplyOnlyVerdict::Allowed => {}
    }
    let server_block = &body[block.clone()];
    let summary = component_lines(server_block)
        .iter()
        .find(|line| line.name == "SUMMARY")
        .map(|line| line.value.clone())
        .unwrap_or_default();
    let Some(new_block) = replace_alarms(server_block, &edit.reminders, &summary) else {
        // Every PUT of a scheduling object reaches the organizer's server.
        return Ok(Event {
            etag: server_etag.or(edit.etag.clone()),
            ..edit
        });
    };
    // If-Match is the CALLER's version, as on every other write: the alarms
    // are spliced into the copy just read, but the reminders being written
    // are the ones the caller saw, so writing them over a copy somebody else
    // changed would drop that change. `reply_only_verdict` lets a reminders-
    // only edit through whatever the version, because reminders are the
    // attendee's own — which is exactly why the ETag has to guard them here.
    let if_match = edit.etag.as_deref().or(server_etag.as_deref());
    let new_body = replace_ranges(body, vec![(block, new_block)]);
    let new_etag = put_resource(client, resource, new_body, if_match, credentials).await?;
    Ok(Event {
        etag: new_etag.or(server_etag).or(edit.etag.clone()),
        updated_at: Utc::now(),
        ..edit
    })
}

/// `block` with its VALARMs replaced by `wanted`, or `None` when that changes
/// nothing.
///
/// Per alarm on the server, read as the user saw it
/// ([`crate::mapping::raw_alarm_reminder`]):
///
/// - a reminder the user still wants is **kept byte for byte**, with its UID,
///   its `X-APPLE-DEFAULT-ALARM` mark and everything else it carries;
/// - one they removed is dropped;
/// - an alarm Aperio never showed is kept: an edit here never deletes what it
///   could not display;
/// - a reminder with no alarm left to claim is rendered, with the SERVER's
///   summary, in the block's own line ending.
///
/// When alarms were added or removed, the event-level `X-APPLE-DEFAULT-ALARM`
/// goes: the alarms are no longer the account's default set. That is the rule
/// the rebuilding write follows too (`mapping::apply_common`).
fn replace_alarms(block: &str, wanted: &[Reminder], server_summary: &str) -> Option<String> {
    let ending = line_ending(block);
    let mut unclaimed: Vec<&Reminder> = wanted
        .iter()
        .filter(|r| !matches!(r.kind, ReminderKind::AppStart))
        .collect();
    let mut drop: Vec<Range<usize>> = Vec::new();
    for range in component_ranges(block, "VALARM") {
        let Some(shown) = crate::mapping::raw_alarm_reminder(&block[range.clone()]) else {
            continue;
        };
        match unclaimed.iter().position(|r| r.kind == shown.kind) {
            // Claimed: the server's own text stays.
            Some(index) => {
                unclaimed.remove(index);
            }
            None => drop.push(range),
        }
    }
    let added: String = unclaimed
        .iter()
        .filter_map(|reminder| render_alarm(reminder, server_summary, ending))
        .collect();
    if drop.is_empty() && added.is_empty() {
        return None;
    }
    // The block's own default-alarm mark, if it has one.
    if let Some(line) = component_lines(block)
        .iter()
        .find(|line| line.name == "X-APPLE-DEFAULT-ALARM")
    {
        drop.push(line.range.clone());
    }
    Some(insert_before_end(&without_ranges(block, &drop), &added))
}

/// One VALARM as text, in `ending`. `None` for a reminder no provider stores
/// (an app-start reminder).
fn render_alarm(reminder: &Reminder, summary: &str, ending: &str) -> Option<String> {
    use icalendar::{Component, EventLike};
    let alarm = crate::mapping::reminder_to_alarm(reminder, summary)?;
    let mut carrier = icalendar::Event::new();
    carrier.uid("alarm-carrier").alarm(alarm);
    let rendered = icalendar::Calendar::new()
        .push(carrier.done())
        .done()
        .to_string();
    let range = component_ranges(&rendered, "VALARM").into_iter().next()?;
    Some(
        rendered[range]
            .split_inclusive('\n')
            // The serialiser stamps every component it writes, but a VALARM
            // has no DTSTAMP (RFC 5545 §3.6.6), and a scheduling server may
            // refuse a property that does not belong there.
            .filter(|line| {
                !line
                    .trim_start()
                    .to_ascii_uppercase()
                    .starts_with("DTSTAMP")
            })
            .map(|line| format!("{}{ending}", line.trim_end_matches(['\r', '\n'])))
            .collect(),
    )
}

/// Whether the components of a resource name one organizer at most. RFC 6638
/// §3.2.4.2 wants every component of a scheduling object to name the same
/// one; a resource that does not is not written.
fn one_organizer(body: &str, blocks: &[VeventBlock]) -> bool {
    let mut seen: Option<String> = None;
    for block in blocks {
        for line in component_lines(&body[block.range.clone()]) {
            if line.name != "ORGANIZER" {
                continue;
            }
            match &seen {
                Some(first) if !same_calendar_user(first, &line.value) => return false,
                Some(_) => {}
                None => seen = Some(line.value.clone()),
            }
        }
    }
    true
}

/// Write one changed occurrence, `event`, into its series' resource.
///
/// The server keeps a changed occurrence as an override: a VEVENT of its own in
/// the series' resource, with the series' UID and a RECURRENCE-ID that names the
/// slot. A PUT replaces the whole resource, so this reads the resource, swaps
/// that one block for the rewritten occurrence, and puts everything else back
/// byte for byte: the master, the other overrides, the time zones. The
/// occurrence's own meeting lines are carried as for a master.
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
    ctx: &WriteCtx,
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
    let own = ctx.identity.clone().unwrap_or_default();
    let blocks = vevent_blocks(&body, cal_url.as_str(), &own)
        .ok_or_else(|| refused("its resource cannot be read block by block"))?;
    let (_, uid) = decode_event_id(series_id);
    let block = blocks.iter().find(|b| {
        override_recurrence_id(&b.event.id) == Some(slot)
            && cal_core::series_master_id(&b.event.id) == uid
    });
    let Some(block) = block else {
        // Nothing stands in the slot yet, so this save is the first change to
        // that one occurrence: write the exception the series does not have
        // (decision 79b). Until now every such save carved the occurrence out
        // of its series instead.
        return mint_override(
            client,
            &resource,
            &body,
            &blocks,
            event,
            uid,
            series_id,
            slot,
            server_etag,
            credentials,
            ctx,
        )
        .await;
    };
    let server_block = &body[block.range.clone()];
    if ctx.schedules && attendee_copy(server_block, ctx)? {
        return write_attendee_copy(
            client,
            &resource,
            &body,
            block.range.clone(),
            &block.event,
            event,
            server_etag,
            credentials,
            ctx,
        )
        .await;
    }
    let plan = plan_block(server_block, &event, ctx)?;
    if plan.change == PeopleChange::Keep && changed_fields(&event, &block.event).is_empty() {
        return Ok(Event {
            etag: server_etag.or(event.etag.clone()),
            ..event
        });
    }
    let recurrence_id = recurrence_id_lines(server_block)
        .ok_or_else(|| refused("its block carries no RECURRENCE-ID"))?;
    let rendered = override_to_vevent(
        &event,
        series_id,
        &recurrence_id,
        PriorAlarms::read_occurrence(&body, slot),
    );
    let new_body = format!(
        "{}{}{}",
        &body[..block.range.start],
        insert_after_head(&rendered, &plan.lines),
        &body[block.range.end..]
    );
    let if_match = event.etag.as_deref().or(server_etag.as_deref());
    let new_etag = put_resource(client, &resource, new_body, if_match, credentials).await?;

    Ok(Event {
        etag: new_etag.or(event.etag.clone()),
        updated_at: Utc::now(),
        ..event
    })
}

/// Add an exception for `slot` to the series' own resource: the write behind
/// "only this occurrence" where the provider can hold one (decision 79b).
///
/// A sibling of [`update_override`], not a branch inside it: that one swaps a
/// block the server already has, this one adds a block the resource has never
/// carried. They share the GET and the PUT and nothing else.
///
/// Refuses rather than writing something else. Every refusal here means the
/// same thing to the user — this occurrence could not be saved on its own, and
/// nothing was changed — so the reason travels as a machine token for the log
/// (decision 92: the carve-out is never done behind the user's back).
#[allow(clippy::too_many_arguments)]
async fn mint_override(
    client: &Client,
    resource: &Url,
    body: &str,
    blocks: &[VeventBlock],
    event: Event,
    uid: &str,
    series_id: &str,
    slot: DateTime<Utc>,
    server_etag: Option<String>,
    credentials: &Credentials,
    ctx: &WriteCtx,
) -> CaldavResult<Event> {
    let refused =
        |why: &str| CaldavError::Forbidden(WriteRefusal::OccurrenceNotWritable.message(why));
    let master = blocks
        .iter()
        .find(|b| {
            override_recurrence_id(&b.event.id).is_none()
                && cal_core::series_master_id(&b.event.id) == uid
        })
        .ok_or_else(|| refused("no-master"))?;
    let master_block = &body[master.range.clone()];
    // An exception belongs to a RULE. Without one there is no occurrence to
    // stand in for, and the resource would grow a second event under one UID.
    if master.event.recurrence.is_none() {
        return Err(refused("not-recurring"));
    }
    // The occurrence was skipped; an exception would bring back a day the user
    // deleted. `add_event_exdate` is the other direction and stays that way.
    if master
        .event
        .recurrence
        .as_ref()
        .is_some_and(|r| r.exceptions.contains(&slot))
    {
        return Err(refused("already-skipped"));
    }
    // Someone else's meeting: RFC 6638 takes only this account's own reply and
    // alarms (decision 77a), and an exception is neither.
    if ctx.schedules && attendee_copy(master_block, ctx)? {
        return Err(CaldavError::Forbidden(
            WriteRefusal::ReplyOnlyInvitation.message("occurrence"),
        ));
    }
    let plan = plan_block(master_block, &event, ctx)?;
    // Inviting people to ONE occurrence of a series that has none would give
    // the new block an ORGANIZER the master lacks, and a resource whose blocks
    // name different organizers is one `one_organizer` refuses to write again.
    if matches!(plan.change, PeopleChange::Invite { .. }) {
        return Err(refused("would-invite"));
    }
    let ending = line_ending(master_block);
    let recurrence_id = mint_recurrence_id(master_block, slot, ending).map_err(|why| {
        refused(match why {
            SlotRefusal::NoStart => "master-without-start",
            SlotRefusal::UnknownZone(_) => "unknown-zone",
            SlotRefusal::AmbiguousLocalTime => "ambiguous-local-time",
        })
    })?;
    // A new block carries no alarms of its own yet: the reminders the edit
    // holds are rendered fresh, and the master's VALARMs stay the master's.
    let rendered = override_to_vevent(&event, series_id, &recurrence_id, PriorAlarms::default());
    let new_body = insert_before_end(body, &insert_after_head(&rendered, &plan.lines));
    let if_match = event.etag.as_deref().or(server_etag.as_deref());
    let new_etag = put_resource(client, resource, new_body, if_match, credentials).await?;
    Ok(Event {
        // The id the next read will mint for this block, so the row the
        // surfaces hold after the save is the row they would load.
        id: format!(
            "{series_id}{}{}",
            cal_core::OVERRIDE_ID_MARKER,
            slot.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ),
        etag: new_etag.or(server_etag).or(event.etag.clone()),
        updated_at: Utc::now(),
        recurrence: None,
        ..event
    })
}

/// PUT `body` over `resource`, guarded by `if_match` when given. Returns the
/// new ETag, when the server sent one.
async fn put_resource(
    client: &Client,
    resource: &Url,
    body: String,
    if_match: Option<&str>,
    credentials: &Credentials,
) -> CaldavResult<Option<String>> {
    let mut headers = auth_header(credentials)?;
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    if let Some(tag) = if_match {
        let value = HeaderValue::from_str(tag).map_err(|e| CaldavError::Config(e.to_string()))?;
        headers.insert(IF_MATCH, value);
    }
    let (response, first_may_have_landed) = client
        .put(resource.clone())
        .headers(headers)
        .body(body)
        .send_retrying_marked()
        .await?;
    // Any refusal of a replay whose first attempt may have landed: the
    // connection died after the first PUT went out, and the answer may be
    // about that one — a 412 because its new ETag refuses the replay, a 429 or
    // a 507 because it was taken. Read as a refusal, a caller would undo
    // around a write that went through: splitting a series deleted its new
    // part while the old part was already cut short (decision 144). So it is
    // what it is, unsure.
    if first_may_have_landed && !response.status().is_success() {
        // The answer itself is kept for the log: it may be the first
        // attempt's, or a refusal the replay met on its own.
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!(
            %resource,
            status,
            body = %body.chars().take(200).collect::<String>(),
            "a replayed PUT was answered with a failure; the first attempt may have been saved",
        );
        return Err(CaldavError::Network(format!(
            "the connection to '{resource}' broke after the change was sent \
             (the replay was answered HTTP {status}); it may have been saved"
        )));
    }
    check_write(response).await
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
fn vevent_blocks(body: &str, calendar_id: &str, own: &OwnIdentity) -> Option<Vec<VeventBlock>> {
    let parsed = parse_calendar_data_as(body, calendar_id, None, own).ok()?;
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

/// Rebuild a resource around a rewritten master: `master_vcal` (a VCALENDAR
/// holding just the new master, from `event_to_ical`) with the RECURRENCE-ID
/// overrides of `body` that `keep` accepts spliced back in, each through
/// `transform`, and the others dropped. A "this and all following" truncation
/// keeps the overrides up to its cutoff; `transform` carries a change to the
/// meeting's guests into every occurrence (see `scheduling::apply_to_block`).
///
/// Overrides are identified from the MAPPED events (so a `TZID`/all-day
/// RECURRENCE-ID is zone-resolved correctly) but their raw VEVENT text is kept
/// byte-for-byte apart from what `transform` changes, so per-instance edits /
/// VALARMs / X-props survive intact. A kept override may name a zone the new
/// master does not, so every VTIMEZONE of `body` that `master_vcal` lacks comes
/// along with them, byte-for-byte too. Returns `None` when the parsed events
/// and raw blocks don't correspond 1:1 (an unmappable VEVENT, an odd shape).
fn merge_with_overrides(
    body: &str,
    master_vcal: &str,
    calendar_id: &str,
    keep: impl Fn(DateTime<Utc>) -> bool,
    transform: impl Fn(&str) -> String,
) -> Option<String> {
    // Only the RECURRENCE-IDs matter here, not who organizes.
    let blocks = vevent_blocks(body, calendar_id, &OwnIdentity::default())?;
    let kept: Vec<String> = blocks
        .iter()
        .filter_map(|b| match override_recurrence_id(&b.event.id) {
            // Master VEVENT → replaced by the new master in `master_vcal`.
            None => None,
            Some(rid) if keep(rid) => Some(transform(&body[b.range.clone()])),
            Some(_) => None,
        })
        .collect();
    if kept.is_empty() {
        return Some(master_vcal.to_string());
    }
    let refs: Vec<&str> = kept.iter().map(String::as_str).collect();
    let merged = splice_overrides_before_end(master_vcal, &refs);
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
/// done the work or should keep looking in the next calendar
/// ([`DeleteWalk`] holds their rule).
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
    /// Server returned 404 to a REPLAYED delete whose first attempt may
    /// have reached it: the first one may have removed the resource, or it
    /// was never at this URL. A direct caller reads it as gone. A walker
    /// keeps looking — a bare id names a different URL in every calendar —
    /// and counts it as done only if no calendar holds the event after all.
    GoneOnReplay,
}

/// Where a walk over every calendar (or task list) stands, DELETE by
/// DELETE: the rule both home-set walkers in `lib.rs` follow.
#[derive(Debug, Default)]
pub(crate) struct DeleteWalk {
    gone_on_replay: bool,
    last_err: Option<CaldavError>,
}

impl DeleteWalk {
    /// Take one calendar's outcome. `true` when the walk is done: the event
    /// was removed there.
    pub(crate) fn step(&mut self, outcome: CaldavResult<DeleteOutcome>) -> bool {
        match outcome {
            Ok(DeleteOutcome::Deleted) => return true,
            Ok(DeleteOutcome::NotFound) => {}
            Ok(DeleteOutcome::GoneOnReplay) => self.gone_on_replay = true,
            // Non-404 errors might be transient (auth hiccup, server
            // hiccup). Remember the last one in case nothing else works,
            // but keep walking — the resource might still live in another
            // calendar we haven't tried yet.
            Err(err) => self.last_err = Some(err),
        }
        false
    }

    /// The walk found no calendar that removed it. A replayed 404 counts as
    /// gone when no calendar failed: the first DELETE may have removed it,
    /// and none of the others holds it. `missing` names it otherwise.
    pub(crate) fn finish(self, missing: String) -> Result<(), CaldavError> {
        match self.last_err {
            Some(err) => Err(err),
            None if self.gone_on_replay => Ok(()),
            None => Err(CaldavError::Http {
                status: 404,
                message: missing,
            }),
        }
    }
}

/// Delete an event from the server. `event_id` is the UID; the URL
/// is reconstructed as `<calendar_url>/<uid>.ics`. When the caller
/// passes an `etag`, an `If-Match` header is added so the server
/// refuses to delete a row that has changed under it.
///
/// 404 is treated as a non-error outcome (`DeleteOutcome::NotFound`, or
/// `GoneOnReplay` when the first attempt may have removed it) —
/// idempotent semantics for "make sure this row is gone". The home-set
/// walker uses the typed outcome to keep searching past 404s for the
/// calendar that actually owns the resource.
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
    let (response, first_may_have_landed) = client
        .delete(resource.clone())
        .headers(headers)
        .send_retrying_marked()
        .await?;
    if response.status() == StatusCode::NOT_FOUND {
        // On a replay, the first DELETE may have removed it before the
        // connection broke. Read as "not here", every calendar answered 404
        // and the event was reported found nowhere: undoing a split's new
        // part said it could not. But it may just as well never have been
        // here, so the walker keeps looking (`DeleteWalk`).
        return Ok(if first_may_have_landed {
            DeleteOutcome::GoneOnReplay
        } else {
            DeleteOutcome::NotFound
        });
    }
    check_write(response).await?;
    Ok(DeleteOutcome::Deleted)
}

/// Skip one occurrence of a recurring series: add `occurrence` to the
/// master's EXDATEs, and drop the override that stands in that slot if there
/// is one. An override moved far from its slot is found all the same: it is
/// recognised by its RECURRENCE-ID, the slot, not by where it now starts.
///
/// Nothing else of the resource changes. The EXDATE is inserted as one raw
/// line into the master's own text, so its alarms, its meeting lines and
/// everything Aperio does not model stay as the server wrote them. On the
/// organizer's copy of a meeting an RFC 6638 server then cancels that one
/// occurrence for its guests (§3.2.1.2); on an attendee's copy it declines
/// it. A resource that cannot be read block by block is not written.
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
    let blocks =
        vevent_blocks(&body, calendar_url.as_str(), &OwnIdentity::default()).ok_or_else(|| {
            CaldavError::Protocol(format!(
            "event '{event_id}': its resource cannot be read block by block; nothing was changed"
        ))
        })?;
    let in_slot = blocks.iter().find(|b| {
        override_recurrence_id(&b.event.id) == Some(occurrence)
            && cal_core::series_master_id(&b.event.id) == want_uid
    });
    let master = blocks.iter().find(|b| {
        override_recurrence_id(&b.event.id).is_none() && decode_event_id(&b.event.id).1 == want_uid
    });
    let new_body = match master {
        Some(master) => {
            let Some(recurrence) = master.event.recurrence.as_ref() else {
                return Err(CaldavError::Discovery(format!(
                    "event '{event_id}' is not recurring"
                )));
            };
            let excluded = recurrence.exceptions.contains(&occurrence);
            if excluded && in_slot.is_none() {
                // Skipped already and nothing stands in the slot: nothing to
                // write, so nothing is sent (no scheduling message either).
                return Ok(());
            }
            let master_text = &body[master.range.clone()];
            let new_master = if excluded {
                master_text.to_string()
            } else {
                let ending = line_ending(master_text);
                insert_after_head(
                    master_text,
                    &format!("{}{ending}", exdate_line(master_text, occurrence)),
                )
            };
            let mut edits = vec![(master.range.clone(), new_master)];
            if let Some(block) = in_slot {
                edits.push((block.range.clone(), String::new()));
            }
            replace_ranges(&body, edits)
        }
        None => {
            let Some(block) = in_slot else {
                return Err(CaldavError::Discovery(format!(
                    "event '{event_id}' missing from its own resource"
                )));
            };
            if blocks.len() == 1 {
                return delete_resource(client, &resource, etag.as_deref(), credentials).await;
            }
            replace_ranges(&body, vec![(block.range.clone(), String::new())])
        }
    };
    put_resource(client, &resource, new_body, etag.as_deref(), credentials).await?;
    Ok(())
}

/// The `EXDATE` line that excludes `occurrence` from `master`.
///
/// An exception must be written the way the series names its occurrences
/// (RFC 5545 §3.8.5.1): a series whose `DTSTART` is a date is excluded by a
/// date, and a UTC date-time excludes nothing there — the occurrence comes
/// back on the next read, and on a meeting the promised cancellation is never
/// sent. An all-day start is local midnight as an instant, so its date is the
/// date it shows.
fn exdate_line(master: &str, occurrence: DateTime<Utc>) -> String {
    let all_day = component_lines(master).iter().any(|line| {
        line.name == "DTSTART"
            && line
                .param("VALUE")
                .is_some_and(|value| value.eq_ignore_ascii_case("DATE"))
    });
    if all_day {
        // An all-day instant is local midnight (`reference_allday_local_midnight`),
        // so its day is the local one: in Berlin the 16th is stored as the
        // 15th at 23:00 UTC, and `EXDATE;VALUE=DATE:20261115` would exclude
        // nothing at all — the bug this line exists to fix.
        format!(
            "EXDATE;VALUE=DATE:{}",
            occurrence
                .with_timezone(&Local)
                .date_naive()
                .format("%Y%m%d")
        )
    } else {
        format!("EXDATE:{}", format_utc_compact(occurrence))
    }
}

/// `body` with each range replaced by its text. The ranges must not overlap.
fn replace_ranges(body: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(range, _)| range.start);
    let mut out = String::with_capacity(body.len());
    let mut from = 0;
    for (range, text) in edits {
        out.push_str(&body[from..range.start]);
        out.push_str(&text);
        from = range.end;
    }
    out.push_str(&body[from..]);
    out
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
    check_write(response).await?;
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
    check_write(put).await?;
    Ok(())
}

/// Surgically set `PARTSTAT` on the `ATTENDEE` line whose value matches
/// `email`, leaving every other line (and its folding) untouched.
/// Returns `None` when no matching attendee is present. Only the edited
/// ATTENDEE line is unfolded — typical attendee lines fit one physical
/// line, so we emit it unfolded and pass everything else through
/// verbatim.
fn set_self_partstat(body: &str, email: &str, partstat: &str) -> Option<String> {
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for component in component_ranges(body, "VEVENT") {
        let block = &body[component.clone()];
        let ending = line_ending(block);
        for line in component_lines(block) {
            // Only the event's own rows: an `ACTION:EMAIL` alarm carries
            // ATTENDEE lines of its own, and they have no PARTSTAT to set
            // (RFC 5545 §3.6.6). `component_lines` skips nested components.
            //
            // And only the row that NAMES this address: a substring match
            // would also flip `atoni@x` when answering as `toni@x`. A row
            // written as a principal path carries the address in its EMAIL
            // parameter, which is how iCloud writes it.
            fn address(value: &str) -> &str {
                strip_mailto_scheme(value).unwrap_or(value).trim()
            }
            let names_self = same_address(address(&line.value), address(email))
                || line
                    .param("EMAIL")
                    .is_some_and(|param| same_address(address(param), address(email)));
            if line.name != "ATTENDEE" || !names_self {
                continue;
            }
            let text = &block[line.range.clone()];
            let rewritten = fold(&replace_partstat(&unfold(text), partstat), ending);
            let at = component.start + line.range.start;
            edits.push((at..component.start + line.range.end, rewritten));
        }
    }
    (!edits.is_empty()).then(|| replace_ranges(body, edits))
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

/// The outcome of a write (PUT or DELETE), reading the body only when it
/// failed. Returns the new ETag on success. A 403 is the server refusing this
/// change, not the credentials, which fail with a 401 (RFC 6638 names its
/// preconditions in a `DAV:error` body, e.g. §3.2.2.1
/// `allowed-attendee-scheduling-object-change`). Any other failure keeps the
/// start of the server's text.
async fn check_write(response: reqwest::Response) -> CaldavResult<Option<String>> {
    let status = response.status();
    if status.is_success() {
        return Ok(extract_etag(&response));
    }
    let body = response.text().await.unwrap_or_default();
    if status == StatusCode::FORBIDDEN {
        let body: String = body.chars().take(4096).collect();
        return Err(CaldavError::Forbidden(
            match crate::xml::dav_error_condition(&body) {
                Some(condition) if condition == "allowed-attendee-scheduling-object-change" => {
                    format!("reply-only-invitation: {condition}")
                }
                Some(condition) => format!("server-refused: {condition}"),
                None => "server-refused: HTTP 403".to_string(),
            },
        ));
    }
    Err(CaldavError::Http {
        status: status.as_u16(),
        message: if body.is_empty() {
            status.canonical_reason().unwrap_or("").to_string()
        } else {
            body.chars().take(200).collect()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the tests still call the preservation-free renderer directly.
    use crate::config::{AuthKind, CaldavAccountConfig};
    use crate::mapping::event_to_ical;
    use chrono::{Local, TimeZone};
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
        let events = report_events(&client(), &cal_url, range, &creds(&server.url()))
            .await
            .and_then(|entries| {
                events_from_entries(entries, cal_url.as_str(), &OwnIdentity::default())
            })
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
            &WriteCtx::default(),
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
        let created = create_event(
            &client(),
            &cal_url,
            sample_new_event(),
            &creds(&base),
            &WriteCtx::default(),
        )
        .await
        .expect("412 after a retried send means the first PUT landed");

        assert!(created.id.contains("@aperio"));
        assert!(created.etag.is_none(), "a 412 carries no ETag");
        let seen = paths.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "exactly one replay");
        assert_eq!(seen[0], seen[1], "the replay must reuse the SAME UID");
    }

    /// A server whose first connection reads the request and dies without an
    /// answer, and whose second answers the replay with `reply`. Returns its
    /// base URL.
    async fn first_attempt_lost_then(reply: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                drop(sock);
            }
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(reply).await;
                let _ = sock.shutdown().await;
            }
        });
        base
    }

    /// A guarded PUT whose first attempt may have landed: the replay's 412 is
    /// that attempt's new ETag, not someone else's change. Reported as a
    /// conflict, a split deleted its new part around a truncate that went
    /// through (decision 144).
    #[tokio::test]
    async fn a_412_on_the_replay_of_a_guarded_put_is_unsure() {
        let base = first_attempt_lost_then(
            b"HTTP/1.1 412 Precondition Failed\r\ncontent-length: 0\r\n\r\n",
        )
        .await;
        let resource = Url::parse(&format!("{base}/calendars/alice/work/abc.ics")).unwrap();
        let err = put_resource(
            &client(),
            &resource,
            standup_body("Cut short"),
            Some("\"etag-1\""),
            &creds(&base),
        )
        .await
        .expect_err("the replay was refused");
        assert!(matches!(err, CaldavError::Network(_)), "{err:?}");
    }

    /// ...and so is any other refusal of that replay: a 429 may be the server
    /// throttling a second copy of what it already stored.
    #[tokio::test]
    async fn any_refusal_of_a_replayed_put_is_unsure() {
        let base =
            first_attempt_lost_then(b"HTTP/1.1 429 Too Many Requests\r\ncontent-length: 0\r\n\r\n")
                .await;
        let resource = Url::parse(&format!("{base}/calendars/alice/work/abc.ics")).unwrap();
        let err = put_resource(
            &client(),
            &resource,
            standup_body("Cut short"),
            None,
            &creds(&base),
        )
        .await
        .expect_err("the replay was refused");
        match err {
            // The replay's answer is kept, for whoever reads the error.
            CaldavError::Network(msg) => assert!(msg.contains("HTTP 429"), "{msg}"),
            other => panic!("expected unsure, got {other:?}"),
        }
    }

    /// Without a replay, a 412 is what it says: the copy moved on.
    #[tokio::test]
    async fn a_412_without_a_replay_is_a_conflict() {
        let mut server = Server::new_async().await;
        let _put = server
            .mock("PUT", "/calendars/alice/work/abc.ics")
            .with_status(412)
            .create_async()
            .await;
        let resource =
            Url::parse(&format!("{}/calendars/alice/work/abc.ics", server.url())).unwrap();
        let err = put_resource(
            &client(),
            &resource,
            standup_body("Cut short"),
            Some("\"etag-1\""),
            &creds(&server.url()),
        )
        .await
        .expect_err("refused");
        assert!(
            matches!(err, CaldavError::Http { status: 412, .. }),
            "{err:?}"
        );
    }

    /// A DELETE whose first attempt may have removed the event: the replay's
    /// 404 says it is gone — or never was at this URL.
    #[tokio::test]
    async fn a_404_on_the_replay_of_a_delete_is_gone_on_replay() {
        let base =
            first_attempt_lost_then(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n").await;
        let cal_url = Url::parse(&format!("{base}/calendars/alice/work/")).unwrap();
        let outcome = delete_event(&client(), &cal_url, "abc", None, &creds(&base))
            .await
            .expect("a 404 is no error");
        assert_eq!(outcome, DeleteOutcome::GoneOnReplay);
    }

    fn gone(status: u16) -> CaldavError {
        CaldavError::Http {
            status,
            message: String::new(),
        }
    }

    /// A replayed 404 in a calendar that never held the event must not end
    /// the walk: the calendar that holds it still gets its DELETE. Stopping
    /// there reported a split's new series deleted while it stood.
    #[test]
    fn a_walk_goes_on_past_a_replayed_404() {
        let mut walk = DeleteWalk::default();
        assert!(
            !walk.step(Ok(DeleteOutcome::GoneOnReplay)),
            "not done in calendar A"
        );
        assert!(walk.step(Ok(DeleteOutcome::Deleted)), "done in calendar B");
    }

    /// ...and when no calendar holds it, the replayed 404 was the first DELETE
    /// removing it: gone, as asked. A failure elsewhere still wins — the event
    /// may live where the DELETE failed.
    #[test]
    fn a_replayed_404_found_nowhere_else_is_gone() {
        let mut walk = DeleteWalk::default();
        walk.step(Ok(DeleteOutcome::GoneOnReplay));
        walk.step(Ok(DeleteOutcome::NotFound));
        assert!(walk.finish("nowhere".into()).is_ok());

        let mut walk = DeleteWalk::default();
        walk.step(Ok(DeleteOutcome::GoneOnReplay));
        walk.step(Err(gone(500)));
        assert!(matches!(
            walk.finish("nowhere".into()),
            Err(CaldavError::Http { status: 500, .. })
        ));

        let mut walk = DeleteWalk::default();
        walk.step(Ok(DeleteOutcome::NotFound));
        assert!(matches!(
            walk.finish("nowhere".into()),
            Err(CaldavError::Http { status: 404, .. })
        ));
    }

    /// A plain event's copy on the server, with `summary` as its title.
    fn standup_body(summary: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:abc-123@aperio\r\nDTSTAMP:20260520T060000Z\r\nSUMMARY:{summary}\r\nDTSTART:20260520T080000Z\r\nDTEND:20260520T083000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        )
    }

    async fn serve_copy(server: &mut Server, body: String) -> mockito::Mock {
        server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .with_status(200)
            .with_header("content-type", "text/calendar")
            .with_header("etag", "\"server-etag\"")
            .with_body(body)
            .create_async()
            .await
    }

    #[tokio::test]
    async fn update_event_sends_if_match_with_existing_etag() {
        let mut server = Server::new_async().await;
        let get = serve_copy(&mut server, standup_body("Old standup")).await;
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
        let updated = update_event(
            &client(),
            sample_existing_event(&cal_url),
            &creds(&server.url()),
            &WriteCtx::default(),
        )
        .await
        .unwrap();
        get.assert_async().await;
        m.assert_async().await;
        assert_eq!(updated.etag.as_deref(), Some("\"new-etag\""));
    }

    #[tokio::test]
    async fn update_reads_the_server_copy_and_puts_apples_mark_back() {
        // Every update reads the server's copy first and hands its alarms to
        // the renderer. Without that the PUT would spell Apple's own default
        // alert as a hand-made alarm and the Calendar app would stop
        // recognising it.
        let mut server = Server::new_async().await;
        let get = serve_copy(
            &mut server,
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
"
            .to_string(),
        )
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
            title: "Standup, moved".into(),
            reminders: vec![cal_core::Reminder {
                kind: cal_core::ReminderKind::Relative { minutes_before: 60 },
                sound: None,
            }],
            ..sample_existing_event(&cal_url)
        };
        update_event(
            &client(),
            existing,
            &creds(&server.url()),
            &WriteCtx::default(),
        )
        .await
        .unwrap();
        get.assert_async().await;
        put.assert_async().await;
    }

    /// A save that changes nothing the server stores (a colour kept on this
    /// device, a sound, a private reminder) is not sent: on a scheduling
    /// server every PUT of a meeting mails its guests (decision 76a).
    #[tokio::test]
    async fn an_update_that_changes_nothing_sends_nothing() {
        let mut server = Server::new_async().await;
        let get = serve_copy(&mut server, standup_body("Standup")).await;
        let put = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .expect(0)
            .with_status(204)
            .create_async()
            .await;

        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut unchanged = sample_existing_event(&cal_url);
        unchanged.color_label = Some(cal_core::ColorLabelId("label".into()));
        let updated = update_event(
            &client(),
            unchanged,
            &creds(&server.url()),
            &WriteCtx::default(),
        )
        .await
        .unwrap();
        get.assert_async().await;
        put.assert_async().await;
        assert_eq!(updated.etag.as_deref(), Some("\"server-etag\""));
    }

    /// A reminder's sound and an app-start reminder live on this device only:
    /// changing just those leaves the server's copy as it is, so nothing is
    /// sent.
    #[tokio::test]
    async fn a_sound_only_edit_sends_nothing() {
        let mut server = Server::new_async().await;
        let get = serve_copy(
            &mut server,
            standup_body("Standup").replacen(
                "END:VEVENT\r\n",
                "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Standup\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\nEND:VEVENT\r\n",
                1,
            ),
        )
        .await;
        let put = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .expect(0)
            .with_status(204)
            .create_async()
            .await;

        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = sample_existing_event(&cal_url);
        edit.reminders = vec![
            cal_core::Reminder {
                kind: cal_core::ReminderKind::Relative { minutes_before: 10 },
                sound: Some(cal_core::SoundConfig {
                    source: cal_core::SoundSource::Silent,
                    volume: 40,
                }),
            },
            cal_core::Reminder {
                kind: cal_core::ReminderKind::AppStart,
                sound: None,
            },
        ];
        update_event(&client(), edit, &creds(&server.url()), &WriteCtx::default())
            .await
            .unwrap();
        get.assert_async().await;
        put.assert_async().await;
    }

    /// Without the server's copy a PUT would drop what it cannot see, a
    /// meeting's guests above all, so a failed read refuses the save.
    #[tokio::test]
    async fn a_failed_read_refuses_the_save() {
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
            .expect(0)
            .with_status(204)
            .create_async()
            .await;

        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let err = update_event(
            &client(),
            sample_existing_event(&cal_url),
            &creds(&server.url()),
            &WriteCtx::default(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, CaldavError::Http { status: 500, .. }),
            "{err:?}"
        );
        put.assert_async().await;
    }

    /// The event as the caller holds it before an update: on the server
    /// already (so it carries an ETag), with no reminders of its own.
    fn sample_existing_event(cal_url: &Url) -> Event {
        Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
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
            scheduling_silenced: false,
        }
    }

    /// A resource whose blocks name different organizers is not written: a
    /// refusal of Aperio's own, said as one (decision 144), not a protocol
    /// error the caller has to treat as maybe written.
    #[tokio::test]
    async fn an_update_aperio_will_not_write_is_a_refusal() {
        let mut server = Server::new_async().await;
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:abc-123@aperio\r\nDTSTAMP:20260520T060000Z\r\nSUMMARY:Standup\r\n\
DTSTART:20260520T080000Z\r\nDTEND:20260520T083000Z\r\nRRULE:FREQ=DAILY\r\n\
ORGANIZER:mailto:alice@example.org\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:abc-123@aperio\r\nDTSTAMP:20260520T060000Z\r\nSUMMARY:Standup\r\n\
RECURRENCE-ID:20260521T080000Z\r\nDTSTART:20260521T090000Z\r\nDTEND:20260521T093000Z\r\n\
ORGANIZER:mailto:bob@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let _get = serve_copy(&mut server, body.to_string()).await;
        let put = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .expect(0)
            .create_async()
            .await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let err = update_event(
            &client(),
            sample_existing_event(&cal_url),
            &creds(&server.url()),
            &WriteCtx::default(),
        )
        .await
        .unwrap_err();
        match err {
            CaldavError::Forbidden(msg) => assert_eq!(msg, "unsafe-to-write: mixed-organizers"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        put.assert_async().await;
    }

    #[tokio::test]
    async fn update_event_412_surfaces_as_conflict() {
        let mut server = Server::new_async().await;
        let _get = serve_copy(&mut server, standup_body("Old standup")).await;
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
            etag: Some("\"stale-etag\"".into()),
            ..sample_existing_event(&cal_url)
        };
        let err = update_event(
            &client(),
            existing,
            &creds(&server.url()),
            &WriteCtx::default(),
        )
        .await
        .unwrap_err();
        match err {
            CaldavError::Http { status, .. } => assert_eq!(status, 412),
            other => panic!("expected 412, got {other:?}"),
        }
    }

    /// The iCloud meeting of live round 5 (E1), as the account read it.
    fn icloud_meeting(cal_url: &Url) -> Event {
        crate::mapping::parse_calendar_data_as(
            crate::mapping::tests::ICLOUD_MEETING,
            cal_url.as_str(),
            Some("/calendars/alice/work/series.ics"),
            &crate::mapping::tests::icloud_identity(),
        )
        .unwrap()
        .remove(0)
    }

    fn icloud_ctx() -> WriteCtx {
        WriteCtx {
            identity: Some(crate::mapping::tests::icloud_identity()),
            schedules: true,
            organizer_address: Some("mailto:toni@example.org".into()),
            writes_color: false,
        }
    }

    /// E1: renaming an iCloud meeting the account organizes, without
    /// "Notify attendees", used to PUT it without ORGANIZER and ATTENDEE, and
    /// iCloud cancelled it for the guest. Now every meeting line goes back as
    /// the server wrote it, folding and parameter order included.
    #[tokio::test]
    async fn a_title_edit_of_an_organized_meeting_keeps_its_people_byte_for_byte() {
        let mut server = Server::new_async().await;
        let (seen, get, put) = serve_series(
            &mut server,
            crate::mapping::tests::ICLOUD_MEETING,
            "\"server-etag\"",
            1,
        )
        .await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = icloud_meeting(&cal_url);
        edit.title = "Aperio R6 renamed".into();
        edit.send_invitations = false;
        update_event(&client(), edit, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap();
        get.assert_async().await;
        put.assert_async().await;
        let body = seen.lock().unwrap()[0].clone();
        for line in [
            "ORGANIZER;CN=Toni Barth;EMAIL=toni@example.org:/aB1/principal/\r\n",
            "ATTENDEE;CN=Toni Barth;CUTYPE=INDIVIDUAL;PARTSTAT=ACCEPTED;EMAIL=toni@example\r\n .org;ROLE=CHAIR:/aB1/principal/\r\n",
            "ATTENDEE;CN=Bob Guest;CUTYPE=INDIVIDUAL;EMAIL=bob@example.net;SCHEDULE-STATUS=\r\n 1.1:mailto:bob@example.net\r\n",
            "SEQUENCE:0\r\n",
            "SUMMARY:Aperio R6 renamed\r\n",
        ] {
            assert!(body.contains(line), "missing {line:?} in\n{body}");
        }
        assert_eq!(body.matches("ORGANIZER").count(), 1, "{body}");
        assert_eq!(body.matches("ATTENDEE").count(), 2, "{body}");
    }

    /// Adding a guest keeps the other rows verbatim and writes one new row.
    #[tokio::test]
    async fn a_new_guest_joins_the_rows_the_server_wrote() {
        let mut server = Server::new_async().await;
        let (seen, _get, put) = serve_series(
            &mut server,
            crate::mapping::tests::ICLOUD_MEETING,
            "\"server-etag\"",
            1,
        )
        .await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = icloud_meeting(&cal_url);
        edit.attendees.push("carol@example.net".into());
        update_event(&client(), edit, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap();
        put.assert_async().await;
        let body = seen.lock().unwrap()[0].clone();
        assert_eq!(body.matches("ATTENDEE").count(), 3, "{body}");
        assert!(
            body.contains("RSVP=TRUE:mailto:carol@\r\n example.net\r\n"),
            "{body}"
        );
        assert!(
            body.contains("SCHEDULE-STATUS=\r\n 1.1:mailto:bob@example.net\r\n"),
            "{body}"
        );
    }

    /// A master update puts every override of its resource back, byte for
    /// byte: nothing is left to a server "re-attaching" what the PUT omitted.
    #[tokio::test]
    async fn a_master_update_puts_every_override_back() {
        let mut server = Server::new_async().await;
        let (seen, _get, put) = serve_series(&mut server, SERIES_BODY, "\"server-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut events = crate::mapping::parse_calendar_data_with_href(
            SERIES_BODY,
            cal_url.as_str(),
            Some("/calendars/alice/work/series.ics"),
        )
        .unwrap();
        let mut master = events.remove(0);
        master.etag = None;
        master.title = "Weekly, renamed".into();
        update_event(
            &client(),
            master,
            &creds(&server.url()),
            &WriteCtx::default(),
        )
        .await
        .unwrap();
        put.assert_async().await;
        let body = seen.lock().unwrap()[0].clone();
        assert!(body.contains(block_of(SERIES_BODY, "Moved far")), "{body}");
        assert!(body.contains(block_of(SERIES_BODY, "Retitled")), "{body}");
        assert!(body.contains("SUMMARY:Weekly\\, renamed"), "{body}");
        assert_eq!(
            body.matches("TZID:Europe/Berlin\r\n").count(),
            1,
            "the rebuilt master names its zone, which is not added twice: {body}"
        );
    }

    /// Skipping an occurrence adds one EXDATE line to the master and drops
    /// the override in that slot; every other byte stays: the meeting lines,
    /// the alarm, Apple's own properties.
    #[tokio::test]
    async fn skipping_an_occurrence_splices_only_the_exdate() {
        let recurring = crate::mapping::tests::ICLOUD_MEETING.replacen(
            "SEQUENCE:0\r\n",
            "RRULE:FREQ=WEEKLY;COUNT=3\r\nSEQUENCE:0\r\n",
            1,
        );
        let recurring: &'static str = Box::leak(recurring.into_boxed_str());
        let mut server = Server::new_async().await;
        let (seen, _get, put) = serve_series(&mut server, recurring, "\"server-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let slot = Utc.with_ymd_and_hms(2026, 11, 16, 15, 0, 0).unwrap();
        add_event_exdate(
            &client(),
            &cal_url,
            "/calendars/alice/work/series.ics|EBFB90D1-F3C9-4E8C-B18E-211595E15C22",
            slot,
            &creds(&server.url()),
        )
        .await
        .unwrap();
        put.assert_async().await;
        let body = seen.lock().unwrap()[0].clone();
        assert_eq!(
            body,
            recurring.replacen(
                "BEGIN:VEVENT\r\n",
                "BEGIN:VEVENT\r\nEXDATE:20261116T150000Z\r\n",
                1
            )
        );
    }

    /// An occurrence the series skips already, with nothing standing in its
    /// slot, needs no write: nothing is sent, so the server sends no
    /// scheduling message either.
    #[tokio::test]
    async fn skipping_a_skipped_occurrence_sends_nothing() {
        let recurring = crate::mapping::tests::ICLOUD_MEETING.replacen(
            "SEQUENCE:0\r\n",
            "RRULE:FREQ=WEEKLY;COUNT=3\r\nEXDATE:20261116T150000Z\r\nSEQUENCE:0\r\n",
            1,
        );
        let recurring: &'static str = Box::leak(recurring.into_boxed_str());
        let mut server = Server::new_async().await;
        let (_seen, get, put) = serve_series(&mut server, recurring, "\"server-etag\"", 0).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        add_event_exdate(
            &client(),
            &cal_url,
            "/calendars/alice/work/series.ics|EBFB90D1-F3C9-4E8C-B18E-211595E15C22",
            Utc.with_ymd_and_hms(2026, 11, 16, 15, 0, 0).unwrap(),
            &creds(&server.url()),
        )
        .await
        .unwrap();
        get.assert_async().await;
        put.assert_async().await;
    }

    /// An invitation someone else organizes, read for this account, as the
    /// host holds it before an edit.
    fn invitation(cal_url: &Url, body: &'static str) -> Vec<Event> {
        crate::mapping::parse_calendar_data_as(
            body,
            cal_url.as_str(),
            Some("/calendars/alice/work/series.ics"),
            &crate::mapping::tests::icloud_identity(),
        )
        .unwrap()
    }

    fn invitation_master(cal_url: &Url, body: &'static str) -> Event {
        let mut event = invitation(cal_url, body).remove(0);
        event.etag = Some("\"server-etag\"".into());
        event
    }

    /// Decision 77a: a save of someone else's meeting writes the server's own
    /// block back with only its alarms changed. Everything the organizer
    /// wrote — the zone, SEQUENCE, STATUS, the people, Apple's own
    /// properties, even DTSTAMP — goes back byte for byte.
    #[tokio::test]
    async fn an_invitation_reminder_edit_changes_only_its_alarms() {
        use crate::mapping::tests::ICLOUD_INVITATION;
        let mut server = Server::new_async().await;
        let (seen, get, put) =
            serve_series(&mut server, ICLOUD_INVITATION, "\"server-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = invitation_master(&cal_url, ICLOUD_INVITATION);
        edit.reminders.push(cal_core::Reminder {
            kind: cal_core::ReminderKind::Relative { minutes_before: 60 },
            sound: Some(cal_core::SoundConfig {
                source: cal_core::SoundSource::Silent,
                volume: 20,
            }),
        });
        update_event(&client(), edit, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap();
        get.assert_async().await;
        put.assert_async().await;
        let sent = seen.lock().unwrap()[0].clone();

        // The one new alarm, before the block's own end.
        assert_eq!(sent.matches("BEGIN:VALARM").count(), 7, "{sent}");
        // The serialiser writes a relative trigger in seconds.
        assert!(sent.contains("TRIGGER:-PT3600S"), "{sent}");
        assert!(
            !sent.contains("BEGIN:VALARM\r\nDTSTAMP"),
            "a VALARM has no DTSTAMP: {sent}"
        );
        let master_end = sent.find("END:VEVENT").unwrap();
        assert!(
            sent[..master_end].contains("TRIGGER:-PT3600S"),
            "in the master's block: {sent}"
        );
        // The alarms the user kept are the server's own text, mark and all.
        for kept in [
            "UID:11111111-1111-4111-8111-111111111111",
            "X-APPLE-DEFAULT-ALARM:TRUE\r\nEND:VALARM",
            "ACTION:AUDIO",
            "ATTACH;VALUE=URI:Basso",
            "TRIGGER;RELATED=END:-PT5M",
            "ATTENDEE:mailto:toni@example.org",
            "TRIGGER:-P1W",
        ] {
            assert!(sent.contains(kept), "kept {kept}: {sent}");
        }
        // The event-level default mark goes: the alarms are no longer the
        // account's default set.
        assert!(
            !sent.contains("X-APPLE-DEFAULT-ALARM:TRUE\r\nX-APPLE-TRAVEL"),
            "the event-level mark is gone: {sent}"
        );
        // And everything else of the resource is untouched.
        for line in [
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Berlin",
            "DTSTART;TZID=Europe/Berlin:20261109T160000",
            "DTSTAMP:20260919T184800Z",
            "CREATED:20260901T090000Z",
            "LAST-MODIFIED:20260918T101500Z",
            "ORGANIZER;CN=Boss;EMAIL=boss@example.net:mailto:boss@example.net",
            "SEQUENCE:2",
            "STATUS:CONFIRMED",
            "TRANSP:OPAQUE",
            "X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC",
            "SUMMARY:Aperio R6 fremde, verschoben",
        ] {
            assert!(sent.contains(line), "kept {line}: {sent}");
        }
        assert_eq!(
            sent.matches("ATTENDEE;CN=Boss").count(),
            1,
            "the people are not rewritten: {sent}"
        );
    }

    /// The alarms are spliced into the copy just read, but the reminders
    /// written are the ones the CALLER saw. So the PUT is guarded by the
    /// caller's version, not by the one the splice came from: a reminders-only
    /// edit passes the verdict whatever the version, and with the fresh ETag a
    /// save from a second device would quietly drop the alarm the first set.
    #[tokio::test]
    async fn an_invitation_reminder_edit_is_guarded_by_the_callers_version() {
        use crate::mapping::tests::ICLOUD_INVITATION;
        let mut server = Server::new_async().await;
        // The resource moved on since this copy was read: the GET answers
        // "server-etag", and only "caller-etag" is accepted on the PUT.
        let (_seen, get, put) =
            serve_series(&mut server, ICLOUD_INVITATION, "\"caller-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = invitation_master(&cal_url, ICLOUD_INVITATION);
        edit.etag = Some("\"caller-etag\"".into());
        edit.reminders.push(cal_core::Reminder {
            kind: cal_core::ReminderKind::Relative { minutes_before: 60 },
            sound: None,
        });
        update_event(&client(), edit, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap();
        get.assert_async().await;
        put.assert_async().await;
    }

    /// An alarm the read shows as a reminder is claimed by that reminder,
    /// whatever its ACTION or the end it is triggered from. Classifying it
    /// more strictly would keep it AND render a second one beside it, and the
    /// reminder would multiply on every save.
    #[tokio::test]
    async fn an_audio_or_end_related_alarm_is_claimed_not_duplicated() {
        use crate::mapping::tests::ICLOUD_INVITATION;
        let mut server = Server::new_async().await;
        let (seen, _get, put) =
            serve_series(&mut server, ICLOUD_INVITATION, "\"server-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = invitation_master(&cal_url, ICLOUD_INVITATION);
        // What the editor shows for this invitation, plus one new reminder.
        assert_eq!(
            edit.reminders
                .iter()
                .map(|r| r.kind.clone())
                .collect::<Vec<_>>(),
            [
                cal_core::ReminderKind::Relative { minutes_before: 15 },
                cal_core::ReminderKind::Relative { minutes_before: 30 },
                cal_core::ReminderKind::Email {
                    minutes_before: 1440
                },
                cal_core::ReminderKind::Relative { minutes_before: 5 },
            ],
            "the read shows an audio alarm and an end-related one as reminders"
        );
        edit.reminders.push(cal_core::Reminder {
            kind: cal_core::ReminderKind::Relative { minutes_before: 60 },
            sound: None,
        });
        update_event(&client(), edit, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap();
        put.assert_async().await;
        let sent = seen.lock().unwrap()[0].clone();
        assert_eq!(sent.matches("ACTION:AUDIO").count(), 1, "{sent}");
        assert_eq!(
            sent.matches("TRIGGER;RELATED=END:-PT5M").count(),
            1,
            "{sent}"
        );
        assert_eq!(sent.matches("TRIGGER:-PT30M").count(), 1, "{sent}");
        assert_eq!(sent.matches("ACTION:EMAIL").count(), 1, "{sent}");
    }

    /// A reminder the user removed goes; one Aperio never showed stays.
    #[tokio::test]
    async fn a_removed_reminder_goes_and_an_unshown_alarm_stays() {
        use crate::mapping::tests::ICLOUD_INVITATION;
        let mut server = Server::new_async().await;
        let (seen, _get, put) =
            serve_series(&mut server, ICLOUD_INVITATION, "\"server-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = invitation_master(&cal_url, ICLOUD_INVITATION);
        edit.reminders.retain(|r| {
            !matches!(
                r.kind,
                cal_core::ReminderKind::Relative { minutes_before: 30 }
            )
        });
        update_event(&client(), edit, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap();
        put.assert_async().await;
        let sent = seen.lock().unwrap()[0].clone();
        assert!(
            !sent.contains("ACTION:AUDIO"),
            "the removed one goes: {sent}"
        );
        assert!(
            sent.contains("TRIGGER:-P1W"),
            "an alarm Aperio never showed is not deleted by an Aperio edit: {sent}"
        );
    }

    /// Only the organizer changes a meeting's title, time or people. The
    /// refusal names the field and nothing is sent.
    #[tokio::test]
    async fn an_invitation_title_edit_is_refused_before_any_put() {
        use crate::mapping::tests::ICLOUD_INVITATION;
        let mut server = Server::new_async().await;
        let (_seen, _get, put) =
            serve_series(&mut server, ICLOUD_INVITATION, "\"server-etag\"", 0).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = invitation_master(&cal_url, ICLOUD_INVITATION);
        edit.title = "Mein eigener Titel".into();
        let err = update_event(&client(), edit, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap_err();
        put.assert_async().await;
        match err {
            CaldavError::Forbidden(message) => {
                assert_eq!(message, "reply-only-invitation: title");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// The same for a series the user tried to end early.
    #[tokio::test]
    async fn an_invitation_series_truncation_is_refused() {
        use crate::mapping::tests::ICLOUD_INVITATION;
        let mut server = Server::new_async().await;
        let (_seen, _get, put) =
            serve_series(&mut server, ICLOUD_INVITATION, "\"server-etag\"", 0).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = invitation_master(&cal_url, ICLOUD_INVITATION);
        if let Some(recurrence) = edit.recurrence.as_mut() {
            recurrence.rrule = "FREQ=WEEKLY;UNTIL=20261123T150000Z".into();
        }
        let err = update_event(&client(), edit, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap_err();
        put.assert_async().await;
        assert!(
            matches!(&err, CaldavError::Forbidden(m) if m == "reply-only-invitation: recurrence"),
            "{err:?}"
        );
    }

    /// A guest the organizer added while the editor was open is not the
    /// user's doing: that is a conflict, not a refusal, and the user is asked
    /// to look at the new copy rather than told they may not do what they did
    /// not do.
    #[tokio::test]
    async fn a_guest_the_organizer_added_is_a_conflict_not_a_refusal() {
        use crate::mapping::tests::ICLOUD_INVITATION;
        let mut server = Server::new_async().await;
        let (_seen, _get, put) =
            serve_series(&mut server, ICLOUD_INVITATION, "\"server-etag\"", 0).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = invitation_master(&cal_url, ICLOUD_INVITATION);
        // The copy the user edited is older than the server's.
        edit.etag = Some("\"older\"".into());
        edit.attendees.push("carol@example.net".into());
        let err = update_event(&client(), edit, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap_err();
        put.assert_async().await;
        assert!(
            matches!(err, CaldavError::Http { status: 412, .. }),
            "{err:?}"
        );
    }

    /// And without any version at all the same rule holds: "unknown" is not
    /// "unchanged".
    #[tokio::test]
    async fn an_invitation_edit_without_an_etag_is_a_conflict_not_a_refusal() {
        use crate::mapping::tests::ICLOUD_INVITATION;
        let mut server = Server::new_async().await;
        let (_seen, _get, put) =
            serve_series(&mut server, ICLOUD_INVITATION, "\"server-etag\"", 0).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = invitation_master(&cal_url, ICLOUD_INVITATION);
        edit.etag = None;
        edit.location = Some("Ein anderer Raum".into());
        let err = update_event(&client(), edit, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap_err();
        put.assert_async().await;
        assert!(
            matches!(err, CaldavError::Http { status: 412, .. }),
            "{err:?}"
        );
    }

    /// An invitation saved without a change sends nothing: every PUT of a
    /// scheduling object is traffic on the organizer's server. A colour the
    /// server sent back is no change either, because iCloud stores none.
    #[tokio::test]
    async fn an_invitation_save_that_changes_nothing_puts_nothing() {
        use crate::mapping::tests::ICLOUD_INVITATION;
        let mut server = Server::new_async().await;
        let (_seen, get, put) =
            serve_series(&mut server, ICLOUD_INVITATION, "\"server-etag\"", 0).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut edit = invitation_master(&cal_url, ICLOUD_INVITATION);
        edit.color_label = Some(cal_core::ColorLabelId("label".into()));
        edit.color_hex = None;
        edit.sound = None;
        let saved = update_event(&client(), edit, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap();
        get.assert_async().await;
        put.assert_async().await;
        assert_eq!(saved.etag.as_deref(), Some("\"server-etag\""));
    }

    /// One changed occurrence of an invitation: its own block gets the
    /// alarms, and the master and the other blocks are untouched.
    #[tokio::test]
    async fn an_invitation_occurrence_reminder_edit_touches_only_its_block() {
        use crate::mapping::tests::ICLOUD_INVITATION;
        let mut server = Server::new_async().await;
        let (seen, _get, put) =
            serve_series(&mut server, ICLOUD_INVITATION, "\"server-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut occurrence = invitation(&cal_url, ICLOUD_INVITATION).remove(1);
        occurrence.etag = Some("\"server-etag\"".into());
        occurrence.reminders.push(cal_core::Reminder {
            kind: cal_core::ReminderKind::Relative { minutes_before: 90 },
            sound: None,
        });
        update_event(&client(), occurrence, &creds(&server.url()), &icloud_ctx())
            .await
            .unwrap();
        put.assert_async().await;
        let sent = seen.lock().unwrap()[0].clone();
        let master_end = sent.find("END:VEVENT").unwrap();
        assert!(
            !sent[..master_end].contains("TRIGGER:-PT5400S"),
            "not in the master: {sent}"
        );
        assert!(
            sent[master_end..].contains("TRIGGER:-PT5400S"),
            "in the occurrence: {sent}"
        );
        assert!(
            sent.contains("SUMMARY:Aperio R6 fremde, verschoben"),
            "{sent}"
        );
        assert_eq!(sent.matches("BEGIN:VALARM").count(), 7, "{sent}");
    }

    /// An all-day series is excluded by a DATE, the way it names its own
    /// occurrences. A UTC date-time excludes nothing there, so the occurrence
    /// would come back — and on an invitation the promised decline would
    /// never be sent.
    #[tokio::test]
    async fn an_all_day_occurrence_is_excluded_by_a_date_exdate() {
        use crate::mapping::tests::ICLOUD_ALL_DAY_INVITATION;
        let mut server = Server::new_async().await;
        let (seen, _get, put) =
            serve_series(&mut server, ICLOUD_ALL_DAY_INVITATION, "\"server-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        add_event_exdate(
            &client(),
            &cal_url,
            "/calendars/alice/work/series.ics|9C1F6A4E-0B77-4E1E-9F6E-51D2A0C9B7A1",
            // Built the way an all-day occurrence is: local midnight as an
            // instant. East of Greenwich that is the day BEFORE in UTC, which
            // is what made the old line exclude nothing.
            Local
                .with_ymd_and_hms(2026, 11, 16, 0, 0, 0)
                .unwrap()
                .with_timezone(&Utc),
            &creds(&server.url()),
        )
        .await
        .unwrap();
        put.assert_async().await;
        let sent = seen.lock().unwrap()[0].clone();
        assert!(sent.contains("EXDATE;VALUE=DATE:20261116"), "{sent}");
        assert!(!sent.contains("EXDATE:20261116T000000Z"), "{sent}");
    }

    /// Answering touches the event's own row and nothing else: an e-mail
    /// alarm carries an ATTENDEE line of its own, which has no PARTSTAT, and
    /// an address that merely contains the account's is somebody else.
    #[tokio::test]
    async fn an_answer_leaves_an_email_alarms_attendee_alone() {
        use crate::mapping::tests::ICLOUD_INVITATION;
        let mut server = Server::new_async().await;
        let (seen, _get, put) =
            serve_series(&mut server, ICLOUD_INVITATION, "\"server-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        respond_to_event(
            &client(),
            &cal_url,
            "/calendars/alice/work/series.ics|9C1F6A4E-0B77-4E1E-9F6E-51D2A0C9B7A1",
            "toni@example.org",
            AttendeeStatus::Accepted,
            true,
            &creds(&server.url()),
        )
        .await
        .unwrap();
        put.assert_async().await;
        let sent = seen.lock().unwrap()[0].clone();
        assert!(
            sent.contains("ATTENDEE:mailto:toni@example.org\r\n"),
            "the e-mail alarm's own row keeps its text, PARTSTAT-free: {sent}"
        );
        // Read as the lines read: a rewritten row is folded again.
        let flat = sent.replace("\r\n ", "");
        assert_eq!(
            flat.matches("PARTSTAT=ACCEPTED").count(),
            3,
            "the account's rows in both blocks, and the organizer's own: {flat}"
        );
        assert!(
            flat.contains(
                "EMAIL=toni@example.org;PARTSTAT=ACCEPTED;ROLE=REQ-PARTICIPANT:/aB1/principal/"
            ),
            "the account's own row in the master: {flat}"
        );
        assert!(
            flat.contains("PARTSTAT=ACCEPTED;ROLE=CHAIR:mailto:boss@example.net"),
            "the organizer's row is untouched: {flat}"
        );
    }

    #[test]
    fn an_answer_does_not_match_a_longer_address() {
        let body = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n\
ATTENDEE;CN=Other;PARTSTAT=NEEDS-ACTION:mailto:atoni@example.org\r\n\
ATTENDEE;CN=Me;PARTSTAT=NEEDS-ACTION:mailto:toni@example.org\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
        let out = set_self_partstat(body, "toni@example.org", "DECLINED").unwrap();
        assert!(
            out.contains("CN=Other;PARTSTAT=NEEDS-ACTION:mailto:atoni@example.org"),
            "{out}"
        );
        assert!(
            out.contains("CN=Me;PARTSTAT=DECLINED:mailto:toni@example.org"),
            "{out}"
        );
    }

    /// A 403 on a write is the server refusing that change, not the
    /// credentials: it names its precondition.
    #[tokio::test]
    async fn a_403_on_a_write_names_the_refused_precondition() {
        let mut server = Server::new_async().await;
        let _get = serve_copy(&mut server, standup_body("Old standup")).await;
        let _put = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .with_status(403)
            .with_body(
                r#"<?xml version="1.0"?><D:error xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><C:allowed-attendee-scheduling-object-change/></D:error>"#,
            )
            .create_async()
            .await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let err = update_event(
            &client(),
            sample_existing_event(&cal_url),
            &creds(&server.url()),
            &WriteCtx::default(),
        )
        .await
        .unwrap_err();
        match err {
            CaldavError::Forbidden(msg) => assert_eq!(
                msg,
                "reply-only-invitation: allowed-attendee-scheduling-object-change"
            ),
            other => panic!("expected Forbidden, got {other:?}"),
        }
    }

    /// A new event names its guests only as a meeting the server schedules:
    /// when the user notifies. It echoes what it wrote, and nobody else.
    #[tokio::test]
    async fn a_new_meeting_names_its_guests_only_when_notifying() {
        use std::sync::{Arc, Mutex};
        let mut server = Server::new_async().await;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let _put = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .expect(2)
            .with_status(201)
            .with_body_from_request(move |request| {
                sink.lock()
                    .unwrap()
                    .push(request.utf8_lossy_body().unwrap().into_owned());
                Vec::new()
            })
            .create_async()
            .await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let mut meeting = sample_new_event();
        meeting.attendees = vec![
            "Doe, Jane <jane@example.net>".into(),
            "toni@example.org".into(),
        ];
        let silent = create_event(
            &client(),
            &cal_url,
            meeting.clone(),
            &creds(&server.url()),
            &icloud_ctx(),
        )
        .await
        .unwrap();
        assert!(silent.attendees.is_empty(), "{:?}", silent.attendees);
        meeting.send_invitations = true;
        let invited = create_event(
            &client(),
            &cal_url,
            meeting,
            &creds(&server.url()),
            &icloud_ctx(),
        )
        .await
        .unwrap();
        assert_eq!(invited.attendees, ["Doe, Jane <jane@example.net>"]);
        assert_eq!(invited.organizer.as_deref(), Some("toni@example.org"));
        let bodies = seen.lock().unwrap().clone();
        assert!(!bodies[0].contains("ATTENDEE"), "{}", bodies[0]);
        assert!(
            bodies[1].contains("ORGANIZER:mailto:toni@example.org\r\n"),
            "{}",
            bodies[1]
        );
        assert!(
            bodies[1].contains("ATTENDEE;CN=\"Doe, Jane\";"),
            "{}",
            bodies[1]
        );
        assert_eq!(
            bodies[1].matches("ATTENDEE").count(),
            1,
            "the account is no guest"
        );
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

    /// A 403 on a delete or on an answer's PUT is the server refusing that
    /// write, like on any other PUT: it names its precondition.
    #[tokio::test]
    async fn a_403_on_a_delete_or_an_answer_names_the_refused_precondition() {
        const REFUSAL: &str =
            r#"<?xml version="1.0"?><D:error xmlns:D="DAV:"><D:need-privileges/></D:error>"#;
        let mut server = Server::new_async().await;
        let _delete = server
            .mock(
                "DELETE",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .with_status(403)
            .with_body(REFUSAL)
            .create_async()
            .await;
        let _get = serve_copy(
            &mut server,
            standup_body("Standup").replacen(
                "END:VEVENT\r\n",
                "ORGANIZER:mailto:boss@example.com\r\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:me@example.com\r\nEND:VEVENT\r\n",
                1,
            ),
        )
        .await;
        let _put = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"^/calendars/alice/work/.+\.ics$".into()),
            )
            .with_status(403)
            .with_body(REFUSAL)
            .create_async()
            .await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let refused = |err: CaldavError| match err {
            CaldavError::Forbidden(msg) => assert_eq!(msg, "server-refused: need-privileges"),
            other => panic!("expected Forbidden, got {other:?}"),
        };
        refused(
            delete_event(
                &client(),
                &cal_url,
                "abc-123@aperio",
                None,
                &creds(&server.url()),
            )
            .await
            .unwrap_err(),
        );
        refused(
            respond_to_event(
                &client(),
                &cal_url,
                "abc-123@aperio",
                "me@example.com",
                AttendeeStatus::Accepted,
                true,
                &creds(&server.url()),
            )
            .await
            .unwrap_err(),
        );
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
            keep_fields: Vec::new(),
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
            scheduling_silenced: false,
        };
        let master_vcal = event_to_ical(&master);
        let until = rrule_until_instant(&master.recurrence.as_ref().unwrap().rrule).unwrap();

        let merged = merge_with_overrides(
            body,
            &master_vcal,
            "https://example.com/cal/",
            |rid| rid <= until,
            str::to_string,
        )
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
            merge_with_overrides(
                body,
                "MASTER",
                "https://example.com/cal/",
                |rid| rid <= until,
                str::to_string,
            ),
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
            &WriteCtx::default(),
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
    async fn a_slot_without_an_exception_gets_one() {
        // Decision 79b: the first change to one occurrence WRITES the
        // exception the series does not have yet. Until now this refused, and
        // the surfaces carved the occurrence out into an event of its own.
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\nDTSTART:20260608T070000Z\r\nDTEND:20260608T073000Z\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\nSUMMARY:Weekly sync\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let mut server = Server::new_async().await;
        let (seen, _get, put) = serve_series(&mut server, body, "\"res-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();

        let saved = update_event(
            &client(),
            moved_override(&cal_url),
            &creds(&server.url()),
            &WriteCtx::default(),
        )
        .await
        .unwrap();
        put.assert_async().await;
        let sent = seen.lock().unwrap()[0].clone();

        // The master's own DTSTART is a bare UTC value, so the exception says
        // the slot the same way (RFC 5545 §3.8.4.4).
        assert!(
            sent.contains("RECURRENCE-ID:20260615T070000Z"),
            "the minted exception names the slot: {sent}"
        );
        assert_eq!(sent.matches("BEGIN:VEVENT").count(), 2, "{sent}");
        // The master is untouched, rule and all.
        assert!(sent.contains("RRULE:FREQ=WEEKLY;BYDAY=MO"), "{sent}");
        assert!(sent.contains("SUMMARY:Weekly sync"), "{sent}");
        // The new block carries the edit, and no rule of its own.
        assert!(sent.contains("SUMMARY:Moved again"), "{sent}");
        assert_eq!(sent.matches("RRULE:").count(), 1, "{sent}");
        // The row that comes back is the row the next read would give.
        assert_eq!(
            saved.id,
            "/calendars/alice/work/series.ics|series-1::rid::2026-06-15T07:00:00Z"
        );
        assert!(saved.recurrence.is_none(), "an exception has no rule");
    }

    #[tokio::test]
    async fn a_minted_exception_says_the_slot_the_way_its_master_does() {
        // A zoned master: the exception copies the zone NAME from the master's
        // own DTSTART, because that is the one a VTIMEZONE in this resource
        // resolves. 29 June 09:00 Berlin is 07:00 UTC in summer time.
        let mut server = Server::new_async().await;
        let (seen, _get, put) = serve_series(&mut server, SERIES_BODY, "\"res-etag\"", 1).await;
        let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
        let edit = Event {
            id: "/calendars/alice/work/series.ics|series-1::rid::2026-06-29T07:00:00Z".into(),
            title: "Just this Monday".into(),
            start: Utc.with_ymd_and_hms(2026, 6, 29, 12, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 6, 29, 12, 30, 0).unwrap(),
            etag: Some("\"res-etag\"".into()),
            ..sample_existing_event(&cal_url)
        };

        update_event(&client(), edit, &creds(&server.url()), &WriteCtx::default())
            .await
            .unwrap();
        put.assert_async().await;
        let sent = seen.lock().unwrap()[0].clone();
        assert!(
            sent.contains("RECURRENCE-ID;TZID=Europe/Berlin:20260629T090000"),
            "the master's own zone, its own spelling: {sent}"
        );
        // The two exceptions the resource already had are still there.
        assert_eq!(sent.matches("BEGIN:VEVENT").count(), 4, "{sent}");
        assert!(sent.contains("X-CUSTOM-PROP:kept byte for byte"), "{sent}");
    }

    #[tokio::test]
    async fn an_exception_is_refused_where_it_would_name_nothing() {
        let cases: [(&str, &str); 2] = [
            (
                // Not a series at all: an exception would be a second event
                // under one UID.
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\nDTSTART:20260608T070000Z\r\nDTEND:20260608T073000Z\r\n\
SUMMARY:Not a series\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
                "not-recurring",
            ),
            (
                // The occurrence was deleted; an exception would bring back a
                // day the user removed.
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:series-1\r\nDTSTART:20260608T070000Z\r\nDTEND:20260608T073000Z\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\nEXDATE:20260615T070000Z\r\nSUMMARY:Weekly sync\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n",
                "already-skipped",
            ),
        ];
        for (body, why) in cases {
            let mut server = Server::new_async().await;
            let (_seen, _get, put) = serve_series(&mut server, body, "\"res-etag\"", 0).await;
            let cal_url = Url::parse(&format!("{}/calendars/alice/work/", server.url())).unwrap();
            let err = update_event(
                &client(),
                moved_override(&cal_url),
                &creds(&server.url()),
                &WriteCtx::default(),
            )
            .await
            .unwrap_err();
            let message = err.to_string();
            assert!(
                message.contains("occurrence-not-writable") && message.contains(why),
                "{why}: {message}"
            );
            // Refused means refused: nothing was written, and the surfaces are
            // told rather than quietly given a carve-out (decision 92).
            put.assert_async().await;
        }
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
            "the rest of the body as it was, the zone once: {sent}"
        );
    }

    /// A master update rebuilds the master and puts the overrides back as
    /// they were. The master here is in UTC and writes no zone of its own,
    /// but a kept override names one: the server's zone goes back with it.
    #[tokio::test]
    async fn a_kept_override_keeps_the_zone_it_names() {
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
        let mut master = crate::mapping::parse_calendar_data_with_href(
            body,
            cal_url.as_str(),
            Some("/calendars/alice/work/series.ics"),
        )
        .unwrap()
        .remove(0);
        master.etag = None;
        master.title = "Weekly sync, renamed".into();
        update_event(
            &client(),
            master,
            &creds(&server.url()),
            &WriteCtx::default(),
        )
        .await
        .unwrap();
        put.assert_async().await;

        let sent = seen.lock().unwrap()[0].clone();
        assert!(sent.contains("SUMMARY:Weekly sync\\, renamed"), "{sent}");
        assert!(
            sent.contains(block_of(body, "SUMMARY:Later")),
            "the override byte for byte: {sent}"
        );
        let zone = &body[body.find("BEGIN:VTIMEZONE").unwrap()
            ..body.find("END:VTIMEZONE\r\n").unwrap() + "END:VTIMEZONE\r\n".len()];
        assert!(sent.contains(zone), "the zone byte for byte: {sent}");
        assert_eq!(sent.matches(zone).count(), 1, "the zone once: {sent}");
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
        let err = report_events(&client(), &cal_url, range, &creds(&server.url()))
            .await
            .and_then(|entries| {
                events_from_entries(entries, cal_url.as_str(), &OwnIdentity::default())
            })
            .unwrap_err();
        match err {
            CaldavError::Http { status, .. } => assert_eq!(status, 403),
            other => panic!("expected 403, got {other:?}"),
        }
    }
}
