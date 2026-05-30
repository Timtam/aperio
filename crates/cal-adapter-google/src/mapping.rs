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
    Calendar, ColorSource, ContainerColor, Event, EventRecurrence, NewEvent, Reminder, ReminderKind,
};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
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
    pub start: EventDateTime,
    pub end: EventDateTime,
    /// RFC 5545 RRULE / EXDATE strings. Each line is one rule.
    #[serde(default)]
    pub recurrence: Option<Vec<String>>,
    #[serde(default)]
    pub created: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated: Option<DateTime<Utc>>,
    /// On a recurring-event instance Google sends a `recurringEventId`
    /// pointing back to the master row. We don't expand instances on
    /// the Google side (we expand client-side via rrule.js), so this
    /// is ignored for now — kept as a field reference for 6d.2.
    #[serde(default, rename = "recurringEventId")]
    pub recurring_event_id: Option<String>,
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
#[derive(Debug, Deserialize)]
pub struct EventDateTime {
    #[serde(default, rename = "dateTime")]
    pub date_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub date: Option<NaiveDate>,
}

impl EventDateTime {
    /// Returns `(utc_datetime, is_all_day)`. All-day dates anchor at
    /// 00:00 UTC, mirroring the iCal adapter's convention so the
    /// frontend's multi-day handling works the same way.
    fn resolve(&self) -> GoogleResult<(DateTime<Utc>, bool)> {
        if let Some(dt) = self.date_time {
            return Ok((dt, false));
        }
        if let Some(d) = self.date {
            let midnight = d.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
            return Ok((Utc.from_utc_datetime(&midnight), true));
        }
        Err(GoogleError::Protocol(
            "event start/end has neither dateTime nor date".into(),
        ))
    }
}

/// Convert one EventEntry into a cal_core::Event. Returns `Ok(None)`
/// for cancelled rows (they're EXDATE-style deletions of recurring
/// instances and we don't surface them).
pub fn map_event(entry: EventEntry, calendar_id: &str) -> GoogleResult<Option<Event>> {
    if entry.status.as_deref() == Some("cancelled") {
        return Ok(None);
    }
    let (start, start_all_day) = entry.start.resolve()?;
    let (end, end_all_day) = entry.end.resolve()?;
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

    Ok(Some(Event {
        id: entry.id,
        calendar_id: calendar_id.to_string(),
        title: entry.summary.unwrap_or_default(),
        description: entry.description,
        location: entry.location,
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
        etag: entry.etag,
    }))
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
    EventWriteBody {
        summary: Some(new.title.clone()),
        description: new.description.clone(),
        location: new.location.clone(),
        start: range_to_write(new.start, new.all_day),
        end: range_to_write(new.end, new.all_day),
        recurrence: new
            .recurrence
            .as_ref()
            .map(|r| recurrence_to_lines(&r.rrule, &r.exceptions)),
        reminders: reminders_to_write(&new.reminders),
    }
}

/// Convert an existing `Event` into a PATCH body. We send every
/// mutable field so PATCH-with-this-body is effectively a full
/// replacement of the user-visible state — simpler than computing
/// a diff and Google handles it the same.
pub fn event_to_body(ev: &Event) -> EventWriteBody {
    EventWriteBody {
        summary: Some(ev.title.clone()),
        description: ev.description.clone(),
        location: ev.location.clone(),
        start: range_to_write(ev.start, ev.all_day),
        end: range_to_write(ev.end, ev.all_day),
        recurrence: ev
            .recurrence
            .as_ref()
            .map(|r| recurrence_to_lines(&r.rrule, &r.exceptions)),
        reminders: reminders_to_write(&ev.reminders),
    }
}

fn range_to_write(when: DateTime<Utc>, all_day: bool) -> EventDateTimeWrite {
    if all_day {
        EventDateTimeWrite {
            date_time: None,
            date: Some(when.date_naive()),
            time_zone: "Etc/UTC".into(),
        }
    } else {
        EventDateTimeWrite {
            date_time: Some(when),
            date: None,
            time_zone: "Etc/UTC".into(),
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
        assert_eq!(ev.start, Utc.with_ymd_and_hms(2026, 7, 4, 0, 0, 0).unwrap());
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
            reminders: vec![Reminder {
                kind: ReminderKind::Relative { minutes_before: 5 },
                sound: None,
            }],
            sound: None,
            attendees: vec![],
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
        let new = NewEvent {
            title: "Urlaub".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 7, 4, 0, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap(),
            all_day: true,
            recurrence: None,
            color_label: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
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
            }),
            color_label: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: None,
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
        };
        let body = new_event_to_body(&new);
        let json = serde_json::to_value(&body).unwrap();
        let overrides = json["reminders"]["overrides"].as_array().unwrap();
        // Only the Relative one made it through.
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0]["method"], "popup");
    }
}
