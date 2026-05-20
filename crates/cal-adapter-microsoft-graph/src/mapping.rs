//! Microsoft Graph JSON ⇄ cal_core conversion.
//!
//! Reference: <https://learn.microsoft.com/en-us/graph/api/resources/event>.
//!
//! Recurrence is the substantive piece. Graph models recurrence as
//! a structured object (`pattern: { type, interval, daysOfWeek, … }`
//! + `range: { type, endDate, … }`) rather than the RFC 5545 RRULE
//! string the rest of Aperio uses. We translate bidirectionally
//! for the common shapes (daily, weekly+BYDAY, absoluteMonthly,
//! absoluteYearly + COUNT/UNTIL). Relative-monthly / relative-
//! yearly patterns ("last Wednesday of every month") parse on the
//! read side but raise a `GraphError::Protocol` on write — the
//! frontend's EventDialog isn't equipped to edit them today, and
//! losing the rule by silently dropping it would be worse than
//! refusing the write.

use cal_core::{
    Calendar, ColorSource, ContainerColor, Event, EventRecurrence, NewEvent,
    Reminder, ReminderKind,
};
use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{GraphError, GraphResult};

// ── Calendar list ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CalendarListResponse {
    #[serde(default)]
    pub value: Vec<CalendarListEntry>,
    /// Graph uses `@odata.nextLink` instead of Google's
    /// `nextPageToken`; we follow it verbatim.
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CalendarListEntry {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "hexColor")]
    pub hex_color: Option<String>,
    /// `false` ⇒ calendar is shared in read-only mode. Aperio maps
    /// that to `Calendar.read_only`.
    #[serde(default, rename = "canEdit")]
    pub can_edit: Option<bool>,
}

pub fn map_calendar(entry: CalendarListEntry) -> Calendar {
    let color = entry.hex_color.and_then(parse_hex_color);
    Calendar {
        id: entry.id,
        name: entry.name,
        color,
        read_only: !entry.can_edit.unwrap_or(true),
        default_sound: None,
    }
}

fn parse_hex_color(raw: String) -> Option<ContainerColor> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('#') || trimmed.len() != 7 {
        return None;
    }
    if !trimmed[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(ContainerColor {
        hex: trimmed.to_ascii_lowercase(),
        source: ColorSource::Native,
    })
}

// ── Events ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EventListResponse {
    #[serde(default)]
    pub value: Vec<EventEntry>,
    #[serde(default, rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EventEntry {
    pub id: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default, rename = "bodyPreview")]
    pub body_preview: Option<String>,
    #[serde(default)]
    pub location: Option<EventLocation>,
    #[serde(rename = "isAllDay", default)]
    pub is_all_day: bool,
    #[serde(rename = "isCancelled", default)]
    pub is_cancelled: bool,
    #[serde(rename = "isReminderOn", default)]
    pub is_reminder_on: bool,
    #[serde(rename = "reminderMinutesBeforeStart", default)]
    pub reminder_minutes_before_start: Option<i64>,
    pub start: GraphDateTime,
    pub end: GraphDateTime,
    #[serde(default)]
    pub recurrence: Option<RecurrenceObject>,
    #[serde(default, rename = "createdDateTime")]
    pub created_date_time: Option<DateTime<Utc>>,
    #[serde(default, rename = "lastModifiedDateTime")]
    pub last_modified_date_time: Option<DateTime<Utc>>,
    #[serde(default, rename = "@odata.etag")]
    pub etag: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EventLocation {
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GraphDateTime {
    /// Graph's `dateTime` field is a naive ISO-8601-ish string
    /// (`2026-05-25T10:00:00.0000000`) without a `Z` suffix even
    /// when `timeZone` is `"UTC"`. We parse it together with the
    /// timezone field.
    #[serde(rename = "dateTime")]
    pub date_time: String,
    #[serde(rename = "timeZone")]
    pub time_zone: String,
}

impl GraphDateTime {
    /// Convert to UTC. For `timeZone == "UTC"` we just append `Z`
    /// before parsing; for anything else we route through
    /// `chrono_tz` (Graph speaks the IANA names natively).
    fn to_utc(&self) -> GraphResult<DateTime<Utc>> {
        // Strip the seven-digit fractional second Graph likes to
        // emit but chrono rejects past three digits.
        let trimmed = trim_fractional_seconds(&self.date_time);
        if self.time_zone.eq_ignore_ascii_case("UTC")
            || self.time_zone.eq_ignore_ascii_case("Etc/UTC")
        {
            let with_z = format!("{trimmed}Z");
            return with_z.parse::<DateTime<Utc>>().map_err(|e| {
                GraphError::Protocol(format!("graph datetime: {e}: {with_z}"))
            });
        }
        let tz: chrono_tz::Tz = self.time_zone.parse().map_err(|e| {
            GraphError::Protocol(format!(
                "unknown timezone '{}': {e:?}",
                self.time_zone
            ))
        })?;
        let naive = chrono::NaiveDateTime::parse_from_str(
            &trimmed,
            "%Y-%m-%dT%H:%M:%S",
        )
        .map_err(|e| GraphError::Protocol(format!("graph naive datetime: {e}")))?;
        tz.from_local_datetime(&naive)
            .single()
            .ok_or_else(|| {
                GraphError::Protocol(format!(
                    "ambiguous local time {naive} in {}",
                    self.time_zone
                ))
            })
            .map(|local| local.with_timezone(&Utc))
    }
}

fn trim_fractional_seconds(raw: &str) -> String {
    // Graph: "2026-05-25T10:00:00.0000000" — too many fractional
    // digits for chrono. Truncate to three. If there's no fraction
    // at all, return the input unchanged.
    if let Some((head, tail)) = raw.split_once('.') {
        // Take at most three fractional digits, drop the rest.
        let mut frac = tail.chars().take(3).collect::<String>();
        // Strip trailing zeros for cleanliness.
        while frac.ends_with('0') {
            frac.pop();
        }
        if frac.is_empty() {
            head.to_string()
        } else {
            format!("{head}.{frac}")
        }
    } else {
        raw.to_string()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RecurrenceObject {
    pub pattern: RecurrencePattern,
    pub range: RecurrenceRange,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RecurrencePattern {
    #[serde(rename = "type")]
    pub kind: String, // daily | weekly | absoluteMonthly | relativeMonthly | absoluteYearly | relativeYearly
    pub interval: u32,
    #[serde(default, rename = "daysOfWeek")]
    pub days_of_week: Vec<String>,
    #[serde(default, rename = "dayOfMonth")]
    pub day_of_month: Option<u32>,
    #[serde(default)]
    pub month: Option<u32>,
    #[serde(default)]
    pub index: Option<String>,
    #[serde(default, rename = "firstDayOfWeek")]
    pub first_day_of_week: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RecurrenceRange {
    #[serde(rename = "type")]
    pub kind: String, // endDate | noEnd | numbered
    #[serde(default, rename = "startDate")]
    pub start_date: Option<String>,
    #[serde(default, rename = "endDate")]
    pub end_date: Option<String>,
    #[serde(default, rename = "numberOfOccurrences")]
    pub number_of_occurrences: Option<u32>,
}

pub fn map_event(entry: EventEntry, calendar_id: &str) -> GraphResult<Option<Event>> {
    if entry.is_cancelled {
        return Ok(None);
    }
    let start = entry.start.to_utc()?;
    let end = entry.end.to_utc()?;
    let recurrence = match entry.recurrence {
        Some(r) => recurrence_to_rrule(&r).map(|rrule| EventRecurrence {
            rrule,
            exceptions: Vec::new(), // Graph models exceptions separately
        }),
        None => None,
    };

    let reminders = if entry.is_reminder_on {
        match entry.reminder_minutes_before_start {
            Some(minutes) => vec![Reminder {
                kind: ReminderKind::Relative {
                    minutes_before: minutes,
                },
                sound: None,
            }],
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let created = entry.created_date_time.unwrap_or_else(Utc::now);
    let updated = entry.last_modified_date_time.unwrap_or(created);

    Ok(Some(Event {
        id: entry.id,
        calendar_id: calendar_id.to_string(),
        title: entry.subject.unwrap_or_default(),
        description: entry.body_preview,
        location: entry.location.and_then(|l| l.display_name),
        start,
        end,
        all_day: entry.is_all_day,
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

// ── Recurrence: Graph ⇄ RRULE ───────────────────────────────────────────

/// Convert Graph's structured recurrence into an RFC 5545 RRULE
/// body (no `RRULE:` prefix — rest of Aperio stores it bare).
/// Returns `None` for relative-monthly / relative-yearly patterns
/// that we can't represent with the simple BYDAY model — the event
/// still surfaces, just without an editable recurrence in the UI.
pub fn recurrence_to_rrule(r: &RecurrenceObject) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    match r.pattern.kind.as_str() {
        "daily" => parts.push("FREQ=DAILY".into()),
        "weekly" => {
            parts.push("FREQ=WEEKLY".into());
            if !r.pattern.days_of_week.is_empty() {
                let by = r
                    .pattern
                    .days_of_week
                    .iter()
                    .filter_map(|d| day_name_to_rrule(d))
                    .collect::<Vec<_>>()
                    .join(",");
                if !by.is_empty() {
                    parts.push(format!("BYDAY={by}"));
                }
            }
        }
        "absoluteMonthly" => {
            parts.push("FREQ=MONTHLY".into());
            if let Some(dom) = r.pattern.day_of_month {
                parts.push(format!("BYMONTHDAY={dom}"));
            }
        }
        "absoluteYearly" => {
            parts.push("FREQ=YEARLY".into());
            if let Some(m) = r.pattern.month {
                parts.push(format!("BYMONTH={m}"));
            }
            if let Some(dom) = r.pattern.day_of_month {
                parts.push(format!("BYMONTHDAY={dom}"));
            }
        }
        // Relative monthly / yearly skip — we'd need BYSETPOS +
        // BYDAY plus careful index→position translation. Not in
        // the 6e.1 scope.
        _ => return None,
    }
    if r.pattern.interval > 1 {
        parts.push(format!("INTERVAL={}", r.pattern.interval));
    }
    match r.range.kind.as_str() {
        "endDate" => {
            if let Some(end) = r.range.end_date.as_deref() {
                if let Ok(d) = NaiveDate::parse_from_str(end, "%Y-%m-%d") {
                    parts.push(format!(
                        "UNTIL={}",
                        d.format("%Y%m%dT235959Z")
                    ));
                }
            }
        }
        "numbered" => {
            if let Some(n) = r.range.number_of_occurrences {
                parts.push(format!("COUNT={n}"));
            }
        }
        // "noEnd" → no UNTIL/COUNT — open-ended series.
        _ => {}
    }
    Some(parts.join(";"))
}

/// Inverse: parse an RRULE body into Graph's structured form.
/// Returns `Err(Protocol)` when the rule uses features Graph can't
/// represent — e.g. BYSETPOS without a known BYDAY pattern.
pub fn rrule_to_recurrence(
    rrule: &str,
    start: DateTime<Utc>,
) -> GraphResult<RecurrenceObject> {
    let parts: std::collections::HashMap<String, String> = rrule
        .split(';')
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.to_ascii_uppercase(), v.to_string()))
        })
        .collect();
    let freq = parts
        .get("FREQ")
        .ok_or_else(|| GraphError::Protocol("RRULE missing FREQ".into()))?
        .as_str();
    let interval = parts
        .get("INTERVAL")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(1);

    let mut pattern = RecurrencePattern {
        kind: String::new(),
        interval,
        days_of_week: Vec::new(),
        day_of_month: None,
        month: None,
        index: None,
        first_day_of_week: Some("monday".into()),
    };
    match freq {
        "DAILY" => pattern.kind = "daily".into(),
        "WEEKLY" => {
            pattern.kind = "weekly".into();
            if let Some(byday) = parts.get("BYDAY") {
                pattern.days_of_week = byday
                    .split(',')
                    .filter_map(rrule_day_to_name)
                    .collect();
            }
            if pattern.days_of_week.is_empty() {
                // Default to the weekday of `start`.
                pattern
                    .days_of_week
                    .push(weekday_to_name(start.weekday()));
            }
        }
        "MONTHLY" => {
            if let Some(dom) = parts.get("BYMONTHDAY").and_then(|s| s.parse().ok()) {
                pattern.kind = "absoluteMonthly".into();
                pattern.day_of_month = Some(dom);
            } else if parts.contains_key("BYDAY") && parts.contains_key("BYSETPOS")
            {
                // Relative monthly — beyond 6e.1 scope.
                return Err(GraphError::Protocol(
                    "relative monthly recurrence not supported on write yet".into(),
                ));
            } else {
                pattern.kind = "absoluteMonthly".into();
                pattern.day_of_month = Some(start.day());
            }
        }
        "YEARLY" => {
            pattern.kind = "absoluteYearly".into();
            pattern.month = parts
                .get("BYMONTH")
                .and_then(|s| s.parse().ok())
                .or(Some(start.month()));
            pattern.day_of_month = parts
                .get("BYMONTHDAY")
                .and_then(|s| s.parse().ok())
                .or(Some(start.day()));
        }
        other => {
            return Err(GraphError::Protocol(format!(
                "unsupported FREQ: {other}"
            )));
        }
    }

    let mut range = RecurrenceRange {
        kind: "noEnd".into(),
        start_date: Some(start.format("%Y-%m-%d").to_string()),
        end_date: None,
        number_of_occurrences: None,
    };
    if let Some(until) = parts.get("UNTIL") {
        // RFC 5545: compact UTC date-time. Graph wants YYYY-MM-DD.
        if let Some(date_part) = until.get(..8) {
            if let Ok(d) = NaiveDate::parse_from_str(date_part, "%Y%m%d") {
                range.kind = "endDate".into();
                range.end_date = Some(d.format("%Y-%m-%d").to_string());
            }
        }
    } else if let Some(count) = parts.get("COUNT").and_then(|s| s.parse().ok()) {
        range.kind = "numbered".into();
        range.number_of_occurrences = Some(count);
    }

    Ok(RecurrenceObject { pattern, range })
}

fn day_name_to_rrule(name: &str) -> Option<&'static str> {
    // Graph uses lowercase full names. RRULE uses two-letter caps.
    match name.to_ascii_lowercase().as_str() {
        "monday" => Some("MO"),
        "tuesday" => Some("TU"),
        "wednesday" => Some("WE"),
        "thursday" => Some("TH"),
        "friday" => Some("FR"),
        "saturday" => Some("SA"),
        "sunday" => Some("SU"),
        _ => None,
    }
}

fn rrule_day_to_name(code: &str) -> Option<String> {
    Some(match code.to_ascii_uppercase().as_str() {
        "MO" => "monday".into(),
        "TU" => "tuesday".into(),
        "WE" => "wednesday".into(),
        "TH" => "thursday".into(),
        "FR" => "friday".into(),
        "SA" => "saturday".into(),
        "SU" => "sunday".into(),
        _ => return None,
    })
}

fn weekday_to_name(w: chrono::Weekday) -> String {
    use chrono::Weekday::*;
    match w {
        Mon => "monday",
        Tue => "tuesday",
        Wed => "wednesday",
        Thu => "thursday",
        Fri => "friday",
        Sat => "saturday",
        Sun => "sunday",
    }
    .to_string()
}

// ── Reverse mapping: cal_core → Graph JSON ──────────────────────────────

#[derive(Debug, Serialize)]
pub struct EventWriteBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<EventBodyWrite>,
    pub start: GraphDateTimeWrite,
    pub end: GraphDateTimeWrite,
    #[serde(rename = "isAllDay")]
    pub is_all_day: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<EventLocationWrite>,
    #[serde(rename = "isReminderOn")]
    pub is_reminder_on: bool,
    #[serde(skip_serializing_if = "Option::is_none", rename = "reminderMinutesBeforeStart")]
    pub reminder_minutes_before_start: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<RecurrenceObject>,
}

#[derive(Debug, Serialize)]
pub struct EventBodyWrite {
    #[serde(rename = "contentType")]
    pub content_type: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct EventLocationWrite {
    #[serde(rename = "displayName")]
    pub display_name: String,
}

#[derive(Debug, Serialize)]
pub struct GraphDateTimeWrite {
    #[serde(rename = "dateTime")]
    pub date_time: String,
    #[serde(rename = "timeZone")]
    pub time_zone: String,
}

pub fn new_event_to_body(new: &NewEvent) -> GraphResult<EventWriteBody> {
    let body = EventWriteBody {
        subject: Some(new.title.clone()),
        body: new.description.clone().map(|c| EventBodyWrite {
            content_type: "text".into(),
            content: c,
        }),
        start: write_datetime(new.start, new.all_day),
        end: write_datetime(new.end, new.all_day),
        is_all_day: new.all_day,
        location: new.location.clone().map(|l| EventLocationWrite {
            display_name: l,
        }),
        is_reminder_on: first_reminder_minutes(&new.reminders).is_some(),
        reminder_minutes_before_start: first_reminder_minutes(&new.reminders),
        recurrence: match new.recurrence.as_ref() {
            Some(r) => Some(rrule_to_recurrence(&r.rrule, new.start)?),
            None => None,
        },
    };
    Ok(body)
}

pub fn event_to_body(ev: &Event) -> GraphResult<EventWriteBody> {
    Ok(EventWriteBody {
        subject: Some(ev.title.clone()),
        body: ev.description.clone().map(|c| EventBodyWrite {
            content_type: "text".into(),
            content: c,
        }),
        start: write_datetime(ev.start, ev.all_day),
        end: write_datetime(ev.end, ev.all_day),
        is_all_day: ev.all_day,
        location: ev.location.clone().map(|l| EventLocationWrite {
            display_name: l,
        }),
        is_reminder_on: first_reminder_minutes(&ev.reminders).is_some(),
        reminder_minutes_before_start: first_reminder_minutes(&ev.reminders),
        recurrence: match ev.recurrence.as_ref() {
            Some(r) => Some(rrule_to_recurrence(&r.rrule, ev.start)?),
            None => None,
        },
    })
}

fn write_datetime(when: DateTime<Utc>, all_day: bool) -> GraphDateTimeWrite {
    if all_day {
        // For all-day events Graph still expects a `dateTime`
        // (not a `date`) but at 00:00:00 in UTC.
        GraphDateTimeWrite {
            date_time: when.format("%Y-%m-%dT00:00:00").to_string(),
            time_zone: "UTC".into(),
        }
    } else {
        GraphDateTimeWrite {
            date_time: when.format("%Y-%m-%dT%H:%M:%S").to_string(),
            time_zone: "UTC".into(),
        }
    }
}

fn first_reminder_minutes(reminders: &[Reminder]) -> Option<i64> {
    // Graph models per-event reminders as a single (minutes,
    // boolean) pair — there's no overrides array like Google's.
    // We take the first Relative reminder; everything else
    // (Absolute, AppStart, Email) stays local-only because Graph
    // has nowhere to put it.
    reminders.iter().find_map(|r| match r.kind {
        ReminderKind::Relative { minutes_before } => Some(minutes_before),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_can_edit_false_maps_to_read_only() {
        let entry: CalendarListEntry = serde_json::from_str(
            r##"{"id":"abc","name":"Shared","hexColor":"#0078d4","canEdit":false}"##,
        )
        .unwrap();
        let cal = map_calendar(entry);
        assert!(cal.read_only);
        assert_eq!(cal.color.unwrap().hex, "#0078d4");
    }

    #[test]
    fn trim_fractional_seconds_drops_excess() {
        assert_eq!(
            trim_fractional_seconds("2026-05-25T10:00:00.0000000"),
            "2026-05-25T10:00:00"
        );
        assert_eq!(
            trim_fractional_seconds("2026-05-25T10:00:00.500"),
            "2026-05-25T10:00:00.5"
        );
        assert_eq!(
            trim_fractional_seconds("2026-05-25T10:00:00"),
            "2026-05-25T10:00:00"
        );
    }

    #[test]
    fn map_event_timed_utc() {
        let raw = r#"{
            "id": "ev1",
            "subject": "Standup",
            "start": { "dateTime": "2026-05-25T10:00:00.0000000", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T10:30:00.0000000", "timeZone": "UTC" },
            "isAllDay": false,
            "isReminderOn": true,
            "reminderMinutesBeforeStart": 15
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert_eq!(ev.title, "Standup");
        assert_eq!(ev.start, Utc.with_ymd_and_hms(2026, 5, 25, 10, 0, 0).unwrap());
        assert_eq!(ev.reminders.len(), 1);
        match ev.reminders[0].kind {
            ReminderKind::Relative { minutes_before } => assert_eq!(minutes_before, 15),
            ref other => panic!("expected Relative, got {other:?}"),
        }
    }

    #[test]
    fn map_event_with_berlin_timezone_converts_to_utc() {
        let raw = r#"{
            "id": "ev1",
            "subject": "Termin",
            "start": { "dateTime": "2026-05-25T12:00:00.0000000", "timeZone": "Europe/Berlin" },
            "end":   { "dateTime": "2026-05-25T13:00:00.0000000", "timeZone": "Europe/Berlin" },
            "isAllDay": false
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        // Berlin is CEST (UTC+2) on 25 May.
        assert_eq!(ev.start, Utc.with_ymd_and_hms(2026, 5, 25, 10, 0, 0).unwrap());
    }

    #[test]
    fn cancelled_event_is_filtered() {
        let raw = r#"{
            "id": "ev-x",
            "isCancelled": true,
            "isAllDay": false,
            "start": { "dateTime": "2026-05-25T10:00:00", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T11:00:00", "timeZone": "UTC" }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        assert!(map_event(entry, "primary").unwrap().is_none());
    }

    #[test]
    fn recurrence_daily_round_trip() {
        let raw = r#"{
            "id": "ev",
            "subject": "Daily",
            "start": { "dateTime": "2026-05-25T10:00:00", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T11:00:00", "timeZone": "UTC" },
            "isAllDay": false,
            "recurrence": {
                "pattern": {"type": "daily", "interval": 2, "daysOfWeek": []},
                "range": {"type": "noEnd", "startDate": "2026-05-25"}
            }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        let rule = ev.recurrence.unwrap().rrule;
        assert!(rule.contains("FREQ=DAILY"));
        assert!(rule.contains("INTERVAL=2"));
        // Round-trip: parse back.
        let recur = rrule_to_recurrence(&rule, ev.start).unwrap();
        assert_eq!(recur.pattern.kind, "daily");
        assert_eq!(recur.pattern.interval, 2);
        assert_eq!(recur.range.kind, "noEnd");
    }

    #[test]
    fn recurrence_weekly_byday_round_trip() {
        let raw = r#"{
            "id": "ev",
            "subject": "Weekly",
            "start": { "dateTime": "2026-05-25T18:00:00", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-25T19:00:00", "timeZone": "UTC" },
            "isAllDay": false,
            "recurrence": {
                "pattern": {"type": "weekly", "interval": 1, "daysOfWeek": ["monday", "wednesday"]},
                "range": {"type": "numbered", "startDate": "2026-05-25", "numberOfOccurrences": 10}
            }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        let rule = ev.recurrence.unwrap().rrule;
        assert!(rule.contains("FREQ=WEEKLY"));
        assert!(rule.contains("BYDAY=MO,WE"));
        assert!(rule.contains("COUNT=10"));
        let recur = rrule_to_recurrence(&rule, ev.start).unwrap();
        assert_eq!(recur.pattern.kind, "weekly");
        assert_eq!(recur.pattern.days_of_week, vec!["monday", "wednesday"]);
        assert_eq!(recur.range.number_of_occurrences, Some(10));
    }

    #[test]
    fn recurrence_absolute_monthly_with_until_round_trip() {
        let raw = r#"{
            "id": "ev",
            "subject": "Bill",
            "start": { "dateTime": "2026-05-15T10:00:00", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-15T11:00:00", "timeZone": "UTC" },
            "isAllDay": false,
            "recurrence": {
                "pattern": {"type": "absoluteMonthly", "interval": 1, "dayOfMonth": 15},
                "range": {"type": "endDate", "startDate": "2026-05-15", "endDate": "2027-05-15"}
            }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        let rule = ev.recurrence.unwrap().rrule;
        assert!(rule.contains("FREQ=MONTHLY"));
        assert!(rule.contains("BYMONTHDAY=15"));
        assert!(rule.contains("UNTIL=20270515T235959Z"));
    }

    #[test]
    fn recurrence_relative_monthly_drops_to_none_on_read() {
        // Read side: we don't surface a recurrence we can't
        // represent, so the event still appears (just as a
        // standalone). Better than failing the whole listing.
        let raw = r#"{
            "id": "ev",
            "subject": "Last Wed",
            "start": { "dateTime": "2026-05-27T10:00:00", "timeZone": "UTC" },
            "end":   { "dateTime": "2026-05-27T11:00:00", "timeZone": "UTC" },
            "isAllDay": false,
            "recurrence": {
                "pattern": {"type": "relativeMonthly", "interval": 1, "daysOfWeek": ["wednesday"], "index": "last"},
                "range": {"type": "noEnd", "startDate": "2026-05-27"}
            }
        }"#;
        let entry: EventEntry = serde_json::from_str(raw).unwrap();
        let ev = map_event(entry, "primary").unwrap().unwrap();
        assert!(ev.recurrence.is_none());
        assert_eq!(ev.title, "Last Wed");
    }

    #[test]
    fn rrule_to_recurrence_relative_monthly_errors() {
        // Write side: explicitly refuse so the user notices instead
        // of silently dropping the rule.
        let err = rrule_to_recurrence(
            "FREQ=MONTHLY;BYDAY=WE;BYSETPOS=-1",
            Utc.with_ymd_and_hms(2026, 5, 27, 10, 0, 0).unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, GraphError::Protocol(_)));
    }

    #[test]
    fn new_event_to_body_carries_reminder_and_recurrence() {
        let new = NewEvent {
            title: "Yoga".into(),
            description: None,
            location: Some("Studio".into()),
            start: Utc.with_ymd_and_hms(2026, 5, 25, 18, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 25, 19, 0, 0).unwrap(),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
                exceptions: Vec::new(),
            }),
            color_label: None,
            reminders: vec![Reminder {
                kind: ReminderKind::Relative {
                    minutes_before: 10,
                },
                sound: None,
            }],
            sound: None,
            attendees: Vec::new(),
        };
        let body = new_event_to_body(&new).unwrap();
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["subject"], "Yoga");
        assert_eq!(json["isReminderOn"], true);
        assert_eq!(json["reminderMinutesBeforeStart"], 10);
        assert_eq!(json["recurrence"]["pattern"]["type"], "weekly");
        assert_eq!(
            json["recurrence"]["pattern"]["daysOfWeek"][0],
            "monday"
        );
        assert_eq!(json["location"]["displayName"], "Studio");
    }
}
