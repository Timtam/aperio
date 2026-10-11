//! Google Calendar JSON ⇄ cal_core conversion.
//!
//! Google's API is documented at
//! <https://developers.google.com/calendar/api/v3/reference>. The
//! response shapes we map here:
//!
//!   CalendarListEntry  → cal_core::Calendar
//!   Event              → cal_core::Event
//!
//! Reminders (`reminders.overrides[]`) and VTODO-equivalent
//! (`tasks` API, not Calendar API) land in Phase 6d.2.

use cal_core::{
    AttendeeStatus, Calendar, ColorSource, ContainerColor, Event, EventRecurrence, NewEvent,
    Reminder, ReminderKind,
};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use cal_core::event_diff::EventField;
use chrono::{
    DateTime, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone,
    Utc,
};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::{GoogleError, GoogleResult};

// ── Calendar list ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CalendarListResponse {
    #[serde(default)]
    pub items: Vec<CalendarListEntry>,
    /// Present when there are more entries; we paginate by passing
    /// this back as the `pageToken` query parameter.
    #[serde(default, rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CalendarListEntry {
    pub id: String,
    pub summary: String,
    /// Hex colour Google associates with this calendar in the user's
    /// settings. Present on most rows.
    #[serde(default, rename = "backgroundColor")]
    pub background_color: Option<String>,
    /// `"owner"`, `"writer"`, `"reader"`, `"freeBusyReader"`. We
    /// treat anything < writer as read-only.
    #[serde(default, rename = "accessRole")]
    pub access_role: Option<String>,
}

/// Convert one CalendarListEntry to cal_core::Calendar.
pub fn map_calendar(entry: CalendarListEntry) -> Calendar {
    let color = entry.background_color.and_then(parse_hex_color);
    let read_only = matches!(
        entry.access_role.as_deref(),
        Some("reader") | Some("freeBusyReader")
    );
    Calendar {
        // Google always schedules server-side; emailing is per-request via
        // the `sendUpdates` query param.
        supports_scheduling: true,
        // Google's per-event colorId isn't mapped into Aperio's color model;
        // per-event colors stay host-local overrides.
        supports_event_color: false,
        always_notifies_attendees: false,
        invitations_reply_only: false,
        stores_occurrence_exceptions: true,
        notifier_name: None,
        color_label: None,
        id: entry.id,
        name: entry.summary,
        color,
        read_only,
        default_sound: None,
    }
}

// ── Recurrence lines ────────────────────────────────────────────────────

/// One line of an event's `recurrence` array, split the way RFC 5545 splits a
/// content line: its name, its parameters, and its value after the first `:`
/// outside quotes — `EXDATE;TZID="Europe/Berlin":20260601T090000`. Names and
/// parameter names are upper-cased; a parameter's quotes are dropped.
struct RecurrenceLine<'a> {
    name: String,
    params: Vec<(String, String)>,
    value: &'a str,
}

impl RecurrenceLine<'_> {
    fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// `line` split into its parts, or `None` when it has no value at all.
fn recurrence_line(line: &str) -> Option<RecurrenceLine<'_>> {
    let mut quoted = false;
    let mut head_parts = Vec::new();
    let mut part_start = 0;
    let mut colon = None;
    for (at, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                head_parts.push(&line[part_start..at]);
                part_start = at + 1;
            }
            ':' if !quoted => {
                colon = Some(at);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    head_parts.push(&line[part_start..colon]);
    let mut head = head_parts.into_iter();
    let name = head.next()?.trim().to_ascii_uppercase();
    let params = head
        .filter_map(|param| {
            let (key, value) = param.split_once('=')?;
            Some((
                key.trim().to_ascii_uppercase(),
                value.trim().trim_matches('"').to_string(),
            ))
        })
        .collect();
    Some(RecurrenceLine {
        name,
        params,
        value: line[colon + 1..].trim(),
    })
}

/// The zone a series' wall clocks are read in: none for a series of days or
/// one on UTC, its zone, or the reason it has none Aperio can use — a name
/// tzdata does not know, which costs a wall clock that needs it.
type SeriesZone = Result<Option<Tz>, &'static str>;

/// The zone tzdata names `name`, in any ASCII case and by any of its links.
fn zone_named(name: &str) -> Option<Tz> {
    cal_core::canonical_zone(name)?.parse().ok()
}

/// The instant a wall-clock time names in `tz`, as RFC 5545 reads one
/// (section 3.3.5) and as the views place an occurrence (`wallToReal` in
/// `shared/recurrence.ts`): a time the clock shows twice is its first
/// reading; a time a clock change skips is read with the offset from before
/// the change. A timed deletion has to land on the occurrence's instant
/// exactly, or it cancels nothing.
pub(crate) fn wall_clock_in(tz: Tz, wall: NaiveDateTime) -> DateTime<Utc> {
    match tz.from_local_datetime(&wall) {
        LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => at.with_timezone(&Utc),
        LocalResult::None => {
            let before = tz
                .offset_from_utc_datetime(&(wall - Duration::days(1)))
                .fix();
            Utc.from_utc_datetime(&(wall - Duration::seconds(before.local_minus_utc().into())))
        }
    }
}

/// A date as Aperio anchors every all-day boundary: the instant of its LOCAL
/// midnight — the app-internal all-day convention shared with the CalDAV
/// adapter, so the views' local-day bucketing and the write-side round-trip
/// line up in any timezone. A zone that skips midnight that day falls back to
/// UTC midnight.
fn local_midnight(day: NaiveDate) -> DateTime<Utc> {
    let midnight = day.and_time(NaiveTime::MIN);
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|l| l.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&midnight))
}

/// The day an all-day slot names — an occurrence's, or a deletion's.
///
/// Aperio anchors an all-day date at local midnight, but a slot does not always
/// sit there. Since 48a an all-day series repeats on the device's calendar
/// days, so its slots ARE local midnight; a slot written by a build before
/// that, or by another client, can still lie an hour or two off it across a
/// clock change. A date whose midnight a clock change skips resolves to that
/// date's UTC midnight ([`local_midnight`]), which west of UTC reads as the
/// evening before. All of them stay within hours of the local midnight they
/// stand for (decision 95), so the day is the one whose midnight lies nearest:
/// twelve hours on, then the local date.
pub(crate) fn all_day_slot(slot: DateTime<Utc>) -> NaiveDate {
    (slot + Duration::hours(12))
        .with_timezone(&Local)
        .date_naive()
}

/// The deletions one EXDATE line names, and why some of it names none Aperio
/// can read.
///
/// Every spelling Google is seen to return, with one or several values:
/// - a date (`20261230`, with or without `VALUE=DATE`): on a series of days
///   its local midnight, as a date start is read. Google ignores a date on a
///   timed series, and so does Aperio;
/// - a UTC date-time (`20260601T070000Z`, with or without
///   `VALUE=DATE-TIME`): that instant. On a series of days, UTC midnight is
///   its date (its local midnight), the form Aperio wrote a date deletion in
///   before decision 205;
/// - a wall clock in a zone (`TZID=Europe/Berlin:20260601T090000`), or
///   without one, in the zone the series repeats in (`start.timeZone`, UTC
///   when it has none): the instant of that wall clock, as Google's own
///   export writes a deletion. On a series of days, its date.
///
/// A zone tzdata does not know costs only the values that need it — a wall
/// clock on a timed series; a UTC instant and a date do not.
fn exdates_of_line(
    line: &RecurrenceLine<'_>,
    all_day: bool,
    series_zone: SeriesZone,
) -> (Vec<DateTime<Utc>>, Option<&'static str>) {
    let zone = match line.param("TZID") {
        Some(name) => zone_named(name)
            .map(Some)
            .ok_or("a zone tzdata does not know"),
        None => series_zone,
    };
    let mut read = Vec::new();
    let mut problem = None;
    for raw in line.value.split(',').map(str::trim) {
        if raw.len() == 8 {
            match NaiveDate::parse_from_str(raw, "%Y%m%d") {
                Ok(day) if all_day => read.push(local_midnight(day)),
                Ok(_) => problem = Some("a date on a timed series, which Google ignores"),
                Err(_) => problem = Some("a value that is no date or date-time"),
            }
        } else if let Ok(utc) = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%SZ") {
            // On a series of days, UTC midnight is its date: how Aperio read
            // and wrote a date deletion before decision 205, and the local
            // midnight of a device on UTC. No device's local midnight of
            // another date falls on it. Read as an instant, it named the next
            // day from UTC+12 on.
            read.push(if all_day && utc.time() == NaiveTime::MIN {
                local_midnight(utc.date())
            } else {
                Utc.from_utc_datetime(&utc)
            });
        } else if let Ok(wall) = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S") {
            match zone {
                _ if all_day => read.push(local_midnight(wall.date())),
                Ok(Some(tz)) => read.push(wall_clock_in(tz, wall)),
                Ok(None) => read.push(Utc.from_utc_datetime(&wall)),
                Err(why) => problem = Some(why),
            }
        } else {
            problem = Some("a value that is no date or date-time");
        }
    }
    (read, problem)
}

/// A recurring master's rule and its deletions, read from Google's
/// `recurrence` lines in any order and any spelling of their names. A line
/// Aperio does not keep (RDATE, EXRULE, an RRULE before the last one, an
/// EXDATE it cannot read) is named in the log, once per run: a save that
/// writes the lines anew leaves it out — every save that is not proven to
/// keep this series' start, end, all-day flag and repeat ([`event_to_body`]).
fn read_recurrence(
    series: &str,
    lines: &[String],
    all_day: bool,
    series_zone: SeriesZone,
) -> (Option<String>, Vec<DateTime<Utc>>) {
    let mut rrule: Option<(String, &str)> = None;
    let mut exdates = Vec::new();
    for raw in lines {
        let Some(line) = recurrence_line(raw) else {
            unread_line(series, raw, "no value");
            continue;
        };
        match line.name.as_str() {
            "RRULE" => {
                if let Some((_, dropped)) = rrule.replace((line.value.to_string(), raw.as_str())) {
                    unread_line(
                        series,
                        dropped,
                        "an earlier RRULE; Aperio keeps the last one",
                    );
                }
            }
            "EXDATE" => {
                let (read, problem) = exdates_of_line(&line, all_day, series_zone);
                exdates.extend(read);
                if let Some(problem) = problem {
                    unread_line(series, raw, problem);
                }
            }
            _ => unread_line(series, raw, "Aperio keeps only RRULE and EXDATE"),
        }
    }
    if rrule.is_none() && !exdates.is_empty() {
        unread_line(series, "EXDATE", "deletions on an event without a rule");
        exdates.clear();
    }
    (rrule.map(|(rule, _)| rule), exdates)
}

/// Warn once per run that `line` of `series` was not read, then only at
/// debug level: the list is read again every half hour, and a log that
/// repeats an expected answer is a log nobody reads.
fn unread_line(series: &str, line: &str, why: &str) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let first = SEEN
        .get_or_init(Default::default)
        .lock()
        .map(|mut seen| seen.insert(format!("{series}\n{line}")))
        .unwrap_or(true);
    if first {
        warn!(
            series = %series,
            line = %line,
            why,
            "google recurrence line not read; a save that writes this series' repeat anew leaves it out"
        );
    } else {
        debug!(series = %series, line = %line, why, "google recurrence line not read");
    }
}

/// A series' recurrence as Google's lines: its rule, and each deletion one
/// line, as Google's own export spells it (decision 205) — a date on a series
/// of days (`EXDATE;VALUE=DATE`, the only form Google's documentation allows
/// there), the wall clock in the series' zone (`EXDATE;TZID=`, the zone its
/// `start.timeZone` names), or a UTC instant on a series without one. A
/// deletion the zone's wall clock cannot name — the second pass of an hour
/// the clock shows twice — goes as its UTC instant.
fn recurrence_to_lines(rec: &EventRecurrence, all_day: bool, zone: Option<&str>) -> Vec<String> {
    let zone = cal_core::series_clock_zone(zone).and_then(|name| Some((name, zone_named(name)?)));
    let mut lines = Vec::with_capacity(1 + rec.exceptions.len());
    // Google expects the RFC 5545 prefix; the rest of Aperio stores
    // the bare rule body.
    lines.push(format!("RRULE:{}", rec.rrule));
    for &deleted in &rec.exceptions {
        lines.push(if all_day {
            format!(
                "EXDATE;VALUE=DATE:{}",
                all_day_slot(deleted).format("%Y%m%d")
            )
        } else {
            match zone {
                Some((name, tz))
                    if wall_clock_in(tz, deleted.with_timezone(&tz).naive_local()) == deleted =>
                {
                    format!(
                        "EXDATE;TZID={name}:{}",
                        deleted.with_timezone(&tz).format("%Y%m%dT%H%M%S")
                    )
                }
                _ => format!("EXDATE:{}", deleted.format("%Y%m%dT%H%M%SZ")),
            }
        });
    }
    lines
}

fn parse_hex_color(raw: String) -> Option<ContainerColor> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('#') || (trimmed.len() != 7 && trimmed.len() != 9) {
        return None;
    }
    let hex6 = &trimmed[..7];
    // Validate the hex digits — Google sometimes sends mixed case.
    if !hex6[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(ContainerColor {
        hex: hex6.to_ascii_lowercase(),
        source: ColorSource::Native,
    })
}

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EventListResponse {
    #[serde(default)]
    pub items: Vec<EventEntry>,
    #[serde(default, rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    /// Present only on the LAST page of a list response — the opaque
    /// cursor for the next incremental (`syncToken`) sync. Google omits
    /// it on intermediate pages (those carry `nextPageToken` instead).
    #[serde(default, rename = "nextSyncToken")]
    pub next_sync_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EventEntry {
    pub id: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    /// `"confirmed"`, `"tentative"`, `"cancelled"`. We skip the
    /// cancelled rows below — they're EXDATE-style deletions that
    /// our recurrence expansion handles separately.
    #[serde(default)]
    pub status: Option<String>,
    // Default so a content-less cancelled-instance tombstone (Google strips
    // start/end on those) still deserializes; `map_event` falls back to
    // `originalStartTime` for it.
    #[serde(default)]
    pub start: EventDateTime,
    #[serde(default)]
    pub end: EventDateTime,
    /// RFC 5545 RRULE / EXDATE strings. Each line is one rule.
    #[serde(default)]
    pub recurrence: Option<Vec<String>>,
    #[serde(default)]
    pub created: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated: Option<DateTime<Utc>>,
    /// On a MODIFIED single instance of a recurring event Google sends a
    /// `recurringEventId` pointing back to the master, plus `originalStartTime`
    /// (the slot it replaces). The master keeps a clean RRULE that still
    /// generates that slot, so without reconciliation the instance would render
    /// TWICE (the master's occurrence at the old time + the moved instance at the
    /// new time). We mint the override's id as `{master}::rid::{original}` so the
    /// shared frontend expander drops the master's occurrence it stands in for —
    /// the same RECURRENCE-ID scheme the CalDAV adapter uses.
    #[serde(default, rename = "recurringEventId")]
    pub recurring_event_id: Option<String>,
    /// The slot a modified instance replaces (present with `recurringEventId`).
    /// "Uniquely identifies the instance within the series even if it was moved"
    /// (Google docs) — i.e. the RECURRENCE-ID instant, NOT the moved start.
    #[serde(default, rename = "originalStartTime")]
    pub original_start_time: Option<EventDateTime>,
    /// Google's per-row ETag for optimistic concurrency control. We
    /// stash it on Event.etag so update / delete can do `If-Match`.
    #[serde(default)]
    pub etag: Option<String>,
    /// Per-event reminder overrides. When `useDefault` is true the
    /// calendar-level defaults apply and `overrides` is meaningful
    /// only if the user explicitly set per-event values *too*. We
    /// surface only the explicit overrides — calendar-level defaults
    /// are out of Aperio's UI scope today.
    #[serde(default)]
    pub reminders: Option<EventReminders>,
    /// Invitees + their RSVP state. Empty on a non-meeting event.
    #[serde(default)]
    pub attendees: Vec<EventAttendeeRead>,
    /// The organizer: their email, and `self` when the calendar this copy
    /// appears on is theirs, the connected account's own (decision 70a).
    #[serde(default)]
    pub organizer: Option<EventOrganizer>,
}

/// One attendee row on a read Google event (`event.attendees[]`).
#[derive(Debug, Deserialize)]
pub struct EventAttendeeRead {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
    /// `needsAction` | `declined` | `tentative` | `accepted`.
    #[serde(default, rename = "responseStatus")]
    pub response_status: Option<String>,
    /// Google marks the organizer's own row (decision 67a). Read-only.
    #[serde(default)]
    pub organizer: bool,
}

#[derive(Debug, Deserialize)]
pub struct EventOrganizer {
    #[serde(default)]
    pub email: Option<String>,
    /// The organizer is the calendar this copy appears on. Read-only; Google
    /// leaves it out when false.
    #[serde(default, rename = "self")]
    pub is_self: bool,
}

#[derive(Debug, Deserialize)]
pub struct EventReminders {
    #[serde(default, rename = "useDefault")]
    pub use_default: bool,
    #[serde(default)]
    pub overrides: Vec<ReminderOverride>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReminderOverride {
    /// `"popup"` (= our `Relative`) or `"email"` (= our `Email`).
    /// `"sms"` is also documented but Google's UI dropped SMS
    /// reminders years ago; we ignore the value defensively if it
    /// shows up.
    pub method: String,
    pub minutes: i64,
}

/// Either `{ "dateTime": "2026-05-25T10:00:00+02:00" }` (timed event)
/// or `{ "date": "2026-05-25" }` (all-day).
#[derive(Debug, Default, Deserialize)]
pub struct EventDateTime {
    #[serde(default, rename = "dateTime")]
    pub date_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub date: Option<NaiveDate>,
    /// IANA zone Google attaches to `dateTime` (e.g. `America/New_York`). Kept so
    /// a recurring master expands DST-correctly instead of drifting in flat UTC.
    #[serde(default, rename = "timeZone")]
    pub time_zone: Option<String>,
}

impl EventDateTime {
    /// Returns `(utc_datetime, is_all_day)`. All-day dates anchor at
    /// LOCAL midnight (expressed as a UTC instant, [`local_midnight`]).
    pub(crate) fn resolve(&self) -> GoogleResult<(DateTime<Utc>, bool)> {
        if let Some(dt) = self.date_time {
            return Ok((dt, false));
        }
        if let Some(d) = self.date {
            return Ok((local_midnight(d), true));
        }
        Err(GoogleError::Protocol(
            "event start/end has neither dateTime nor date".into(),
        ))
    }
}

// Separator between a recurring series' id and the RECURRENCE-ID instant an
// override replaces — e.g. `{master}::rid::2026-06-14T13:00:00Z`. One marker for
// every adapter, kept in the core; `shared/recurrence.ts` splits the series id
// back out and skips the master occurrence the override stands in for.
use cal_core::OVERRIDE_ID_MARKER as RECURRENCE_ID_MARKER;

/// The cal-core id for `entry`: a MODIFIED single instance of a recurring event
/// (carrying `recurringEventId` + `originalStartTime`) becomes
/// `{master}::rid::{originalStart}` so the shared expander drops the master
/// occurrence it stands in for; everything else keeps its native Google id.
fn event_id_for(
    id: String,
    recurring_event_id: Option<&str>,
    original_start: Option<&EventDateTime>,
) -> GoogleResult<String> {
    match (recurring_event_id, original_start) {
        (Some(master), Some(orig)) => {
            let (orig_utc, _) = orig.resolve()?;
            Ok(format!(
                "{master}{RECURRENCE_ID_MARKER}{}",
                orig_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            ))
        }
        _ => Ok(id),
    }
}

/// Convert one EventEntry into a cal_core::Event. Returns `Ok(None)` only for a
/// cancelled WHOLE event (a tombstone the caller removes). A cancelled recurring
/// INSTANCE is surfaced as a `cancelled` RECURRENCE-ID override: the master's
/// RRULE still generates that slot, so we need the override present to SUPPRESS it
/// (via `expandAll`) — the show-cancelled filter then hides the override itself,
/// so a deleted occurrence vanishes instead of ghosting at its old time.
pub fn map_event(entry: EventEntry, calendar_id: &str) -> GoogleResult<Option<Event>> {
    let cancelled = entry.status.as_deref() == Some("cancelled");
    // Only a recurring-instance exception carries recurringEventId +
    // originalStartTime; a cancelled row without them is a whole-event tombstone.
    let is_instance = entry.recurring_event_id.is_some() && entry.original_start_time.is_some();
    if cancelled && !is_instance {
        return Ok(None);
    }
    // A cancelled instance is often a content-less tombstone (no start/end); fall
    // back to originalStartTime — the slot it vacates and the RECURRENCE-ID the
    // override suppresses. A confirmed event/instance carries its real start.
    let (start, start_all_day) = match entry.start.resolve() {
        Ok(v) => v,
        Err(_) if is_instance => entry
            .original_start_time
            .as_ref()
            .expect("is_instance implies original_start_time")
            .resolve()?,
        Err(e) => return Err(e),
    };
    let (end, end_all_day) = match entry.end.resolve() {
        Ok(v) => v,
        // No end on a tombstone → zero-duration at the start (it's hidden anyway).
        Err(_) if is_instance => (start, start_all_day),
        Err(e) => return Err(e),
    };
    // Either both or neither end of the range should be all-day. If
    // they disagree (Google quirk), trust the start.
    let all_day = start_all_day || end_all_day;

    // Recurrence comes as a list of lines (RRULE, EXDATE, RDATE). We keep the
    // rule verbatim and read each deletion as an instant — the convention the
    // local and CalDAV adapters use — in whatever spelling Google returns it.
    // A timed series' wall clocks are its `start.timeZone`'s, the zone Google
    // documents it expands in.
    let series_zone: SeriesZone = match entry.start.time_zone.as_deref() {
        _ if all_day => Ok(None),
        None | Some("") => Ok(None),
        Some(name) => zone_named(name)
            .map(Some)
            .ok_or("a series zone tzdata does not know"),
    };
    let (rrule, exceptions) = match entry.recurrence.as_deref() {
        Some(lines) => read_recurrence(&entry.id, lines, all_day, series_zone),
        None => (None, Vec::new()),
    };
    let recurrence = rrule.map(|r| EventRecurrence {
        rrule: r,
        exceptions,
        // Carry Google's IANA zone for a timed recurring master so the frontend
        // expands it DST-correctly; all-day + plain UTC stay on the UTC path.
        tzid: if all_day {
            None
        } else {
            entry
                .start
                .time_zone
                .clone()
                .filter(|t| !t.is_empty() && t != "Etc/UTC")
        },
    });

    let created = entry.created.unwrap_or_else(Utc::now);
    let updated = entry.updated.unwrap_or(created);

    // Reminders: Google's `useDefault=true` means "fall back to the
    // calendar-level defaults". Aperio doesn't expose those today,
    // so we surface only explicit per-event overrides. Anything
    // with an unknown method gets dropped (Google occasionally
    // emits `sms` for legacy rows).
    let reminders = entry
        .reminders
        .map(|r| {
            r.overrides
                .into_iter()
                .filter_map(reminder_from_override)
                .collect()
        })
        .unwrap_or_default();

    // Attendees: the editable flat list ("Name <email>" / bare email) AND the
    // per-attendee RSVP state, without the organizer (decision 67a), whose row
    // Google flags. `organizer.self` says whether the account organizes the
    // event (decision 70a).
    let organized_by_me = entry.organizer.as_ref().map(|o| o.is_self);
    let people = cal_core::attendee::people_from_read(
        entry.organizer.and_then(|o| o.email),
        organized_by_me,
        entry.attendees.into_iter().filter_map(|a| {
            let status = a
                .response_status
                .as_deref()
                .map(google_status)
                .unwrap_or_default();
            Some(cal_core::attendee::ReadAttendee {
                email: a.email?,
                name: a.display_name,
                status,
                is_organizer: a.organizer,
            })
        }),
    );

    let id = event_id_for(
        entry.id,
        entry.recurring_event_id.as_deref(),
        entry.original_start_time.as_ref(),
    )?;

    // Diagnostics for the recurring-occurrence delete/suppress path. A recurring
    // master MUST carry its IANA zone (`tzid`) so the frontend expands it
    // DST-correctly; without it, occurrences on the far side of a DST boundary
    // drift an hour and a cancelled override (whose RECURRENCE-ID is the
    // DST-correct instant) can't match and suppress them → the deleted occurrence
    // ghosts. Also log that a cancelled instance arrived as a suppressing override
    // at all, with the exact instant it should cancel.
    if let Some(rec) = recurrence.as_ref() {
        debug!(
            id = %id,
            rrule = %rec.rrule,
            tzid = ?rec.tzid,
            start = %start,
            "google recurring master mapped"
        );
    }
    if cancelled {
        debug!(id = %id, start = %start, "google cancelled-instance override mapped");
    }

    Ok(Some(Event {
        keep_attendees: false,
        keep_fields: Vec::new(),
        clear_attendees: false,
        organized_elsewhere: people.organized_elsewhere,
        send_invitations: false,
        truncate_tail_overrides: false,
        accepts_exception_loss: false,
        deletions_not_restored: Vec::new(),
        id,
        calendar_id: calendar_id.to_string(),
        title: entry.summary.unwrap_or_default(),
        description: entry.description,
        location: entry.location,
        start,
        end,
        all_day,
        recurrence,
        color_label: None,
        // Google's colorId isn't mapped; per-event colors are host-local overrides.
        color_hex: None,
        reminders,
        sound: None,
        attendees: people.attendees,
        created_at: created,
        updated_at: updated,
        etag: entry.etag,
        organizer: people.organizer,
        attendee_responses: people.attendee_responses,
        // `false` for a normal event; `true` for a cancelled recurring instance
        // surfaced as a suppressing override (a cancelled WHOLE event returned
        // `None` above).
        cancelled,
        scheduling_silenced: false,
    }))
}

/// Map Google's `responseStatus` string to the normalised RSVP enum.
fn google_status(s: &str) -> AttendeeStatus {
    match s {
        "accepted" => AttendeeStatus::Accepted,
        "declined" => AttendeeStatus::Declined,
        "tentative" => AttendeeStatus::Tentative,
        _ => AttendeeStatus::NeedsAction,
    }
}

fn reminder_from_override(o: ReminderOverride) -> Option<Reminder> {
    let kind = match o.method.as_str() {
        "popup" => ReminderKind::Relative {
            minutes_before: o.minutes,
        },
        "email" => ReminderKind::Email {
            minutes_before: o.minutes,
        },
        _ => return None,
    };
    Some(Reminder { kind, sound: None })
}

// ── Reverse mapping: cal_core → Google JSON ─────────────────────────────

/// JSON payload for `POST /events` and `PATCH /events/{id}`. Only
/// the fields Aperio cares about — Google ignores unknown shapes
/// silently on read and rejects them on write, so we keep it tight.
#[derive(Debug, Serialize)]
pub struct EventWriteBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Always on a create; `None` leaves Google's own start, end and repeat
    /// together on a PATCH that kept them ([`event_to_body`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<EventDateTimeWrite>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<EventDateTimeWrite>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Vec<String>>,
    pub reminders: EventRemindersWrite,
    /// Attendees are written whenever there are any (Google stores them);
    /// whether Google EMAILS them is governed by the `sendUpdates` query
    /// param on the request, not the body. `None` leaves the array out, so
    /// Google keeps its own; `Some` of an empty list clears it, which only a
    /// host-confirmed removal of every invitee asks for (decision 74a).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attendees: Option<Vec<EventAttendeeWrite>>,
}

#[derive(Debug, Serialize)]
pub struct EventAttendeeWrite {
    pub email: String,
    #[serde(rename = "displayName", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Map Aperio's flat `"Name <email>"` / bare-email entries to Google's
/// attendee objects, dropping any entry without a usable address.
fn attendees_to_write(attendees: &[String]) -> Vec<EventAttendeeWrite> {
    attendees
        .iter()
        .filter_map(|entry| {
            let (name, email) = cal_core::attendee::parse(entry);
            (!email.is_empty()).then_some(EventAttendeeWrite {
                email,
                display_name: name,
            })
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct EventDateTimeWrite {
    /// Either `dateTime` (timed) or `date` (all-day) is set, never
    /// both — same shape as the read side.
    #[serde(skip_serializing_if = "Option::is_none", rename = "dateTime")]
    pub date_time: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<NaiveDate>,
    /// Google insists on an IANA timezone next to `dateTime`.
    /// "Etc/UTC" is the safest default — the timestamp we send is
    /// already in UTC and Google won't second-guess it.
    #[serde(rename = "timeZone")]
    pub time_zone: String,
}

#[derive(Debug, Serialize)]
pub struct EventRemindersWrite {
    /// We set this to `false` whenever the user has provided
    /// per-event reminders so Google doesn't merge in the
    /// calendar-level defaults on top.
    #[serde(rename = "useDefault")]
    pub use_default: bool,
    pub overrides: Vec<ReminderOverride>,
}

/// Convert a `NewEvent` (caller's payload) into Google's wire body.
pub fn new_event_to_body(new: &NewEvent) -> EventWriteBody {
    // The core's rule: an all-day series hands Google no zone (decision 46a).
    let tzid = cal_core::written_series_zone(
        new.recurrence.as_ref().and_then(|r| r.tzid.as_deref()),
        new.all_day,
    );
    EventWriteBody {
        summary: Some(new.title.clone()),
        description: new.description.clone(),
        location: new.location.clone(),
        start: Some(range_to_write(new.start, new.all_day, tzid)),
        end: Some(range_to_write(new.end, new.all_day, tzid)),
        recurrence: new
            .recurrence
            .as_ref()
            .map(|r| recurrence_to_lines(r, new.all_day, tzid)),
        reminders: reminders_to_write(&new.reminders),
        attendees: Some(attendees_to_write(&new.attendees)).filter(|list| !list.is_empty()),
    }
}

/// Convert an existing `Event` into a PATCH body: every mutable field, but
/// those the edit kept where Google's copy may hold more than this device's —
/// the invitees (decision 71a), and the start, end and repeat (205).
pub fn event_to_body(ev: &Event) -> EventWriteBody {
    // The core's rule: an all-day series hands Google no zone (decision 46a).
    let tzid = cal_core::written_series_zone(
        ev.recurrence.as_ref().and_then(|r| r.tzid.as_deref()),
        ev.all_day,
    );
    // The start, end, all-day flag and repeat go together, or not at all:
    // Google expands the lines from the start's wall clock in its zone, and
    // a deletion names an occurrence the start makes. Left out together
    // when the edit kept them all (decisions 106, 205) — a PATCH replaces the
    // whole array, Google's copy may hold lines Aperio does not read, and a
    // kept field may be another device's newer change, which this device's
    // start must not be paired with. Only a copy proven to be the one the
    // editor opened names kept fields.
    let keeps_slot = [
        EventField::Start,
        EventField::End,
        EventField::AllDay,
        EventField::Recurrence,
    ]
    .iter()
    .all(|field| ev.keep_fields.contains(field));
    EventWriteBody {
        summary: Some(ev.title.clone()),
        description: ev.description.clone(),
        location: ev.location.clone(),
        start: (!keeps_slot).then(|| range_to_write(ev.start, ev.all_day, tzid)),
        end: (!keeps_slot).then(|| range_to_write(ev.end, ev.all_day, tzid)),
        recurrence: if keeps_slot {
            None
        } else {
            ev.recurrence
                .as_ref()
                .map(|r| recurrence_to_lines(r, ev.all_day, tzid))
        },
        reminders: reminders_to_write(&ev.reminders),
        // Left out of the PATCH when the edit did not change the invitees
        // (decision 71a): a PATCH replaces the whole array, and Google's copy
        // holds the organizer's row, which Aperio does not show. An empty
        // array only when the host says every invitee was removed (74a).
        attendees: if ev.keep_attendees {
            None
        } else if ev.clear_attendees {
            Some(Vec::new())
        } else {
            Some(attendees_to_write(&ev.attendees)).filter(|list| !list.is_empty())
        },
    }
}

fn range_to_write(when: DateTime<Utc>, all_day: bool, tzid: Option<&str>) -> EventDateTimeWrite {
    if all_day {
        EventDateTimeWrite {
            date_time: None,
            // The boundary instant is a LOCAL midnight expressed in UTC;
            // `date_naive()` on it would emit the UTC day — one early for
            // users east of UTC. Read the day off the local clock. The
            // internal end is already exclusive, matching Google's
            // exclusive `end.date`.
            date: Some(when.with_timezone(&Local).date_naive()),
            time_zone: "Etc/UTC".into(),
        }
    } else {
        EventDateTimeWrite {
            date_time: Some(when),
            date: None,
            // A zoned recurring master keeps its IANA zone so Google expands it
            // DST-correctly; a one-off instant is exact, so UTC is fine.
            time_zone: tzid.unwrap_or("Etc/UTC").to_string(),
        }
    }
}

fn reminders_to_write(reminders: &[Reminder]) -> EventRemindersWrite {
    let overrides: Vec<ReminderOverride> =
        reminders.iter().filter_map(reminder_to_override).collect();
    if overrides.is_empty() {
        // No per-event reminders — let Google use whatever the
        // calendar defaults are. Setting `useDefault: false` with
        // an empty overrides array would silently strip them.
        EventRemindersWrite {
            use_default: true,
            overrides,
        }
    } else {
        EventRemindersWrite {
            use_default: false,
            overrides,
        }
    }
}

fn reminder_to_override(r: &Reminder) -> Option<ReminderOverride> {
    match r.kind {
        ReminderKind::Relative { minutes_before } => Some(ReminderOverride {
            method: "popup".into(),
            minutes: minutes_before,
        }),
        ReminderKind::Email { minutes_before } => Some(ReminderOverride {
            method: "email".into(),
            minutes: minutes_before,
        }),
        // Google supports neither an absolute reminder time nor an
        // "on app start" notion — both get dropped on write. The
        // user keeps these locally (Aperio's own scheduler picks
        // them up) but they don't round-trip via Google.
        ReminderKind::Absolute { .. } | ReminderKind::AppStart => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_with_writer_role_is_writable() {
        let entry = CalendarListEntry {
            id: "primary".into(),
            summary: "My Calendar".into(),
            background_color: Some("#1e88e5".into()),
            access_role: Some("owner".into()),
        };
        let cal = map_calendar(entry);
        assert_eq!(cal.id, "primary");
        assert_eq!(cal.name, "My Calendar");
        assert!(!cal.read_only);
        assert_eq!(cal.color.as_ref().unwrap().hex, "#1e88e5");
    }

    #[test]
    fn calendar_with_reader_role_is_read_only() {
        let entry = CalendarListEntry {
            id: "holidays@group.v.calendar.google.com".into(),
            summary: "Holidays in Germany".into(),
            background_color: Some("#3f51b5".into()),
            access_role: Some("reader".into()),
        };
        let cal = map_calendar(entry);
        assert!(cal.read_only);
    }

    #[test]
    fn map_event_timed_round_trip() {
        let raw = r#"{
            "id": "ev-1",
            "summary": "Standup",
            "description": "daily sync",
            "start": { "dateTime": "2026-05-25T10:00:00+02:00" },
            "end":   { "dateTime": "2026-05-25T10:30:00+02:00" },
            "status": "confirmed",
            "etag": "\"123\""
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.title, "Standup");
        assert!(!ev.all_day);
        assert_eq!(ev.etag.as_deref(), Some("\"123\""));
        assert!(ev.recurrence.is_none());
    }

    #[test]
    fn map_event_modified_instance_gets_recurrence_id() {
        // A moved single occurrence of a recurring series. Google's clean master
        // RRULE still generates the 10:00 slot, so without a RECURRENCE-ID the
        // frontend would render BOTH the master's 10:00 occurrence and this 14:00
        // instance. The id must carry `::rid::{originalStart}` so the expander
        // drops the master's occurrence.
        let raw = r#"{
            "id": "master-1_20260614T100000Z",
            "summary": "Standup (moved)",
            "start": { "dateTime": "2026-06-14T14:00:00Z" },
            "end":   { "dateTime": "2026-06-14T14:30:00Z" },
            "status": "confirmed",
            "recurringEventId": "master-1",
            "originalStartTime": { "dateTime": "2026-06-14T10:00:00Z" }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.id, "master-1::rid::2026-06-14T10:00:00Z");
        // The event itself sits at its MOVED time and is a plain single.
        assert_eq!(
            ev.start,
            Utc.with_ymd_and_hms(2026, 6, 14, 14, 0, 0).unwrap()
        );
        assert!(ev.recurrence.is_none());
    }

    #[test]
    fn modified_instance_original_start_normalises_to_utc() {
        // originalStartTime with an offset must resolve to the same UTC instant
        // the master's (UTC) expansion produces, or the frontend match misses.
        let raw = r#"{
            "id": "m2_20260614T100000Z",
            "summary": "Moved",
            "start": { "dateTime": "2026-06-14T16:00:00+02:00" },
            "end":   { "dateTime": "2026-06-14T16:30:00+02:00" },
            "status": "confirmed",
            "recurringEventId": "m2",
            "originalStartTime": { "dateTime": "2026-06-14T12:00:00+02:00" }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.id, "m2::rid::2026-06-14T10:00:00Z");
    }

    #[test]
    fn event_id_for_leaves_plain_events_untouched() {
        // No recurringEventId → the native Google id is kept verbatim.
        assert_eq!(event_id_for("ev-9".into(), None, None).unwrap(), "ev-9");
        // A master (recurringEventId but... only instances carry originalStartTime)
        // is defensively left alone when originalStartTime is absent.
        assert_eq!(
            event_id_for("ev-9".into(), Some("master"), None).unwrap(),
            "ev-9"
        );
    }

    #[test]
    fn map_event_reads_attendees_and_organizer() {
        let raw = r#"{
            "id": "ev-mtg",
            "summary": "Planning",
            "start": { "dateTime": "2026-05-25T10:00:00Z" },
            "end":   { "dateTime": "2026-05-25T11:00:00Z" },
            "organizer": { "email": "boss@example.com", "self": false },
            "attendees": [
              { "email": "boss@example.com", "displayName": "The Boss", "responseStatus": "accepted", "organizer": true },
              { "email": "me@example.com", "displayName": "Me", "responseStatus": "needsAction" },
              { "email": "skeptic@example.com", "responseStatus": "declined" }
            ]
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.organizer.as_deref(), Some("boss@example.com"));
        // Flat editable list: "Name <email>" when named, bare otherwise; never
        // the organizer (decision 67a).
        assert_eq!(ev.attendees, ["Me <me@example.com>", "skeptic@example.com"]);
        // Per-attendee RSVP state, in the same order.
        assert_eq!(ev.attendee_responses.len(), 2);
        assert_eq!(ev.attendee_responses[0].status, AttendeeStatus::NeedsAction);
        assert_eq!(ev.attendee_responses[1].status, AttendeeStatus::Declined);
        assert_eq!(ev.attendee_responses[0].name.as_deref(), Some("Me"));
        // The boss organizes it, not the account (decision 70a).
        assert!(ev.organized_elsewhere);
    }

    /// Google flags the organizer's row, and the flag wins over the address:
    /// the same account can be spelled gmail.com and googlemail.com.
    #[test]
    fn the_flagged_organizer_row_goes_whatever_its_spelling() {
        let raw = r#"{
            "id": "ev-own",
            "start": { "dateTime": "2026-05-25T10:00:00Z" },
            "end":   { "dateTime": "2026-05-25T11:00:00Z" },
            "organizer": { "email": "toni@gmail.com", "self": true },
            "attendees": [
              { "email": "toni@googlemail.com", "responseStatus": "accepted", "organizer": true },
              { "email": "bob@example.com", "responseStatus": "accepted" }
            ]
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.attendees, ["bob@example.com"]);
        assert!(!ev.organized_elsewhere, "the account organizes it");
    }

    /// An edit that left the invitees alone leaves them out of the PATCH,
    /// which would replace Google's whole array (decision 71a).
    #[test]
    fn an_unchanged_attendee_list_stays_out_of_the_patch() {
        let raw = r#"{
            "id": "ev-own",
            "start": { "dateTime": "2026-05-25T10:00:00Z" },
            "end":   { "dateTime": "2026-05-25T11:00:00Z" },
            "attendees": [ { "email": "bob@example.com", "responseStatus": "accepted" } ]
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let mut ev = map_event(entry, "primary").unwrap().unwrap();
        let body = serde_json::to_string(&event_to_body(&ev)).unwrap();
        assert!(body.contains("bob@example.com"), "{body}");
        ev.keep_attendees = true;
        let body = serde_json::to_string(&event_to_body(&ev)).unwrap();
        assert!(!body.contains("attendees"), "{body}");
    }

    /// Removing every invitee sends an empty array, which clears Google's,
    /// but only when the host says so (decision 74a): an empty list alone
    /// leaves the array out.
    #[test]
    fn removing_the_last_invitee_sends_an_empty_array() {
        let raw = r#"{
            "id": "ev-own",
            "start": { "dateTime": "2026-05-25T10:00:00Z" },
            "end":   { "dateTime": "2026-05-25T11:00:00Z" }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let mut ev = map_event(entry, "primary").unwrap().unwrap();
        let body = serde_json::to_value(event_to_body(&ev)).unwrap();
        assert!(body.get("attendees").is_none(), "{body}");
        ev.clear_attendees = true;
        let body = serde_json::to_value(event_to_body(&ev)).unwrap();
        assert_eq!(body["attendees"], serde_json::json!([]));
    }

    #[test]
    fn map_event_all_day() {
        let raw = r#"{
            "id": "ev-vacation",
            "summary": "Urlaub",
            "start": { "date": "2026-07-04" },
            "end":   { "date": "2026-07-19" }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert!(ev.all_day);
        // All-day boundaries anchor at LOCAL midnight of the wire date —
        // assert via the same Local construction so the test holds in any
        // timezone it runs in, and check the instant lands on the right
        // LOCAL calendar day.
        assert_eq!(
            ev.start,
            Local
                .from_local_datetime(
                    &NaiveDate::from_ymd_opt(2026, 7, 4)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap()
                )
                .earliest()
                .unwrap()
                .with_timezone(&Utc),
        );
        assert_eq!(
            ev.start.with_timezone(&Local).date_naive(),
            NaiveDate::from_ymd_opt(2026, 7, 4).unwrap(),
        );
    }

    /// Wire round-trip for the all-day boundaries: the dates Google sent
    /// must serialise back unchanged (write derives the LOCAL day of the
    /// local-midnight instants the read anchored). Guards the off-by-one
    /// the CalDAV adapter had for users east of UTC.
    #[test]
    fn all_day_dates_round_trip_through_write() {
        let raw = r#"{
            "id": "ev-conf",
            "summary": "Conference",
            "start": { "date": "2026-06-10" },
            "end":   { "date": "2026-06-12" }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        let write = event_to_body(&ev);
        assert_eq!(
            write.start.as_ref().unwrap().date,
            Some(NaiveDate::from_ymd_opt(2026, 6, 10).unwrap()),
        );
        assert_eq!(
            write.end.as_ref().unwrap().date,
            Some(NaiveDate::from_ymd_opt(2026, 6, 12).unwrap()),
        );
        assert!(write.start.as_ref().unwrap().date_time.is_none());
    }

    #[test]
    fn map_event_with_rrule_and_exdate() {
        let raw = r#"{
            "id": "ev-weekly",
            "summary": "Yoga",
            "start": { "dateTime": "2026-05-25T18:00:00Z" },
            "end":   { "dateTime": "2026-05-25T19:00:00Z" },
            "recurrence": [
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                "EXDATE;VALUE=DATE-TIME:20260601T180000Z"
            ]
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        let rec = ev.recurrence.unwrap();
        assert_eq!(rec.rrule, "FREQ=WEEKLY;BYDAY=MO");
        assert_eq!(rec.exceptions.len(), 1);
        assert_eq!(
            rec.exceptions[0],
            Utc.with_ymd_and_hms(2026, 6, 1, 18, 0, 0).unwrap()
        );
    }

    #[test]
    fn map_event_carries_recurrence_timezone() {
        // Google attaches the IANA zone to a timed recurring master; it must ride
        // onto EventRecurrence so the frontend expands it DST-correctly instead
        // of drifting an hour across DST.
        let raw = r#"{
            "id": "ev-zoned",
            "summary": "OAGDU",
            "start": { "dateTime": "2025-12-15T00:00:00Z", "timeZone": "America/New_York" },
            "end":   { "dateTime": "2025-12-15T01:00:00Z", "timeZone": "America/New_York" },
            "recurrence": ["RRULE:FREQ=MONTHLY;BYDAY=2SU"]
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(
            ev.recurrence.unwrap().tzid.as_deref(),
            Some("America/New_York")
        );
    }

    #[test]
    fn map_event_etc_utc_timezone_stays_unzoned() {
        // Google's "Etc/UTC" default needn't trigger the zoned path (no DST).
        let raw = r#"{
            "id": "ev-utc",
            "start": { "dateTime": "2026-01-01T12:00:00Z", "timeZone": "Etc/UTC" },
            "end":   { "dateTime": "2026-01-01T12:30:00Z", "timeZone": "Etc/UTC" },
            "recurrence": ["RRULE:FREQ=DAILY"]
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.recurrence.unwrap().tzid, None);
    }

    #[test]
    fn event_to_body_sends_recurrence_timezone() {
        // A zoned recurring master writes its IANA zone so Google expands it
        // DST-correctly on its side (parity with the read path).
        let ev = Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
            id: "ev-z".into(),
            calendar_id: "primary".into(),
            title: "OAGDU".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2025, 12, 15, 0, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2025, 12, 15, 1, 0, 0).unwrap(),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=MONTHLY;BYDAY=2SU".into(),
                exceptions: vec![],
                tzid: Some("America/New_York".into()),
            }),
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            created_at: Utc.with_ymd_and_hms(2025, 12, 15, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2025, 12, 15, 0, 0, 0).unwrap(),
            etag: None,
            organizer: None,
            attendee_responses: vec![],
            send_invitations: false,
            truncate_tail_overrides: false,
            accepts_exception_loss: false,
            deletions_not_restored: Vec::new(),
            cancelled: false,
            scheduling_silenced: false,
        };
        let json = serde_json::to_value(event_to_body(&ev)).unwrap();
        assert_eq!(json["start"]["timeZone"], "America/New_York");
        assert_eq!(json["end"]["timeZone"], "America/New_York");
    }

    /// Decision 46a: an all-day series hands Google no zone, whatever it
    /// stores; its days go out as dates.
    #[test]
    fn event_to_body_sends_no_zone_for_an_all_day_series() {
        let midnight = Local.with_ymd_and_hms(2026, 10, 19, 0, 0, 0).unwrap();
        let ev = Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
            id: "ev-all-day".into(),
            calendar_id: "primary".into(),
            title: "All-day".into(),
            description: None,
            location: None,
            start: midnight.with_timezone(&Utc),
            end: (midnight + chrono::Duration::days(1)).with_timezone(&Utc),
            all_day: true,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
                exceptions: vec![],
                tzid: Some("America/New_York".into()),
            }),
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            created_at: Utc.with_ymd_and_hms(2026, 9, 15, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 9, 15, 0, 0, 0).unwrap(),
            etag: None,
            organizer: None,
            attendee_responses: vec![],
            send_invitations: false,
            truncate_tail_overrides: false,
            accepts_exception_loss: false,
            deletions_not_restored: Vec::new(),
            cancelled: false,
            scheduling_silenced: false,
        };
        let json = serde_json::to_value(event_to_body(&ev)).unwrap();
        for side in ["start", "end"] {
            assert_eq!(json[side]["timeZone"], "Etc/UTC", "{side}: {json}");
            assert!(json[side]["dateTime"].is_null(), "{side}: {json}");
        }
        assert_eq!(json["start"]["date"], "2026-10-19");
    }

    #[test]
    fn cancelled_event_is_filtered() {
        let raw = r#"{
            "id": "ev-deleted",
            "status": "cancelled",
            "start": { "dateTime": "2026-05-25T10:00:00+02:00" },
            "end":   { "dateTime": "2026-05-25T10:30:00+02:00" }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        assert!(map_event(entry, "primary").unwrap().is_none());
    }

    #[test]
    fn cancelled_instance_becomes_suppressing_override() {
        // A deleted single occurrence arrives as a content-less tombstone (no
        // start/end) carrying recurringEventId + originalStartTime. It must surface
        // as a `cancelled` RECURRENCE-ID override so the expander drops the master's
        // slot (the show-cancelled filter then hides the override) — otherwise the
        // master keeps generating the deleted occurrence.
        let raw = r#"{
            "id": "master-1_20260614T100000Z",
            "status": "cancelled",
            "recurringEventId": "master-1",
            "originalStartTime": { "dateTime": "2026-06-14T10:00:00Z" }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary")
            .unwrap()
            .expect("surfaced as a suppressing override, not dropped");
        assert_eq!(ev.id, "master-1::rid::2026-06-14T10:00:00Z");
        assert!(ev.cancelled);
        assert!(ev.recurrence.is_none());
        // Falls back to originalStartTime for its (hidden) position.
        assert_eq!(
            ev.start,
            Utc.with_ymd_and_hms(2026, 6, 14, 10, 0, 0).unwrap()
        );
    }

    #[test]
    fn invalid_hex_color_falls_back_to_none() {
        let entry = CalendarListEntry {
            id: "x".into(),
            summary: "X".into(),
            background_color: Some("not-a-color".into()),
            access_role: Some("owner".into()),
        };
        let cal = map_calendar(entry);
        assert!(cal.color.is_none());
    }

    #[test]
    fn map_event_reads_per_event_reminder_overrides() {
        let raw = r#"{
            "id": "ev-r",
            "summary": "Standup",
            "start": { "dateTime": "2026-05-25T10:00:00Z" },
            "end":   { "dateTime": "2026-05-25T10:30:00Z" },
            "reminders": {
                "useDefault": false,
                "overrides": [
                    {"method": "popup", "minutes": 10},
                    {"method": "email", "minutes": 60},
                    {"method": "sms",   "minutes": 30}
                ]
            }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.reminders.len(), 2);
        match &ev.reminders[0].kind {
            ReminderKind::Relative { minutes_before } => {
                assert_eq!(*minutes_before, 10)
            }
            other => panic!("expected Relative, got {other:?}"),
        }
        match &ev.reminders[1].kind {
            ReminderKind::Email { minutes_before } => {
                assert_eq!(*minutes_before, 60)
            }
            other => panic!("expected Email, got {other:?}"),
        }
    }

    #[test]
    fn new_event_to_body_round_trip_timed() {
        let new = NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Standup".into(),
            description: Some("daily".into()),
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 25, 10, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 25, 10, 30, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: vec![Reminder {
                kind: ReminderKind::Relative { minutes_before: 5 },
                sound: None,
            }],
            sound: None,
            attendees: vec![],
            send_invitations: false,
        };
        let body = new_event_to_body(&new);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["summary"], "Standup");
        assert_eq!(json["start"]["dateTime"], "2026-05-25T10:00:00Z");
        assert_eq!(json["start"]["timeZone"], "Etc/UTC");
        assert!(json["start"].get("date").is_none());
        assert_eq!(json["reminders"]["useDefault"], false);
        assert_eq!(json["reminders"]["overrides"][0]["method"], "popup");
        assert_eq!(json["reminders"]["overrides"][0]["minutes"], 5);
    }

    #[test]
    fn new_event_to_body_all_day_sends_date_not_datetime() {
        // All-day instants the way the frontend produces them: LOCAL
        // midnights (end exclusive), expressed in UTC — so the asserted
        // wire dates hold in any timezone the test runs in.
        let local_midnight = |y: i32, m: u32, d: u32| {
            Local
                .from_local_datetime(
                    &NaiveDate::from_ymd_opt(y, m, d)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                )
                .earliest()
                .unwrap()
                .with_timezone(&Utc)
        };
        let new = NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Urlaub".into(),
            description: None,
            location: None,
            start: local_midnight(2026, 7, 4),
            end: local_midnight(2026, 7, 19),
            all_day: true,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            send_invitations: false,
        };
        let body = new_event_to_body(&new);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["start"]["date"], "2026-07-04");
        assert!(json["start"].get("dateTime").is_none());
        // Empty reminders → useDefault=true so Google's calendar
        // defaults stay in effect.
        assert_eq!(json["reminders"]["useDefault"], true);
    }

    #[test]
    fn event_to_body_serialises_recurrence_with_exdates() {
        let ev = Event {
            keep_attendees: false,
            keep_fields: Vec::new(),
            clear_attendees: false,
            organized_elsewhere: false,
            id: "ev-1".into(),
            calendar_id: "primary".into(),
            title: "Yoga".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 25, 18, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 25, 19, 0, 0).unwrap(),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
                exceptions: vec![Utc.with_ymd_and_hms(2026, 6, 1, 18, 0, 0).unwrap()],
                tzid: None,
            }),
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            send_invitations: false,
            truncate_tail_overrides: false,
            accepts_exception_loss: false,
            deletions_not_restored: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: None,
            organizer: None,
            attendee_responses: vec![],
            cancelled: false,
            scheduling_silenced: false,
        };
        let body = event_to_body(&ev);
        let json = serde_json::to_value(&body).unwrap();
        let rec = &json["recurrence"];
        assert_eq!(rec[0], "RRULE:FREQ=WEEKLY;BYDAY=MO");
        // A series without a zone writes its deletion as the UTC instant.
        assert_eq!(rec[1], "EXDATE:20260601T180000Z");
    }

    #[test]
    fn absolute_and_app_start_reminders_drop_on_write() {
        let new = NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "x".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 25, 10, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 25, 10, 30, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: vec![
                Reminder {
                    kind: ReminderKind::AppStart,
                    sound: None,
                },
                Reminder {
                    kind: ReminderKind::Absolute { at: Utc::now() },
                    sound: None,
                },
                Reminder {
                    kind: ReminderKind::Relative { minutes_before: 15 },
                    sound: None,
                },
            ],
            sound: None,
            attendees: vec![],
            send_invitations: false,
        };
        let body = new_event_to_body(&new);
        let json = serde_json::to_value(&body).unwrap();
        let overrides = json["reminders"]["overrides"].as_array().unwrap();
        // Only the Relative one made it through.
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0]["method"], "popup");
    }

    // ── Deletions in every spelling (decision 205) ──────────────────────

    /// A recurring master as Google lists it: timed in `zone`, or all-day
    /// when `zone` is `None`, with these recurrence lines.
    fn master_with(lines: &[&str], zone: Option<&str>) -> Event {
        let (start, end) = match zone {
            Some(tz) => (
                serde_json::json!({ "dateTime": "2026-05-25T07:00:00Z", "timeZone": tz }),
                serde_json::json!({ "dateTime": "2026-05-25T08:00:00Z", "timeZone": tz }),
            ),
            None => (
                serde_json::json!({ "date": "2026-05-25" }),
                serde_json::json!({ "date": "2026-05-26" }),
            ),
        };
        let raw = serde_json::json!({
            "id": "series-1",
            "summary": "Series",
            "start": start,
            "end": end,
            "recurrence": lines,
        });
        let entry: EventEntry = serde_json::from_value(raw).unwrap();
        map_event(entry, "primary").unwrap().unwrap()
    }

    fn deletions(ev: &Event) -> Vec<DateTime<Utc>> {
        ev.recurrence.as_ref().unwrap().exceptions.clone()
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    /// Google's own export spells a deletion as the wall clock in the
    /// series' zone. Read before as nothing, so the next save erased it.
    #[test]
    fn a_deletion_in_the_series_zone_is_read() {
        let ev = master_with(
            &[
                "RRULE:FREQ=WEEKLY",
                "EXDATE;TZID=Europe/Berlin:20260601T090000",
            ],
            Some("Europe/Berlin"),
        );
        assert_eq!(deletions(&ev), [utc(2026, 6, 1, 7, 0)]);
    }

    #[test]
    fn several_deletions_on_one_line_are_each_read() {
        let ev = master_with(
            &[
                "RRULE:FREQ=DAILY",
                "EXDATE;TZID=America/Montreal:20240831T130000,20240901T130000",
            ],
            Some("America/Montreal"),
        );
        assert_eq!(
            deletions(&ev),
            [utc(2024, 8, 31, 17, 0), utc(2024, 9, 1, 17, 0)]
        );
    }

    /// A date names a day of a series of days, at the local midnight a date
    /// start is read at. Read at UTC midnight, as before, it lay hours off
    /// that midnight: the same-day reading (decision 95, within 12 hours)
    /// still found the day from UTC-11 to UTC+11, but at UTC+12 (Auckland in
    /// June) it lay half a day from both neighbours and deleted neither, and
    /// beyond (Auckland in summer, Tonga) it deleted the next day. On a UTC
    /// machine both readings coincide, so the anchor itself only shows off UTC.
    #[test]
    fn a_date_deletion_names_its_day_on_a_series_of_days() {
        let day = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
        for line in [
            "EXDATE;VALUE=DATE:20260601",
            "exdate;value=date:20260601",
            // Google quoted the zone and kept it on a date (2011).
            r#"EXDATE;TZID="America/Vancouver";VALUE=DATE:20260601"#,
            // A wall clock on a series of days names its date.
            "EXDATE;TZID=Europe/Helsinki:20260601T000000",
        ] {
            let ev = master_with(&["RRULE:FREQ=DAILY", line], None);
            assert_eq!(deletions(&ev), [local_midnight(day)], "{line}");
            assert_eq!(all_day_slot(deletions(&ev)[0]), day, "{line}");
        }
    }

    /// A zone tzdata does not know costs only a wall clock on a timed series:
    /// a UTC instant and a date need no zone, and main read them.
    #[test]
    fn an_unknown_zone_costs_only_the_wall_clock() {
        let ev = master_with(
            &[
                "RRULE:FREQ=WEEKLY",
                "EXDATE;TZID=W. Europe Standard Time:20260601T070000Z",
                "EXDATE;TZID=Mars/Olympus:20260608T090000",
            ],
            Some("Europe/Berlin"),
        );
        assert_eq!(deletions(&ev), [utc(2026, 6, 1, 7, 0)]);
        let day = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
        for line in [
            r#"EXDATE;TZID="Customized Time Zone";VALUE=DATE:20260601"#,
            "EXDATE;TZID=Mars/Olympus:20260601T000000",
        ] {
            let ev = master_with(&["RRULE:FREQ=DAILY", line], None);
            assert_eq!(deletions(&ev), [local_midnight(day)], "{line}");
        }
    }

    /// Aperio wrote a date deletion of a series of days as UTC midnight
    /// before decision 205; it is that date, also where UTC midnight lies
    /// half a day or more from the local one. Another instant stays one.
    #[test]
    fn utc_midnight_on_a_series_of_days_is_its_date() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();
        for line in [
            "EXDATE:20261010T000000Z",
            "EXDATE;VALUE=DATE-TIME:20261010T000000Z",
        ] {
            let ev = master_with(&["RRULE:FREQ=DAILY", line], None);
            assert_eq!(deletions(&ev), [local_midnight(day)], "{line}");
        }
        let ev = master_with(&["RRULE:FREQ=DAILY", "EXDATE:20261010T110000Z"], None);
        assert_eq!(deletions(&ev), [utc(2026, 10, 10, 11, 0)]);
    }

    /// A series zone tzdata does not know cannot place a wall clock either:
    /// it is left out, as under an unknown `TZID`, not read as UTC.
    #[test]
    fn an_unknown_series_zone_costs_its_wall_clocks() {
        let ev = master_with(
            &[
                "RRULE:FREQ=WEEKLY",
                "EXDATE:20260601T090000",
                "EXDATE:20260608T070000Z",
            ],
            Some("Mars/Olympus"),
        );
        assert_eq!(deletions(&ev), [utc(2026, 6, 8, 7, 0)]);
    }

    #[test]
    fn a_utc_deletion_reads_as_before() {
        for line in [
            "EXDATE:20260601T070000Z",
            "EXDATE;VALUE=DATE-TIME:20260601T070000Z",
        ] {
            let ev = master_with(&["RRULE:FREQ=WEEKLY", line], Some("Europe/Berlin"));
            assert_eq!(deletions(&ev), [utc(2026, 6, 1, 7, 0)], "{line}");
        }
    }

    #[test]
    fn the_lines_are_read_in_any_order() {
        let ev = master_with(
            &[
                "EXDATE;TZID=Europe/Berlin:20260601T090000",
                "RRULE:FREQ=WEEKLY",
            ],
            Some("Europe/Berlin"),
        );
        assert_eq!(ev.recurrence.as_ref().unwrap().rrule, "FREQ=WEEKLY");
        assert_eq!(deletions(&ev), [utc(2026, 6, 1, 7, 0)]);
    }

    /// The occurrence the views place for a wall clock a clock change
    /// repeats or skips, as RFC 5545 reads it: the first reading, and the
    /// offset from before the gap.
    #[test]
    fn a_deletion_at_a_clock_change_lands_where_the_occurrence_does() {
        let ev = master_with(
            &[
                "RRULE:FREQ=DAILY",
                "EXDATE;TZID=Europe/Berlin:20261025T023000",
                "EXDATE;TZID=Europe/Berlin:20260329T023000",
            ],
            Some("Europe/Berlin"),
        );
        assert_eq!(
            deletions(&ev),
            [utc(2026, 10, 25, 0, 30), utc(2026, 3, 29, 1, 30)]
        );
    }

    #[test]
    fn a_deletion_without_a_zone_is_a_wall_clock_in_the_series_zone() {
        let ev = master_with(
            &["RRULE:FREQ=WEEKLY", "EXDATE:20190617T090000"],
            Some("America/Chicago"),
        );
        assert_eq!(deletions(&ev), [utc(2019, 6, 17, 14, 0)]);
    }

    /// Google returns a rule with parameters too; read as no rule, the series
    /// was a single event.
    #[test]
    fn a_rule_with_parameters_is_read() {
        let ev = master_with(
            &["RRULE;X-EVOLUTION-ENDDATE=20200120:FREQ=WEEKLY;BYDAY=MO"],
            Some("Europe/Berlin"),
        );
        assert_eq!(ev.recurrence.unwrap().rrule, "FREQ=WEEKLY;BYDAY=MO");
    }

    /// What names no deletion Aperio can read stays out, and the series with
    /// it: an unknown zone, a date on a timed series (Google ignores it),
    /// lines Aperio does not keep.
    #[test]
    fn what_names_no_readable_deletion_is_left_out() {
        let ev = master_with(
            &[
                "RRULE:FREQ=WEEKLY",
                "EXDATE;TZID=Mars/Olympus:20260601T090000",
                "EXDATE;VALUE=DATE:20260608",
                "RDATE:20260610T070000Z",
                "EXDATE:not-a-date",
            ],
            Some("Europe/Berlin"),
        );
        assert_eq!(ev.recurrence.as_ref().unwrap().rrule, "FREQ=WEEKLY");
        assert!(deletions(&ev).is_empty(), "{:?}", deletions(&ev));
        // Of several rules, the last one is kept.
        let ev = master_with(
            &["RRULE:FREQ=DAILY", "RRULE:FREQ=WEEKLY;BYDAY=MO"],
            Some("Europe/Berlin"),
        );
        assert_eq!(ev.recurrence.unwrap().rrule, "FREQ=WEEKLY;BYDAY=MO");
        // Deletions without a rule make no series.
        let ev = master_with(&["EXDATE:20260601T070000Z"], Some("Europe/Berlin"));
        assert!(ev.recurrence.is_none());
    }

    /// The written lines, as Google's export spells them, for a series that
    /// repeats in `zone` (timed) or on days (`None`).
    fn written(exceptions: Vec<DateTime<Utc>>, zone: Option<&str>) -> Vec<String> {
        let rec = EventRecurrence {
            rrule: "FREQ=DAILY".into(),
            exceptions,
            tzid: zone.map(str::to_string),
        };
        recurrence_to_lines(&rec, zone.is_none(), zone)
    }

    #[test]
    fn deletions_are_written_as_google_spells_them() {
        assert_eq!(
            written(vec![utc(2026, 6, 1, 7, 0)], Some("Europe/Berlin")),
            [
                "RRULE:FREQ=DAILY",
                "EXDATE;TZID=Europe/Berlin:20260601T090000"
            ]
        );
        // A zone spelled another way is written as the start names it.
        assert_eq!(
            written(vec![utc(2026, 6, 1, 7, 0)], Some("europe/berlin"))[1],
            "EXDATE;TZID=europe/berlin:20260601T090000"
        );
        // A wall clock the clock change skips was read with the offset from
        // before it; it goes back as the reading after it, the same instant.
        assert_eq!(
            written(vec![utc(2026, 3, 29, 1, 30)], Some("Europe/Berlin"))[1],
            "EXDATE;TZID=Europe/Berlin:20260329T033000"
        );
        // The second pass of the hour Berlin shows twice has no wall clock of
        // its own; it goes as its instant.
        assert_eq!(
            written(
                vec![utc(2026, 10, 25, 0, 30), utc(2026, 10, 25, 1, 30)],
                Some("Europe/Berlin")
            )[1..],
            [
                "EXDATE;TZID=Europe/Berlin:20261025T023000",
                "EXDATE:20261025T013000Z"
            ]
        );
        // A series on UTC, or in a zone tzdata does not know.
        for zone in ["Etc/UTC", "Mars/Olympus"] {
            assert_eq!(
                written(vec![utc(2026, 6, 1, 7, 0)], Some(zone))[1],
                "EXDATE:20260601T070000Z",
                "{zone}"
            );
        }
        // A series of days: the day, the only form Google's documentation
        // allows there; before, a date-time it forbids.
        let day = NaiveDate::from_ymd_opt(2026, 12, 30).unwrap();
        assert_eq!(
            written(vec![local_midnight(day)], None)[1],
            "EXDATE;VALUE=DATE:20261230"
        );
    }

    /// What is written is read back as the same deletions.
    #[test]
    fn written_deletions_read_back_as_themselves() {
        let timed = vec![utc(2026, 6, 1, 7, 0), utc(2026, 10, 25, 1, 30)];
        let lines = written(timed.clone(), Some("Europe/Berlin"));
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(deletions(&master_with(&refs, Some("Europe/Berlin"))), timed);
        let days = vec![local_midnight(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap())];
        let lines = written(days.clone(), None);
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(deletions(&master_with(&refs, None)), days);
    }

    /// An edit that kept the start, the end, the kind of day and the repeat
    /// leaves them to Google, together: a PATCH replaces the whole array,
    /// Google's copy may hold lines Aperio does not read, and a kept start may
    /// be stale, which must not be paired with Google's lines. One that
    /// changed any of them writes all of them.
    #[test]
    fn a_kept_repeat_stays_out_of_the_patch() {
        let mut ev = master_with(
            &[
                "RRULE:FREQ=WEEKLY",
                "EXDATE;TZID=Europe/Berlin:20260601T090000",
            ],
            Some("Europe/Berlin"),
        );
        ev.title = "Renamed".into();
        ev.keep_fields = vec![
            EventField::Description,
            EventField::Start,
            EventField::End,
            EventField::AllDay,
            EventField::Recurrence,
        ];
        let json = serde_json::to_value(event_to_body(&ev)).unwrap();
        for left_alone in ["recurrence", "start", "end"] {
            assert!(json.get(left_alone).is_none(), "{left_alone}: {json}");
        }
        assert_eq!(json["summary"], "Renamed");
        for moved in [
            EventField::Start,
            EventField::End,
            EventField::AllDay,
            EventField::Recurrence,
        ] {
            let mut edit = ev.clone();
            edit.keep_fields.retain(|field| *field != moved);
            let json = serde_json::to_value(event_to_body(&edit)).unwrap();
            assert_eq!(json["start"]["timeZone"], "Europe/Berlin", "{moved:?}");
            assert!(json["end"]["dateTime"].is_string(), "{moved:?}");
            assert_eq!(
                json["recurrence"],
                serde_json::json!([
                    "RRULE:FREQ=WEEKLY",
                    "EXDATE;TZID=Europe/Berlin:20260601T090000"
                ]),
                "{moved:?}"
            );
        }
    }
}
