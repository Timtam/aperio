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

use crate::Event;

/// The id of the SERIES this event belongs to.
///
/// A provider-sent override for one modified occurrence carries the master's
/// id in front of the marker; everything keyed per event — private reminders,
/// colour overrides, group membership — is keyed by the master, because a
/// recurring appointment is one appointment. The marker is minted by the
/// CalDAV adapter (`RECURRENCE_ID_MARKER` in `adapter-caldav`'s mapping).
///
/// # It is NOT the whole of the frontend's `seriesIdOf`
///
/// `shared/recurrence.ts` answers the same question for TWO kinds of row, and
/// only one of them exists here. An EXPANDED OCCURRENCE — a synthetic row the
/// frontend mints for each turn of a recurrence — carries a `series_id` field,
/// and `seriesIdOf` reads it. [`crate::Event`] has no such field, because the
/// core and the host never expand: their batches hold masters and
/// provider-sent overrides, nothing else.
///
/// So this is the override half alone, and the difference is load-bearing
/// rather than cosmetic. A stored signature that describes one OCCURRENCE of a
/// series is findable in a frontend's expanded batch and is not findable in an
/// unexpanded one — which is why anchoring a table of such signatures has to
/// happen where the expansion is.
pub fn series_master_id(event_id: &str) -> &str {
    match event_id.find("::rid::") {
        Some(idx) => &event_id[..idx],
        None => event_id,
    }
}

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
///
/// An ALL-DAY candidate answers on the day, a timed one on the instant — see
/// [`starts_the_same`].
pub fn plan_repairs(
    rows: &[Anchored],
    calendar_id: &str,
    events: &[Event],
    range: (DateTime<Utc>, DateTime<Utc>),
) -> Vec<Repair> {
    if rows.is_empty() || events.is_empty() {
        return Vec::new();
    }
    // One rule, next door in `event_group`. This was a local closure that only
    // trimmed the ends, while the frontends' copy also collapsed inner
    // whitespace — so a title whose spacing changed after joining stopped
    // being re-findable here while still reading as "the same" there. See
    // [`crate::normalized_title`].
    let normalize = crate::normalized_title;
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
            // Compared as an INSTANT, never as text. One moment has three
            // spellings in this codebase — chrono's `to_rfc3339` writes
            // `…09:00:00+00:00`, serde's `DateTime` writes `…09:00:00Z`, and a
            // frontend's `toISOString` writes `…09:00:00.000Z` — and a
            // signature is written by whichever side last touched the row.
            // Comparing the text therefore called every frontend-written row
            // stale on sight and rewrote it, once per row, for no change at
            // all.
            //
            // A signature that will not parse is not a signature: writing a
            // readable one over it is the repair, so it counts as differing.
            let start_matches = row
                .starts_at
                .parse::<DateTime<Utc>>()
                .is_ok_and(|stored| stored == ev.start);
            if ev.title != row.title || !start_matches || row.calendar_id != calendar_id {
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
            .filter(|ev| normalize(&ev.title) == wanted_title && starts_the_same(ev, wanted_start))
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

/// Whether this event starts when a stored signature says its appointment did.
///
/// An all-day event agrees on the DAY, a timed one on the instant. Without
/// this, an all-day row stops being findable the moment anything shifts its
/// stored instant — and something does: an all-day start is LOCAL midnight
/// expressed as a UTC instant (`adapter-caldav`'s mapping says so at the top),
/// so the same birthday is a different instant either side of a DST boundary
/// or after the user moves country.
///
/// # Two things that look like bugs and are not
///
/// **The CANDIDATE's flag decides for both sides.** A signature is a title and
/// a start; it cannot say whether the appointment it describes was all-day,
/// because `starts_at` is an instant either way. So a timed row can in
/// principle match an all-day event on the same day. The uniqueness rule
/// contains it — that only bites when the day holds exactly one event of that
/// name and it is all-day — and closing it properly means widening every
/// signature, which is a migration for a case nobody has hit. The frontend
/// half this was ported from carries the same limit, written down the same
/// way.
///
/// **The day is the UTC day, which is not always the day the user sees.** For
/// a reader east of UTC, local midnight on the 10th is the 9th at 22:00Z, so
/// this compares "the 9th". That is correct here because it is used as a KEY,
/// not as a date: both sides of the comparison are derived from stored
/// instants the same way, so they agree. Deriving the calendar day the user
/// sees would need the device's timezone, which the core may never read.
fn starts_the_same(ev: &Event, wanted: DateTime<Utc>) -> bool {
    if ev.all_day {
        ev.start.date_naive() == wanted.date_naive()
    } else {
        ev.start == wanted
    }
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

    /// Everything keyed per event is keyed by the SERIES: a provider-sent
    /// override for one occurrence is the same appointment.
    #[test]
    fn the_series_master_id_drops_an_occurrence_suffix() {
        assert_eq!(series_master_id("abc"), "abc");
        assert_eq!(
            series_master_id("https://dav/e.ics|uid-1::rid::2026-06-15T09:00:00+00:00"),
            "https://dav/e.ics|uid-1"
        );
    }

    /// The half this function does NOT do, written down so the gap is a
    /// decision rather than an oversight.
    ///
    /// The frontend's `seriesIdOf` answers for two kinds of row. The second is
    /// an EXPANDED OCCURRENCE, which the frontend mints per turn of a
    /// recurrence with an id of the form `master@<instant>` and a `series_id`
    /// field. This function would hand such an id straight back — there is no
    /// `::rid::` in it and no field to read — and that is correct here,
    /// because no batch the core is ever handed contains one: the core and the
    /// host do not expand.
    ///
    /// It is pinned because the consequence is silent. Feed an expanded batch
    /// to [`plan_repairs`] and every occurrence reads as its own series, so a
    /// recurring appointment's rows look like ambiguity and nothing is
    /// repaired at all.
    #[test]
    fn an_expanded_occurrence_id_is_not_its_series() {
        assert_eq!(
            series_master_id("https://dav/e.ics|uid-1@2026-06-15T09:00:00.000Z"),
            "https://dav/e.ics|uid-1@2026-06-15T09:00:00.000Z"
        );
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

    /// The same moment, spelled the three ways this codebase spells it, is one
    /// moment.
    ///
    /// A signature is written by whichever side last touched the row, and the
    /// sides do not agree on the text: chrono's `to_rfc3339` writes `+00:00`,
    /// serde's `DateTime` writes `Z`, and a frontend's `toISOString` writes
    /// `.000Z`. Comparing the text called every frontend-written row stale the
    /// first time it was looked at and rewrote it — a write nobody asked for,
    /// on every table anchored this way.
    #[test]
    fn one_instant_spelled_three_ways_is_not_a_change() {
        let start = at(1);
        let events = [event("ev", "cal", "Zahnarzt", start)];
        for spelling in [
            "2026-06-01T09:00:00+00:00",
            "2026-06-01T09:00:00Z",
            "2026-06-01T09:00:00.000Z",
            // Same moment, said in another zone. Still the same moment.
            "2026-06-01T11:00:00+02:00",
        ] {
            let row = Anchored {
                event_id: "ev".into(),
                calendar_id: "cal".into(),
                title: "Zahnarzt".into(),
                starts_at: spelling.into(),
            };
            assert!(
                plan_repairs(&[row], "cal", &events, week_of(1)).is_empty(),
                "{spelling} is the event's own start and must not read as a change",
            );
        }
    }

    /// A signature that will not parse is not a signature. Writing a readable
    /// one over it is the repair, not something to leave alone.
    #[test]
    fn an_unreadable_start_is_rewritten() {
        let start = at(1);
        let events = [event("ev", "cal", "Zahnarzt", start)];
        let row = Anchored {
            event_id: "ev".into(),
            calendar_id: "cal".into(),
            title: "Zahnarzt".into(),
            starts_at: "irgendwann".into(),
        };
        assert_eq!(
            plan_repairs(&[row], "cal", &events, week_of(1)),
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

    fn all_day_event(id: &str, calendar_id: &str, title: &str, start: DateTime<Utc>) -> Event {
        let mut ev = event(id, calendar_id, title, start);
        ev.all_day = true;
        ev
    }

    /// An all-day appointment agrees on the DAY, because its instant moves
    /// without the appointment moving.
    ///
    /// An all-day start is local midnight expressed as a UTC instant, so the
    /// same birthday is a different instant either side of a DST boundary or
    /// after the user moves country. Compared as instants, such a row simply
    /// stops being findable the next time its id is reminted — silently, which
    /// is the failure the signature exists to prevent.
    #[test]
    fn an_all_day_row_is_found_on_its_day() {
        // Stored at 00:00, the event now says 01:00 — the same day, a
        // different instant.
        let stored = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let now_at = Utc.with_ymd_and_hms(2026, 6, 1, 1, 0, 0).unwrap();
        assert_eq!(
            plan_repairs(
                &[row("old", "cal", "Geburtstag Anna", stored)],
                "cal",
                &[all_day_event("new", "cal", "Geburtstag Anna", now_at)],
                week_of(1),
            ),
            vec![Repair::Repoint {
                event_id: "old".into(),
                to: "new".into(),
            }],
            "an all-day copy on the same day is the same appointment",
        );
    }

    /// The day rule is for all-day events ONLY. A timed appointment an hour
    /// later is a different appointment, and reading the two as one would
    /// attach a row to something nobody pointed it at.
    #[test]
    fn a_timed_row_still_answers_only_to_its_instant() {
        let stored = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let now_at = Utc.with_ymd_and_hms(2026, 6, 1, 1, 0, 0).unwrap();
        assert!(
            plan_repairs(
                &[row("old", "cal", "Zahnarzt", stored)],
                "cal",
                &[event("new", "cal", "Zahnarzt", now_at)],
                week_of(1),
            )
            .is_empty(),
            "same day, different hour, not all-day: not the same appointment",
        );
    }

    /// Widening to the day widens what can be ambiguous, and ambiguity still
    /// repairs nothing. Two all-day events of one name on one day leave the
    /// row where it is.
    #[test]
    fn two_all_day_copies_on_one_day_are_still_ambiguous() {
        let stored = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        assert!(plan_repairs(
            &[row("old", "cal", "Betriebsausflug", stored)],
            "cal",
            &[
                all_day_event("a", "cal", "Betriebsausflug", stored),
                all_day_event(
                    "b",
                    "cal",
                    "Betriebsausflug",
                    Utc.with_ymd_and_hms(2026, 6, 1, 2, 0, 0).unwrap()
                ),
            ],
            week_of(1),
        )
        .is_empty());
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
