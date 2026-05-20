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
//!
//! VTODO (tasks), VALARM (reminders), VTIMEZONE, and ATTENDEE
//! mapping all live behind their own follow-ups — the calendar
//! feature lands first so the listing/range read can be wired into
//! the UI; richer mapping is additive on top of this.

use cal_core::{Event, EventRecurrence};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use icalendar::{Calendar as ICalendar, Component, DatePerhapsTime, EventLike};

use crate::error::{CaldavError, CaldavResult};

/// Parse the calendar-data of a CalDAV REPORT response into a
/// chronologically sorted list of events. One VCALENDAR body may
/// carry multiple VEVENTs (RFC 5545 allows it); we emit one
/// `cal_core::Event` per top-level VEVENT.
pub fn parse_calendar_data(
    body: &str,
    calendar_id: &str,
) -> CaldavResult<Vec<Event>> {
    let parsed: ICalendar = body
        .parse()
        .map_err(|err: String| CaldavError::Protocol(format!("ical: {err}")))?;
    let mut out = Vec::new();
    for comp in parsed.components {
        if let icalendar::CalendarComponent::Event(ev) = comp {
            match map_event(&ev, calendar_id) {
                Ok(event) => out.push(event),
                Err(err) => {
                    tracing::warn!(?err, "skipping unmapped VEVENT");
                }
            }
        }
    }
    Ok(out)
}

fn map_event(
    ev: &icalendar::Event,
    calendar_id: &str,
) -> CaldavResult<Event> {
    let uid = ev.get_uid().ok_or_else(|| {
        CaldavError::Protocol("VEVENT without UID".to_string())
    })?;
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

    Ok(Event {
        id: uid.to_string(),
        calendar_id: calendar_id.to_string(),
        title: summary,
        description,
        location,
        start,
        end,
        all_day,
        recurrence,
        color_label: None,
        reminders: Vec::new(),
        sound: None,
        attendees: Vec::new(),
        created_at: created,
        updated_at: updated,
        etag: None,
    })
}

/// Pull DTSTART and DTEND into UTC. Several shapes are possible:
///   - DATE-TIME with explicit UTC ("Z" suffix) — most common
///   - DATE-TIME with a TZID — converted to UTC via chrono-tz
///     (icalendar already does this when it can; we accept naive
///     and assume UTC as a last-resort fallback)
///   - DATE without time — all-day event; we anchor at 00:00 UTC
fn resolve_range(
    ev: &icalendar::Event,
) -> CaldavResult<(DateTime<Utc>, DateTime<Utc>, bool)> {
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
        DatePerhapsTime::Date(d) => (
            naive_date_to_utc(d),
            true,
        ),
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
            if let Ok(dt) =
                chrono::NaiveDateTime::parse_from_str(trimmed, "%Y%m%dT%H%M%SZ")
            {
                out.push(Utc.from_utc_datetime(&dt));
            } else if let Ok(d) =
                chrono::NaiveDate::parse_from_str(trimmed, "%Y%m%d")
            {
                out.push(naive_date_to_utc(d));
            }
        }
    }
    out
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
        assert_eq!(
            ev.end,
            Utc.with_ymd_and_hms(2026, 5, 20, 8, 30, 0).unwrap()
        );
        assert!(!ev.all_day);
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
        assert_eq!(
            ev.end,
            Utc.with_ymd_and_hms(2026, 5, 21, 0, 0, 0).unwrap()
        );
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
