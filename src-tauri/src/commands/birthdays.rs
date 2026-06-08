//! Birthday calendars (DESIGN.md §10.3).
//!
//! One synthesised, read-only calendar per contact list — populated
//! from the contacts' `birthday` field. Surfaced through the same
//! `list_calendars` and `get_events` command paths the real
//! calendars use, so every view (Week, Day, Month, Year, Agenda)
//! picks them up without per-view changes.
//!
//! Design properties:
//!
//!   - **No persistence.** The events aren't stored anywhere — they
//!     are computed on each `get_events` call from the live
//!     contacts list. Renaming a contact or editing a birthday in
//!     the address book is reflected on the next render with no
//!     migration / sync step.
//!   - **`read_only = true`.** The frontend's existing read-only
//!     handling hides the rename / delete affordances for the
//!     calendar AND the chip-context menu's edit / move actions
//!     for each event. The user can still uncheck the layer in
//!     the sidebar to hide it.
//!   - **Synthetic ids.** Calendar id =
//!     `aperio-birthdays:{contact_list_id}`. Event id =
//!     `aperio-birthday:{contact_id}:{year}`. The
//!     `aperio-` prefix makes them unambiguously synthesised at
//!     the command-layer (every real adapter mints opaque ids
//!     that never start that way).
//!   - **Yearly expansion.** Instead of emitting one master event
//!     with an `RRULE:FREQ=YEARLY` and relying on the expansion
//!     engine, we materialise one event per occurrence year that
//!     overlaps the requested range. Cheaper, simpler, and
//!     avoids the engine having to know "this is a synthesised
//!     master" when computing reminders / drag-drop / chip
//!     navigation.
//!
//! Hidden lists: a contact list with zero contacts that carry a
//! birthday produces no birthday calendar at all — there's nothing
//! useful to show. The aggregator filters those out before they
//! reach the sidebar.

use cal_adapter_local::LocalAdapter;
use cal_core::{Calendar, ColorSource, ContactsFeature, ContainerColor, DateRange, Event};
use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use std::sync::Arc;

use crate::cache::CacheStore;
use crate::registry::{AdapterRegistry, LOCAL_ID};

/// Prefix every synthesised birthday calendar id carries. The
/// frontend doesn't strictly need to know about it — read-only
/// is enough to gate edit affordances — but exposing the prefix
/// keeps the contract explicit for future tools (e.g. plugin
/// authors who want to skip these in a sync extension).
pub const BIRTHDAY_CALENDAR_PREFIX: &str = "aperio-birthdays:";

/// Reasonable default colour for a birthday layer: a warm pink
/// distinct enough from the typical default-blue calendar palette
/// that the layer reads as "special" at a glance, but muted enough
/// not to dominate the grid. Per DESIGN.md §10.3 the layer reads as
/// "Geburtstage – {list name}"; the colour is consistent across
/// every synthesised layer rather than inheriting from the
/// contact list, which would conflict with the user's chosen
/// colour for the address book itself.
const BIRTHDAY_LAYER_COLOR_HEX: &str = "#e91e63";

/// Build the calendar id for a contact list. Stable across runs —
/// the underlying contact list id never changes — so the user's
/// per-calendar visibility selection persists.
pub fn birthday_calendar_id(contact_list_id: &str) -> String {
    format!("{BIRTHDAY_CALENDAR_PREFIX}{contact_list_id}")
}

/// Inverse: extract the contact list id from a synthesised
/// calendar id, or `None` for non-birthday ids.
pub fn underlying_contact_list_id(calendar_id: &str) -> Option<&str> {
    calendar_id.strip_prefix(BIRTHDAY_CALENDAR_PREFIX)
}

pub fn is_birthday_calendar_id(calendar_id: &str) -> bool {
    calendar_id.starts_with(BIRTHDAY_CALENDAR_PREFIX)
}

/// Walk every contact list (local + registered external adapters)
/// and emit a synthetic birthday calendar for each one that has
/// at least one contact with a birthday set. Returns `(calendar,
/// account_id)` pairs so the caller can stamp the registry's
/// account-routing alongside the listing.
///
/// External contacts are read from the host SNAPSHOT CACHE, never the
/// adapter: a birthday layer is a convenience derived from already-synced
/// contacts and must never trigger a network fetch from inside
/// `list_calendars`. Probing each external book over the wire — the EWS
/// Global Address List alone is ~2000 contacts behind a slow ResolveNames
/// walk — would block the whole calendar listing for tens of seconds on
/// every startup, even though the calendars themselves are cached. The
/// layer simply reflects whatever contacts are in the cache; it updates on
/// the next listing once a contacts background refresh repopulates it.
pub async fn list_birthday_calendars(
    local: &LocalAdapter,
    registry: &Arc<AdapterRegistry>,
    cache: &Arc<CacheStore>,
) -> Vec<(Calendar, String)> {
    let mut out = Vec::new();

    // Local adapter — in-process SQLite, cheap to read directly.
    match local.list_contact_lists().await {
        Ok(lists) => {
            for list in lists {
                if list_has_birthdays(local, &list.id).await {
                    out.push((
                        synthesise_calendar(&list.id, &list.name),
                        LOCAL_ID.to_string(),
                    ));
                }
            }
        }
        Err(err) => {
            tracing::warn!(?err, "birthday: local list_contact_lists failed");
        }
    }

    // External contact accounts — read the cached lists + contacts only.
    for (account_id, _adapter) in registry.snapshot_contact_adapters() {
        let lists = cache.read_contact_lists(&account_id).unwrap_or_default();
        for list in lists {
            let contacts = cache
                .read_contacts(&account_id, &list.id)
                .unwrap_or_default();
            if contacts.iter().any(|c| c.birthday.is_some()) {
                out.push((
                    synthesise_calendar(&list.id, &list.name),
                    account_id.clone(),
                ));
            }
        }
    }
    out
}

/// Compute the birthday events that fall within `range` for the
/// underlying contact list of `synthesised_calendar_id`. Routing
/// happens via the prefix — anything that isn't a birthday
/// calendar id returns `None` (the caller falls back to the
/// regular adapter path).
pub async fn synthesise_birthday_events(
    local: &LocalAdapter,
    registry: &Arc<AdapterRegistry>,
    cache: &Arc<CacheStore>,
    synthesised_calendar_id: &str,
    range: DateRange,
) -> Option<Vec<Event>> {
    let list_id = underlying_contact_list_id(synthesised_calendar_id)?;
    // The list id alone tells us where to look — every contact list id is
    // unique across local + external adapters because each mints its own
    // (UUID for local, server URL for CalDAV / addressbooks). Try the local
    // adapter first (in-process), then the host snapshot CACHE for external
    // books — never the adapter, so rendering a birthday layer can't block
    // on a network contact fetch.
    if let Ok(contacts) = local.get_contacts(list_id).await {
        if !contacts.is_empty() {
            return Some(events_for_contacts(
                contacts,
                synthesised_calendar_id,
                range,
            ));
        }
    }
    // Resolve the owning account via the route map; fall back to scanning
    // every contact account's cache if the route isn't registered yet.
    let accounts: Vec<String> = registry
        .account_for_contact_list(list_id)
        .map(|a| vec![a])
        .unwrap_or_else(|| {
            registry
                .snapshot_contact_adapters()
                .into_iter()
                .map(|(account_id, _adapter)| account_id)
                .collect()
        });
    for account_id in accounts {
        let contacts = cache
            .read_contacts(&account_id, list_id)
            .unwrap_or_default();
        if !contacts.is_empty() {
            return Some(events_for_contacts(
                contacts,
                synthesised_calendar_id,
                range,
            ));
        }
    }
    // List exists but has no cached contacts. Empty Vec rather than None so
    // the caller treats this as "successful read with zero results" — the
    // typical empty calendar UX.
    Some(Vec::new())
}

// ── Internals ──────────────────────────────────────────────────────────

fn synthesise_calendar(contact_list_id: &str, list_name: &str) -> Calendar {
    Calendar {
        color_label: None,
        supports_scheduling: false,
        // Synthetic, read-only birthday layer — no per-event color to store.
        supports_event_color: false,
        id: birthday_calendar_id(contact_list_id),
        // English default; the user can re-localise via the
        // existing local-override path (DESIGN.md §6.5) since
        // `read_only = true` triggers the "fallback to local
        // override" branch in `rename_container`.
        name: format!("Birthdays – {list_name}"),
        color: Some(ContainerColor {
            hex: BIRTHDAY_LAYER_COLOR_HEX.to_string(),
            source: ColorSource::Native,
        }),
        read_only: true,
        default_sound: None,
    }
}

async fn list_has_birthdays(adapter: &LocalAdapter, list_id: &str) -> bool {
    match adapter.get_contacts(list_id).await {
        Ok(contacts) => contacts.iter().any(|c| c.birthday.is_some()),
        Err(_) => false,
    }
}

/// Materialise one all-day event per (contact, occurrence-year)
/// overlapping `range`. We use yearly expansion in-place rather
/// than emitting a master with RRULE so the rest of the codebase
/// (chip rendering, ARIA labels, navigation) doesn't have to
/// know "this is a virtual recurrence".
fn events_for_contacts(
    contacts: Vec<cal_core::Contact>,
    calendar_id: &str,
    range: DateRange,
) -> Vec<Event> {
    let mut out = Vec::new();
    let start_year = range.start.naive_utc().year();
    let end_year = range.end.naive_utc().year();
    // Cap the year span at a reasonable upper bound. Asking for
    // ten years of birthdays is fine; asking for a thousand
    // would balloon the result set without anyone noticing in
    // the UI. Real-world ranges max out at the year view (one
    // year) so this guard is defensive against the agenda view
    // ever growing a "next decade" preset.
    let safe_end_year = end_year.min(start_year + 100);

    for contact in contacts {
        let Some(bday) = contact.birthday else {
            continue;
        };
        for year in start_year..=safe_end_year {
            // Feb 29 on a non-leap year falls off the calendar.
            // We don't shift it to Feb 28 or Mar 1 — the user's
            // address book has the canonical date and we render
            // it on years that have it. Same tradeoff iOS Contacts
            // makes.
            let Some(date) = NaiveDate::from_ymd_opt(year, bday.month(), bday.day()) else {
                continue;
            };
            let Some(midnight) = date.and_hms_opt(0, 0, 0) else {
                continue;
            };
            let start = Utc.from_utc_datetime(&midnight);
            // All-day events span midnight-to-midnight per the
            // existing convention in WeekView / DayView; the
            // end is start + 1 day. The 24-hour window is what
            // the frontend uses to render the chip in the
            // all-day lane.
            let end = start + chrono::Duration::days(1);
            // Skip occurrences that don't overlap the requested
            // range. The bounds check is inclusive on both ends
            // — the agenda view's range often includes the
            // start of the next day, and Year view spans Dec 31
            // 23:59:59 to Jan 1 00:00:00 of the next year.
            if end <= range.start || start >= range.end {
                continue;
            }
            out.push(Event {
                send_invitations: false,
                id: format!("aperio-birthday:{}:{}", contact.id, year,),
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

/// The age (in years) the contact reaches on this birthday occurrence,
/// as a plain number string (e.g. `"39"`). The frontend renders the
/// localized "Wird X Jahre alt" line from it — UI prose lives in the
/// app's i18n, never baked into a synthesised event here. Returns `None`
/// when the birth year is missing or a clearly bogus placeholder (RFC
/// 6350 leaves pre-1900 dates underdefined) or the age isn't positive;
/// the title (the contact's name) then stands alone.
fn birthday_description(occurrence_year: i32, birthday: &NaiveDate) -> Option<String> {
    let birth_year = birthday.year();
    let today_year = Utc::now().naive_utc().year();
    if birth_year < 1900 || birth_year > today_year + 1 {
        // Year omitted in the source vCard or clearly bogus.
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
    use cal_core::Contact;
    use chrono::NaiveDate;

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
        // 2024, 2025, 2026 → three occurrences.
        assert_eq!(events.len(), 3);
        for ev in &events {
            assert_eq!(ev.calendar_id, "cal");
            assert!(ev.all_day);
            assert_eq!(ev.title, "Max");
            // Description carries the age as a bare number string when the
            // birth year is sensible (2024-1985=39, 2025→40, 2026→41) — the
            // frontend renders the localized "Wird N Jahre alt" from it.
            let age: i32 = ev.description.as_deref().unwrap().parse().unwrap();
            assert!((39..=41).contains(&age));
        }
        // Years 2024 / 2025 / 2026 appear in id.
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
        // 2023 and 2025 are non-leap → only 2024 should produce
        // an event.
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
        // Birthday January 5; range June through August → zero.
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
        assert_eq!(cal.color.as_ref().unwrap().hex, BIRTHDAY_LAYER_COLOR_HEX,);
    }

    #[test]
    fn birthday_description_emits_age_when_birth_year_is_sensible() {
        let desc = birthday_description(2026, &NaiveDate::from_ymd_opt(1985, 4, 17).unwrap());
        assert_eq!(desc.as_deref(), Some("41"));
    }

    #[test]
    fn birthday_description_omits_age_for_placeholder_year() {
        // vCard with no birth year sometimes encodes as year 1604
        // or similar sentinel; we skip the age in that case.
        let desc = birthday_description(2026, &NaiveDate::from_ymd_opt(1604, 5, 1).unwrap());
        assert!(desc.is_none());
    }
}
