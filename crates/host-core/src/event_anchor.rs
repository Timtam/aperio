//! How a row that names a foreign event finds it again.
//!
//! Aperio keeps several things ABOUT events that live on a provider: which
//! events mean one appointment (`event_groups`, migration 0035), the reminders
//! it keeps to itself (`event_local_reminders`, 0043), the colour a calendar
//! cannot store (`event_color_overrides`, 0026 + 0044), and which meeting it
//! minted for an event (`event_meetings`, 0034 + 0045). Each names its event by
//! the provider's id — and those ids change underneath us. A re-bootstrap
//! remints them, moving an event between calendars remints it, Exchange bakes a
//! change token into its ids and remints them unprompted.
//!
//! A row that stored the id alone would then point at nothing, in silence. So
//! each stores a SIGNATURE beside it — the title and start the appointment had,
//! and the calendar it lives in — and is repaired where the events that prove
//! it are already in hand.
//!
//! The tables differ; the DECISION does not, and it is the subtle part. This
//! module holds it once: what to refresh, what to repoint, and — mostly — what
//! to leave alone. Each caller applies the answer to its own table.

use chrono::{DateTime, Utc};

use cal_core::Event;

use crate::reminders::series_master_id;

/// One stored row, reduced to what deciding needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchored {
    /// The event id the row is stored under — its key, whatever the table
    /// calls it.
    pub event_id: String,
    /// The calendar the row believes its event lives in. Empty means the row
    /// predates its table's signature and has never been seen since.
    pub calendar_id: String,
    /// The title the appointment had. Half of the signature.
    pub title: String,
    /// The start it had, RFC 3339. The other half.
    pub starts_at: String,
}

/// What to do about one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repair {
    /// The row's event is here. Write down what it looks like NOW, so a later
    /// rename or move cannot make it unfindable.
    Refresh {
        event_id: String,
        calendar_id: String,
        title: String,
        starts_at: String,
    },
    /// The row's id answers to nothing, and exactly one appointment matches
    /// what it remembers. Point it there.
    Repoint { event_id: String, to: String },
}

/// Decide what to repair for one calendar's rows, given the events just
/// fetched for `range`.
///
/// Nothing is decided lightly, because every repair is silent: a row moved onto
/// the wrong appointment says nothing, and neither does one that quietly stops
/// matching. A repair is only proposed when it cannot be anything else:
///
///   * **Only this calendar's rows may be called vanished.** The same
///     appointment routinely exists in several calendars — once where
///     colleagues see it, once copied where it is read aloud. Without this,
///     rendering one calendar would find the copy in the other and take the
///     row with it, and alternating renders would pass it back and forth
///     forever.
///   * **Ambiguity repairs nothing.** Two appointments of the same name at the
///     same time leave the row where it is. A row that cannot be resolved is
///     visible and fixable; one silently attached to the wrong appointment is
///     neither.
///   * **A row is only judged against a batch that could contain it.** The
///     window is `range` widened by the events actually in hand — a recurring
///     master's start can be months before the week being rendered, and those
///     are exactly the rows these tables are keyed for. Outside it, "not here"
///     means nothing.
///
/// A row counts as present under EITHER id its event answers to: the series
/// master, which is what the write paths bind to, and the row's own id, since
/// a provider-sent override of one occurrence carries the master's in front of
/// the marker. Recognising both keeps such a row where it is instead of
/// quietly reclassifying it as a whole-series binding.
pub fn plan_repairs(
    rows: &[Anchored],
    calendar_id: &str,
    events: &[Event],
    range: (DateTime<Utc>, DateTime<Utc>),
) -> Vec<Repair> {
    if rows.is_empty() || events.is_empty() {
        return Vec::new();
    }
    let normalize = |s: &str| s.trim().to_lowercase();
    let mut present: std::collections::HashMap<&str, &Event> = std::collections::HashMap::new();
    for ev in events {
        present.insert(ev.id.as_str(), ev);
        present.entry(series_master_id(&ev.id)).or_insert(ev);
    }
    // What this batch can speak for.
    let (mut lower, mut upper) = range;
    for ev in events {
        lower = lower.min(ev.start);
        upper = upper.max(ev.start);
    }

    let mut out = Vec::new();
    for row in rows {
        if let Some(ev) = present.get(row.event_id.as_str()) {
            let starts_at = ev.start.to_rfc3339();
            if ev.title != row.title || starts_at != row.starts_at || row.calendar_id != calendar_id
            {
                out.push(Repair::Refresh {
                    event_id: row.event_id.clone(),
                    calendar_id: calendar_id.to_string(),
                    title: ev.title.clone(),
                    starts_at,
                });
            }
            continue;
        }
        // Another calendar's row is not missing — it was never in this batch.
        if row.calendar_id != calendar_id {
            continue;
        }
        let wanted_title = normalize(&row.title);
        if wanted_title.is_empty() {
            // No signature at all (a row from before its table had one, or an
            // appointment with no title): nothing to match on.
            continue;
        }
        let Ok(wanted_start) = row.starts_at.parse::<DateTime<Utc>>() else {
            continue;
        };
        if wanted_start < lower || wanted_start > upper {
            continue;
        }
        // Collapse to the series before asking whether the answer is unique: a
        // master and a provider-sent override of one of its occurrences are
        // two rows for ONE appointment.
        let mut candidates: Vec<&str> = events
            .iter()
            .filter(|ev| normalize(&ev.title) == wanted_title && ev.start == wanted_start)
            .map(|ev| series_master_id(&ev.id))
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        let [found] = candidates.as_slice() else {
            continue;
        };
        out.push(Repair::Repoint {
            event_id: row.event_id.clone(),
            to: (*found).to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn event(id: &str, calendar_id: &str, title: &str, start: DateTime<Utc>) -> Event {
        Event {
            id: id.into(),
            calendar_id: calendar_id.into(),
            title: title.into(),
            description: None,
            location: None,
            start,
            end: start + Duration::hours(1),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: start,
            updated_at: start,
            etag: None,
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
        }
    }

    fn row(event_id: &str, calendar_id: &str, title: &str, start: DateTime<Utc>) -> Anchored {
        Anchored {
            event_id: event_id.into(),
            calendar_id: calendar_id.into(),
            title: title.into(),
            starts_at: start.to_rfc3339(),
        }
    }

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, day, 9, 0, 0).unwrap()
    }

    fn week_of(day: u32) -> (DateTime<Utc>, DateTime<Utc>) {
        (at(day) - Duration::days(1), at(day) + Duration::days(1))
    }

    #[test]
    fn a_present_row_is_refreshed_only_when_something_differs() {
        let start = at(1);
        let events = [event("ev", "cal", "Zahnarzt", start)];
        // Signature already correct: nothing to do.
        assert!(plan_repairs(
            &[row("ev", "cal", "Zahnarzt", start)],
            "cal",
            &events,
            week_of(1)
        )
        .is_empty());
        // Renamed since: the row learns the new name where it stands.
        assert_eq!(
            plan_repairs(
                &[row("ev", "cal", "Alt", start)],
                "cal",
                &events,
                week_of(1)
            ),
            vec![Repair::Refresh {
                event_id: "ev".into(),
                calendar_id: "cal".into(),
                title: "Zahnarzt".into(),
                starts_at: start.to_rfc3339(),
            }],
        );
    }

    /// A row that has never been seen (no calendar, no signature) is anchored
    /// the first time its event turns up.
    #[test]
    fn an_unanchored_row_is_adopted_when_its_event_appears() {
        let start = at(1);
        assert_eq!(
            plan_repairs(
                &[row("ev", "", "", start)],
                "cal",
                &[event("ev", "cal", "Zahnarzt", start)],
                week_of(1),
            ),
            vec![Repair::Refresh {
                event_id: "ev".into(),
                calendar_id: "cal".into(),
                title: "Zahnarzt".into(),
                starts_at: start.to_rfc3339(),
            }],
        );
    }

    #[test]
    fn a_reminted_id_is_found_again() {
        let start = at(1);
        assert_eq!(
            plan_repairs(
                &[row("old", "cal", "Zahnarzt", start)],
                "cal",
                &[event("new", "cal", " zahnarzt ", start)],
                week_of(1),
            ),
            vec![Repair::Repoint {
                event_id: "old".into(),
                to: "new".into(),
            }],
        );
    }

    /// The copy of one appointment in another calendar is a different event.
    #[test]
    fn another_calendars_row_is_never_touched() {
        let start = at(1);
        assert!(plan_repairs(
            &[row("work:evt", "work", "Jour fixe", start)],
            "private",
            &[event("private:evt", "private", "Jour fixe", start)],
            week_of(1),
        )
        .is_empty());
    }

    #[test]
    fn ambiguity_repairs_nothing() {
        let start = at(1);
        assert!(plan_repairs(
            &[row("old", "cal", "Standup", start)],
            "cal",
            &[
                event("a", "cal", "Standup", start),
                event("b", "cal", "Standup", start),
            ],
            week_of(1),
        )
        .is_empty());
    }

    /// A master and a provider-sent override of one of its occurrences are ONE
    /// appointment, so they do not read as an ambiguous answer.
    #[test]
    fn a_master_and_its_own_override_are_one_answer() {
        let start = at(1);
        assert_eq!(
            plan_repairs(
                &[row("old", "cal", "Standup", start)],
                "cal",
                &[
                    event("master", "cal", "Standup", start),
                    event(
                        "master::rid::2026-06-01T09:00:00+00:00",
                        "cal",
                        "Standup",
                        start
                    ),
                ],
                week_of(1),
            ),
            vec![Repair::Repoint {
                event_id: "old".into(),
                to: "master".into(),
            }],
        );
    }

    /// A row bound to a single occurrence stays where it is rather than being
    /// quietly promoted to the whole series.
    #[test]
    fn a_row_bound_to_an_occurrence_is_left_on_it() {
        let start = at(1);
        let occurrence = "master::rid::2026-06-01T09:00:00+00:00";
        assert!(plan_repairs(
            &[row(occurrence, "cal", "Standup", start)],
            "cal",
            &[event(occurrence, "cal", "Standup", start)],
            week_of(1),
        )
        .is_empty());
    }

    #[test]
    fn a_row_outside_what_the_batch_covers_is_left_alone() {
        let start = at(1);
        // The rendered week is far away, and holds an unrelated appointment
        // that happens to share the name.
        assert!(plan_repairs(
            &[row("old", "cal", "Standup", start)],
            "cal",
            &[event("other", "cal", "Standup", at(20))],
            week_of(20),
        )
        .is_empty());
    }

    /// The window includes the events actually in hand, or a recurring
    /// master — whose start can be months before the week being rendered —
    /// could never be repaired at all.
    #[test]
    fn the_batchs_own_events_widen_the_window() {
        let dtstart = Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0).unwrap();
        assert_eq!(
            plan_repairs(
                &[row("old", "cal", "Standup", dtstart)],
                "cal",
                &[event("new", "cal", "Standup", dtstart)],
                week_of(20),
            ),
            vec![Repair::Repoint {
                event_id: "old".into(),
                to: "new".into(),
            }],
        );
    }
}
