//! `CalendarFeature` implementation plus local-only management methods
//! (`create_calendar`, `update_calendar`, `delete_calendar`).
//!
//! The trait methods are designed for external providers where calendars
//! pre-exist on the server. Local calendars are user-created, so we expose
//! inherent methods on [`LocalAdapter`] for those — they are not part of
//! the public adapter trait surface.

use async_trait::async_trait;
use cal_core::{
    Calendar, CalendarFeature, ColorLabelId, ContainerColor, DateRange, Event, EventRecurrence,
    FreeBusy, NewEvent, Reminder, SoundConfig,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::mapping::{
    decode_json, encode_json, fmt_utc, opt_text, parse_utc, read_bool, read_container_color,
    read_sound, req_text, write_container_color, write_sound,
};
use crate::{map_sql_err, LocalAdapter, SOURCE_ID};

impl LocalAdapter {
    /// Create a new local calendar. Returns the freshly-inserted row.
    pub fn create_calendar(
        &self,
        name: &str,
        color: Option<ContainerColor>,
        color_label: Option<ColorLabelId>,
        default_sound: Option<SoundConfig>,
    ) -> cal_core::Result<Calendar> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_s = fmt_utc(&now);
        let (color_hex, color_source) = write_container_color(&color);
        let default_sound_json = write_sound(&default_sound)?;

        self.db
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "INSERT INTO calendars (
                    id, source, name, color_hex, color_source, color_label_id,
                    read_only, default_sound, created_at, updated_at
                 ) VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?)",
                params![
                    id,
                    SOURCE_ID,
                    name,
                    color_hex,
                    color_source,
                    color_label.as_ref().map(|c| c.as_str()),
                    default_sound_json,
                    now_s,
                    now_s,
                ],
            )
            .map_err(map_sql_err)?;

        Ok(Calendar {
            id,
            name: name.to_string(),
            color,
            color_label,
            read_only: false,
            default_sound,
            supports_scheduling: false,
            // Local calendars store a per-event color natively (on the
            // event row), so the host routes recolors through update_event.
            supports_event_color: true,
        })
    }

    /// Rename a calendar and/or change its color/sound.
    pub fn update_calendar(&self, calendar: Calendar) -> cal_core::Result<Calendar> {
        let now_s = fmt_utc(&Utc::now());
        let (color_hex, color_source) = write_container_color(&calendar.color);
        let default_sound_json = write_sound(&calendar.default_sound)?;

        let changed = self
            .db
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE calendars
                    SET name = ?, color_hex = ?, color_source = ?,
                        color_label_id = ?, default_sound = ?, updated_at = ?
                  WHERE id = ?",
                params![
                    calendar.name,
                    color_hex,
                    color_source,
                    calendar.color_label.as_ref().map(|c| c.as_str()),
                    default_sound_json,
                    now_s,
                    calendar.id,
                ],
            )
            .map_err(map_sql_err)?;

        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "calendar '{}' not found",
                calendar.id
            )));
        }
        Ok(calendar)
    }

    /// Delete a calendar. Events are removed via `ON DELETE CASCADE`.
    pub fn delete_calendar(&self, id: &str) -> cal_core::Result<()> {
        let changed = self
            .db
            .lock()
            .expect("db mutex poisoned")
            .execute("DELETE FROM calendars WHERE id = ?", params![id])
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "calendar '{id}' not found"
            )));
        }
        Ok(())
    }

    /// Fetch a single event by id. Returns `Ok(None)` when the row
    /// does not exist. Used by the reminders overview to open an
    /// item the user picks from the list — the overview only stores
    /// item ids, not full payloads.
    pub fn get_event_by_id(&self, id: &str) -> cal_core::Result<Option<Event>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, calendar_id, title, description, location,
                        start_utc, end_utc, all_day, rrule, rrule_exceptions,
                        color_label_id, reminders, sound, attendees,
                        created_at, updated_at, etag, rrule_tzid
                   FROM events WHERE id = ?",
            )
            .map_err(map_sql_err)?;
        let row = stmt
            .query_row(params![id], |r| Ok(row_to_event(r)))
            .optional()
            .map_err(map_sql_err)?;
        match row {
            None => Ok(None),
            Some(res) => res.map(Some),
        }
    }

    /// Read one calendar row by id, returning `None` if it doesn't
    /// exist. Used by the conflict-detection path so it can compare
    /// the proposed patch against the live row.
    pub fn get_calendar_by_id(&self, id: &str) -> cal_core::Result<Option<Calendar>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, name, color_hex, color_source, read_only, default_sound,
                        color_label_id
                   FROM calendars WHERE id = ?",
            )
            .map_err(map_sql_err)?;
        let row = stmt
            .query_row(params![id], |r| {
                Ok((
                    req_text(r, 0),
                    req_text(r, 1),
                    read_container_color(r, 2, 3),
                    read_bool(r, 4),
                    read_sound(r, 5),
                    opt_text(r, 6),
                ))
            })
            .optional()
            .map_err(map_sql_err)?;
        let Some(parts) = row else {
            return Ok(None);
        };
        let (id, name, color, read_only, sound, color_label) = parts;
        Ok(Some(Calendar {
            id: id?,
            name: name?,
            color: color?,
            color_label: color_label?.map(ColorLabelId),
            read_only: read_only?,
            default_sound: sound?,
            supports_scheduling: false,
            supports_event_color: true,
        }))
    }

    /// Append a single date to a recurring event's EXDATE list so the
    /// expansion engine skips that occurrence. Used by the UI's
    /// "edit / delete this occurrence only" flow — the master row's
    /// other columns (start, title, ...) are left untouched.
    pub fn add_event_exdate(
        &self,
        event_id: &str,
        occurrence_utc: DateTime<Utc>,
    ) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let row: Option<(Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT rrule, rrule_exceptions FROM events WHERE id = ?",
                params![event_id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sql_err)?;

        let (rrule, exceptions) = match row {
            Some(v) => v,
            None => {
                return Err(cal_core::Error::NotFound(format!(
                    "event '{event_id}' not found"
                )))
            }
        };
        if rrule.is_none() {
            return Err(cal_core::Error::InvalidInput(format!(
                "event '{event_id}' is not recurring"
            )));
        }

        let mut list: Vec<DateTime<Utc>> = match exceptions {
            None => Vec::new(),
            Some(s) => decode_json(&s)?,
        };
        if !list.contains(&occurrence_utc) {
            list.push(occurrence_utc);
        }
        let exc_json = encode_json(&list)?;
        let now_s = fmt_utc(&Utc::now());

        conn.execute(
            "UPDATE events SET rrule_exceptions = ?, updated_at = ? WHERE id = ?",
            params![exc_json, now_s, event_id],
        )
        .map_err(map_sql_err)?;
        Ok(())
    }
}

#[async_trait]
impl CalendarFeature for LocalAdapter {
    async fn list_calendars(&self) -> cal_core::Result<Vec<Calendar>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, name, color_hex, color_source, read_only, default_sound,
                        color_label_id
                   FROM calendars
                  ORDER BY name COLLATE NOCASE",
            )
            .map_err(map_sql_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    req_text(row, 0),
                    req_text(row, 1),
                    read_container_color(row, 2, 3),
                    read_bool(row, 4),
                    read_sound(row, 5),
                    opt_text(row, 6),
                ))
            })
            .map_err(map_sql_err)?;

        let mut out = Vec::new();
        for r in rows {
            let (id, name, color, read_only, sound, color_label) = r.map_err(map_sql_err)?;
            out.push(Calendar {
                supports_scheduling: false,
                supports_event_color: true,
                id: id?,
                name: name?,
                color: color?,
                color_label: color_label?.map(ColorLabelId),
                read_only: read_only?,
                default_sound: sound?,
            });
        }
        Ok(out)
    }

    async fn get_events(
        &self,
        calendar_id: &str,
        range: DateRange,
    ) -> cal_core::Result<Vec<Event>> {
        let conn = self.db().lock().expect("db mutex poisoned");
        // Phase 1: simple temporal filter. Recurrence expansion (RRULE)
        // happens in Phase 4 once we wire up an evaluator — for now we
        // return rows whose stored start/end intersect the requested range.
        let mut stmt = conn.prepare(EVENT_SELECT_PREFIX).map_err(map_sql_err)?;

        let start_s = fmt_utc(&range.start);
        let end_s = fmt_utc(&range.end);
        let rows = stmt
            .query_map(
                // calendar_id, range.end, range.start, range.end — the
                // trailing end feeds the recurring-master clause.
                params![calendar_id, end_s, start_s, end_s],
                row_to_event_result,
            )
            .map_err(map_sql_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql_err)??);
        }
        Ok(out)
    }

    async fn create_event(&self, calendar_id: &str, event: NewEvent) -> cal_core::Result<Event> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let stored = persist_new_event(self, calendar_id, &id, now, event)?;
        Ok(stored)
    }

    async fn update_event(&self, event: Event) -> cal_core::Result<Event> {
        let now = Utc::now();
        let mut event = event;
        event.updated_at = now;

        let conn = self.db().lock().expect("db mutex poisoned");
        let reminders_json = encode_json(&event.reminders)?;
        let attendees_json = encode_json(&event.attendees)?;
        let sound_json = write_sound(&event.sound)?;
        let (rrule, exceptions, rrule_tzid) = split_recurrence(&event.recurrence)?;

        let changed = conn
            .execute(
                "UPDATE events
                    SET calendar_id = ?, title = ?, description = ?, location = ?,
                        start_utc = ?, end_utc = ?, all_day = ?, rrule = ?,
                        rrule_exceptions = ?, rrule_tzid = ?, color_label_id = ?,
                        reminders = ?, sound = ?, attendees = ?,
                        updated_at = ?, etag = ?
                  WHERE id = ?",
                params![
                    event.calendar_id,
                    event.title,
                    event.description,
                    event.location,
                    fmt_utc(&event.start),
                    fmt_utc(&event.end),
                    event.all_day as i64,
                    rrule,
                    exceptions,
                    rrule_tzid,
                    event.color_label.as_ref().map(|c| c.as_str()),
                    reminders_json,
                    sound_json,
                    attendees_json,
                    fmt_utc(&event.updated_at),
                    event.etag,
                    event.id,
                ],
            )
            .map_err(map_sql_err)?;

        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "event '{}' not found",
                event.id
            )));
        }
        Ok(event)
    }

    async fn delete_event(
        &self,
        event_id: &str,
        _send_cancellations: bool,
    ) -> cal_core::Result<()> {
        let conn = self.db().lock().expect("db mutex poisoned");
        let changed = conn
            .execute("DELETE FROM events WHERE id = ?", params![event_id])
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "event '{event_id}' not found"
            )));
        }
        Ok(())
    }

    async fn get_free_busy(
        &self,
        _emails: &[&str],
        _range: DateRange,
    ) -> cal_core::Result<Vec<FreeBusy>> {
        // Local calendars have no notion of remote attendees, so a free/busy
        // lookup is meaningless. We return an empty list rather than an
        // error so a unified "across all adapters" query stays simple.
        Ok(Vec::new())
    }

    async fn add_event_exdate(
        &self,
        event_id: &str,
        occurrence: DateTime<Utc>,
    ) -> cal_core::Result<()> {
        // Re-use the inherent method that already implements the
        // read-modify-write for the JSON-stored exception list.
        LocalAdapter::add_event_exdate(self, event_id, occurrence)
    }

    fn calendar_color(&self, calendar_id: &str) -> Option<ContainerColor> {
        let conn = self.db().lock().expect("db mutex poisoned");
        conn.query_row(
            "SELECT color_hex, color_source FROM calendars WHERE id = ?",
            params![calendar_id],
            |row| Ok(read_container_color(row, 0, 1)),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|res| res.ok())
        .flatten()
    }

    async fn rename_calendar(&self, calendar_id: &str, new_name: &str) -> cal_core::Result<()> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(cal_core::Error::InvalidInput(
                "calendar name must not be empty".into(),
            ));
        }
        let changed = self
            .db
            .lock()
            .expect("db mutex poisoned")
            .execute(
                "UPDATE calendars SET name = ? WHERE id = ?",
                params![trimmed, calendar_id],
            )
            .map_err(map_sql_err)?;
        if changed == 0 {
            return Err(cal_core::Error::NotFound(format!(
                "calendar '{calendar_id}' not found"
            )));
        }
        Ok(())
    }
}

// Range read for the calendar views. The first clause is the plain
// half-open interval overlap for one-off events. The second keeps every
// RECURRING master whose series BEGINS before the range end, regardless
// of where its first occurrence's `end_utc` falls — a weekly event
// created a year ago has its stored `start_utc`/`end_utc` in the past but
// still recurs into the current view, and the frontend (rrule.js) expands
// the in-range occurrences. Without it the master fails the overlap test
// and silently vanishes from future months (the same class of bug the
// host snapshot cache had for external recurring events). Bind order:
// calendar_id, range.end, range.start, range.end.
const EVENT_SELECT_PREFIX: &str =
    "SELECT id, calendar_id, title, description, location, start_utc, end_utc,
            all_day, rrule, rrule_exceptions, color_label_id, reminders, sound,
            attendees, created_at, updated_at, etag, rrule_tzid
       FROM events
      WHERE calendar_id = ?
        AND ( (start_utc < ? AND end_utc > ?)
              OR (rrule IS NOT NULL AND start_utc < ?) )
      ORDER BY start_utc";

pub(crate) fn split_recurrence(
    rec: &Option<EventRecurrence>,
) -> cal_core::Result<(Option<String>, Option<String>, Option<String>)> {
    match rec {
        None => Ok((None, None, None)),
        Some(r) => {
            let exc = encode_json(&r.exceptions)?;
            Ok((Some(r.rrule.clone()), Some(exc), r.tzid.clone()))
        }
    }
}

fn combine_recurrence(
    rrule: Option<String>,
    exceptions: Option<String>,
    tzid: Option<String>,
) -> cal_core::Result<Option<EventRecurrence>> {
    match rrule {
        None => Ok(None),
        Some(rrule) => {
            let exceptions: Vec<DateTime<Utc>> = match exceptions {
                None => Vec::new(),
                Some(s) => decode_json(&s)?,
            };
            Ok(Some(EventRecurrence {
                rrule,
                exceptions,
                tzid,
            }))
        }
    }
}

fn persist_new_event(
    adapter: &LocalAdapter,
    calendar_id: &str,
    id: &str,
    now: DateTime<Utc>,
    event: NewEvent,
) -> cal_core::Result<Event> {
    let reminders_json = encode_json(&event.reminders)?;
    let attendees_json = encode_json(&event.attendees)?;
    let sound_json = write_sound(&event.sound)?;
    let (rrule, exceptions, rrule_tzid) = split_recurrence(&event.recurrence)?;

    let now_s = fmt_utc(&now);
    adapter
        .db()
        .lock()
        .expect("db mutex poisoned")
        .execute(
            "INSERT INTO events (
                id, calendar_id, title, description, location, start_utc,
                end_utc, all_day, rrule, rrule_exceptions, rrule_tzid,
                color_label_id, reminders, sound, attendees, created_at,
                updated_at, etag
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
            params![
                id,
                calendar_id,
                event.title,
                event.description,
                event.location,
                fmt_utc(&event.start),
                fmt_utc(&event.end),
                event.all_day as i64,
                rrule,
                exceptions,
                rrule_tzid,
                event.color_label.as_ref().map(|c| c.as_str()),
                reminders_json,
                sound_json,
                attendees_json,
                now_s,
                now_s,
            ],
        )
        .map_err(map_sql_err)?;

    Ok(Event {
        send_invitations: false,
        id: id.to_string(),
        calendar_id: calendar_id.to_string(),
        title: event.title,
        description: event.description,
        location: event.location,
        start: event.start,
        end: event.end,
        all_day: event.all_day,
        recurrence: event.recurrence,
        color_label: event.color_label,
        // Transport-only; local stores its color on `color_label`. Carried
        // through verbatim (it is `None` for every local write).
        color_hex: event.color_hex,
        reminders: event.reminders,
        sound: event.sound,
        attendees: event.attendees,
        created_at: now,
        updated_at: now,
        etag: None,
        organizer: None,
        attendee_responses: Vec::new(),
    })
}

fn row_to_event_result(row: &rusqlite::Row<'_>) -> rusqlite::Result<cal_core::Result<Event>> {
    Ok(row_to_event(row))
}

pub(crate) fn row_to_event(row: &rusqlite::Row<'_>) -> cal_core::Result<Event> {
    let id = req_text(row, 0)?;
    let calendar_id = req_text(row, 1)?;
    let title = req_text(row, 2)?;
    let description = opt_text(row, 3)?;
    let location = opt_text(row, 4)?;
    let start = parse_utc(&req_text(row, 5)?)?;
    let end = parse_utc(&req_text(row, 6)?)?;
    let all_day = read_bool(row, 7)?;
    let rrule = opt_text(row, 8)?;
    let exceptions = opt_text(row, 9)?;
    let color_label = opt_text(row, 10)?.map(cal_core::ColorLabelId);
    let reminders: Vec<Reminder> = decode_json(&req_text(row, 11)?)?;
    let sound = read_sound(row, 12)?;
    let attendees: Vec<String> = decode_json(&req_text(row, 13)?)?;
    let created_at = parse_utc(&req_text(row, 14)?)?;
    let updated_at = parse_utc(&req_text(row, 15)?)?;
    let etag = opt_text(row, 16)?;
    let rrule_tzid = opt_text(row, 17)?;
    let recurrence = combine_recurrence(rrule, exceptions, rrule_tzid)?;

    Ok(Event {
        send_invitations: false,
        id,
        calendar_id,
        title,
        description,
        location,
        start,
        end,
        all_day,
        recurrence,
        color_label,
        // Native color lives on `color_label`; the transport-only hex is
        // never read from the local row.
        color_hex: None,
        reminders,
        sound,
        attendees,
        created_at,
        updated_at,
        etag,
        // Local events have no organizer/RSVP metadata; those are
        // read-only fields populated only by external providers.
        organizer: None,
        attendee_responses: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_test_db;
    use cal_core::{Adapter, CalendarFeature, Capability, ColorSource, Credentials};
    use chrono::Duration;

    fn make_adapter() -> LocalAdapter {
        LocalAdapter::new(open_test_db())
    }

    #[tokio::test]
    async fn create_and_list_calendar() {
        let a = make_adapter();
        let cal = a
            .create_calendar(
                "Work",
                Some(ContainerColor {
                    hex: "#1e88e5".into(),
                    source: ColorSource::Custom,
                }),
                None,
                None,
            )
            .unwrap();
        let cals = a.list_calendars().await.unwrap();
        assert_eq!(cals.len(), 1);
        assert_eq!(cals[0].id, cal.id);
        assert_eq!(cals[0].name, "Work");
        assert_eq!(
            cals[0].color.as_ref().map(|c| c.hex.as_str()),
            Some("#1e88e5")
        );
    }

    #[tokio::test]
    async fn rename_calendar() {
        let a = make_adapter();
        let mut cal = a.create_calendar("Work", None, None, None).unwrap();
        cal.name = "Office".into();
        a.update_calendar(cal.clone()).unwrap();
        let cals = a.list_calendars().await.unwrap();
        assert_eq!(cals[0].name, "Office");
    }

    #[tokio::test]
    async fn delete_calendar_cascades_events() {
        let a = make_adapter();
        let cal = a.create_calendar("Work", None, None, None).unwrap();
        let start = Utc::now();
        a.create_event(
            &cal.id,
            NewEvent {
                title: "Standup".into(),
                description: None,
                location: None,
                start,
                end: start + Duration::minutes(15),
                all_day: false,
                recurrence: None,
                color_label: None,
                color_hex: None,
                reminders: vec![],
                sound: None,
                attendees: vec![],
                send_invitations: false,
            },
        )
        .await
        .unwrap();

        a.delete_calendar(&cal.id).unwrap();

        // Calendar gone, events gone (FK cascade).
        let cals = a.list_calendars().await.unwrap();
        assert!(cals.is_empty());
        let conn = a.db().lock().unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn rename_calendar_updates_the_row() {
        let a = make_adapter();
        let cal = a.create_calendar("Old", None, None, None).unwrap();
        a.rename_calendar(&cal.id, "New").await.unwrap();
        let cals = a.list_calendars().await.unwrap();
        assert_eq!(cals.len(), 1);
        assert_eq!(cals[0].name, "New");
    }

    #[tokio::test]
    async fn rename_calendar_rejects_empty_name() {
        let a = make_adapter();
        let cal = a.create_calendar("Old", None, None, None).unwrap();
        let err = a.rename_calendar(&cal.id, "   ").await.unwrap_err();
        assert!(matches!(err, cal_core::Error::InvalidInput(_)));
        // Original name unchanged.
        assert_eq!(a.list_calendars().await.unwrap()[0].name, "Old");
    }

    #[tokio::test]
    async fn rename_calendar_returns_not_found_for_unknown_id() {
        let a = make_adapter();
        let err = a
            .rename_calendar("does-not-exist", "Whatever")
            .await
            .unwrap_err();
        assert!(matches!(err, cal_core::Error::NotFound(_)));
    }

    #[tokio::test]
    async fn event_range_query_is_temporal_intersection() {
        let a = make_adapter();
        let cal = a.create_calendar("Work", None, None, None).unwrap();
        let base = "2026-05-19T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let mk = |offset_h: i64| NewEvent {
            title: format!("E{offset_h}"),
            description: None,
            location: None,
            start: base + Duration::hours(offset_h),
            end: base + Duration::hours(offset_h + 1),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: vec![],
            sound: None,
            attendees: vec![],
            send_invitations: false,
        };
        for h in [0, 5, 24] {
            a.create_event(&cal.id, mk(h)).await.unwrap();
        }

        // Range that covers only the first two events.
        let range = DateRange {
            start: base - Duration::hours(1),
            end: base + Duration::hours(8),
        };
        let evs = a.get_events(&cal.id, range).await.unwrap();
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].title, "E0");
        assert_eq!(evs[1].title, "E5");
    }

    #[tokio::test]
    async fn recurrence_timezone_round_trips_through_storage() {
        // A zoned recurring master must persist its IANA tzid (the new synced
        // column), so a series created in Aperio expands DST-correctly on re-read
        // instead of drifting in UTC.
        let a = make_adapter();
        let cal = a.create_calendar("Work", None, None, None).unwrap();
        let start = "2025-12-15T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let created = a
            .create_event(
                &cal.id,
                NewEvent {
                    title: "OAGDU".into(),
                    description: None,
                    location: None,
                    start,
                    end: start + Duration::hours(1),
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
                    send_invitations: false,
                },
            )
            .await
            .unwrap();
        // Re-read from storage (not the create return value).
        let range = DateRange {
            start: "2026-07-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap(),
            end: "2026-08-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap(),
        };
        let evs = a.get_events(&cal.id, range).await.unwrap();
        let master = evs
            .iter()
            .find(|e| e.id == created.id)
            .expect("master read back");
        assert_eq!(
            master.recurrence.as_ref().unwrap().tzid.as_deref(),
            Some("America/New_York")
        );
    }

    #[tokio::test]
    async fn recurring_master_with_past_start_survives_a_future_range() {
        let a = make_adapter();
        let cal = a.create_calendar("Work", None, None, None).unwrap();
        let past = "2025-01-06T10:00:00Z".parse::<DateTime<Utc>>().unwrap();
        // A weekly series that began over a year before the queried month.
        a.create_event(
            &cal.id,
            NewEvent {
                title: "Weekly".into(),
                description: None,
                location: None,
                start: past,
                end: past + Duration::hours(1),
                all_day: false,
                recurrence: Some(EventRecurrence {
                    rrule: "FREQ=WEEKLY".into(),
                    exceptions: vec![],
                    tzid: None,
                }),
                color_label: None,
                color_hex: None,
                reminders: vec![],
                sound: None,
                attendees: vec![],
                send_invitations: false,
            },
        )
        .await
        .unwrap();
        // A one-off on the same past day — the control: a non-recurring
        // event must NOT leak into an unrelated future range.
        a.create_event(
            &cal.id,
            NewEvent {
                title: "OneOff".into(),
                description: None,
                location: None,
                start: past,
                end: past + Duration::hours(1),
                all_day: false,
                recurrence: None,
                color_label: None,
                color_hex: None,
                reminders: vec![],
                sound: None,
                attendees: vec![],
                send_invitations: false,
            },
        )
        .await
        .unwrap();

        let june = DateRange {
            start: "2026-06-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap(),
            end: "2026-07-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap(),
        };
        let evs = a.get_events(&cal.id, june).await.unwrap();
        // The recurring master survives so the frontend can expand June's
        // occurrences; the past one-off does not.
        assert_eq!(
            evs.len(),
            1,
            "recurring master must survive, one-off must not"
        );
        assert_eq!(evs[0].title, "Weekly");
    }

    #[tokio::test]
    async fn update_event_persists_changes() {
        let a = make_adapter();
        let cal = a.create_calendar("Work", None, None, None).unwrap();
        let start = Utc::now();
        let mut ev = a
            .create_event(
                &cal.id,
                NewEvent {
                    title: "Standup".into(),
                    description: None,
                    location: None,
                    start,
                    end: start + Duration::minutes(15),
                    all_day: false,
                    recurrence: None,
                    color_label: None,
                    color_hex: None,
                    reminders: vec![],
                    sound: None,
                    attendees: vec![],
                    send_invitations: false,
                },
            )
            .await
            .unwrap();
        ev.title = "Daily Standup".into();
        let saved = a.update_event(ev.clone()).await.unwrap();
        assert_eq!(saved.title, "Daily Standup");

        let range = DateRange {
            start: start - Duration::hours(1),
            end: start + Duration::hours(1),
        };
        let evs = a.get_events(&cal.id, range).await.unwrap();
        assert_eq!(evs[0].title, "Daily Standup");
    }

    #[tokio::test]
    async fn delete_event_returns_not_found_when_missing() {
        let a = make_adapter();
        let err = a.delete_event("does-not-exist", false).await.unwrap_err();
        assert!(matches!(err, cal_core::Error::NotFound(_)));
    }

    #[tokio::test]
    async fn add_event_exdate_appends_and_dedups() {
        let a = make_adapter();
        let cal = a.create_calendar("Work", None, None, None).unwrap();
        let start = Utc::now();
        let ev = a
            .create_event(
                &cal.id,
                NewEvent {
                    title: "Weekly".into(),
                    description: None,
                    location: None,
                    start,
                    end: start + Duration::hours(1),
                    all_day: false,
                    recurrence: Some(EventRecurrence {
                        rrule: "FREQ=WEEKLY".into(),
                        exceptions: vec![],
                        tzid: None,
                    }),
                    color_label: None,
                    color_hex: None,
                    reminders: vec![],
                    sound: None,
                    attendees: vec![],
                    send_invitations: false,
                },
            )
            .await
            .unwrap();

        let occ = start + Duration::days(7);
        a.add_event_exdate(&ev.id, occ).unwrap();
        // Calling a second time with the same date must not duplicate.
        a.add_event_exdate(&ev.id, occ).unwrap();

        let range = DateRange {
            start: start - Duration::hours(1),
            end: start + Duration::hours(1),
        };
        let evs = a.get_events(&cal.id, range).await.unwrap();
        let stored = evs.into_iter().find(|e| e.id == ev.id).unwrap();
        let exceptions = stored.recurrence.unwrap().exceptions;
        assert_eq!(exceptions.len(), 1);
        assert_eq!(exceptions[0], occ);
    }

    #[tokio::test]
    async fn add_event_exdate_rejects_non_recurring() {
        let a = make_adapter();
        let cal = a.create_calendar("Work", None, None, None).unwrap();
        let start = Utc::now();
        let ev = a
            .create_event(
                &cal.id,
                NewEvent {
                    title: "Once".into(),
                    description: None,
                    location: None,
                    start,
                    end: start + Duration::hours(1),
                    all_day: false,
                    recurrence: None,
                    color_label: None,
                    color_hex: None,
                    reminders: vec![],
                    sound: None,
                    attendees: vec![],
                    send_invitations: false,
                },
            )
            .await
            .unwrap();
        let err = a.add_event_exdate(&ev.id, start).unwrap_err();
        assert!(matches!(err, cal_core::Error::InvalidInput(_)));
    }

    #[tokio::test]
    async fn authenticate_is_a_noop_for_local() {
        let a = make_adapter();
        let token = a.authenticate(Credentials::default()).await.unwrap();
        assert!(token.access_token.is_empty());
        assert!(a.capabilities().contains(&Capability::Calendar));
        assert!(a.capabilities().contains(&Capability::Tasks));
    }
}
