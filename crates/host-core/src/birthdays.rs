//! Birthday calendars — the pure synthesis (DESIGN.md §10.3).
//!
//! One synthesised, read-only calendar per contact list, populated from the
//! contacts' `birthday` field, surfaced through the same `list_calendars` /
//! `get_events` paths real calendars use. This module holds the Tauri-free,
//! cache-free pieces so BOTH the desktop commands and the mobile cal-ffi Host
//! synthesise identical birthday calendars + events:
//!
//!   - the id helpers (`birthday_calendar_id` / `underlying_contact_list_id` /
//!     `is_birthday_calendar_id`),
//!   - `synthesise_calendar` (the read-only `Calendar` for a contact list),
//!   - `events_for_contacts` (one all-day event per contact×occurrence-year in
//!     range — yearly expansion in-place, no RRULE master).
//!
//! The ORCHESTRATION (walking local + external contact books, reading the
//! snapshot cache) stays platform-side: desktop `commands::birthdays` uses the
//! `CacheStore`; the mobile Host synthesises from the local address book only
//! (it has no contact cache yet).

use cal_core::{Calendar, ColorSource, Contact, ContainerColor, DateRange, Event};
use chrono::{Datelike, NaiveDate, TimeZone, Utc};

/// Prefix every synthesised birthday calendar id carries. The `aperio-` prefix
/// makes these unambiguously synthesised (every real adapter mints opaque ids
/// that never start that way).
pub const BIRTHDAY_CALENDAR_PREFIX: &str = "aperio-birthdays:";

/// A warm pink, consistent across every synthesised layer (DESIGN.md §10.3) —
/// distinct from the default-blue palette but muted enough not to dominate.
const BIRTHDAY_LAYER_COLOR_HEX: &str = "#e91e63";

/// Build the calendar id for a contact list. Stable across runs (the underlying
/// contact list id never changes) so the user's per-calendar visibility sticks.
pub fn birthday_calendar_id(contact_list_id: &str) -> String {
    format!("{BIRTHDAY_CALENDAR_PREFIX}{contact_list_id}")
}

/// Inverse: extract the contact list id from a synthesised calendar id, or
/// `None` for non-birthday ids.
pub fn underlying_contact_list_id(calendar_id: &str) -> Option<&str> {
    calendar_id.strip_prefix(BIRTHDAY_CALENDAR_PREFIX)
}

pub fn is_birthday_calendar_id(calendar_id: &str) -> bool {
    calendar_id.starts_with(BIRTHDAY_CALENDAR_PREFIX)
}

/// The read-only synthetic `Calendar` for a contact list.
pub fn synthesise_calendar(contact_list_id: &str, list_name: &str) -> Calendar {
    Calendar {
        color_label: None,
        supports_scheduling: false,
        // Synthetic, read-only birthday layer — no per-event color to store.
        supports_event_color: false,
        id: birthday_calendar_id(contact_list_id),
        // English default; the user can re-localise via the existing
        // local-override path (DESIGN.md §6.5) since `read_only = true` triggers
        // the "fallback to local override" branch in `rename_container`.
        name: format!("Birthdays – {list_name}"),
        color: Some(ContainerColor {
            hex: BIRTHDAY_LAYER_COLOR_HEX.to_string(),
            source: ColorSource::Native,
        }),
        read_only: true,
        default_sound: None,
    }
}

/// Materialise one all-day event per (contact, occurrence-year) overlapping
/// `range`. Yearly expansion in-place (rather than an RRULE master) so the rest
/// of the codebase — chip rendering, ARIA labels, navigation — doesn't have to
/// know "this is a virtual recurrence".
pub fn events_for_contacts(
    contacts: Vec<Contact>,
    calendar_id: &str,
    range: DateRange,
) -> Vec<Event> {
    let mut out = Vec::new();
    let start_year = range.start.naive_utc().year();
    let end_year = range.end.naive_utc().year();
    // Cap the year span defensively (an agenda view should never ask for a
    // century of birthdays, but guard against it ballooning the result set).
    let safe_end_year = end_year.min(start_year + 100);

    for contact in contacts {
        let Some(bday) = contact.birthday else {
            continue;
        };
        for year in start_year..=safe_end_year {
            // Feb 29 on a non-leap year falls off the calendar — we don't shift
            // it (same tradeoff iOS Contacts makes); render it on years that
            // have it.
            let Some(date) = NaiveDate::from_ymd_opt(year, bday.month(), bday.day()) else {
                continue;
            };
            let Some(midnight) = date.and_hms_opt(0, 0, 0) else {
                continue;
            };
            let start = Utc.from_utc_datetime(&midnight);
            // All-day events span midnight-to-midnight (the existing convention);
            // the end is start + 1 day.
            let end = start + chrono::Duration::days(1);
            // Skip occurrences that don't overlap the requested range (inclusive
            // bounds — Year view spans Dec 31 23:59:59 to Jan 1 of the next).
            if end <= range.start || start >= range.end {
                continue;
            }
            out.push(Event {
                send_invitations: false,
                id: format!("aperio-birthday:{}:{}", contact.id, year),
                calendar_id: calendar_id.to_string(),
                title: contact.display_name.clone(),
                description: birthday_description(year, &bday),
                location: None,
                start,
                end,
                all_day: true,
                recurrence: None,
                color_label: None,
                color_hex: None,
                reminders: Vec::new(),
                sound: None,
                attendees: Vec::new(),
                created_at: contact.created_at,
                updated_at: contact.updated_at,
                etag: None,
                organizer: None,
                attendee_responses: Vec::new(),
            });
        }
    }
    out
}

/// The age the contact reaches on this occurrence, as a bare number string (the
/// frontend renders the localized "turns N" line). `None` when the birth year
/// is missing / a bogus placeholder (RFC 6350 leaves pre-1900 underdefined) or
/// the age isn't positive — the title (the name) then stands alone.
fn birthday_description(occurrence_year: i32, birthday: &NaiveDate) -> Option<String> {
    let birth_year = birthday.year();
    let today_year = Utc::now().naive_utc().year();
    if birth_year < 1900 || birth_year > today_year + 1 {
        return None;
    }
    let age = occurrence_year - birth_year;
    if age <= 0 {
        return None;
    }
    Some(age.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_contact(name: &str, bday: Option<NaiveDate>) -> Contact {
        Contact {
            id: format!("c-{name}"),
            list_id: "list-x".into(),
            display_name: name.into(),
            given_name: None,
            family_name: None,
            organization: None,
            emails: Vec::new(),
            phone_numbers: Vec::new(),
            birthday: bday,
            notes: None,
            members: None,
            has_photo: false,
            addresses: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: None,
        }
    }

    #[test]
    fn calendar_id_round_trips_through_prefix_helpers() {
        let id = birthday_calendar_id("list-1234");
        assert_eq!(id, "aperio-birthdays:list-1234");
        assert!(is_birthday_calendar_id(&id));
        assert_eq!(underlying_contact_list_id(&id), Some("list-1234"));
        assert!(!is_birthday_calendar_id("calendar-x"));
        assert!(underlying_contact_list_id("calendar-x").is_none());
    }

    #[test]
    fn events_for_contacts_emits_one_per_year_in_range() {
        let contacts = vec![make_contact(
            "Max",
            Some(NaiveDate::from_ymd_opt(1985, 4, 17).unwrap()),
        )];
        let start = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59).unwrap();
        let range = DateRange::new(start, end);
        let events = events_for_contacts(contacts, "cal", range);
        assert_eq!(events.len(), 3);
        for ev in &events {
            assert_eq!(ev.calendar_id, "cal");
            assert!(ev.all_day);
            assert_eq!(ev.title, "Max");
            let age: i32 = ev.description.as_deref().unwrap().parse().unwrap();
            assert!((39..=41).contains(&age));
        }
        let years: Vec<_> = events
            .iter()
            .map(|e| e.id.rsplit(':').next().unwrap().to_string())
            .collect();
        assert!(years.contains(&"2024".to_string()));
        assert!(years.contains(&"2026".to_string()));
    }

    #[test]
    fn events_for_contacts_skips_feb_29_on_non_leap_years() {
        let contacts = vec![make_contact(
            "Leapling",
            Some(NaiveDate::from_ymd_opt(2000, 2, 29).unwrap()),
        )];
        let start = Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2025, 12, 31, 23, 59, 59).unwrap();
        let events = events_for_contacts(contacts, "cal", DateRange::new(start, end));
        assert_eq!(events.len(), 1);
        assert!(events[0].id.ends_with(":2024"));
    }

    #[test]
    fn events_for_contacts_drops_contacts_without_birthday() {
        let contacts = vec![
            make_contact(
                "Has-Birthday",
                Some(NaiveDate::from_ymd_opt(1990, 6, 1).unwrap()),
            ),
            make_contact("No-Birthday", None),
        ];
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59).unwrap();
        let events = events_for_contacts(contacts, "cal", DateRange::new(start, end));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Has-Birthday");
    }

    #[test]
    fn events_for_contacts_skips_dates_outside_range() {
        let contacts = vec![make_contact(
            "Out-Of-Range",
            Some(NaiveDate::from_ymd_opt(1995, 1, 5).unwrap()),
        )];
        let start = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 8, 31, 23, 59, 59).unwrap();
        let events = events_for_contacts(contacts, "cal", DateRange::new(start, end));
        assert!(events.is_empty());
    }

    #[test]
    fn synthesise_calendar_is_read_only_with_birthday_colour() {
        let cal = synthesise_calendar("list-id", "Family");
        assert!(cal.read_only);
        assert_eq!(cal.name, "Birthdays – Family");
        assert_eq!(cal.id, "aperio-birthdays:list-id");
        assert_eq!(cal.color.as_ref().unwrap().hex, BIRTHDAY_LAYER_COLOR_HEX);
    }

    #[test]
    fn birthday_description_emits_age_when_birth_year_is_sensible() {
        let desc = birthday_description(2026, &NaiveDate::from_ymd_opt(1985, 4, 17).unwrap());
        assert_eq!(desc.as_deref(), Some("41"));
    }

    #[test]
    fn birthday_description_omits_age_for_placeholder_year() {
        let desc = birthday_description(2026, &NaiveDate::from_ymd_opt(1604, 5, 1).unwrap());
        assert!(desc.is_none());
    }
}
