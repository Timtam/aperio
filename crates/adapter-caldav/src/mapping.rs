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
//!   - DTSTART VALUE=DATE → all_day = true, LOCAL midnight as a UTC instant
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
//! On WRITE, a zoned recurring master emits `DTSTART;TZID=<zone>` together with
//! a derived `VTIMEZONE` (see [`vtimezone`](crate::vtimezone)) so RFC 5545 §3.6.5
//! is satisfied; the reader still takes `RRULE`/`DTSTART` verbatim and resolves
//! the zone itself, so it doesn't parse the `VTIMEZONE` body back. ATTENDEE
//! mapping on read still lives behind a follow-up task.

use cal_core::{AttendeeStatus, Event, EventRecurrence, NewEvent, Reminder, ReminderKind};
use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use icalendar::{Calendar as ICalendar, Component, DatePerhapsTime, EventLike, Property};

use crate::error::{CaldavError, CaldavResult};
use crate::identity::OwnIdentity;

/// The one VALARM property Aperio insists on authoring: it is the event's own
/// title, so it has to follow the title being written. Everything else on an
/// UNCHANGED alarm is the server's to keep — including the TRIGGER, whose exact
/// spelling matters: `reminder_to_alarm` renders a relative trigger through
/// chrono, which prints total seconds (`-PT3600S`), where Apple wrote `-PT1H`.
/// Same instant, different text, and re-spelling an alarm we did not touch is
/// exactly the kind of damage this preservation exists to avoid.
const ALARM_PROPS_WE_AUTHOR: [&str; 1] = ["DESCRIPTION"];

/// Apple's mark on an alarm — or on a whole event — that says "this is the
/// ACCOUNT's default alert, not one somebody chose here".
const APPLE_DEFAULT_ALARM: &str = "X-APPLE-DEFAULT-ALARM";

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
    parse_calendar_data_as(body, calendar_id, href, &OwnIdentity::default())
}

/// [`parse_calendar_data_with_href`] for the account `own`: whether it
/// organizes each event, and which of the event's rows are its own, is read
/// from the server's own text (see `map_event`). An empty `own` knows no
/// address: then an event with an ORGANIZER counts as someone else's.
pub fn parse_calendar_data_as(
    body: &str,
    calendar_id: &str,
    href: Option<&str>,
    own: &OwnIdentity,
) -> CaldavResult<Vec<Event>> {
    let parsed: ICalendar = body
        .parse()
        .map_err(|err: String| CaldavError::Protocol(format!("ical: {err}")))?;
    let mut out = Vec::new();
    for comp in parsed.components {
        if let icalendar::CalendarComponent::Event(ev) = comp {
            match map_event(&ev, calendar_id, href, own) {
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

// Separator in an override instance's id between the recurring series'
// `{href}|{uid}` and the RECURRENCE-ID instant it replaces — e.g.
// `…|uid::rid::2026-06-14T13:00:00Z`. Chosen so it can't appear in a CalDAV
// href/UID; the frontend (`shared/recurrence.ts`, kept in sync) splits a series
// id back out of it and skips the master occurrence the override stands in for.
// One marker for every adapter, kept in the core.
use cal_core::OVERRIDE_ID_MARKER as RECURRENCE_ID_MARKER;

fn map_event(
    ev: &icalendar::Event,
    calendar_id: &str,
    href: Option<&str>,
    own: &OwnIdentity,
) -> CaldavResult<Event> {
    let uid = ev
        .get_uid()
        .ok_or_else(|| CaldavError::Protocol("VEVENT without UID".to_string()))?;
    let summary = ev.get_summary().unwrap_or("").to_string();
    let description = ev.get_description().map(|s| s.to_string());
    let location = ev.get_location().map(|s| s.to_string());

    let (start, end, all_day, start_tzid) = resolve_range(ev)?;

    // RFC 5545 RECURRENCE-ID marks this VEVENT as a single *modified instance*
    // (an override) of a recurring series — not the master. iCloud packs the
    // master and its overrides into ONE resource (same href, same UID); without
    // telling them apart they collide on the `{href}|{uid}` id and the cache
    // upsert drops one — taking the master's RRULE, and thus the whole series,
    // with it. Parse it like DTSTART (TZID / all-day aware) so the instant lines
    // up with the master's expanded occurrence on the frontend.
    let recurrence_id = ev.get_recurrence_id().map(|d| datetime_to_utc(d).0);

    // An override is a single instance, so it must NOT carry a series rule (a
    // server may still send RRULE on it; ignore it). Masters keep rule + EXDATE.
    let recurrence = if recurrence_id.is_some() {
        None
    } else {
        ev.property_value("RRULE").map(|r| EventRecurrence {
            rrule: r.to_string(),
            exceptions: collect_exdates(ev),
            // Carry the master DTSTART's zone so the frontend expands the rule in
            // local wall-clock time (DST-correct). `None` for floating/Z/all-day.
            tzid: start_tzid,
        })
    };

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

    // RFC 7986 COLOR → transport hex. Accepts `#RRGGBB` (what Aperio writes)
    // and known CSS3 color names; anything else is ignored. The host maps
    // this back to a color label on read.
    let color_hex = ev.property_value("COLOR").and_then(parse_color);

    // Encode the server href into the id (`{href}|{uid}`) when we have
    // it, so removed hrefs from a sync-collection delta map onto the
    // cache's native_id; without it, fall back to the bare UID.
    let base_id = match href {
        Some(h) if !h.is_empty() => format!("{h}|{uid}"),
        _ => uid.to_string(),
    };
    // An override shares the master's `{href}|{uid}`; suffix the replaced
    // occurrence instant (`…::rid::2026-06-14T13:00:00Z`) so it gets its own
    // cache row instead of clobbering the master. The frontend strips the marker
    // to recover the series id + the occurrence it stands in for; `native_id()`
    // still splits at the first `|`, so a resource deletion maps to both rows.
    let id = match &recurrence_id {
        Some(rid) => format!(
            "{base_id}{RECURRENCE_ID_MARKER}{}",
            rid.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ),
        None => base_id,
    };

    // ORGANIZER + ATTENDEE;PARTSTAT (RFC 5545). ATTENDEE is multi-valued
    // (one property per invitee, stored under `multi_properties`). The
    // organizer is never an invitee (decision 67a); RFC 5545 has no per-row
    // flag for it, so its ATTENDEE is the one that names the same calendar
    // user as ORGANIZER. A server may name a calendar user by a principal path
    // or a urn rather than a `mailto:`, with the address in an EMAIL parameter
    // (iCloud does, live measurement M1); see `calendar_user_address`.
    // Whether the account organizes the event is read from the ORGANIZER
    // line itself, its value or its EMAIL, against every address of the
    // account (decision 70a): the same rule the write side uses
    // (`scheduling::plan_block`). An empty `own` answers nothing.
    let organizer_prop = ev.properties().get("ORGANIZER");
    let organizer_value = organizer_prop.map(|p| p.value().trim().to_string());
    let organizer = organizer_prop
        .map(calendar_user_address)
        .filter(|s| !s.is_empty());
    let organized_by_me = organizer_prop
        .filter(|_| !own.is_empty())
        .map(|p| own.names(p.value(), p.params().get("EMAIL").map(|e| e.value())));
    let rows = ev
        .multi_properties()
        .get("ATTENDEE")
        .into_iter()
        .flatten()
        .map(|p| cal_core::attendee::ReadAttendee {
            email: calendar_user_address(p),
            name: p.params().get("CN").map(|c| c.value().trim().to_string()),
            status: p
                .params()
                .get("PARTSTAT")
                .map(|c| caldav_partstat(c.value()))
                .unwrap_or_default(),
            // The organizer's own row, and on the account's own meeting any
            // row that names the account (iCloud's ROLE=CHAIR row): the host
            // of a meeting is never its guest (67a).
            is_organizer: organizer_value
                .as_deref()
                .is_some_and(|org| same_calendar_user(org, p.value()))
                || (organized_by_me == Some(true)
                    && own.names(p.value(), p.params().get("EMAIL").map(|e| e.value()))),
        });
    let people = cal_core::attendee::people_from_read(organizer, organized_by_me, rows);

    // RFC 5545 `STATUS:CANCELLED` — the event was cancelled. Aperio keeps it
    // visible (subject to the show-cancelled setting) but never schedules
    // reminders for it. Any other STATUS (TENTATIVE/CONFIRMED) reads as active.
    let cancelled = ev
        .property_value("STATUS")
        .map(|s| s.trim().eq_ignore_ascii_case("CANCELLED"))
        .unwrap_or(false);

    Ok(Event {
        keep_attendees: false,
        clear_attendees: false,
        organized_elsewhere: people.organized_elsewhere,
        send_invitations: false,
        truncate_tail_overrides: false,
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
        color_hex,
        reminders,
        sound: None,
        attendees: people.attendees,
        created_at: created,
        updated_at: updated,
        etag: None,
        organizer: people.organizer,
        attendee_responses: people.attendee_responses,
        cancelled,
    })
}

/// Whether an iCalendar body has an ORGANIZER line at all: only then does a
/// read need the account's own addresses. A property starts a line; a folded
/// continuation starts with a space, so text inside a DESCRIPTION does not
/// count.
pub fn names_an_organizer(body: &str) -> bool {
    body.split('\n').any(|line| {
        line.get(..9)
            .is_some_and(|name| name.eq_ignore_ascii_case("ORGANIZER"))
            && line[9..].starts_with([';', ':'])
    })
}

/// The address a calendar-user property (ORGANIZER, ATTENDEE) names, the way
/// Aperio shows it: a `mailto:` value without its scheme; for any other value
/// (a principal path, a urn) the EMAIL parameter, the server's own statement
/// of the address (RFC 7986 §6.2); and the value itself when there is none.
fn calendar_user_address(prop: &Property) -> String {
    let value = prop.value().trim();
    if let Some(address) = strip_mailto_scheme(value) {
        return address.trim().to_string();
    }
    prop.params()
        .get("EMAIL")
        .map(|e| e.value().trim().to_string())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| value.to_string())
}

/// Whether two calendar-user values name the same user: two `mailto:`s by
/// address, anything else by its exact text apart from one trailing slash.
pub(crate) fn same_calendar_user(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    match (strip_mailto_scheme(a), strip_mailto_scheme(b)) {
        (Some(a), Some(b)) => cal_core::attendee::same_address(a, b),
        (None, None) => !a.is_empty() && a.trim_end_matches('/') == b.trim_end_matches('/'),
        _ => false,
    }
}

/// The address after a `mailto:` scheme in any case, or `None` for another
/// kind of value.
pub(crate) fn strip_mailto_scheme(value: &str) -> Option<&str> {
    value
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
        .then(|| &value[7..])
}

/// Map an RFC 5545 `PARTSTAT` to the normalised RSVP enum. DELEGATED and
/// any unknown value fall through to `NeedsAction`.
fn caldav_partstat(s: &str) -> AttendeeStatus {
    match s.trim().to_ascii_uppercase().as_str() {
        "ACCEPTED" => AttendeeStatus::Accepted,
        "DECLINED" => AttendeeStatus::Declined,
        "TENTATIVE" => AttendeeStatus::Tentative,
        _ => AttendeeStatus::NeedsAction,
    }
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

/// The reminder one raw VALARM block shows, read by the rule the READ uses
/// ([`parse_valarms`]).
///
/// A write that replaces an attendee's alarms has to decide, per alarm on the
/// server, whether the user still wants it. It must decide it the way the
/// user saw it: `ACTION:AUDIO` and `TRIGGER;RELATED=END` both reach the
/// editor as plain reminders, so a stricter rule (such as
/// [`PriorAlarms::claim`]'s) would leave such an alarm in place *and* render
/// a second one beside it — a reminder that multiplies on every save.
///
/// `None` for an alarm the read never showed either (no TRIGGER, a duration
/// Aperio does not model): those stay untouched.
pub(crate) fn raw_alarm_reminder(raw: &str) -> Option<Reminder> {
    let body = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Aperio//read//EN\r\n\
BEGIN:VEVENT\r\nUID:alarm-read\r\nDTSTAMP:20260101T000000Z\r\n{}\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n",
        raw.trim_end_matches(['\r', '\n'])
    );
    let parsed: ICalendar = body.parse().ok()?;
    parsed
        .components
        .iter()
        .find_map(|component| match component {
            icalendar::CalendarComponent::Event(ev) => parse_valarms(ev).into_iter().next(),
            _ => None,
        })
}

/// The alarms already on the server's copy of an event, so a write doesn't
/// destroy what it cannot rebuild.
///
/// A VALARM carries more than the reminder Aperio models. Apple writes the
/// ACCOUNT's default alert into the appointment itself, as a VALARM marked
/// `X-APPLE-DEFAULT-ALARM:TRUE` — that marker is how the Calendar app later
/// tells its own default from an alert somebody set deliberately — and every
/// alarm gets a `UID` besides. Rebuilding the VEVENT from core fields spelled
/// the reminder right and threw the rest away, so an appointment made on an
/// iPhone came back from any Aperio edit with Apple's own default looking
/// hand-made.
///
/// The rule is per alarm and turns on whether the reminder CHANGED. An alarm
/// whose reminder is untouched is written back with the previous copy's
/// properties laid over the ones we generate — same trigger, same identity,
/// same marker. One the user moved or removed keeps nothing: it is not the
/// account's default any more, and claiming otherwise would be a worse lie
/// than losing the mark.
#[derive(Debug, Clone, Default)]
pub struct PriorAlarms {
    alarms: Vec<PriorAlarm>,
    /// The marker as it sat on the VEVENT itself (iCloud writes it in both
    /// places). Only re-emitted for an alarm set that came through untouched.
    event_marker: Option<Property>,
}

#[derive(Debug, Clone)]
struct PriorAlarm {
    /// What this VALARM parses to, so matching runs on the reminder Aperio
    /// understands rather than on raw TRIGGER text. It has to: Apple writes
    /// `-PT1H` and we would write `-PT3600S` for the very same reminder, so
    /// comparing the text would match nothing at all.
    ///
    /// `None` for an alarm this crate cannot put back the way it found it —
    /// see [`faithful_reminder`]. Such an alarm is never claimed, so the mark
    /// falls away with it and the write is exactly what it was before any of
    /// this existed. Recording it at all is the point: the alarm still counts
    /// towards "did the set survive", which is how a VALARM Aperio silently
    /// drops stops being mistaken for one that came through untouched.
    reminder: Option<Reminder>,
    properties: Vec<Property>,
    multi_properties: Vec<Property>,
    /// Apple's mark, held apart from the rest because it may only come back
    /// for a set nobody touched — see [`PriorAlarms`].
    default_marker: Option<Property>,
    /// Set once this alarm has been claimed, so two identical reminders can't
    /// both inherit the same alarm's identity.
    claimed: bool,
}

impl PriorAlarms {
    /// Read the alarms off the MASTER VEVENT of a resource body.
    ///
    /// Overrides (the VEVENTs carrying RECURRENCE-ID) are skipped on purpose:
    /// an override's alarms belong to that one occurrence, and a write of that
    /// occurrence reads them with [`Self::read_occurrence`]. A body we cannot
    /// parse yields nothing, which costs only the preservation — never the
    /// write.
    pub fn read(body: &str) -> Self {
        Self::read_where(body, |ev| ev.property_value("RECURRENCE-ID").is_none())
    }

    /// Read the alarms off the override VEVENT that stands in for the
    /// occurrence at `slot`, for a write of that one occurrence.
    pub fn read_occurrence(body: &str, slot: DateTime<Utc>) -> Self {
        Self::read_where(body, |ev| {
            ev.get_recurrence_id().map(|d| datetime_to_utc(d).0) == Some(slot)
        })
    }

    fn read_where(body: &str, wanted: impl Fn(&icalendar::Event) -> bool) -> Self {
        let Ok(parsed) = body.parse::<ICalendar>() else {
            return Self::default();
        };
        for comp in &parsed.components {
            let icalendar::CalendarComponent::Event(ev) = comp else {
                continue;
            };
            if !wanted(ev) {
                continue;
            }
            return Self {
                alarms: prior_alarms_of(ev),
                event_marker: ev.properties().get(APPLE_DEFAULT_ALARM).cloned(),
            };
        }
        Self::default()
    }

    /// Claim the previous alarm this reminder came from, if it is still there
    /// unchanged, and return its index. Claiming is what makes two identical
    /// reminders take two different alarms rather than the same one twice.
    fn claim(&mut self, reminder: &Reminder) -> Option<usize> {
        let idx = self.alarms.iter().position(|a| {
            !a.claimed && a.reminder.as_ref().is_some_and(|r| r.kind == reminder.kind)
        })?;
        self.alarms[idx].claimed = true;
        Some(idx)
    }

    /// True once every alarm the server had has been claimed by a reminder we
    /// are writing — i.e. the alarm set came through this edit untouched. Only
    /// then may the VEVENT-level marker stay: it speaks for all of them.
    fn all_claimed(&self) -> bool {
        self.alarms.iter().all(|a| a.claimed)
    }
}

/// Split every VALARM on `ev` into the reminder it means and the properties
/// Aperio does not write itself.
fn prior_alarms_of(ev: &icalendar::Event) -> Vec<PriorAlarm> {
    let mut out = Vec::new();
    for child in ev.components() {
        if child.component_kind() != "VALARM" {
            continue;
        }
        let action = child
            .property_value("ACTION")
            .map(|s| s.to_ascii_uppercase())
            .unwrap_or_default();
        let properties: Vec<Property> = child
            .properties()
            .iter()
            .filter(|(key, _)| {
                !ALARM_PROPS_WE_AUTHOR.contains(&key.as_str())
                    && key.as_str() != APPLE_DEFAULT_ALARM
            })
            .map(|(_, prop)| prop.clone())
            .collect();
        let multi_properties: Vec<Property> = child
            .multi_properties()
            .values()
            .flatten()
            .cloned()
            .collect();
        out.push(PriorAlarm {
            reminder: faithful_reminder(child, &action, &properties, &multi_properties),
            default_marker: child.properties().get(APPLE_DEFAULT_ALARM).cloned(),
            properties,
            multi_properties,
            claimed: false,
        });
    }
    out
}

/// The reminder this VALARM stands for — but ONLY when Aperio could put the
/// alarm back the way it found it. `None` means "hands off": the alarm is left
/// unclaimed, so nothing is inherited from it and the whole set counts as
/// changed.
///
/// Preserving half of something is worse than not preserving it. Each check
/// below is a way the write would otherwise hand the server an alarm that says
/// something the server did not say:
///
/// * A TRIGGER Aperio cannot read is an alarm Aperio also cannot SHOW — the
///   reminder never reaches the editor, and the rebuild drops the VALARM
///   entirely. Apple's "1 week before" default (`TRIGGER:-P1W`) is exactly
///   this. Counting it as unchanged would stamp "still the account's default"
///   on a set the write is about to shrink, and an account that believes that
///   may put its missing alarm back — resurrecting a reminder nobody has.
/// * `RELATED=END` means "before the END", which Aperio reads as "before the
///   start" and would keep writing as such. Inheriting the trigger text
///   freezes a meaning the editor never showed; writing our own silently moves
///   an alarm nobody touched. Neither is honest, so the alarm is left alone.
/// * An ACTION we would not have written (AUDIO, where we build DISPLAY) is
///   not just a label: RFC 5545 §3.6.6 gives each action its own set of
///   allowed properties, and an AUDIO alarm carrying the DESCRIPTION we author
///   is a body a strict server answers with 403.
/// * A parameter whose value needs quoting does not survive this crate's
///   writer — it re-quotes only for `:` and `;`, so `X-ADDRESS="Hauptstr. 1,
///   Berlin"` comes back unquoted and reads as a LIST of values. Until now
///   these properties were dropped on every write, so nothing round-tripped
///   through that gap; sending them back mangled would be a new kind of
///   damage, not a repair.
fn faithful_reminder(
    alarm: &impl Component,
    action: &str,
    properties: &[Property],
    multi_properties: &[Property],
) -> Option<Reminder> {
    let trigger = alarm.properties().get("TRIGGER")?;
    if trigger
        .params()
        .get("RELATED")
        .is_some_and(|p| !p.value().eq_ignore_ascii_case("START"))
    {
        return None;
    }
    let kind = resolve_trigger(trigger, action)?;
    if action != action_we_would_write(&kind) {
        return None;
    }
    let reproducible = |prop: &Property| {
        prop.params()
            .values()
            .all(|param| !param.value().contains([',', '"']))
    };
    if !properties.iter().chain(multi_properties).all(reproducible) {
        return None;
    }
    Some(Reminder { kind, sound: None })
}

/// The ACTION [`reminder_to_alarm`] emits for this kind. Kept beside it so the
/// two cannot drift apart.
fn action_we_would_write(kind: &ReminderKind) -> &'static str {
    match kind {
        ReminderKind::Email { .. } => "EMAIL",
        _ => "DISPLAY",
    }
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

/// The UTC instant an override id (`…::rid::<rfc3339>`) replaces, or `None` for a
/// master / plain-event id. Reads the instant the mapper already resolved (via
/// the provider's RECURRENCE-ID, zone-corrected), so it's exact even for a
/// `TZID`-qualified or all-day RECURRENCE-ID.
pub(crate) fn override_recurrence_id(event_id: &str) -> Option<DateTime<Utc>> {
    cal_core::split_override_id(event_id)
        .ok()
        .flatten()
        .map(|(_, slot)| slot)
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
fn resolve_range(
    ev: &icalendar::Event,
) -> CaldavResult<(DateTime<Utc>, DateTime<Utc>, bool, Option<String>)> {
    let start = ev
        .get_start()
        .ok_or_else(|| CaldavError::Protocol("VEVENT without DTSTART".into()))?;
    let (start_utc, all_day, start_tzid) = datetime_to_utc(start);
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
    Ok((start_utc, end_utc, all_day, start_tzid))
}

/// Resolve a DATE/DATE-TIME to a UTC instant, the all-day flag, and — only for
/// the zoned (`WithTimezone`) shape — the IANA tzid, so a recurring master can
/// be expanded in its own zone (DST-correct) rather than flattened to UTC.
fn datetime_to_utc(value: DatePerhapsTime) -> (DateTime<Utc>, bool, Option<String>) {
    match value {
        DatePerhapsTime::Date(d) => (naive_date_to_utc(d), true, None),
        DatePerhapsTime::DateTime(dt) => {
            // icalendar's CalendarDateTime is an enum with three
            // shapes (UTC / local-with-tz / floating). We normalise
            // each to UTC; floating times are read as UTC because
            // there's no better answer without per-event tz config.
            match dt {
                icalendar::CalendarDateTime::Utc(u) => (u, false, None),
                icalendar::CalendarDateTime::WithTimezone { date_time, tzid } => {
                    let resolved = resolve_with_tzid(date_time, &tzid)
                        .unwrap_or_else(|| Utc.from_utc_datetime(&date_time));
                    (resolved, false, Some(tzid))
                }
                icalendar::CalendarDateTime::Floating(naive) => {
                    (Utc.from_utc_datetime(&naive), false, None)
                }
            }
        }
    }
}

/// Anchor a DATE value (all-day boundary) at LOCAL midnight, expressed
/// as a UTC instant — the app-internal all-day convention. Anchoring at
/// UTC midnight instead would shift the rendered day for any user west
/// of UTC (June 10 00:00 UTC is June 9 in the evening in the Americas),
/// and would break the write-side round-trip (which reads the local day
/// back off the instant). DST edge: a zone can skip midnight on a
/// transition day; fall forward to the first valid local time then.
fn naive_date_to_utc(d: NaiveDate) -> DateTime<Utc> {
    let midnight = NaiveDateTime::new(d, NaiveTime::from_hms_opt(0, 0, 0).unwrap());
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|l| l.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&midnight))
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
        // A zoned series' EXDATE carries a TZID param (`EXDATE;TZID=…:<naive>`),
        // exactly like its DTSTART. Resolve the naive values in that zone — else
        // they parse to nothing and the deleted occurrence reappears, because its
        // instant has to match the now zone-correct expansion.
        let tzid = prop.params().get("TZID").map(|p| p.value());
        for token in prop.value().split(',') {
            let trimmed = token.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, "%Y%m%dT%H%M%SZ") {
                out.push(Utc.from_utc_datetime(&dt));
            } else if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y%m%dT%H%M%S") {
                // Naive date-time: resolve in the EXDATE's TZID when present,
                // else read as UTC (the floating-DTSTART convention).
                out.push(
                    tzid.and_then(|tz| resolve_with_tzid(naive, tz))
                        .unwrap_or_else(|| Utc.from_utc_datetime(&naive)),
                );
            } else if let Ok(d) = NaiveDate::parse_from_str(trimmed, "%Y%m%d") {
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
///
/// Writes no ORGANIZER or ATTENDEE: the lines that make an event a meeting
/// are the write path's to add (see [`crate::scheduling`]).
pub fn new_event_to_ical(uid: &str, event: &NewEvent) -> String {
    let mut ical_ev = icalendar::Event::new();
    // A brand-new appointment has no previous copy, so there is nothing to
    // preserve — and Apple's default-alert marker is Apple's to apply, never
    // ours to forge.
    apply_common(&mut ical_ev, uid, event, &mut PriorAlarms::default());
    let mut cal = ICalendar::new();
    cal.push(ical_ev.done());
    with_vtimezone(cal.to_string(), event)
}

/// Inject the `VTIMEZONE` that `apply_common` implies. When the event is a
/// zoned recurring master we emit `DTSTART;TZID=<zone>` — and RFC 5545 §3.6.5
/// requires the referenced `TZID` to be DEFINED by a `VTIMEZONE` in the same
/// `VCALENDAR`. Without it iCloud can't resolve the zone to compute a
/// `COUNT`-bounded rule's Nth occurrence and silently drops the whole
/// recurrence (a "repeat 2×" event lands only on day one). The `icalendar`
/// crate has no timezone component, so we splice the generated block in front
/// of the first `VEVENT` (timezones must precede the components referencing
/// them). No `TZID` emitted → nothing to inject, so the string is returned
/// unchanged for all-day and non-recurring (bare-UTC) events.
fn with_vtimezone(mut ical: String, event: &NewEvent) -> String {
    // The core's rule: an all-day series writes no zone (decision 46a).
    let Some(tzid) = cal_core::written_series_zone(
        event.recurrence.as_ref().and_then(|r| r.tzid.as_deref()),
        event.all_day,
    ) else {
        return ical;
    };
    // Seed the DST-rule probe with the event's start year. A yearly rule is
    // stable across adjacent years for every modern zone, so the exact year
    // only has to be in the right era. `vtimezone_for` parses `tzid` itself and
    // yields `None` for UTC / an unresolvable zone — the same zones for which
    // `apply_common` falls back to a bare-UTC `DTSTART` (no `TZID` to define).
    let ref_year = event.start.year();
    let Some(vtz) = crate::vtimezone::vtimezone_for(tzid, ref_year) else {
        return ical;
    };
    if let Some(pos) = ical.find("BEGIN:VEVENT") {
        ical.insert_str(pos, &vtz);
    }
    ical
}

/// Render an existing event back to iCal for the update PUT.
///
/// Rebuilds the VEVENT from core fields, so it knows nothing about the copy it
/// replaces. Callers that hold that copy should use
/// [`event_to_ical_preserving`] instead: it keeps the parts of an unchanged
/// alarm that Aperio does not model — Apple's default-alert marker above all.
pub fn event_to_ical(event: &Event) -> String {
    event_to_ical_preserving(event, PriorAlarms::default())
}

/// [`event_to_ical`], with the alarms of the server's current copy in hand.
///
/// See [`PriorAlarms`] for what survives and why. `prior` is consumed because
/// each of its alarms may be claimed only once.
///
/// Like every renderer here it writes no ORGANIZER or ATTENDEE: a meeting's
/// lines are carried from the server's copy by the write path (see
/// [`crate::scheduling`]).
pub fn event_to_ical_preserving(event: &Event, mut prior: PriorAlarms) -> String {
    let new = NewEvent {
        organized_elsewhere: false,
        organizer: None,
        title: event.title.clone(),
        description: event.description.clone(),
        location: event.location.clone(),
        start: event.start,
        end: event.end,
        all_day: event.all_day,
        recurrence: event.recurrence.clone(),
        color_label: event.color_label.clone(),
        color_hex: event.color_hex.clone(),
        reminders: event.reminders.clone(),
        sound: event.sound.clone(),
        attendees: event.attendees.clone(),
        send_invitations: event.send_invitations,
    };
    // The iCalendar `UID` must be the bare provider UID — NOT our composite
    // `{href}|{uid}` row id. Writing the composite changes the event's UID
    // on the server: for an event whose resource name differs from its UID
    // (e.g. anything created on an iPhone), iCloud then treats the PUT as a
    // *different* event and spawns a duplicate. `decode_event_id` yields the
    // bare uid for both composite and legacy bare ids.
    let (_, uid) = decode_event_id(&event.id);
    let mut ical_ev = icalendar::Event::new();
    apply_common(&mut ical_ev, uid, &new, &mut prior);
    let mut cal = ICalendar::new();
    cal.push(ical_ev.done());
    with_vtimezone(cal.to_string(), &new)
}

/// Render `event`, one changed occurrence, as the VEVENT that stands in for it
/// in its series' resource.
///
/// `series_id` is the series' own id (`{href}|{uid}`): an override shares its
/// master's UID. `recurrence_id` is the RECURRENCE-ID property exactly as the
/// server's copy spells it, folded lines and line endings included. It has to
/// match the master's DTSTART in kind and zone, and copying it is the one way
/// to be sure it still does. The rest is rebuilt from core fields, as for a
/// master, and keeps the alarms `prior` recognises.
///
/// Returns the VEVENT block alone, without the VCALENDAR around it.
pub fn override_to_vevent(
    event: &Event,
    series_id: &str,
    recurrence_id: &str,
    prior: PriorAlarms,
) -> String {
    let mut occurrence = event.clone();
    occurrence.id = series_id.to_string();
    // One occurrence carries no rule of its own.
    occurrence.recurrence = None;
    let vcal = event_to_ical_preserving(&occurrence, prior);
    let begin = "BEGIN:VEVENT\r\n";
    let end = "END:VEVENT\r\n";
    // The LAST `END:VEVENT`: a text value can end in those very characters, and
    // only `END:VCALENDAR` follows the one VEVENT's own end line. Its `BEGIN`
    // comes before any text value, so the first one is the right one.
    let (Some(from), Some(to)) = (vcal.find(begin), vcal.rfind(end)) else {
        // `icalendar` always writes both lines; this is unreachable short of a
        // change in that crate, and the caller's PUT would then fail loudly.
        return vcal;
    };
    let mut block = String::with_capacity(to + end.len() - from + recurrence_id.len() + 2);
    block.push_str(begin);
    block.push_str(recurrence_id);
    if !recurrence_id.ends_with('\n') {
        block.push_str("\r\n");
    }
    block.push_str(&vcal[from + begin.len()..to + end.len()]);
    block
}

fn apply_common(
    ical_ev: &mut icalendar::Event,
    uid: &str,
    event: &NewEvent,
    prior: &mut PriorAlarms,
) {
    ical_ev.uid(uid);
    ical_ev.summary(&event.title);
    if let Some(desc) = &event.description {
        ical_ev.description(desc);
    }
    if let Some(loc) = &event.location {
        ical_ev.location(loc);
    }
    if event.all_day {
        // DATE values are CALENDAR days. The internal instants are local
        // midnights expressed in UTC, so the day must be read off the
        // LOCAL clock — `date_naive()` on the UTC instant would emit the
        // UTC day, which for a user east of UTC is the day BEFORE
        // (June 10 00:00 +02:00 is June 9 22:00 UTC → "June 9").
        ical_ev.starts(event.start.with_timezone(&Local).date_naive());
        // For all-day events DTEND is exclusive — RFC 5545 §3.6.1.
        // Aperio stores end as the local start-of-next-day already, so
        // the local day of `event.end` is exactly the right exclusive
        // boundary.
        ical_ev.ends(event.end.with_timezone(&Local).date_naive());
    } else if let Some(tzid) = cal_core::written_series_zone(
        event.recurrence.as_ref().and_then(|r| r.tzid.as_deref()),
        event.all_day,
    )
    .filter(|t| !t.eq_ignore_ascii_case("UTC"))
    {
        // A zoned recurring master must keep its TZID on write-back, else the
        // next read flattens DTSTART to a bare UTC instant, drops the zone, and
        // the rule re-expands in UTC — the DST drift returns. Emit
        // DTSTART/DTEND;TZID=<zone>:<local wall-clock>. UNTIL + EXDATE stay UTC
        // `Z` (RFC 5545 requires UTC there), which the read side already parses.
        // A literal `TZID=UTC` (a server can round-trip one) is excluded here so
        // it takes the bare-UTC `else` branch: `vtimezone_for` emits no VTIMEZONE
        // for UTC, so writing `DTSTART;TZID=UTC` would be an undefined reference
        // (the very RFC 5545 §3.6.5 violation this path exists to avoid).
        match (
            zoned_datetime(event.start, tzid),
            zoned_datetime(event.end, tzid),
        ) {
            (Some(start), Some(end)) => {
                ical_ev.starts(start);
                ical_ev.ends(end);
            }
            // A zone we can't resolve — fall back to UTC rather than a broken value.
            _ => {
                ical_ev.starts(event.start);
                ical_ev.ends(event.end);
            }
        }
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
    // Built first and pushed second: whether Apple's mark may come back is a
    // fact about the WHOLE set, and that is not known until every reminder has
    // been matched against the server's copy.
    let mut built = Vec::with_capacity(event.reminders.len());
    for reminder in &event.reminders {
        let Some(mut alarm) = reminder_to_alarm(reminder, &event.title) else {
            continue;
        };
        // An alarm the user left alone keeps what the server's copy said about
        // it — its own UID, its X-properties, and the exact TRIGGER text. Only
        // DESCRIPTION is ours, because it is the title we are writing.
        let claimed = prior.claim(reminder);
        if let Some(idx) = claimed {
            let kept = &prior.alarms[idx];
            for prop in &kept.properties {
                alarm.append_property(prop.clone());
            }
            for prop in &kept.multi_properties {
                alarm.append_multi_property(prop.clone());
            }
        }
        built.push((alarm, claimed));
    }
    // Apple's mark says "this is the ACCOUNT's default alert". It survives only
    // an edit that left the alarms alone: every alarm the server had claimed,
    // and none added beside them. The moment the set changes it is the user's
    // choice, not the account's default — and handing a changed set back still
    // marked invites the account to "repair" it into what its default says,
    // which would resurrect a reminder the user had just deleted.
    let set_untouched =
        !built.is_empty() && built.iter().all(|(_, c)| c.is_some()) && prior.all_claimed();
    for (mut alarm, claimed) in built {
        if set_untouched {
            if let Some(marker) = claimed.and_then(|i| prior.alarms[i].default_marker.clone()) {
                alarm.append_property(marker);
            }
        }
        ical_ev.alarm(alarm);
    }
    // The mark sits on the VEVENT as well, where it speaks for all of them.
    if set_untouched {
        if let Some(marker) = prior.event_marker.clone() {
            ical_ev.append_property(marker);
        }
    }
    // No ORGANIZER or ATTENDEE here. On an RFC 6638 server they are the
    // invitation itself, so the write path carries them from the server's
    // copy instead of rebuilding them (see `crate::scheduling`).
    // RFC 7986 COLOR. `color_hex` is only ever `Some` when the provider is
    // meant to store the color natively: the host resolves the event's color
    // label to a hex for color-capable calendars and leaves it `None` for
    // non-capable ones, and the adapter clears it for iCloud (which would
    // email attendees on a COLOR-bearing PUT). So emitting unconditionally
    // here is safe.
    if let Some(hex) = &event.color_hex {
        ical_ev.add_property("COLOR", hex);
    }
    // DTSTAMP is mandatory per RFC 5545 — icalendar adds it
    // automatically on serialise.
}

/// Translate a `cal_core::Reminder` into a VALARM component. Returns
/// `None` for kinds that have no iCal equivalent (currently only
/// `AppStart` — that's an Aperio-local concept the server has no
/// place for, and forging a fake VALARM would mislead the bridge
/// devices).
pub(crate) fn reminder_to_alarm(
    reminder: &Reminder,
    fallback_summary: &str,
) -> Option<icalendar::Alarm> {
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

pub(crate) fn format_utc_compact(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%dT%H%M%SZ").to_string()
}

/// A `DTSTART`/`DTEND` value carrying its IANA zone (`;TZID=<zone>:<local
/// wall-clock>`), so a zoned recurring master round-trips with its zone intact.
/// Returns `None` for a zone `chrono-tz` can't resolve, so the caller can fall
/// back to a plain UTC instant rather than emit a broken property.
fn zoned_datetime(instant: DateTime<Utc>, tzid: &str) -> Option<DatePerhapsTime> {
    let tz: chrono_tz::Tz = tzid.parse().ok()?;
    Some(DatePerhapsTime::DateTime(
        icalendar::CalendarDateTime::WithTimezone {
            date_time: instant.with_timezone(&tz).naive_local(),
            tzid: tzid.to_string(),
        },
    ))
}

/// Parse an RFC 7986 `COLOR` property value into a normalised `#rrggbb`
/// transport hex. Accepts `#RGB` / `#RRGGBB` hex (case-insensitive, `#RGB`
/// expanded) — what Aperio itself writes — and a known CSS3 color keyword
/// (the format RFC 7986 actually prescribes). Anything else returns `None`,
/// so an unrecognised value is dropped rather than guessed at.
fn parse_color(raw: &str) -> Option<String> {
    let s = raw.trim();
    if let Some(body) = s.strip_prefix('#') {
        return normalise_hex(body);
    }
    css3_name_to_hex(&s.to_ascii_lowercase()).map(|h| h.to_string())
}

/// Validate + normalise the body of a hex color (no leading `#`) to lowercase
/// `#rrggbb`. `RGB` shorthand expands to `RRGGBB`.
fn normalise_hex(body: &str) -> Option<String> {
    let body = body.trim();
    let expanded = match body.len() {
        3 => body.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => body.to_string(),
        _ => return None,
    };
    if !expanded.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{}", expanded.to_ascii_lowercase()))
}

/// Map a CSS3 color keyword (already lowercased) to its `#rrggbb` hex. Covers
/// the common keywords; unknown names return `None`. RFC 7986 says `COLOR`
/// carries a CSS3 name, but Aperio always writes hex, so this only matters for
/// a color a *foreign* client wrote.
fn css3_name_to_hex(name: &str) -> Option<&'static str> {
    Some(match name {
        "black" => "#000000",
        "silver" => "#c0c0c0",
        "gray" | "grey" => "#808080",
        "white" => "#ffffff",
        "maroon" => "#800000",
        "red" => "#ff0000",
        "purple" => "#800080",
        "fuchsia" | "magenta" => "#ff00ff",
        "green" => "#008000",
        "lime" => "#00ff00",
        "olive" => "#808000",
        "yellow" => "#ffff00",
        "navy" => "#000080",
        "blue" => "#0000ff",
        "teal" => "#008080",
        "aqua" | "cyan" => "#00ffff",
        "orange" => "#ffa500",
        "tomato" => "#ff6347",
        "coral" => "#ff7f50",
        "gold" => "#ffd700",
        "salmon" => "#fa8072",
        "crimson" => "#dc143c",
        "pink" => "#ffc0cb",
        "hotpink" => "#ff69b4",
        "indigo" => "#4b0082",
        "violet" => "#ee82ee",
        "skyblue" => "#87ceeb",
        "cornflowerblue" => "#6495ed",
        "royalblue" => "#4169e1",
        "steelblue" => "#4682b4",
        "turquoise" => "#40e0d0",
        "seagreen" => "#2e8b57",
        "forestgreen" => "#228b22",
        "limegreen" => "#32cd32",
        "olivedrab" => "#6b8e23",
        "chocolate" => "#d2691e",
        "brown" => "#a52a2a",
        "tan" => "#d2b48c",
        "khaki" => "#f0e68c",
        "lavender" => "#e6e6fa",
        "plum" => "#dda0dd",
        "orchid" => "#da70d6",
        "slategray" | "slategrey" => "#708090",
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn maps_organizer_and_attendee_partstats() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:mtg-1@aperio\r
SUMMARY:Planning\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
ORGANIZER;CN=The Boss:mailto:boss@example.com\r
ATTENDEE;CN=The Boss;PARTSTAT=ACCEPTED:mailto:boss@example.com\r
ATTENDEE;CN=Me;PARTSTAT=NEEDS-ACTION:mailto:me@example.com\r
ATTENDEE;PARTSTAT=DECLINED:mailto:skeptic@example.com\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        let ev = &events[0];
        assert_eq!(ev.organizer.as_deref(), Some("boss@example.com"));
        // Flat editable list: "Name <email>" when CN present, else bare; never
        // the organizer, whose ATTENDEE is the one with ORGANIZER's address
        // (decision 67a).
        assert_eq!(ev.attendees, ["Me <me@example.com>", "skeptic@example.com"]);
        assert_eq!(ev.attendee_responses.len(), 2);
        assert_eq!(ev.attendee_responses[0].status, AttendeeStatus::NeedsAction);
        assert_eq!(ev.attendee_responses[1].status, AttendeeStatus::Declined);
        assert_eq!(ev.attendee_responses[0].name.as_deref(), Some("Me"));
        // Not yet known to be the account's: that takes its own address.
        assert!(ev.organized_elsewhere);
    }

    /// The organizer's ATTENDEE matches ORGANIZER whatever its case and
    /// `mailto:` spelling.
    #[test]
    fn the_organizers_attendee_goes_whatever_its_spelling() {
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:mtg-2@aperio\r\n\
DTSTART:20260520T080000Z\r\nDTEND:20260520T090000Z\r\n\
ORGANIZER:MAILTO:Boss@Example.com\r\n\
ATTENDEE;PARTSTAT=ACCEPTED:mailto:boss@example.com\r\n\
ATTENDEE;PARTSTAT=ACCEPTED:mailto:bob@example.com\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        assert_eq!(events[0].attendees, ["bob@example.com"]);
    }

    /// Whether the account organizes an event takes its own addresses (RFC
    /// 6638 calendar-user-address-set): it organizes the event whose ORGANIZER
    /// is any of them, not only the first.
    #[test]
    fn the_account_organizes_what_its_address_organizes() {
        let body = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:mtg-3@aperio\r\n\
DTSTART:20260520T080000Z\r\nDTEND:20260520T090000Z\r\n\
ORGANIZER:mailto:me@example.com\r\nATTENDEE:mailto:bob@example.com\r\n\
END:VEVENT\r\nBEGIN:VEVENT\r\nUID:own@aperio\r\n\
DTSTART:20260520T080000Z\r\nDTEND:20260520T090000Z\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
        let own = |list: &[&str]| {
            OwnIdentity::from_hrefs(
                &list.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                &url::Url::parse("https://dav.example.com/principals/me/").unwrap(),
            )
        };
        let read =
            |identity: &OwnIdentity| parse_calendar_data_as(body, "cal-1", None, identity).unwrap();
        let events = read(&own(&["mailto:ME@example.com"]));
        assert!(
            !events[0].organized_elsewhere,
            "its ORGANIZER is the account"
        );
        assert!(
            !events[1].organized_elsewhere,
            "no organizer: the account's own"
        );
        let events = read(&own(&["mailto:me@alias.example", "mailto:me@example.com"]));
        assert!(
            !events[0].organized_elsewhere,
            "its ORGANIZER is the account's second address"
        );
        let events = read(&own(&["mailto:someone@example.com"]));
        assert!(events[0].organized_elsewhere);
        let events = read(&OwnIdentity::default());
        assert!(
            events[0].organized_elsewhere,
            "none reported: not confirmed"
        );
        assert!(!events[1].organized_elsewhere);
    }

    /// A meeting the account organizes, the way iCloud stores it (live
    /// measurement M1, anonymised): ORGANIZER and the account's own CHAIR row
    /// name a principal path, with the address in EMAIL. Aperio shows the
    /// organizer by that address, drops the CHAIR row from the invitees, and
    /// recognises the account by its principal path.
    pub(crate) const ICLOUD_MEETING: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Apple Inc.//iPhone OS 26.0//EN\r\n\
BEGIN:VEVENT\r\n\
ATTENDEE;CN=Toni Barth;CUTYPE=INDIVIDUAL;PARTSTAT=ACCEPTED;EMAIL=toni@example\r\n \
.org;ROLE=CHAIR:/aB1/principal/\r\n\
ATTENDEE;CN=Bob Guest;CUTYPE=INDIVIDUAL;EMAIL=bob@example.net;SCHEDULE-STATUS=\r\n \
1.1:mailto:bob@example.net\r\n\
DTEND;TZID=Europe/Berlin:20261109T163000\r\n\
DTSTAMP:20260919T184800Z\r\n\
DTSTART;TZID=Europe/Berlin:20261109T160000\r\n\
ORGANIZER;CN=Toni Barth;EMAIL=toni@example.org:/aB1/principal/\r\n\
SEQUENCE:0\r\n\
SUMMARY:Aperio R6 eigene\r\n\
UID:EBFB90D1-F3C9-4E8C-B18E-211595E15C22\r\n\
X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Reminder\r\n\
TRIGGER:-PT15M\r\n\
UID:5D446DBE-3179-48F1-AC45-96C95B46851B\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    /// An invitation as iCloud stores it on the attendee's side: someone
    /// else's ORGANIZER, the account's own row spelled as a principal path
    /// with its EMAIL, a zone, Apple's own properties, and alarms of every
    /// kind the read shows — a display alarm carrying Apple's default mark,
    /// an audio alarm, an e-mail alarm with an ATTENDEE line of its own, one
    /// triggered from the end, and one whose trigger Aperio does not model.
    /// A weekly series with one changed occurrence.
    pub(crate) const ICLOUD_INVITATION: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Apple Inc.//iPhone OS 26.0//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Europe/Berlin\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19701025T030000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
ATTENDEE;CN=Boss;CUTYPE=INDIVIDUAL;EMAIL=boss@example.net;PARTSTAT=ACCEPTED;RO\r\n \
LE=CHAIR:mailto:boss@example.net\r\n\
ATTENDEE;CN=Toni Barth;CUTYPE=INDIVIDUAL;EMAIL=toni@example.org;PARTSTAT=NEEDS\r\n \
-ACTION;ROLE=REQ-PARTICIPANT:/aB1/principal/\r\n\
CREATED:20260901T090000Z\r\n\
DTEND;TZID=Europe/Berlin:20261109T163000\r\n\
DTSTAMP:20260919T184800Z\r\n\
DTSTART;TZID=Europe/Berlin:20261109T160000\r\n\
LAST-MODIFIED:20260918T101500Z\r\n\
ORGANIZER;CN=Boss;EMAIL=boss@example.net:mailto:boss@example.net\r\n\
RRULE:FREQ=WEEKLY;COUNT=4\r\n\
SEQUENCE:2\r\n\
STATUS:CONFIRMED\r\n\
SUMMARY:Aperio R6 fremde\r\n\
TRANSP:OPAQUE\r\n\
UID:9C1F6A4E-0B77-4E1E-9F6E-51D2A0C9B7A1\r\n\
X-APPLE-DEFAULT-ALARM:TRUE\r\n\
X-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Aperio R6 fremde\r\n\
TRIGGER:-PT15M\r\n\
UID:11111111-1111-4111-8111-111111111111\r\n\
X-APPLE-DEFAULT-ALARM:TRUE\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:AUDIO\r\n\
ATTACH;VALUE=URI:Basso\r\n\
TRIGGER:-PT30M\r\n\
UID:22222222-2222-4222-8222-222222222222\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:EMAIL\r\n\
ATTENDEE:mailto:toni@example.org\r\n\
DESCRIPTION:By mail\r\n\
SUMMARY:By mail\r\n\
TRIGGER:-P1D\r\n\
UID:33333333-3333-4333-8333-333333333333\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:After the end\r\n\
TRIGGER;RELATED=END:-PT5M\r\n\
UID:44444444-4444-4444-8444-444444444444\r\n\
END:VALARM\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:A week before\r\n\
TRIGGER:-P1W\r\n\
UID:55555555-5555-4555-8555-555555555555\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
ATTENDEE;CN=Toni Barth;EMAIL=toni@example.org;PARTSTAT=NEEDS-ACTION:/aB1/princ\r\n \
ipal/\r\n\
DTEND;TZID=Europe/Berlin:20261116T173000\r\n\
DTSTAMP:20260919T184800Z\r\n\
DTSTART;TZID=Europe/Berlin:20261116T170000\r\n\
ORGANIZER;CN=Boss;EMAIL=boss@example.net:mailto:boss@example.net\r\n\
RECURRENCE-ID;TZID=Europe/Berlin:20261116T160000\r\n\
SEQUENCE:3\r\n\
SUMMARY:Aperio R6 fremde, verschoben\r\n\
UID:9C1F6A4E-0B77-4E1E-9F6E-51D2A0C9B7A1\r\n\
BEGIN:VALARM\r\n\
ACTION:DISPLAY\r\n\
DESCRIPTION:Aperio R6 fremde\r\n\
TRIGGER:-PT15M\r\n\
UID:66666666-6666-4666-8666-666666666666\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    /// The same invitation as an all-day series, the way a provider writes
    /// one: `DTSTART;VALUE=DATE`.
    pub(crate) const ICLOUD_ALL_DAY_INVITATION: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
ATTENDEE;CN=Toni Barth;EMAIL=toni@example.org;PARTSTAT=ACCEPTED:/aB1/principal\r\n \
/\r\n\
DTEND;VALUE=DATE:20261110\r\n\
DTSTAMP:20260919T184800Z\r\n\
DTSTART;VALUE=DATE:20261109\r\n\
ORGANIZER;CN=Boss;EMAIL=boss@example.net:mailto:boss@example.net\r\n\
RRULE:FREQ=WEEKLY;COUNT=4\r\n\
SUMMARY:Ganztags fremd\r\n\
UID:9C1F6A4E-0B77-4E1E-9F6E-51D2A0C9B7A1\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    /// The account's addresses as iCloud lists them (M1, anonymised).
    pub(crate) fn icloud_identity() -> OwnIdentity {
        OwnIdentity::from_hrefs(
            &[
                "/1974/principal".into(),
                "/aB1/principal/".into(),
                "urn:uuid:1974".into(),
                "mailto:toni@example.org".into(),
            ],
            &url::Url::parse("https://p42-caldav.icloud.com/1974/principal/").unwrap(),
        )
    }

    #[test]
    fn a_principal_spelled_organizer_is_the_accounts() {
        let events =
            parse_calendar_data_as(ICLOUD_MEETING, "cal-1", None, &icloud_identity()).unwrap();
        assert_eq!(events[0].organizer.as_deref(), Some("toni@example.org"));
        assert_eq!(events[0].attendees, ["Bob Guest <bob@example.net>"]);
        assert_eq!(events[0].attendee_responses.len(), 1);
        assert!(!events[0].organized_elsewhere);
        // Another account sees the same copy as someone else's meeting.
        let other = OwnIdentity::from_hrefs(
            &["/zZ9/principal/".into(), "mailto:bob@example.net".into()],
            &url::Url::parse("https://p42-caldav.icloud.com/9999/principal/").unwrap(),
        );
        let events = parse_calendar_data_as(ICLOUD_MEETING, "cal-1", None, &other).unwrap();
        assert!(events[0].organized_elsewhere);
    }

    /// The read decides by the ORGANIZER line itself, as the write does: its
    /// principal path names the account even when its EMAIL is an alias the
    /// server did not list. And on the account's own meeting a row naming
    /// another of its addresses is not a guest either.
    #[test]
    fn the_read_knows_the_account_by_the_organizer_line_itself() {
        let body = ICLOUD_MEETING
            .replace(
                "ORGANIZER;CN=Toni Barth;EMAIL=toni@example.org:/aB1/principal/",
                "ORGANIZER;CN=Toni Barth;EMAIL=alias@me.example:/aB1/principal/",
            )
            .replace(
                "SEQUENCE:0\r\n",
                "ATTENDEE;CN=Toni;PARTSTAT=ACCEPTED:urn:uuid:1974\r\nSEQUENCE:0\r\n",
            );
        let events = parse_calendar_data_as(&body, "cal-1", None, &icloud_identity()).unwrap();
        assert!(
            !events[0].organized_elsewhere,
            "the principal path is the account's"
        );
        assert_eq!(events[0].attendees, ["Bob Guest <bob@example.net>"]);
        // Without the account's addresses nothing is known about it.
        let events = parse_calendar_data(&body, "cal-1").unwrap();
        assert!(events[0].organized_elsewhere);
    }

    #[test]
    fn only_a_line_that_starts_with_organizer_names_one() {
        assert!(names_an_organizer(ICLOUD_MEETING));
        assert!(names_an_organizer("BEGIN:VEVENT\norganizer:mailto:a@x\n"));
        assert!(!names_an_organizer(
            "BEGIN:VEVENT\r\nDESCRIPTION:ask the\r\n ORGANIZER:first\r\nSUMMARY:x\r\n"
        ));
        assert!(!names_an_organizer("BEGIN:VEVENT\r\nORGANIZERS:x\r\n"));
    }

    #[test]
    fn calendar_users_compare_by_their_own_rules() {
        assert!(same_calendar_user("mailto:A@x", "MAILTO:a@x"));
        assert!(same_calendar_user("/aB1/principal/", "/aB1/principal"));
        assert!(!same_calendar_user("/aB1/principal/", "/ab1/principal/"));
        assert!(!same_calendar_user("mailto:a@x", "/a@x/"));
        assert!(!same_calendar_user("", ""));
    }

    #[test]
    fn maps_status_cancelled_to_cancelled_flag() {
        let cancelled = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:mtg-x@aperio\r
SUMMARY:Cancelled Planning\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
STATUS:CANCELLED\r
END:VEVENT\r
END:VCALENDAR\r
";
        let ev = &parse_calendar_data(cancelled, "cal-1").unwrap()[0];
        assert!(ev.cancelled);

        // A CONFIRMED (or absent) STATUS reads as active.
        let confirmed = cancelled.replace("STATUS:CANCELLED", "STATUS:CONFIRMED");
        let ev = &parse_calendar_data(&confirmed, "cal-1").unwrap()[0];
        assert!(!ev.cancelled);
    }

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
    fn master_and_recurrence_id_override_get_distinct_ids() {
        // A recurring series whose 2nd occurrence was edited: iCloud sends the
        // master (RRULE) and the override (RECURRENCE-ID) as TWO VEVENTs sharing
        // one UID in ONE resource. Before the fix they got the same `{href}|{uid}`
        // id and the cache upsert dropped one — losing the master's RRULE, so the
        // whole series vanished. They must now map to distinct rows.
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:series-1@aperio\r
SUMMARY:Daily standup\r
DTSTART:20260519T090000Z\r
DTEND:20260519T093000Z\r
RRULE:FREQ=DAILY;COUNT=5\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:series-1@aperio\r
RECURRENCE-ID:20260520T090000Z\r
SUMMARY:Daily standup (moved)\r
DTSTART:20260520T140000Z\r
DTEND:20260520T143000Z\r
END:VEVENT\r
END:VCALENDAR\r
";
        let href = "/calendars/home/series-1.ics";
        let events =
            parse_calendar_data_with_href(body, "https://example.test/calendars/home/", Some(href))
                .unwrap();
        assert_eq!(events.len(), 2, "both the master and the override map");

        let master = events
            .iter()
            .find(|e| e.recurrence.is_some())
            .expect("the master keeps its RRULE");
        let override_ev = events
            .iter()
            .find(|e| e.recurrence.is_none())
            .expect("the override drops the series rule");

        assert_eq!(master.id, format!("{href}|series-1@aperio"));
        assert_eq!(
            override_ev.id,
            format!("{href}|series-1@aperio::rid::2026-05-20T09:00:00Z"),
            "the override id carries the replaced occurrence instant"
        );
        assert_ne!(master.id, override_ev.id, "no id collision → no clobber");
        // host-core's `native_id()` splits at the first `|`, so the suffix still
        // resolves to the resource href for delta deletions.
        assert_eq!(override_ev.id.split('|').next(), Some(href));
        // The override renders at its moved time; the master series is intact.
        assert_eq!(
            override_ev.start,
            Utc.with_ymd_and_hms(2026, 5, 20, 14, 0, 0).unwrap()
        );
        assert!(master.recurrence.as_ref().unwrap().rrule.contains("DAILY"));
    }

    #[test]
    fn zoned_recurring_master_carries_its_dtstart_timezone() {
        // The reporter's "oagdu" shape: a monthly 2nd-Sunday 19:00 Eastern series.
        // The DTSTART tzid must ride onto EventRecurrence so the frontend expands
        // in local wall-clock time instead of flattening to UTC and drifting an
        // hour (and a day) once the series crosses the EST->EDT boundary.
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:oagdu@aperio\r
SUMMARY:OAGDU meeting\r
DTSTART;TZID=America/New_York:20251214T190000\r
DTEND;TZID=America/New_York:20251214T200000\r
RRULE:FREQ=MONTHLY;BYDAY=2SU\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        let ev = &events[0];
        // 19:00 EST (UTC-5) -> 00:00 UTC the next day.
        assert_eq!(
            ev.start,
            Utc.with_ymd_and_hms(2025, 12, 15, 0, 0, 0).unwrap()
        );
        let rec = ev.recurrence.as_ref().expect("recurring master");
        assert_eq!(rec.tzid.as_deref(), Some("America/New_York"));
        assert!(rec.rrule.contains("BYDAY=2SU"));
    }

    #[test]
    fn utc_and_floating_recurring_masters_carry_no_timezone() {
        // UTC (`Z`) and floating DTSTART have no DST ambiguity; leaving tzid None
        // keeps them on the unchanged UTC expansion path (no behaviour change).
        for dtstart in ["20260101T120000Z", "20260101T120000"] {
            let body = format!(
                "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:u@aperio\r
SUMMARY:Daily\r
DTSTART:{dtstart}\r
RRULE:FREQ=DAILY\r
END:VEVENT\r
END:VCALENDAR\r
"
            );
            let events = parse_calendar_data(&body, "cal-1").unwrap();
            assert_eq!(events[0].recurrence.as_ref().unwrap().tzid, None);
        }
    }

    #[test]
    fn write_back_preserves_a_zoned_recurring_masters_timezone() {
        // Editing a zoned recurring master in Aperio must NOT flatten its DTSTART
        // to a bare UTC instant — that drops the zone and re-introduces the DST
        // drift on the next read. The TZID has to round-trip.
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:oagdu@aperio\r
SUMMARY:OAGDU meeting\r
DTSTART;TZID=America/New_York:20251214T190000\r
DTEND;TZID=America/New_York:20251214T200000\r
RRULE:FREQ=MONTHLY;BYDAY=2SU\r
END:VEVENT\r
END:VCALENDAR\r
";
        let ev = parse_calendar_data(body, "cal-1").unwrap().remove(0);
        let ical = event_to_ical(&ev);
        assert!(
            ical.contains("DTSTART;TZID=America/New_York"),
            "expected a zoned DTSTART in the PUT body, got:\n{ical}"
        );
        // Re-reading keeps both the exact instant and the zone (no drift on edit).
        let reread = parse_calendar_data(&ical, "cal-1").unwrap().remove(0);
        assert_eq!(reread.start, ev.start);
        assert_eq!(
            reread.recurrence.as_ref().unwrap().tzid.as_deref(),
            Some("America/New_York")
        );
    }

    #[test]
    fn zoned_recurring_master_carries_a_vtimezone_before_the_vevent() {
        // The reason iCloud dropped a COUNT-bounded recurrence: a
        // DTSTART;TZID=… with no matching VTIMEZONE is invalid per RFC 5545
        // §3.6.5, and iCloud can't resolve the zone to bound the rule. The PUT
        // body must now DEFINE the referenced zone, and the VTIMEZONE must
        // precede the VEVENT that references it.
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:meal@aperio\r
SUMMARY:Essensplan\r
DTSTART;TZID=Europe/Berlin:20260520T173000\r
DTEND;TZID=Europe/Berlin:20260520T180000\r
RRULE:FREQ=DAILY;COUNT=2\r
END:VEVENT\r
END:VCALENDAR\r
";
        let ev = parse_calendar_data(body, "cal-1").unwrap().remove(0);
        let ical = event_to_ical(&ev);

        let vtz = ical
            .find("BEGIN:VTIMEZONE")
            .expect("a zoned recurring master must define its VTIMEZONE");
        let vevent = ical.find("BEGIN:VEVENT").expect("still has the VEVENT");
        assert!(vtz < vevent, "VTIMEZONE must precede the VEVENT:\n{ical}");
        assert!(
            ical.contains("TZID:Europe/Berlin\r\n"),
            "VTIMEZONE names the referenced zone:\n{ical}"
        );
        // The COUNT rule and the zoned DTSTART both survive the round-trip.
        assert!(ical.contains("RRULE:FREQ=DAILY;COUNT=2"), "{ical}");
        assert!(
            ical.contains("DTSTART;TZID=Europe/Berlin:20260520T173000"),
            "{ical}"
        );
    }

    #[test]
    fn non_recurring_and_all_day_events_get_no_vtimezone() {
        // A bare-UTC single event references no TZID, so it needs no VTIMEZONE.
        let single = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:lunch@aperio\r
SUMMARY:Lunch\r
DTSTART:20260520T080000Z\r
DTEND:20260520T083000Z\r
END:VEVENT\r
END:VCALENDAR\r
";
        let ev = parse_calendar_data(single, "cal-1").unwrap().remove(0);
        assert!(
            !event_to_ical(&ev).contains("VTIMEZONE"),
            "a non-recurring UTC event must not carry a VTIMEZONE"
        );

        // An all-day recurring event writes DATE values (no time, no TZID).
        let all_day = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:standup@aperio\r
SUMMARY:Standup\r
DTSTART;VALUE=DATE:20260520\r
RRULE:FREQ=WEEKLY;COUNT=4\r
END:VEVENT\r
END:VCALENDAR\r
";
        let ev = parse_calendar_data(all_day, "cal-1").unwrap().remove(0);
        assert!(
            !event_to_ical(&ev).contains("VTIMEZONE"),
            "an all-day recurring event must not carry a VTIMEZONE"
        );
    }

    /// Decision 46a: an all-day series that stores a zone, for example one
    /// kept after switching a timed series to all-day, still writes DATE
    /// values with no TZID and no VTIMEZONE (the core's `written_series_zone`).
    #[test]
    fn an_all_day_series_with_a_stored_zone_gets_no_vtimezone() {
        let all_day = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:standup@aperio\r
SUMMARY:Standup\r
DTSTART;VALUE=DATE:20260520\r
RRULE:FREQ=WEEKLY;COUNT=4\r
END:VEVENT\r
END:VCALENDAR\r
";
        let mut ev = parse_calendar_data(all_day, "cal-1").unwrap().remove(0);
        assert!(ev.all_day);
        ev.recurrence
            .as_mut()
            .expect("the series keeps its rule")
            .tzid = Some("Europe/Berlin".into());
        let ical = event_to_ical(&ev);
        assert!(!ical.contains("VTIMEZONE"), "no VTIMEZONE:\n{ical}");
        assert!(!ical.contains("TZID="), "no TZID:\n{ical}");
        assert!(ical.contains("DTSTART;VALUE=DATE:"), "DATE start:\n{ical}");
    }

    #[test]
    fn literal_tzid_utc_recurring_writes_bare_utc_not_an_undefined_tzid() {
        // A server can round-trip a recurring master with a literal TZID=UTC. We
        // must NOT write DTSTART;TZID=UTC (vtimezone_for emits no VTIMEZONE for
        // UTC, so that would be an undefined TZID reference — RFC 5545 §3.6.5).
        // Instead it takes the bare-UTC path: DTSTART:...Z, no TZID, no VTIMEZONE.
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:utc@aperio\r
SUMMARY:UTC sync\r
DTSTART;TZID=UTC:20260520T170000\r
DTEND;TZID=UTC:20260520T173000\r
RRULE:FREQ=DAILY;COUNT=2\r
END:VEVENT\r
END:VCALENDAR\r
";
        let ev = parse_calendar_data(body, "cal-1").unwrap().remove(0);
        // Precondition: the reader really did surface tzid = Some("UTC").
        assert_eq!(ev.recurrence.as_ref().unwrap().tzid.as_deref(), Some("UTC"));

        let ical = event_to_ical(&ev);
        assert!(!ical.contains("VTIMEZONE"), "no VTIMEZONE for UTC:\n{ical}");
        assert!(
            !ical.contains("TZID=UTC"),
            "no dangling TZID=UTC ref:\n{ical}"
        );
        assert!(
            ical.contains("DTSTART:20260520T170000Z"),
            "writes a bare-UTC DTSTART:\n{ical}"
        );
        // The recurrence itself still round-trips.
        assert!(ical.contains("RRULE:FREQ=DAILY;COUNT=2"), "{ical}");
    }

    #[test]
    fn zoned_exdate_is_resolved_in_its_timezone() {
        // EXDATE;TZID on a zoned series must resolve to the same UTC instant the
        // zone-correct expansion produces, so a deleted occurrence stays deleted
        // instead of reappearing. Before the fix the naive value parsed to
        // nothing and was dropped.
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:oagdu@aperio\r
SUMMARY:OAGDU meeting\r
DTSTART;TZID=America/New_York:20251214T190000\r
RRULE:FREQ=MONTHLY;BYDAY=2SU\r
EXDATE;TZID=America/New_York:20260712T190000\r
END:VEVENT\r
END:VCALENDAR\r
";
        let ev = parse_calendar_data(body, "cal-1").unwrap().remove(0);
        let rec = ev.recurrence.as_ref().unwrap();
        // 2026-07-12 19:00 EDT = 23:00 UTC — the same instant the July occurrence
        // expands to.
        assert_eq!(
            rec.exceptions,
            vec![Utc.with_ymd_and_hms(2026, 7, 12, 23, 0, 0).unwrap()]
        );
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
        // DATE values anchor at LOCAL midnight (the app-internal all-day
        // convention) — compare against the same Local construction so the
        // assertion holds in any timezone the test runs in.
        assert_eq!(ev.start, local_midnight_utc(2026, 5, 20));
        // Missing DTEND on an all-day event: end of day.
        assert_eq!(
            ev.end,
            local_midnight_utc(2026, 5, 20) + chrono::Duration::days(1)
        );
        // The instant renders on the right LOCAL calendar day.
        assert_eq!(
            ev.start.with_timezone(&Local).date_naive(),
            NaiveDate::from_ymd_opt(2026, 5, 20).unwrap(),
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
    fn write_then_read_roundtrips_an_event() {
        let event = NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Sprint planning".into(),
            description: Some("agenda in the doc".into()),
            location: Some("room 3.12".into()),
            start: Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap(),
            all_day: false,
            recurrence: Some(EventRecurrence {
                rrule: "FREQ=WEEKLY;BYDAY=WE".into(),
                exceptions: vec![Utc.with_ymd_and_hms(2026, 6, 3, 8, 0, 0).unwrap()],
                tzid: None,
            }),
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
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

    /// All-day instants the way the frontend produces them: LOCAL
    /// midnight of the calendar day, expressed in UTC. Tests built on
    /// this stay correct in whatever timezone they run in.
    fn local_midnight_utc(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        let date = NaiveDate::from_ymd_opt(y, m, d).unwrap();
        naive_date_to_utc(date)
    }

    #[test]
    fn write_all_day_uses_value_date() {
        let event = NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Birthday".into(),
            description: None,
            location: None,
            // The frontend sends local midnights; end is EXCLUSIVE
            // (single-day event covering May 20 → end May 21).
            start: local_midnight_utc(2026, 5, 20),
            end: local_midnight_utc(2026, 5, 21),
            all_day: true,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
        };
        let body = new_event_to_ical("bday-uid", &event);
        assert!(
            body.contains("DTSTART;VALUE=DATE:20260520"),
            "expected VALUE=DATE DTSTART, got: {body}",
        );
        assert!(
            body.contains("DTEND;VALUE=DATE:20260521"),
            "expected exclusive VALUE=DATE DTEND, got: {body}",
        );
    }

    /// The reported iCloud bug: a two-day all-day event (June 10–11,
    /// created at local midnights with an exclusive end of June 12) must
    /// hit the wire as DTSTART June 10 / DTEND June 12 — NOT shifted to
    /// the UTC day (June 9 for a UTC+2 user), and NOT one day short.
    #[test]
    fn write_two_day_all_day_keeps_local_days_and_exclusive_end() {
        let event = NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Conference".into(),
            description: None,
            location: None,
            start: local_midnight_utc(2026, 6, 10),
            end: local_midnight_utc(2026, 6, 12),
            all_day: true,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
        };
        let body = new_event_to_ical("conf-uid", &event);
        assert!(body.contains("DTSTART;VALUE=DATE:20260610"), "{body}");
        assert!(body.contains("DTEND;VALUE=DATE:20260612"), "{body}");
    }

    /// Server → Aperio → server round-trip: DATE boundaries read from a
    /// server body must serialise back to the SAME dates, regardless of
    /// the machine's timezone.
    #[test]
    fn all_day_date_boundaries_round_trip() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:conf@aperio\r
SUMMARY:Conference\r
DTSTART;VALUE=DATE:20260610\r
DTEND;VALUE=DATE:20260612\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        let ev = &events[0];
        assert!(ev.all_day);
        let rewritten = event_to_ical(ev);
        assert!(
            rewritten.contains("DTSTART;VALUE=DATE:20260610"),
            "{rewritten}"
        );
        assert!(
            rewritten.contains("DTEND;VALUE=DATE:20260612"),
            "{rewritten}"
        );
    }

    /// The renderer writes no meeting lines at all, whatever the event asks
    /// for: they are the write path's to carry or add (`crate::scheduling`).
    #[test]
    fn the_renderer_writes_no_meeting_lines() {
        let event = NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Review".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: vec!["Alice <alice@example.com>".into(), "bob@example.com".into()],
            send_invitations: true,
        };
        let body = new_event_to_ical("uid-1", &event);
        assert!(!body.contains("ORGANIZER"), "{body}");
        assert!(!body.contains("ATTENDEE"), "{body}");
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
            organized_elsewhere: false,
            organizer: None,
            title: "Sync".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
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
            send_invitations: false,
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

    /// A body shaped like what iCloud actually stores for an appointment
    /// created on an iPhone: the account's default alert, written into the
    /// event as a marked VALARM with an identity of its own.
    fn icloud_body_with_default_alarm() -> &'static str {
        "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//Apple Inc.//iCloud Calendar//EN\r
BEGIN:VEVENT\r
UID:evt-1\r
DTSTAMP:20260520T060000Z\r
SUMMARY:Zahnarzt\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
X-APPLE-DEFAULT-ALARM:TRUE\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:Event reminder\r
TRIGGER:-PT1H\r
UID:alarm-1\r
X-WR-ALARMUID:alarm-1\r
X-APPLE-DEFAULT-ALARM:TRUE\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
"
    }

    /// The event as Aperio holds it — read back from that same body, which is
    /// how it really reaches a write, with the reminders the user left behind.
    fn event_with_reminders(kinds: Vec<ReminderKind>) -> Event {
        let mut event = parse_calendar_data(icloud_body_with_default_alarm(), "cal-1")
            .unwrap()
            .remove(0);
        event.reminders = kinds
            .into_iter()
            .map(|kind| Reminder { kind, sound: None })
            .collect();
        event
    }

    #[test]
    fn keeps_apples_default_alarm_mark_when_the_reminder_is_untouched() {
        // The whole point. Apple recognises its own default alert by this
        // mark; rebuilding the VEVENT dropped it, so an appointment made on an
        // iPhone came back from a plain title edit with Apple's default
        // looking hand-made.
        let prior = PriorAlarms::read(icloud_body_with_default_alarm());
        let event = event_with_reminders(vec![ReminderKind::Relative { minutes_before: 60 }]);
        let out = event_to_ical_preserving(&event, prior);

        // Counted per LEVEL, not in total: two marks on the alarm and none on
        // the event would satisfy a bare count of two.
        let (event_part, alarm_part) = out.split_once("BEGIN:VALARM").unwrap();
        assert!(
            event_part.contains("X-APPLE-DEFAULT-ALARM:TRUE"),
            "the mark is missing from the event itself:\n{out}"
        );
        assert!(
            alarm_part.contains("X-APPLE-DEFAULT-ALARM:TRUE"),
            "the mark is missing from the alarm:\n{out}"
        );
        assert_eq!(out.matches("X-APPLE-DEFAULT-ALARM").count(), 2, "{out}");
        // Anchored to a whole line: `X-WR-ALARMUID:alarm-1` contains the same
        // text, so a bare substring test cannot see the loss it names.
        assert!(
            out.contains("\r\nUID:alarm-1\r\n"),
            "the alarm's own identity was lost:\n{out}"
        );
        assert!(
            out.contains("X-WR-ALARMUID:alarm-1"),
            "an unknown X-property was dropped:\n{out}"
        );
        // The server's own spelling survives: we would write -PT3600S for the
        // very same reminder, and re-spelling an alarm nobody touched is
        // exactly the damage this avoids.
        assert!(
            out.contains("TRIGGER:-PT1H"),
            "the untouched alarm was re-spelled:\n{out}"
        );
        // DESCRIPTION stays ours — it is the title being written.
        assert!(out.contains("DESCRIPTION:Zahnarzt"), "{out}");
    }

    #[test]
    fn drops_the_mark_once_the_user_moves_the_reminder() {
        // 60 → 30 is the user's choice, not the account's default any more.
        let prior = PriorAlarms::read(icloud_body_with_default_alarm());
        let event = event_with_reminders(vec![ReminderKind::Relative { minutes_before: 30 }]);
        let out = event_to_ical_preserving(&event, prior);

        assert!(
            !out.contains("X-APPLE-DEFAULT-ALARM"),
            "a reminder the user moved must not still claim to be the account default:\n{out}"
        );
        assert!(
            !out.contains("UID:alarm-1"),
            "a changed reminder must not inherit the old alarm's identity:\n{out}"
        );
        assert!(out.contains("TRIGGER:-PT1800S"), "{out}");
    }

    #[test]
    fn drops_the_mark_once_the_user_adds_a_reminder_beside_it() {
        // The kept alarm is still Apple's, but the SET is no longer the
        // account's default — and handing a changed set back still marked
        // invites the account to repair it into what its default says.
        let prior = PriorAlarms::read(icloud_body_with_default_alarm());
        let event = event_with_reminders(vec![
            ReminderKind::Relative { minutes_before: 60 },
            ReminderKind::Relative { minutes_before: 10 },
        ]);
        let out = event_to_ical_preserving(&event, prior);

        assert!(
            !out.contains("X-APPLE-DEFAULT-ALARM"),
            "an extended set must not stay marked as the account default:\n{out}"
        );
        // The untouched alarm still keeps its identity — that is not the mark.
        assert!(out.contains("UID:alarm-1"), "{out}");
    }

    #[test]
    fn removing_the_reminder_takes_the_mark_with_it() {
        let prior = PriorAlarms::read(icloud_body_with_default_alarm());
        let event = event_with_reminders(Vec::new());
        let out = event_to_ical_preserving(&event, prior);

        assert!(!out.contains("BEGIN:VALARM"), "{out}");
        assert!(!out.contains("X-APPLE-DEFAULT-ALARM"), "{out}");
    }

    #[test]
    fn two_identical_reminders_take_two_different_alarms() {
        // Both alarms fire an hour before and are indistinguishable after the
        // parse. Each may be claimed once, or one alarm's identity would be
        // written twice and the other's lost.
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
BEGIN:VEVENT\r
UID:evt-1\r
DTSTAMP:20260520T060000Z\r
SUMMARY:Zahnarzt\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:a\r
TRIGGER:-PT1H\r
UID:alarm-a\r
END:VALARM\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:b\r
TRIGGER:-PT60M\r
UID:alarm-b\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
";
        let prior = PriorAlarms::read(body);
        let event = event_with_reminders(vec![
            ReminderKind::Relative { minutes_before: 60 },
            ReminderKind::Relative { minutes_before: 60 },
        ]);
        let out = event_to_ical_preserving(&event, prior);

        assert!(out.contains("UID:alarm-a"), "{out}");
        assert!(out.contains("UID:alarm-b"), "{out}");
        assert_eq!(out.matches("UID:alarm-a").count(), 1, "{out}");
    }

    #[test]
    fn an_overrides_alarms_are_not_the_masters() {
        // One resource, master plus a modified occurrence. `event_to_ical`
        // only ever writes the master, so only the master's alarms may be
        // inherited — the override's belong to that one occurrence.
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
BEGIN:VEVENT\r
UID:evt-1\r
DTSTAMP:20260520T060000Z\r
SUMMARY:Serie\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
RRULE:FREQ=WEEKLY\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:master\r
TRIGGER:-PT1H\r
UID:alarm-master\r
END:VALARM\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:evt-1\r
RECURRENCE-ID:20260527T080000Z\r
DTSTAMP:20260520T060000Z\r
SUMMARY:Serie\r
DTSTART:20260527T090000Z\r
DTEND:20260527T100000Z\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:override\r
TRIGGER:-PT1H\r
UID:alarm-override\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
";
        let prior = PriorAlarms::read(body);
        let event = event_with_reminders(vec![ReminderKind::Relative { minutes_before: 60 }]);
        let out = event_to_ical_preserving(&event, prior);

        assert!(out.contains("UID:alarm-master"), "{out}");
        assert!(!out.contains("alarm-override"), "{out}");
    }

    #[test]
    fn a_new_appointment_never_forges_the_mark() {
        // Apple's mark is Apple's to apply. A create has no previous copy, and
        // event_to_ical without one must behave exactly as it always did.
        let event = event_with_reminders(vec![ReminderKind::Relative { minutes_before: 60 }]);
        let out = event_to_ical(&event);
        assert!(!out.contains("X-APPLE-DEFAULT-ALARM"), "{out}");
        assert!(out.contains("TRIGGER:-PT3600S"), "{out}");
    }

    /// An event carrying one alarm Aperio can read and one it cannot.
    fn body_with_an_unreadable_alarm(unreadable: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r
VERSION:2.0\r
BEGIN:VEVENT\r
UID:evt-1\r
DTSTAMP:20260520T060000Z\r
SUMMARY:Zahnarzt\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
X-APPLE-DEFAULT-ALARM:TRUE\r
{unreadable}BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:Event reminder\r
TRIGGER:-PT1H\r
UID:alarm-hour\r
X-APPLE-DEFAULT-ALARM:TRUE\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
"
        )
    }

    #[test]
    fn an_alarm_aperio_cannot_read_still_counts_as_part_of_the_set() {
        // Apple's default-alert menu offers "1 week before", and iCloud stores
        // that as TRIGGER:-P1W — which this crate's duration parser cannot
        // read. The reminder never reaches the editor and the rebuild drops
        // the VALARM, so the set the PUT carries is SMALLER than the server's.
        // Calling that untouched would tell the account its default is intact
        // and invite it to put the missing alarm back.
        let prior = PriorAlarms::read(&body_with_an_unreadable_alarm(
            "BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:Event reminder\r
TRIGGER:-P1W\r
UID:alarm-week\r
X-APPLE-DEFAULT-ALARM:TRUE\r
END:VALARM\r
",
        ));
        let event = event_with_reminders(vec![ReminderKind::Relative { minutes_before: 60 }]);
        let out = event_to_ical_preserving(&event, prior);

        assert!(
            !out.contains("X-APPLE-DEFAULT-ALARM"),
            "an alarm went missing from the set, so it is not the account default any more:\n{out}"
        );
    }

    #[test]
    fn an_alarm_relative_to_the_end_is_left_alone() {
        // RELATED=END means "before the END". Aperio reads it as "before the
        // start" and shows it that way, so neither keeping the text nor
        // writing our own is honest — the alarm is not ours to touch.
        let prior = PriorAlarms::read(&body_with_an_unreadable_alarm(
            "BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:Event reminder\r
TRIGGER;RELATED=END:-PT15M\r
UID:alarm-end\r
END:VALARM\r
",
        ));
        let event = event_with_reminders(vec![
            ReminderKind::Relative { minutes_before: 15 },
            ReminderKind::Relative { minutes_before: 60 },
        ]);
        let out = event_to_ical_preserving(&event, prior);

        assert!(
            !out.contains("UID:alarm-end"),
            "an alarm Aperio misreads must not have its identity carried over:\n{out}"
        );
        assert!(!out.contains("X-APPLE-DEFAULT-ALARM"), "{out}");
    }

    #[test]
    fn an_audio_alarm_is_not_dressed_up_as_a_display_one() {
        // RFC 5545 §3.6.6 gives each ACTION its own allowed properties, and an
        // AUDIO alarm may not carry a DESCRIPTION. Inheriting the ACTION while
        // still authoring the DESCRIPTION built exactly that body — which a
        // validating server answers with 403.
        let prior = PriorAlarms::read(
            "BEGIN:VCALENDAR\r
VERSION:2.0\r
BEGIN:VEVENT\r
UID:evt-1\r
DTSTAMP:20260520T060000Z\r
SUMMARY:Zahnarzt\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
BEGIN:VALARM\r
ACTION:AUDIO\r
TRIGGER:-PT1H\r
UID:alarm-audio\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
",
        );
        let event = event_with_reminders(vec![ReminderKind::Relative { minutes_before: 60 }]);
        let out = event_to_ical_preserving(&event, prior);

        assert!(
            !out.contains("ACTION:AUDIO"),
            "we build a DISPLAY alarm; wearing the old ACTION makes it invalid:\n{out}"
        );
        assert!(out.contains("ACTION:DISPLAY"), "{out}");
    }

    #[test]
    fn a_property_this_crate_cannot_re_quote_is_not_carried_over() {
        // The writer only re-quotes a parameter value containing `:` or `;`,
        // so a comma inside one comes back UNQUOTED — and RFC 5545 §3.2 then
        // reads it as a list of values. Every real street address has a comma
        // in it. These properties used to be dropped on every write; handing
        // them back mangled would be new damage, not a repair.
        let prior = PriorAlarms::read(
            "BEGIN:VCALENDAR\r
VERSION:2.0\r
BEGIN:VEVENT\r
UID:evt-1\r
DTSTAMP:20260520T060000Z\r
SUMMARY:Zahnarzt\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
BEGIN:VALARM\r
ACTION:DISPLAY\r
DESCRIPTION:Event reminder\r
TRIGGER:-PT1H\r
UID:alarm-1\r
X-APPLE-STRUCTURED-LOCATION;VALUE=URI;X-ADDRESS=\"Hauptstr. 1, Berlin\":geo:52.5,13.4\r
END:VALARM\r
END:VEVENT\r
END:VCALENDAR\r
",
        );
        let event = event_with_reminders(vec![ReminderKind::Relative { minutes_before: 60 }]);
        let out = event_to_ical_preserving(&event, prior);

        assert!(
            !out.contains("X-ADDRESS"),
            "a parameter we cannot re-quote must not be written back:\n{out}"
        );
        assert!(
            !out.contains("UID:alarm-1"),
            "the alarm is not reproducible, so nothing of it is inherited:\n{out}"
        );
    }

    #[test]
    fn an_unreadable_previous_copy_costs_only_the_preservation() {
        let prior = PriorAlarms::read("not an icalendar body at all");
        let event = event_with_reminders(vec![ReminderKind::Relative { minutes_before: 60 }]);
        let out = event_to_ical_preserving(&event, prior);
        assert!(
            out.contains("BEGIN:VALARM"),
            "the write itself must stand:\n{out}"
        );
        assert!(!out.contains("X-APPLE-DEFAULT-ALARM"), "{out}");
    }

    /// The far end of the chain in `shared/contracts/calendarDefaultReminders.json`.
    ///
    /// host-core pins the near end — the stored pref becoming a new
    /// appointment's own reminder (`wire_contract` in host-core's reminders.rs,
    /// same fixture). This pins what a CalDAV server then receives, which is
    /// the only thing an iPhone or a voice assistant can act on.
    ///
    /// Together the two replace the part of the manual device test that a
    /// machine can check: set a calendar's default to "attached", create an
    /// appointment, and see the alarm arrive at the provider. What is left for
    /// a real device is only what Apple and Amazon do with it afterwards.
    mod calendar_default_reminders_contract {
        use super::*;

        /// `include_str!`, so moving the fixture breaks the BUILD rather than
        /// quietly skipping the contract.
        const CONTRACT: &str =
            include_str!("../../../shared/contracts/calendarDefaultReminders.json");

        /// The reminders a contract sample describes, built from its
        /// language-neutral `entries` rather than from a Rust literal — the
        /// point is that the shape comes from the shared file.
        fn attached_reminders_of(sample_name: &str) -> Vec<Reminder> {
            let contract: serde_json::Value =
                serde_json::from_str(CONTRACT).expect("the contract fixture is valid JSON");
            let sample = contract["samples"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["name"] == sample_name)
                .unwrap_or_else(|| panic!("no contract sample called '{sample_name}'"))
                .clone();
            sample["entries"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["attach"].as_bool().unwrap())
                .map(|e| Reminder {
                    kind: match e["kind"].as_str().unwrap() {
                        "relative" => ReminderKind::Relative {
                            minutes_before: e["minutes_before"].as_i64().unwrap(),
                        },
                        "absolute" => ReminderKind::Absolute {
                            at: e["at"].as_str().unwrap().parse().unwrap(),
                        },
                        other => panic!("contract sample uses an unhandled kind '{other}'"),
                    },
                    sound: None,
                })
                .collect()
        }

        fn new_appointment(reminders: Vec<Reminder>) -> NewEvent {
            NewEvent {
                organized_elsewhere: false,
                organizer: None,
                title: "Zahnarzt".into(),
                description: None,
                location: None,
                start: Utc.with_ymd_and_hms(2026, 9, 6, 8, 0, 0).unwrap(),
                end: Utc.with_ymd_and_hms(2026, 9, 6, 9, 0, 0).unwrap(),
                all_day: false,
                recurrence: None,
                color_label: None,
                color_hex: None,
                reminders,
                sound: None,
                attendees: Vec::new(),
                send_invitations: false,
            }
        }

        #[test]
        fn a_calendar_default_reaches_the_wire_as_a_valarm() {
            let reminders = attached_reminders_of("mixed-list-both-placements");
            assert_eq!(reminders.len(), 1, "one attached entry in that sample");
            let out = new_event_to_ical("evt-1", &new_appointment(reminders));

            assert_eq!(
                out.matches("BEGIN:VALARM").count(),
                1,
                "the attached default has to arrive as exactly one alarm:\n{out}"
            );
            assert!(out.contains("ACTION:DISPLAY"), "{out}");
            // An hour before, in the spelling this crate produces.
            assert!(
                out.contains("TRIGGER:-PT3600S"),
                "the alarm does not fire an hour before:\n{out}"
            );
        }

        #[test]
        fn an_appointment_with_no_reminders_carries_no_alarm() {
            // The other half of the promise: a calendar whose defaults all
            // stay in Aperio must not put anything on the wire, or every
            // client of that calendar would ring for a setting the user
            // deliberately kept local.
            let out = new_event_to_ical("evt-1", &new_appointment(Vec::new()));
            assert!(!out.contains("BEGIN:VALARM"), "{out}");
        }

        #[test]
        fn an_absolute_default_reaches_the_wire_at_its_exact_instant() {
            let reminders = attached_reminders_of("absolute");
            let out = new_event_to_ical("evt-1", &new_appointment(reminders));
            assert!(
                out.contains("TRIGGER;VALUE=DATE-TIME:20260906T130000Z"),
                "the fixed moment did not survive:\n{out}"
            );
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

    fn color_new_event(color_hex: Option<&str>) -> NewEvent {
        NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Painted".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: color_hex.map(str::to_string),
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
        }
    }

    #[test]
    fn writes_color_property_only_when_color_hex_set() {
        let with = new_event_to_ical("uid-c", &color_new_event(Some("#4285f4")));
        assert!(
            with.contains("COLOR:#4285f4"),
            "expected COLOR line, got:\n{with}"
        );
        // Absent color_hex → no COLOR property at all.
        let without = new_event_to_ical("uid-c", &color_new_event(None));
        assert!(
            !without.contains("COLOR:"),
            "unexpected COLOR line:\n{without}"
        );
    }

    #[test]
    fn reads_color_hex_property_into_color_hex() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:colored@aperio\r
SUMMARY:Colored\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
COLOR:#FF0000\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        // Hex is normalised to lowercase #rrggbb; color_label stays None
        // (the host maps the hex back to a label).
        assert_eq!(events[0].color_hex.as_deref(), Some("#ff0000"));
        assert!(events[0].color_label.is_none());
    }

    #[test]
    fn reads_css3_color_name_into_hex() {
        let body = "BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//test//EN\r
BEGIN:VEVENT\r
UID:named@aperio\r
SUMMARY:Named\r
DTSTART:20260520T080000Z\r
DTEND:20260520T090000Z\r
COLOR:tomato\r
END:VEVENT\r
END:VCALENDAR\r
";
        let events = parse_calendar_data(body, "cal-1").unwrap();
        assert_eq!(events[0].color_hex.as_deref(), Some("#ff6347"));
    }

    #[test]
    fn parse_color_accepts_hex_and_names_and_rejects_garbage() {
        assert_eq!(parse_color("#abc").as_deref(), Some("#aabbcc"));
        assert_eq!(parse_color("#AABBCC").as_deref(), Some("#aabbcc"));
        assert_eq!(parse_color(" #4285f4 ").as_deref(), Some("#4285f4"));
        assert_eq!(parse_color("cornflowerblue").as_deref(), Some("#6495ed"));
        assert_eq!(parse_color("GREY").as_deref(), Some("#808080"));
        assert_eq!(parse_color("not-a-color"), None);
        assert_eq!(parse_color("#12"), None);
        assert_eq!(parse_color("#nothex"), None);
    }

    #[test]
    fn color_round_trips_through_write_then_read() {
        let body = new_event_to_ical("rt-uid", &color_new_event(Some("#34a853")));
        let parsed = parse_calendar_data(&body, "cal-1").unwrap();
        assert_eq!(parsed[0].color_hex.as_deref(), Some("#34a853"));
    }

    /// A text value may end in the very characters of the end line. Cut there,
    /// the block would lose its start, its UID and its end, and the PUT the
    /// whole series goes out in would carry a broken VEVENT.
    #[test]
    fn an_override_block_ends_at_its_own_end_line() {
        let mut event = event_with_reminders(Vec::new());
        event.description = Some("pasted from a file\nEND:VEVENT".into());
        event.location = Some("END:VEVENT".into());
        let block = override_to_vevent(
            &event,
            "/cal/e.ics|series-1",
            "RECURRENCE-ID:20260615T070000Z\r\n",
            PriorAlarms::default(),
        );
        assert!(
            block.starts_with("BEGIN:VEVENT\r\nRECURRENCE-ID:"),
            "{block}"
        );
        assert!(block.ends_with("\r\nEND:VEVENT\r\n"), "{block}");
        assert!(block.contains("\r\nDTSTART"), "{block}");
        assert!(block.contains("\r\nUID:series-1\r\n"), "{block}");
        assert_eq!(
            block
                .split("\r\n")
                .filter(|line| *line == "END:VEVENT")
                .count(),
            1,
            "{block}"
        );
        assert!(!block.contains("END:VCALENDAR"), "{block}");
    }
}
