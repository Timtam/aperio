//! iCalendar ⇄ cal-core conversion.
//!
//! CalDAV servers return events as VCALENDAR bodies, one VEVENT per
//! file. The `icalendar` crate parses them into a typed tree; this
//! module pulls the bits we care about into `cal_core::Event` so the
//! rest of Aperio doesn't have to learn the iCal vocabulary.
//!
//! What we map today:
//!   - UID         → Event.id
//!   - SUMMARY     → Event.title
//!   - DESCRIPTION → Event.description
//!   - LOCATION    → Event.location
//!   - DTSTART     → Event.start (UTC)
//!   - DTEND       → Event.end   (UTC; falls back to DTSTART when absent)
//!   - DTSTART VALUE=DATE → all_day = true, start of day in UTC
//!   - RRULE       → EventRecurrence.rrule (verbatim)
//!   - EXDATE      → EventRecurrence.exceptions
//!   - CREATED     → Event.created_at (fallback: DTSTAMP, then now)
//!   - LAST-MODIFIED → Event.updated_at (same fallback chain)
//!   - VALARM       → Event.reminders (RFC 5545 §3.6.6, bidirectional):
//!                    relative TRIGGER (-PT1H, -P1D, …) → Relative,
//!                    absolute TRIGGER;VALUE=DATE-TIME    → Absolute,
//!                    ACTION:EMAIL                       → Email,
//!                    ACTION:DISPLAY / AUDIO             → Relative or Absolute
//!
//! VTIMEZONE and ATTENDEE mapping still live behind follow-up tasks.

use cal_core::{Event, EventRecurrence, NewEvent, Reminder, ReminderKind};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use icalendar::{Calendar as ICalendar, Component, DatePerhapsTime, EventLike};

use crate::error::{CaldavError, CaldavResult};

/// Parse the calendar-data of a CalDAV REPORT response into a
/// chronologically sorted list of events. One VCALENDAR body may
/// carry multiple VEVENTs (RFC 5545 allows it); we emit one
/// `cal_core::Event` per top-level VEVENT.
pub fn parse_calendar_data(body: &str, calendar_id: &str) -> CaldavResult<Vec<Event>> {
    parse_calendar_data_with_href(body, calendar_id, None)
}

/// Like [`parse_calendar_data`], but encodes the server's resource `href`
/// into each event id as `{href}|{uid}` (the same shape `tasks`/`contacts`
/// use). This gives the cache a `native_id` (= href) that an RFC 6578
/// sync-collection deletion — reported by href — can match directly, so
/// the events delta does per-resource removals instead of a full re-list.
/// `href = None` falls back to the bare-UID id (back-compat for callers
/// that don't have the href, e.g. unit tests).
pub fn parse_calendar_data_with_href(
    body: &str,
    calendar_id: &str,
    href: Option<&str>,
) -> CaldavResult<Vec<Event>> {
    let parsed: ICalendar = body
        .parse()
        .map_err(|err: String| CaldavError::Protocol(format!("ical: {err}")))?;
    let mut out = Vec::new();
    for comp in parsed.components {
        if let icalendar::CalendarComponent::Event(ev) = comp {
            match map_event(&ev, calendar_id, href) {
                Ok(event) => out.push(event),
                Err(err) => {
                    tracing::warn!(?err, "skipping unmapped VEVENT");
                }
            }
        }
    }
    Ok(out)
}

/// Split an event id into `(Some(href), uid)` for the composite
/// `{href}|{uid}` shape, or `(None, id)` for a bare UID (freshly created
/// events before refetch, plus rows persisted by older Aperio versions).
/// Mirrors `tasks::decode_id`.
pub fn decode_event_id(event_id: &str) -> (Option<&str>, &str) {
    match event_id.split_once('|') {
        Some((href, uid)) if !href.is_empty() => (Some(href), uid),
        _ => (None, event_id),
    }
}

fn map_event(ev: &icalendar::Event, calendar_id: &str, href: Option<&str>) -> CaldavResult<Event> {
    let uid = ev
        .get_uid()
        .ok_or_else(|| CaldavError::Protocol("VEVENT without UID".to_string()))?;
    let summary = ev.get_summary().unwrap_or("").to_string();
    let description = ev.get_description().map(|s| s.to_string());
    let location = ev.get_location().map(|s| s.to_string());

    let (start, end, all_day) = resolve_range(ev)?;

    let rrule = ev.property_value("RRULE").map(|s| s.to_string());
    let exceptions = collect_exdates(ev);
    let recurrence = rrule.map(|r| EventRecurrence {
        rrule: r,
        exceptions,
    });

    // Created / updated fallback chain. icalendar exposes a few
    // typed accessors but they don't always give us UTC straight,
    // so we read the raw property and parse defensively.
    let created = read_utc(ev, "CREATED")
        .or_else(|| read_utc(ev, "DTSTAMP"))
        .unwrap_or_else(Utc::now);
    let updated = read_utc(ev, "LAST-MODIFIED")
        .or_else(|| read_utc(ev, "DTSTAMP"))
        .unwrap_or(created);

    let reminders = parse_valarms(ev);

    // Encode the server href into the id (`{href}|{uid}`) when we have
    // it, so removed hrefs from a sync-collection delta map onto the
    // cache's native_id; without it, fall back to the bare UID.
    let id = match href {
        Some(h) if !h.is_empty() => format!("{h}|{uid}"),
        _ => uid.to_string(),
    };

    Ok(Event {
        id,
        calendar_id: calendar_id.to_string(),
        title: summary,
        description,
        location,
        start,
        end,
        all_day,
        recurrence,
        color_label: None,
        reminders,
        sound: None,
        attendees: Vec::new(),
        created_at: created,
        updated_at: updated,
        etag: None,
    })
}

/// Walk every child component on a VEVENT, pick out the VALARMs, and
/// translate each into a `cal_core::Reminder`. Unknown actions
/// (PROCEDURE, X-…) are skipped because the local Reminder engine
/// has no place to land them; the server keeps them on the row
/// untouched the next time we read the same VEVENT.
fn parse_valarms(ev: &icalendar::Event) -> Vec<Reminder> {
    let mut out = Vec::new();
    for child in ev.components() {
        if child.component_kind() != "VALARM" {
            continue;
        }
        let action = child
            .property_value("ACTION")
            .map(|s| s.to_ascii_uppercase())
            .unwrap_or_default();
        let Some(trigger_prop) = child.properties().get("TRIGGER") else {
            continue;
        };
        let kind = match resolve_trigger(trigger_prop, &action) {
            Some(k) => k,
            None => continue,
        };
        out.push(Reminder { kind, sound: None });
    }
    out
}

/// Decide whether `TRIGGER` carries an absolute date-time or a
/// relative ISO 8601 duration, and combine that with the ACTION
/// to pick the right `ReminderKind`.
fn resolve_trigger(trigger: &icalendar::Property, action: &str) -> Option<ReminderKind> {
    let raw = trigger.value();
    let is_absolute = trigger
        .params()
        .get("VALUE")
        .map(|p| p.value().eq_ignore_ascii_case("DATE-TIME"))
        .unwrap_or(false)
        || looks_like_compact_utc(raw);

    if is_absolute {
        // VALUE=DATE-TIME triggers fire at an exact wall-clock time.
        // Aperio's Email reminder is always relative-to-event, so an
        // absolute trigger maps to Absolute regardless of action.
        let at = parse_compact_utc(raw)?;
        return Some(ReminderKind::Absolute { at });
    }

    let minutes_before = parse_iso_duration_to_minutes_before(raw)?;
    if action == "EMAIL" {
        Some(ReminderKind::Email { minutes_before })
    } else {
        // DISPLAY / AUDIO / anything that doesn't say "EMAIL" maps
        // to a local pop-up reminder. The user can change the kind
        // in the Aperio editor later.
        Some(ReminderKind::Relative { minutes_before })
    }
}

fn looks_like_compact_utc(s: &str) -> bool {
    // Crude shape check: 16 chars ending with Z and containing T.
    // Good enough to disambiguate triggers that don't carry the
    // VALUE=DATE-TIME parameter (some servers omit it).
    s.len() == 16 && s.ends_with('Z') && s.contains('T')
}

fn parse_compact_utc(s: &str) -> Option<DateTime<Utc>> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ")
        .ok()
        .map(|dt| Utc.from_utc_datetime(&dt))
}

/// Parse an RFC 5545 ISO 8601 `DURATION` string into Aperio's
/// "minutes before the reference" convention. iCal's
/// `TRIGGER:-PT1H` ("fire 1 hour before") becomes
/// `minutes_before = 60`. A positive iCal duration (fire *after*
/// the reference) becomes a negative `minutes_before`.
///
/// We support the subset of RFC 5545 that real-world servers emit:
/// optional sign, `P`, optional `<n>D`, optional `T<n>H<n>M<n>S`.
/// Anything else returns `None` and the caller drops the alarm.
fn parse_iso_duration_to_minutes_before(raw: &str) -> Option<i64> {
    let (sign, rest) = if let Some(r) = raw.strip_prefix('-') {
        (-1i64, r)
    } else if let Some(r) = raw.strip_prefix('+') {
        (1i64, r)
    } else {
        (1i64, raw)
    };
    let rest = rest.strip_prefix('P')?;
    let (days_part, time_part) = match rest.find('T') {
        Some(idx) => (&rest[..idx], &rest[idx + 1..]),
        None => (rest, ""),
    };
    let mut total_minutes: i64 = 0;
    if !days_part.is_empty() {
        let days: i64 = days_part.strip_suffix('D')?.parse().ok()?;
        total_minutes += days * 1440;
    }
    let mut buf = String::new();
    for c in time_part.chars() {
        if c.is_ascii_digit() {
            buf.push(c);
            continue;
        }
        if buf.is_empty() {
            return None;
        }
        let n: i64 = buf.parse().ok()?;
        buf.clear();
        match c {
            'H' => total_minutes += n * 60,
            'M' => total_minutes += n,
            // Round seconds to the nearest minute — Aperio's reminder
            // resolution is one minute and CalDAV servers very rarely
            // emit sub-minute triggers.
            'S' => total_minutes += (n + 30) / 60,
            _ => return None,
        }
    }
    if !buf.is_empty() {
        // Trailing digits with no unit suffix — malformed.
        return None;
    }
    // iCal sign convention: negative duration = before reference.
    // Aperio's minutes_before convention: positive = before.
    // Flip the sign to get from one to the other.
    Some(-sign * total_minutes)
}

/// Pull DTSTART and DTEND into UTC. Several shapes are possible:
///   - DATE-TIME with explicit UTC ("Z" suffix) — most common
///   - DATE-TIME with a TZID — converted to UTC via chrono-tz
///     (icalendar already does this when it can; we accept naive
///     and assume UTC as a last-resort fallback)
///   - DATE without time — all-day event; we anchor at 00:00 UTC
fn resolve_range(ev: &icalendar::Event) -> CaldavResult<(DateTime<Utc>, DateTime<Utc>, bool)> {
    let start = ev
        .get_start()
        .ok_or_else(|| CaldavError::Protocol("VEVENT without DTSTART".into()))?;
    let (start_utc, all_day) = datetime_to_utc(start);
    // Fall back to DTSTART when DTEND is missing — RFC 5545 §3.8.2.2
    // says a missing DTEND means a zero-duration event for date-time
    // and "until end of day" for date. We honour both.
    let end_utc = match ev.get_end() {
        Some(end) => datetime_to_utc(end).0,
        None => {
            if all_day {
                start_utc + chrono::Duration::days(1)
            } else {
                start_utc
            }
        }
    };
    Ok((start_utc, end_utc, all_day))
}

fn datetime_to_utc(value: DatePerhapsTime) -> (DateTime<Utc>, bool) {
    match value {
        DatePerhapsTime::Date(d) => (naive_date_to_utc(d), true),
        DatePerhapsTime::DateTime(dt) => {
            // icalendar's CalendarDateTime is an enum with three
            // shapes (UTC / local-with-tz / floating). We normalise
            // each to UTC; floating times are read as UTC because
            // there's no better answer without per-event tz config.
            match dt {
                icalendar::CalendarDateTime::Utc(u) => (u, false),
                icalendar::CalendarDateTime::WithTimezone { date_time, tzid } => {
                    let resolved = resolve_with_tzid(date_time, &tzid)
                        .unwrap_or_else(|| Utc.from_utc_datetime(&date_time));
                    (resolved, false)
                }
                icalendar::CalendarDateTime::Floating(naive) => {
                    (Utc.from_utc_datetime(&naive), false)
                }
            }
        }
    }
}

fn naive_date_to_utc(d: NaiveDate) -> DateTime<Utc> {
    let midnight = NaiveDateTime::new(d, NaiveTime::from_hms_opt(0, 0, 0).unwrap());
    Utc.from_utc_datetime(&midnight)
}

fn resolve_with_tzid(naive: NaiveDateTime, tzid: &str) -> Option<DateTime<Utc>> {
    use chrono_tz::Tz;
    let tz: Tz = tzid.parse().ok()?;
    let local = tz.from_local_datetime(&naive).single()?;
    Some(local.with_timezone(&Utc))
}

fn read_utc(ev: &icalendar::Event, prop: &str) -> Option<DateTime<Utc>> {
    let raw = ev.property_value(prop)?;
    // Common format: YYYYMMDDTHHMMSSZ (UTC). Falls back to
    // YYYYMMDD-only when servers emit pure dates.
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%SZ") {
        return Some(Utc.from_utc_datetime(&dt));
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(raw, "%Y%m%d") {
        return Some(naive_date_to_utc(d));
    }
    // RFC 3339 — some servers like to be helpful.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    None
}

fn collect_exdates(ev: &icalendar::Event) -> Vec<DateTime<Utc>> {
    // EXDATE is RFC 5545 a "multi-property": it can appear several
    // times on one VEVENT and each occurrence may carry comma-
    // separated values. icalendar stores those under
    // `multi_properties()` rather than the regular `properties()`
    // map, so `property_value("EXDATE")` would return None even
    // when EXDATE lines exist.
    let Some(props) = ev.multi_properties().get("EXDATE") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for prop in props {
        for token in prop.value().split(',') {
            let trimmed = token.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y%m%dT%H%M%SZ") {
                out.push(Utc.from_utc_datetime(&dt));
            } else if let Ok(d) = chrono::NaiveDate::parse_from_str(trimmed, "%Y%m%d") {
                out.push(naive_date_to_utc(d));
            }
        }
    }
    out
}

/// Render an event into a single VCALENDAR/VEVENT body suitable for
/// PUT to a CalDAV resource URL. The UID is supplied separately so
/// the caller can pick it from either an existing event (update) or
/// a fresh UUID (create).
///
/// Only the same fields the *reader* understands are emitted —
/// emitting more would round-trip data the rest of Aperio can't see
/// anyway and the next read would silently drop them.
pub fn new_event_to_ical(uid: &str, event: &NewEvent) -> String {
    let mut ical_ev = icalendar::Event::new();
    apply_common(&mut ical_ev, uid, event);
    let mut cal = ICalendar::new();
    cal.push(ical_ev.done());
    cal.to_string()
}

/// Render an existing event back to iCal for the update PUT.
pub fn event_to_ical(event: &Event) -> String {
    let new = NewEvent {
        title: event.title.clone(),
        description: event.description.clone(),
        location: event.location.clone(),
        start: event.start,
        end: event.end,
        all_day: event.all_day,
        recurrence: event.recurrence.clone(),
        color_label: event.color_label.clone(),
        reminders: event.reminders.clone(),
        sound: event.sound.clone(),
        attendees: event.attendees.clone(),
    };
    // The iCalendar `UID` must be the bare provider UID — NOT our composite
    // `{href}|{uid}` row id. Writing the composite changes the event's UID
    // on the server: for an event whose resource name differs from its UID
    // (e.g. anything created on an iPhone), iCloud then treats the PUT as a
    // *different* event and spawns a duplicate. `decode_event_id` yields the
    // bare uid for both composite and legacy bare ids.
    let (_, uid) = decode_event_id(&event.id);
    new_event_to_ical(uid, &new)
}

fn apply_common(ical_ev: &mut icalendar::Event, uid: &str, event: &NewEvent) {
    ical_ev.uid(uid);
    ical_ev.summary(&event.title);
    if let Some(desc) = &event.description {
        ical_ev.description(desc);
    }
    if let Some(loc) = &event.location {
        ical_ev.location(loc);
    }
    if event.all_day {
        ical_ev.starts(event.start.date_naive());
        // For all-day events DTEND is exclusive — RFC 5545 §3.6.1.
        // Aperio stores end as the start-of-next-day already, so
        // emitting `event.end.date_naive()` is exactly the right
        // exclusive boundary.
        ical_ev.ends(event.end.date_naive());
    } else {
        ical_ev.starts(event.start);
        ical_ev.ends(event.end);
    }
    if let Some(rec) = &event.recurrence {
        ical_ev.add_property("RRULE", &rec.rrule);
        for exdate in &rec.exceptions {
            ical_ev.add_multi_property("EXDATE", &format_utc_compact(*exdate));
        }
    }
    // VALARM children — one per Reminder so iCloud's iOS / Alexa
    // bridge sees the reminders we set, and we keep their default
    // ones when we round-trip an event we read back.
    for reminder in &event.reminders {
        if let Some(alarm) = reminder_to_alarm(reminder, &event.title) {
            ical_ev.alarm(alarm);
        }
    }
    // DTSTAMP is mandatory per RFC 5545 — icalendar adds it
    // automatically on serialise.
}

/// Translate a `cal_core::Reminder` into a VALARM component. Returns
/// `None` for kinds that have no iCal equivalent (currently only
/// `AppStart` — that's an Aperio-local concept the server has no
/// place for, and forging a fake VALARM would mislead the bridge
/// devices).
fn reminder_to_alarm(reminder: &Reminder, fallback_summary: &str) -> Option<icalendar::Alarm> {
    use icalendar::{Alarm, Trigger};

    let summary = if fallback_summary.is_empty() {
        "Aperio reminder".to_string()
    } else {
        fallback_summary.to_string()
    };

    match &reminder.kind {
        ReminderKind::Relative { minutes_before } => {
            // chrono::Duration is what icalendar's Trigger consumes.
            // Convert our minutes_before (positive = before) to the
            // signed iCal duration (negative = before).
            let dur = chrono::Duration::minutes(-*minutes_before);
            Some(Alarm::display(&summary, Trigger::from(dur)).done())
        }
        ReminderKind::Absolute { at } => Some(Alarm::display(&summary, Trigger::from(*at)).done()),
        ReminderKind::Email { minutes_before } => {
            // icalendar 0.16 doesn't expose a typed `Alarm::email`
            // constructor yet (the source carries a TODO). We build
            // a DISPLAY alarm and override ACTION to EMAIL by hand
            // so the wire format still says ACTION:EMAIL.
            let dur = chrono::Duration::minutes(-*minutes_before);
            let mut alarm = Alarm::display(&summary, Trigger::from(dur));
            alarm.add_property("ACTION", "EMAIL");
            Some(alarm.done())
        }
        ReminderKind::AppStart => None,
    }
}

fn format_utc_compact(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%dT%H%M%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_a_minimal_utc_event() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:event-1@aperio\r
SUMMARY:Standup\r
DTSTART:20260520T080000Z\r
DTEND:20260520T083000Z\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.id, "event-1@aperio");
        assert_eq!(ev.calendar_id, "cal-1");
        assert_eq!(ev.title, "Standup");
        assert_eq!(
            ev.start,
            Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap()
        );
        assert_eq!(ev.end, Utc.with_ymd_and_hms(2026, 5, 20, 8, 30, 0).unwrap());
        assert!(!ev.all_day);
    }

    #[test]
    fn event_to_ical_writes_bare_uid_not_the_composite_row_id() {
        // A synced iCloud event whose resource href differs from its UID
        // (the normal case for anything created on an iPhone): the row id is
        // the composite `{href}|{uid}`. The update PUT body's UID must be the
        // BARE uid — writing the composite changes the event's UID on the
        // server, and iCloud then treats the PUT as a different event and
        // spawns a duplicate.
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:REAL-UID-9876\r
SUMMARY:Lunch\r
DTSTART:20260520T080000Z\r
DTEND:20260520T083000Z\r
END:VEVENT\r
END:VCALENDAR\r
";
        let href = "/calendars/home/ABCDEF12-3456.ics";
        let events =
            parse_calendar_data_with_href(body, "https://example.test/calendars/home/", Some(href))
                .unwrap();
        let ev = &events[0];
        // Precondition: the row id is the composite, not the bare uid.
        assert_eq!(ev.id, format!("{href}|REAL-UID-9876"));

        let ical = event_to_ical(ev);
        assert!(
            ical.contains("UID:REAL-UID-9876"),
            "expected the bare UID in the PUT body, got:\n{ical}"
        );
        assert!(
            !ical.contains(&format!("UID:{href}")),
            "the composite row id leaked into the iCalendar UID:\n{ical}"
        );
    }

    #[test]
    fn maps_an_all_day_event() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:birthday@aperio\r
SUMMARY:Birthday\r
DTSTART;VALUE=DATE:20260520\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        let ev = &events[0];
        assert!(ev.all_day);
        assert_eq!(
            ev.start,
            Utc.with_ymd_and_hms(2026, 5, 20, 0, 0, 0).unwrap()
        );
        // Missing DTEND on an all-day event: end of day.
        assert_eq!(ev.end, Utc.with_ymd_and_hms(2026, 5, 21, 0, 0, 0).unwrap());
    }

    #[test]
    fn maps_rrule_and_exdate() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:weekly@aperio\r
SUMMARY:Weekly\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
RRULE:FREQ=WEEKLY;BYDAY=WE\r
EXDATE:20260603T080000Z\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        let ev = &events[0];
        let rec = ev.recurrence.as_ref().unwrap();
        assert!(rec.rrule.contains("FREQ=WEEKLY"));
        assert_eq!(rec.exceptions.len(), 1);
        assert_eq!(
            rec.exceptions[0],
            Utc.with_ymd_and_hms(2026, 6, 3, 8, 0, 0).unwrap()
        );
    }

    #[test]
    fn write_then_read_roundtrips_an_event() {
        let event = NewEvent {
            title: "Sprint planning".into(),
            description: Some("agenda in the doc".into()),
            location: Some("room 3.12".into()),
            start: Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap(),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=WE".into(),
                exceptions: vec![Utc.with_ymd_and_hms(2026, 6, 3, 8, 0, 0).unwrap()],
            }),
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
        };
        let uid = "abcdef-12345@aperio";
        let body = new_event_to_ical(uid, &event);
        // The reader must see exactly the fields we wrote.
        let parsed = parse_calendar_data(&body, "cal-1").unwrap();
        assert_eq!(parsed.len(), 1);
        let read = &parsed[0];
        assert_eq!(read.id, uid);
        assert_eq!(read.title, "Sprint planning");
        assert_eq!(read.description.as_deref(), Some("agenda in the doc"));
        assert_eq!(read.location.as_deref(), Some("room 3.12"));
        assert_eq!(read.start, event.start);
        assert_eq!(read.end, event.end);
        let rec = read.recurrence.as_ref().unwrap();
        assert!(rec.rrule.contains("WEEKLY"));
        assert_eq!(rec.exceptions.len(), 1);
    }

    #[test]
    fn write_all_day_uses_value_date() {
        let event = NewEvent {
            title: "Birthday".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 20, 0, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 21, 0, 0, 0).unwrap(),
            all_day: true,
            recurrence: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
        };
        let body = new_event_to_ical("bday-uid", &event);
        assert!(
            body.contains("DTSTART;VALUE=DATE:20260520"),
            "expected VALUE=DATE DTSTART, got: {body}",
        );
    }

    #[test]
    fn reads_icloud_style_display_alarm_one_hour_before() {
        // Shape iCloud uses for its default reminder: VALARM with
        // ACTION:DISPLAY + TRIGGER:-PT1H inside the VEVENT.
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:meeting@aperio\r
SUMMARY:Meeting\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:Meeting\r
TRIGGER:-PT1H\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        assert_eq!(events.len(), 1);
        let reminders = &events[0].reminders;
        assert_eq!(reminders.len(), 1);
        match &reminders[0].kind {
            ReminderKind::Relative { minutes_before } => {
                assert_eq!(*minutes_before, 60);
            }
            other => panic!("expected Relative, got {other:?}"),
        }
    }

    #[test]
    fn reads_day_before_and_absolute_alarms() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:multi-alarm@aperio\r
SUMMARY:Conference\r
DTSTART:20260520T080000Z\r
DTEND:20260520T180000Z\r
BEGIN:VALARM\r
ACTION:AUDIO\r
TRIGGER:-P1D\r
END:VALARM\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:Pack laptop\r
TRIGGER;VALUE=DATE-TIME:20260519T200000Z\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        let reminders = &events[0].reminders;
        assert_eq!(reminders.len(), 2);
        assert!(matches!(
            reminders[0].kind,
            ReminderKind::Relative {
                minutes_before: 1440
            }
        ));
        match &reminders[1].kind {
            ReminderKind::Absolute { at } => {
                assert_eq!(*at, Utc.with_ymd_and_hms(2026, 5, 19, 20, 0, 0).unwrap());
            }
            other => panic!("expected Absolute, got {other:?}"),
        }
    }

    #[test]
    fn maps_email_action_to_email_reminder() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:email-alarm@aperio\r
SUMMARY:Renewal\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
BEGIN:VALARM\r
ACTION:EMAIL\r
ATTENDEE:mailto:user@example.com\r
DESCRIPTION:Renewal due\r
SUMMARY:Renewal\r
TRIGGER:-PT15M\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        match &events[0].reminders[0].kind {
            ReminderKind::Email { minutes_before } => {
                assert_eq!(*minutes_before, 15);
            }
            other => panic!("expected Email, got {other:?}"),
        }
    }

    #[test]
    fn skips_unknown_actions() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:procedure@aperio\r
SUMMARY:Legacy\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
BEGIN:VALARM\r
ACTION:PROCEDURE\r
TRIGGER:-PT5M\r
END:VALARM\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
TRIGGER:-PT10M\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
";
        // We treat PROCEDURE the same as any other DISPLAY-shaped
        // reminder (relative trigger, local pop-up) because dropping
        // it would lose information. EMAIL is the only action
        // category we treat specially.
        let events = parse_calendar_data(body, "cal-1").unwrap();
        assert_eq!(events[0].reminders.len(), 2);
    }

    #[test]
    fn round_trips_reminders_through_write_then_read() {
        let event = NewEvent {
            title: "Sync".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            reminders: vec![
                Reminder {
                    kind: ReminderKind::Relative { minutes_before: 60 },
                    sound: None,
                },
                Reminder {
                    kind: ReminderKind::Absolute {
                        at: Utc.with_ymd_and_hms(2026, 5, 20, 7, 30, 0).unwrap(),
                    },
                    sound: None,
                },
            ],
            sound: None,
            attendees: Vec::new(),
        };
        let body = new_event_to_ical("round-trip-uid", &event);
        let parsed = parse_calendar_data(&body, "cal-1").unwrap();
        let reminders = &parsed[0].reminders;
        assert_eq!(reminders.len(), 2);
        assert!(matches!(
            reminders[0].kind,
            ReminderKind::Relative { minutes_before: 60 }
        ));
        assert!(matches!(reminders[1].kind, ReminderKind::Absolute { .. }));
    }

    #[test]
    fn parses_duration_edge_cases() {
        assert_eq!(parse_iso_duration_to_minutes_before("-PT1H"), Some(60));
        assert_eq!(parse_iso_duration_to_minutes_before("-PT15M"), Some(15));
        assert_eq!(parse_iso_duration_to_minutes_before("-P1D"), Some(1440));
        assert_eq!(
            parse_iso_duration_to_minutes_before("-P1DT12H"),
            Some(1440 + 720)
        );
        assert_eq!(parse_iso_duration_to_minutes_before("-PT1H30M"), Some(90));
        // No sign = positive iCal duration = fires *after* reference,
        // so minutes_before is negative.
        assert_eq!(parse_iso_duration_to_minutes_before("PT15M"), Some(-15));
        // Garbage input returns None.
        assert_eq!(parse_iso_duration_to_minutes_before("foo"), None);
        assert_eq!(parse_iso_duration_to_minutes_before("-PT"), Some(0));
        assert_eq!(parse_iso_duration_to_minutes_before("-PT5X"), None);
    }

    /// Real iCloud VEVENT bodies — paraphrased from a live capture —
    /// include several X-WR-* / X-APPLE-* properties on the VEVENT
    /// itself AND on the VALARM. Some carriers also folded the
    /// DESCRIPTION line. The parser has to walk past all of that and
    /// still pick out the relative TRIGGER on the inner VALARM.
    #[test]
    fn reads_real_world_icloud_vevent_with_extra_properties() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//Apple Inc.//iCloud Calendar//EN\r
CALSCALE:GREGORIAN\r
X-WR-CALNAME:Home\r
BEGIN:VEVENT\r
UID:91FA80F1-1234-5678-9ABC-DEF012345678\r
DTSTAMP:20260520T060000Z\r
CREATED:20260519T120000Z\r
LAST-MODIFIED:20260519T120000Z\r
SUMMARY:Zahnarzt\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
SEQUENCE:0\r
X-APPLE-DEFAULT-ALARM:TRUE\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:Event reminder\r
TRIGGER:-PT15M\r
UID:91FA80F1-AAAA-BBBB-CCCC-DDDDDDDDDDDD\r
X-WR-ALARMUID:91FA80F1-AAAA-BBBB-CCCC-DDDDDDDDDDDD\r
X-APPLE-DEFAULT-ALARM:TRUE\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        assert_eq!(events.len(), 1, "expected one VEVENT");
        let reminders = &events[0].reminders;
        assert_eq!(reminders.len(), 1, "iCloud VALARM was dropped during parse",);
        match &reminders[0].kind {
            ReminderKind::Relative { minutes_before } => {
                assert_eq!(*minutes_before, 15);
            }
            other => panic!("expected Relative reminder, got {other:?}"),
        }
    }

    #[test]
    fn ignores_non_vevent_components() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VTODO\r
UID:task@aperio\r
SUMMARY:Task\r
END:VTODO\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        assert!(events.is_empty());
    }
}
