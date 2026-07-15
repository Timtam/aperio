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
    AttendeeResponse, AttendeeStatus, Calendar, ColorSource, ContainerColor, Event,
    EventRecurrence, NewEvent, Reminder, ReminderKind,
};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

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
        color_label: None,
        id: entry.id,
        name: entry.summary,
        color,
        read_only,
        default_sound: None,
    }
}

/// Parse one EXDATE value from a `RECURRENCE` line. Accepts the two
/// common iCal shapes: `YYYYMMDDTHHMMSSZ` (compact UTC date-time) and
/// `YYYYMMDD` (date-only, anchored at 00:00 UTC).
fn parse_exdate_value(raw: &str) -> Option<DateTime<Utc>> {
    if raw.len() == 8 {
        let d = NaiveDate::parse_from_str(raw, "%Y%m%d").ok()?;
        let mid = d.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        return Some(Utc.from_utc_datetime(&mid));
    }
    if raw.ends_with('Z') {
        let naive = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%SZ").ok()?;
        return Some(Utc.from_utc_datetime(&naive));
    }
    None
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
    /// The meeting organizer. Present on meetings; carries the
    /// organizer's email (and a `self` flag we don't need).
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
}

#[derive(Debug, Deserialize)]
pub struct EventOrganizer {
    #[serde(default)]
    pub email: Option<String>,
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
    /// LOCAL midnight (expressed as a UTC instant) — the app-internal
    /// all-day convention shared with the CalDAV adapter, so the views'
    /// local-day bucketing and the write-side round-trip line up in any
    /// timezone. DST edge: a zone can skip midnight on a transition
    /// day; fall forward to the first valid local time then.
    fn resolve(&self) -> GoogleResult<(DateTime<Utc>, bool)> {
        if let Some(dt) = self.date_time {
            return Ok((dt, false));
        }
        if let Some(d) = self.date {
            let midnight = d.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
            let anchored = Local
                .from_local_datetime(&midnight)
                .earliest()
                .map(|l| l.with_timezone(&Utc))
                .unwrap_or_else(|| Utc.from_utc_datetime(&midnight));
            return Ok((anchored, true));
        }
        Err(GoogleError::Protocol(
            "event start/end has neither dateTime nor date".into(),
        ))
    }
}

/// Separator between a recurring series' id and the RECURRENCE-ID instant an
/// override replaces — e.g. `{master}::rid::2026-06-14T13:00:00Z`. Must match the
/// CalDAV adapter and `shared/recurrence.ts`, which split the series id back out
/// and skip the master occurrence the override stands in for.
const RECURRENCE_ID_MARKER: &str = "::rid::";

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

    // Recurrence comes as a list of lines (RRULE, EXDATE, RDATE). We
    // keep the RRULE verbatim and parse EXDATEs into `DateTime<Utc>`
    // — same convention the local + CalDAV adapters use.
    let (rrule, exceptions) = match entry.recurrence {
        Some(lines) => {
            let mut rrule = None;
            let mut exdates: Vec<DateTime<Utc>> = Vec::new();
            for line in lines {
                if let Some(rest) = line.strip_prefix("RRULE:") {
                    rrule = Some(rest.to_string());
                } else if let Some(rest) = line.strip_prefix("EXDATE") {
                    // EXDATE can carry params (e.g. `EXDATE;VALUE=DATE:...`).
                    // We split on the first `:` and parse each comma-
                    // separated value as either YYYYMMDDTHHMMSSZ
                    // (date-time, common) or YYYYMMDD (date-only,
                    // anchored at 00:00 UTC like the iCal adapter).
                    if let Some((_, values)) = rest.split_once(':') {
                        for raw in values.split(',') {
                            if let Some(parsed) = parse_exdate_value(raw.trim()) {
                                exdates.push(parsed);
                            }
                        }
                    }
                }
            }
            (rrule, exdates)
        }
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

    // Attendees: keep the editable flat list ("Name <email>" / bare
    // email) AND the per-attendee RSVP state.
    let mut attendees = Vec::new();
    let mut attendee_responses = Vec::new();
    for a in entry.attendees {
        let Some(email) = a.email.filter(|e| !e.trim().is_empty()) else {
            continue;
        };
        let name = a.display_name.filter(|n| !n.trim().is_empty());
        attendees.push(format_attendee(name.as_deref(), &email));
        attendee_responses.push(AttendeeResponse {
            status: a
                .response_status
                .as_deref()
                .map(google_status)
                .unwrap_or_default(),
            name,
            email,
        });
    }
    let organizer = entry
        .organizer
        .and_then(|o| o.email)
        .filter(|e| !e.trim().is_empty());

    let id = event_id_for(
        entry.id,
        entry.recurring_event_id.as_deref(),
        entry.original_start_time.as_ref(),
    )?;

    Ok(Some(Event {
        send_invitations: false,
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
        attendees,
        created_at: created,
        updated_at: updated,
        etag: entry.etag,
        organizer,
        attendee_responses,
        // `false` for a normal event; `true` for a cancelled recurring instance
        // surfaced as a suppressing override (a cancelled WHOLE event returned
        // `None` above).
        cancelled,
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

/// Render an attendee for the editable flat list: `"Name <email>"` when
/// a distinct display name exists, else the bare email — matching the
/// format the AttendeePicker and the write path use.
fn format_attendee(name: Option<&str>, email: &str) -> String {
    match name {
        Some(n) if n.trim() != email => format!("{} <{}>", n.trim(), email),
        _ => email.to_string(),
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
    pub start: EventDateTimeWrite,
    pub end: EventDateTimeWrite,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Vec<String>>,
    pub reminders: EventRemindersWrite,
    /// Attendees are always written (Google stores them); whether Google
    /// EMAILS them is governed by the `sendUpdates` query param on the
    /// request, not the body.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<EventAttendeeWrite>,
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
    let tzid = new.recurrence.as_ref().and_then(|r| r.tzid.as_deref());
    EventWriteBody {
        summary: Some(new.title.clone()),
        description: new.description.clone(),
        location: new.location.clone(),
        start: range_to_write(new.start, new.all_day, tzid),
        end: range_to_write(new.end, new.all_day, tzid),
        recurrence: new
            .recurrence
            .as_ref()
            .map(|r| recurrence_to_lines(&r.rrule, &r.exceptions)),
        reminders: reminders_to_write(&new.reminders),
        attendees: attendees_to_write(&new.attendees),
    }
}

/// Convert an existing `Event` into a PATCH body. We send every
/// mutable field so PATCH-with-this-body is effectively a full
/// replacement of the user-visible state — simpler than computing
/// a diff and Google handles it the same.
pub fn event_to_body(ev: &Event) -> EventWriteBody {
    let tzid = ev.recurrence.as_ref().and_then(|r| r.tzid.as_deref());
    EventWriteBody {
        summary: Some(ev.title.clone()),
        description: ev.description.clone(),
        location: ev.location.clone(),
        start: range_to_write(ev.start, ev.all_day, tzid),
        end: range_to_write(ev.end, ev.all_day, tzid),
        recurrence: ev
            .recurrence
            .as_ref()
            .map(|r| recurrence_to_lines(&r.rrule, &r.exceptions)),
        reminders: reminders_to_write(&ev.reminders),
        attendees: attendees_to_write(&ev.attendees),
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

fn recurrence_to_lines(rrule: &str, exceptions: &[DateTime<Utc>]) -> Vec<String> {
    let mut lines = Vec::with_capacity(1 + exceptions.len());
    // Google expects the RFC 5545 prefix; the rest of Aperio stores
    // the bare rule body.
    lines.push(format!("RRULE:{rrule}"));
    for ex in exceptions {
        // EXDATE in compact UTC form, one per line.
        lines.push(format!(
            "EXDATE;VALUE=DATE-TIME:{}",
            ex.format("%Y%m%dT%H%M%SZ")
        ));
    }
    lines
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
              { "email": "boss@example.com", "displayName": "The Boss", "responseStatus": "accepted" },
              { "email": "me@example.com", "responseStatus": "needsAction" },
              { "email": "skeptic@example.com", "responseStatus": "declined" }
            ]
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.organizer.as_deref(), Some("boss@example.com"));
        // Flat editable list: "Name <email>" when named, bare otherwise.
        assert_eq!(ev.attendees[0], "The Boss <boss@example.com>");
        assert_eq!(ev.attendees[1], "me@example.com");
        // Per-attendee RSVP state.
        assert_eq!(ev.attendee_responses.len(), 3);
        assert_eq!(ev.attendee_responses[0].status, AttendeeStatus::Accepted);
        assert_eq!(ev.attendee_responses[1].status, AttendeeStatus::NeedsAction);
        assert_eq!(ev.attendee_responses[2].status, AttendeeStatus::Declined);
        assert_eq!(ev.attendee_responses[0].name.as_deref(), Some("The Boss"));
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
            write.start.date,
            Some(NaiveDate::from_ymd_opt(2026, 6, 10).unwrap()),
        );
        assert_eq!(
            write.end.date,
            Some(NaiveDate::from_ymd_opt(2026, 6, 12).unwrap()),
        );
        assert!(write.start.date_time.is_none());
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
            cancelled: false,
        };
        let json = serde_json::to_value(event_to_body(&ev)).unwrap();
        assert_eq!(json["start"]["timeZone"], "America/New_York");
        assert_eq!(json["end"]["timeZone"], "America/New_York");
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
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: None,
            organizer: None,
            attendee_responses: vec![],
            cancelled: false,
        };
        let body = event_to_body(&ev);
        let json = serde_json::to_value(&body).unwrap();
        let rec = &json["recurrence"];
        assert_eq!(rec[0], "RRULE:FREQ=WEEKLY;BYDAY=MO");
        assert_eq!(rec[1], "EXDATE;VALUE=DATE-TIME:20260601T180000Z");
    }

    #[test]
    fn absolute_and_app_start_reminders_drop_on_write() {
        let new = NewEvent {
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
}
