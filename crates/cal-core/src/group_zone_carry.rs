//! Carrying a series' zone change to its copies (DESIGN-series-time-zone.md,
//! section 23b and stage 13; decisions 143a to 146a, 120a).
//!
//! After a whole-series save that changed the zone of a series timed before
//! and after, the one carry question names the zone first among the changes
//! (143a). Each writable copy that is a timed series then moves onto the new
//! clock by ONE wall shift `S_C`, start, rule, cancelling exceptions and end
//! together ([`crate::series_clock_change`]'s mover); a time the new clock
//! skips refuses the move. This rule answers, for one copy, what that write
//! is, or why there is none.
//!
//! # How far a copy moves
//!
//! The copy is paired with the series by instants, never by its start day:
//!
//! - `y` is the series' first occurrence on its old pattern, and `x` the
//!   copy's occurrence on its own pattern that is nearest `y` and less than
//!   12 hours away (both patterns reach past their starts and ends). Then
//!   `S_C = wall_zA(x) + S_A − wall_zC(x)`: `x` read on the series' old clock,
//!   moved as the series' occurrences moved (`S_A`, the applied zone answers'
//!   `movedMinutes` plus any date or time move made after the choice, on the
//!   new clock), minus `x` read on the copy's own old clock. A true copy lands
//!   on the series; one in the other half-year comes to the series' hour and
//!   day move (144a); one at another hour keeps its distance (145a).
//! - Without such an `x`, a copy occurrence within 12 hours of the series'
//!   first NEW occurrence means the copy already sits where the series now
//!   sits: it keeps its instants.
//! - Otherwise its occurrences are not on the series' days, and it is
//!   refused. The 12 hours are Claude's technical choice (DESIGN, stage 13,
//!   for Toni's collected check).
//! - A rule with several times a day on either side pairs at the copy's own
//!   start, with no day check.
//!
//! # What it answers
//!
//! The copy's target clock is its own where that is the series' new clock
//! (both UTC, the same canonical zone, or the same rules from today on, 120a),
//! else the series' new zone. On its own clock with `S_C` zero and its length
//! right it is [`ZoneCarry::Unchanged`] and is not written. A shift equal to
//! the copy's own "as the rule says" or "as they fall now" gives exactly that
//! answer, so an identical copy ends identical to the series; any other moves
//! it as a whole, like a drag.
//!
//! A copy that is no series there, is all-day there, cannot store the zone,
//! is off the series' days or whose rule cannot move is refused with that
//! reason (146a); the surfaces still carry its title, location and
//! description. Row facts come before calendar facts, so the offer and the
//! report name the same reason.
//!
//! Pinned by `tests/fixtures/groupZoneCarry.json`; the surfaces replay it.

use chrono::{DateTime, Duration, NaiveDate, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::series_clock_change::{ClockMove, StartMove};
use crate::wall_clock::clock_of;
use crate::{
    canonical_zone, series_clock_change, series_clock_zone, ClockChangeAnswer, ClockChangeQuestion,
    ClockChangeRefusal, SeriesTimeZones,
};

/// A series as stored: its rule, start and zone.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SeriesFacts {
    /// The rule, with or without `RRULE:`.
    pub rrule: String,
    /// The start, an RFC 3339 instant.
    pub start: String,
    /// The zone name it stores; `None` for none (UTC).
    pub tzid: Option<String>,
}

/// A copy as its calendar stores it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct CopyFacts {
    /// Its rule; `None` or empty where it is no series there (a single
    /// event, a device calendar's row, an occurrence row).
    pub rrule: Option<String>,
    /// RFC 3339 instants.
    pub start: String,
    pub end: String,
    /// The instants its exceptions cancel.
    #[serde(default)]
    pub exceptions: Vec<String>,
    /// The zone name it stores; `None` for none (UTC).
    pub tzid: Option<String>,
    pub all_day: bool,
}

/// One question: the series before and after its save, how its occurrences
/// moved, and one copy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ZoneCarryQuestion {
    /// The series as it was stored before the save.
    pub anchor_before: SeriesFacts,
    /// The series as it was saved; `tzid` `None` for UTC.
    pub anchor_after: SeriesFacts,
    /// The start the applied zone answers left (the session's zone answer's
    /// `start`), on the new clock.
    pub applied_start: String,
    /// How far the applied zone answers moved every occurrence, in wall
    /// minutes: the sum of their `movedMinutes`.
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub moved_minutes: i64,
    /// The saved length in milliseconds, only where the save changed the end
    /// against the applied series; `None` keeps each copy's own length.
    #[cfg_attr(feature = "ts-export", ts(type = "number | null"))]
    pub new_length_ms: Option<i64>,
    pub copy: CopyFacts,
    /// The zones the copy's calendar stores for a series (97a), in the
    /// manifest's form.
    #[cfg_attr(feature = "ts-export", ts(type = "\"all\" | \"none\" | Array<string>"))]
    pub time_zones: SeriesTimeZones,
    /// The copy's calendar stores a series' end as a date (manifest
    /// `recurrence.ends_by_date`, 140a). JSON without the field reads
    /// `false`.
    #[serde(default)]
    pub ends_by_date: bool,
    /// Today, from the surface's clock: the rule groups are compared from it
    /// on (120a, 123a).
    pub today: DateTime<Utc>,
}

/// Why a copy does not get the zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ZoneCarryRefusal {
    /// It has no rule there: the zone does not apply to it.
    NotASeries,
    /// It is all-day there.
    AllDay,
    /// Its calendar cannot store the series' new zone.
    ZoneNotStorable,
    /// Its occurrences are not on the series' days.
    OffPattern,
    /// Its rule cannot move by its shift; `rule` says why.
    RuleCannotMove,
}

/// What a copy gets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum ZoneCarry {
    /// It already stands as the series does, on its own clock: nothing is
    /// written.
    Unchanged,
    /// The copy as it is written on its target clock.
    Changed {
        rrule: String,
        /// RFC 3339.
        start: String,
        /// RFC 3339: the new start plus the saved length, or the copy's own.
        end: String,
        /// RFC 3339, in the order they came.
        exceptions: Vec<String>,
        /// The zone it is written with, as spelled: its own where it keeps
        /// its clock, the series' otherwise; `None` for UTC.
        tzid: Option<String>,
        /// How far, in wall minutes, every occurrence moves; a changed
        /// occurrence's slot moves by it too (95a). Zero where only the end
        /// changes.
        #[serde(rename = "shiftMinutes")]
        #[cfg_attr(feature = "ts-export", ts(type = "number"))]
        shift_minutes: i64,
        /// How many days its first occurrence moves for the zone, read on
        /// the new clock before and after (a date move made after the zone
        /// choice left out): negative is earlier ("rücken vor", 138a).
        #[serde(rename = "zoneDays")]
        #[cfg_attr(feature = "ts-export", ts(type = "number"))]
        zone_days: i64,
    },
    /// The copy does not get the zone.
    Refused {
        reason: ZoneCarryRefusal,
        /// The rule's own reason, for [`ZoneCarryRefusal::RuleCannotMove`].
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts-export", ts(optional))]
        rule: Option<ClockChangeRefusal>,
    },
}

/// How close a copy's occurrence must stand to the series' to pair with it.
const PAIRING: i64 = 12;

/// How far a shift may reach: further than any date a stored series names,
/// short of any a clock cannot hold, so no wall time overflows.
const MAX_SHIFT_MINUTES: i64 = 1_000 * 366 * 24 * 60;

fn instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn instant_text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// The canonical zone a series' clock is, `None` for UTC (every UTC name,
/// none, and a name tzdata does not know).
fn named_clock(tzid: Option<&str>) -> Option<&'static str> {
    series_clock_zone(tzid).and_then(canonical_zone)
}

/// Whether two stored zones are one clock (120a): both UTC, the same
/// canonical zone, or the same rules from `today` on.
fn same_clock(a: Option<&str>, b: Option<&str>, today: DateTime<Utc>) -> bool {
    match (named_clock(a), named_clock(b)) {
        (None, None) => true,
        (Some(a), Some(b)) => crate::zone_list::same_rules(a, b, today),
        _ => false,
    }
}

/// A series' first occurrence on its old pattern: the first day from its
/// start's on which it repeats, at its time; `None` for a rule with several
/// times a day or none in eight of its periods.
fn first_occurrence(series: &ClockMove) -> Option<DateTime<Utc>> {
    series.occurrence_time()?;
    let days = (8 * 366 * i64::from(series.interval())).min(150_000);
    (0..=days).find_map(|n| {
        series
            .start_day()
            .checked_add_signed(Duration::days(n))
            .and_then(|day| series.occurrence_on(day))
    })
}

/// The copy's occurrence nearest `y` and less than [`PAIRING`] hours from
/// it, on the day `y` reads as on the copy's clock or the one before or
/// after; on a tie the earlier.
fn nearest(copy: &ClockMove, y: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let day: NaiveDate = copy.old_clock().wall(y).date();
    (-1..=1)
        .filter_map(|k| day.checked_add_signed(Duration::days(k)))
        .filter_map(|day| copy.occurrence_on(day))
        .map(|x| ((x - y).abs(), x))
        .filter(|(gap, _)| *gap < Duration::hours(PAIRING))
        .min()
        .map(|(_, x)| x)
}

/// What one copy gets when a series' zone change is carried to it.
pub fn zone_carry(q: &ZoneCarryQuestion) -> ZoneCarry {
    let refused = |reason| ZoneCarry::Refused { reason, rule: None };
    let cannot_move = |rule| ZoneCarry::Refused {
        reason: ZoneCarryRefusal::RuleCannotMove,
        rule: Some(rule),
    };
    let unreadable = cannot_move(ClockChangeRefusal::Unreadable);

    // (1) Row facts first.
    let Some(copy_rule) = q
        .copy
        .rrule
        .as_deref()
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
    else {
        return refused(ZoneCarryRefusal::NotASeries);
    };
    if q.copy.all_day {
        return refused(ZoneCarryRefusal::AllDay);
    }

    // (2) The clocks: the series' old and new, the copy's own.
    let (series_old, _) = clock_of(q.anchor_before.tzid.as_deref());
    let (series_new, _) = clock_of(q.anchor_after.tzid.as_deref());
    let (copy_old, _) = clock_of(q.copy.tzid.as_deref());

    // (3) How far the series' occurrences moved: the zone answers, plus the
    // move made after the choice (Δ), read on the new clock.
    let (Some(applied), Some(saved), Some(copy_start), Some(copy_end)) = (
        instant(&q.applied_start),
        instant(&q.anchor_after.start),
        instant(&q.copy.start),
        instant(&q.copy.end),
    ) else {
        return unreadable;
    };
    if q.moved_minutes.unsigned_abs() > MAX_SHIFT_MINUTES.unsigned_abs() {
        return unreadable;
    }
    let delta = series_new.wall(saved) - series_new.wall(applied);
    let series_shift = Duration::minutes(q.moved_minutes) + delta;

    // (5) The copy's target clock: its own where that is the series' new
    // clock (120a), the series' otherwise. Asked first: the copy is read for
    // a move onto it.
    let keeps_clock = same_clock(
        q.copy.tzid.as_deref(),
        q.anchor_after.tzid.as_deref(),
        q.today,
    );
    let target_tzid = if keeps_clock {
        q.copy.tzid.as_deref()
    } else {
        q.anchor_after.tzid.as_deref()
    };
    let (target, _) = clock_of(target_tzid);
    let copy_question = ClockChangeQuestion {
        rrule: copy_rule.to_string(),
        start: q.copy.start.clone(),
        exceptions: q.copy.exceptions.clone(),
        from_tzid: q.copy.tzid.clone(),
        to_tzid: target_tzid.map(str::to_string),
        ends_by_date: q.ends_by_date,
    };
    let Some(copy) = ClockMove::read(&copy_question) else {
        return unreadable;
    };
    let before_question = ClockChangeQuestion {
        rrule: q.anchor_before.rrule.clone(),
        start: q.anchor_before.start.clone(),
        exceptions: Vec::new(),
        from_tzid: q.anchor_before.tzid.clone(),
        to_tzid: q.anchor_after.tzid.clone(),
        ends_by_date: false,
    };
    let Some(series) = ClockMove::read(&before_question) else {
        return unreadable;
    };

    // (4) Pair the copy with the series by instants. `x` is the copy's
    // paired occurrence; the move made after the choice counts in the shift
    // only where the copy follows the series' old occurrences.
    let wall_shift_at = |x: DateTime<Utc>| series_old.wall(x) + series_shift - copy_old.wall(x);
    let (shift, delta_taken) =
        if series.occurrence_time().is_none() || copy.occurrence_time().is_none() {
            (wall_shift_at(copy_start), delta)
        } else {
            let Some(y) = first_occurrence(&series) else {
                return refused(ZoneCarryRefusal::OffPattern);
            };
            match nearest(&copy, y) {
                Some(x) => (wall_shift_at(x), delta),
                None => {
                    // Already where the series now sits: it keeps its instants.
                    let after_question = ClockChangeQuestion {
                        rrule: q.anchor_after.rrule.clone(),
                        start: q.anchor_after.start.clone(),
                        exceptions: Vec::new(),
                        from_tzid: q.anchor_after.tzid.clone(),
                        to_tzid: q.anchor_after.tzid.clone(),
                        ends_by_date: false,
                    };
                    let x = ClockMove::read(&after_question)
                        .as_ref()
                        .and_then(first_occurrence)
                        .and_then(|y| nearest(&copy, y));
                    match x {
                        Some(x) => (series_new.wall(x) - copy_old.wall(x), Duration::zero()),
                        None => return refused(ZoneCarryRefusal::OffPattern),
                    }
                }
            }
        };

    let own_length = (copy_end - copy_start).num_milliseconds().max(0);
    let length = q.new_length_ms.map_or(own_length, |ms| ms.max(0));
    let written_tzid = series_clock_zone(target_tzid).map(str::to_string);
    let end_after = |start: DateTime<Utc>| {
        Duration::try_milliseconds(length)
            .and_then(|length| start.checked_add_signed(length))
            .map(instant_text)
    };

    // (6) On its own clock, not moved, its length right: nothing to write.
    if keeps_clock && shift.is_zero() {
        if length == own_length {
            return ZoneCarry::Unchanged;
        }
        let Some(end) = end_after(copy_start) else {
            return unreadable;
        };
        return ZoneCarry::Changed {
            rrule: copy_rule.to_string(),
            start: q.copy.start.clone(),
            end,
            exceptions: q.copy.exceptions.clone(),
            tzid: written_tzid,
            shift_minutes: 0,
            zone_days: 0,
        };
    }

    // (7) Calendar facts: the series' zone where the copy takes it. UTC is
    // stored by every calendar that stores a rule.
    if !keeps_clock {
        if let Some(zone) = named_clock(q.anchor_after.tzid.as_deref()) {
            if !q.time_zones.stores(zone) {
                return refused(ZoneCarryRefusal::ZoneNotStorable);
            }
        }
    }

    // (8) The move: the copy's own answer where the shift is one of them, so
    // an identical copy ends identical to the series; else as a whole.
    let answer = if shift == copy.rule_shift() {
        series_clock_change(&copy_question).map(|change| change.as_rule)
    } else if shift == copy.wall_shift() {
        Some(copy.moved_by(shift, StartMove::Keep))
    } else {
        Some(copy.moved_by(shift, StartMove::Shift))
    };
    let (rrule, start, exceptions) = match answer {
        Some(ClockChangeAnswer::Changed {
            rrule,
            start,
            exceptions,
            ..
        }) => (rrule, start, exceptions),
        Some(ClockChangeAnswer::Refused { reason }) => return cannot_move(reason),
        None => return unreadable,
    };

    // (9) The end: the saved length, or the copy's own.
    let Some(end) = instant(&start).and_then(end_after) else {
        return unreadable;
    };

    // (10) How far its first occurrence moves for the zone, read on the new
    // clock before and after.
    let first = first_occurrence(&copy).unwrap_or(copy_start);
    let moved = copy_old.wall(first) + shift - delta_taken;
    let zone_days = (moved.date() - target.wall(first).date()).num_days();

    // (11) Written as the target clock is spelled; UTC as none.
    ZoneCarry::Changed {
        rrule,
        start,
        end,
        exceptions,
        tzid: written_tzid,
        shift_minutes: shift.num_minutes(),
        zone_days,
    }
}

/// [`zone_carry`] over the wire: a question as JSON in, the answer as JSON
/// out. An error only for JSON that is no question at all.
pub fn zone_carry_json(question_json: &str) -> Result<String, serde_json::Error> {
    let question: ZoneCarryQuestion = serde_json::from_str(question_json)?;
    serde_json::to_string(&zone_carry(&question))
}

#[cfg(test)]
mod contract {
    //! The shared fixture.

    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/groupZoneCarry.json"
        ));
        serde_json::from_str(text).expect("the fixture parses")
    }

    #[test]
    fn every_row_gives_its_answer() {
        let doc = fixture();
        let rows = doc["rows"].as_array().expect("rows");
        // Anti-silence: the rows the rule turns on, by name.
        for needed in NEEDED {
            assert!(
                rows.iter().any(|r| r["name"] == *needed),
                "the fixture lost {needed}"
            );
        }
        // Every row that differs, by name, rather than the first.
        let differing: Vec<String> = rows
            .iter()
            .filter_map(|row| {
                let answer: Value = serde_json::from_str(
                    &zone_carry_json(&row["question"].to_string()).expect("a question"),
                )
                .expect("json");
                (answer != row["expect"]).then(|| {
                    format!(
                        "{}: got {answer}, want {}",
                        row["name"].as_str().unwrap_or("?"),
                        row["expect"]
                    )
                })
            })
            .collect();
        assert!(
            differing.is_empty(),
            "{} row(s) differ:\n{}",
            differing.len(),
            differing.join("\n")
        );
    }

    /// The rows each decision and each closed hole turns on.
    pub(super) const NEEDED: &[&str] = &[
        "a-true-copy-as-they-fall-now",
        "a-true-copy-as-the-rule-says",
        "a-copy-in-the-other-half-year-comes-to-the-series-hour",
        "copy-in-june-anchor-in-january-as-they-fall-now",
        "copy-in-january-anchor-in-june-as-the-rule-says",
        "copy-in-january-anchor-in-june-as-they-fall-now",
        "a-copy-an-hour-later-keeps-its-distance",
        "a-copy-already-repaired-is-unchanged",
        "a-copy-already-repaired-gets-a-new-end",
        "a-copy-on-berlin-at-the-old-instants-moves-a-day-earlier",
        "a-copy-on-berlin-from-june-pairs-by-instants",
        "a-copy-in-vienna-keeps-vienna",
        "to-utc-a-copy-on-etc-utc-is-unchanged",
        "to-utc-a-copy-on-berlin-is-written-without-a-zone",
        "troll-on-a-calendar-that-cannot-store-it",
        "a-calendar-that-stores-no-zone",
        "a-copy-without-a-rule",
        "an-all-day-copy",
        "a-copy-whose-rule-cannot-be-read",
        "every-other-week-in-the-other-parity",
        "a-move-into-the-spring-gap",
        "a-rule-with-its-own-hour",
        "a-whole-move-with-a-deletion-and-an-end",
        "across-midnight-with-an-end-on-the-last-occurrence",
        "across-midnight-with-a-guarded-end-by-date",
        "across-midnight-with-a-guarded-end-as-a-cut",
        "a-deletion-that-cancels-nothing-stays",
        "zone-then-a-date-one-day-later",
        "zone-then-a-date-says-only-the-zone-days",
        "zone-and-a-new-length",
        "two-choices-summed",
        "an-hourly-copy",
    ];

    #[test]
    fn text_that_is_no_question_is_an_error() {
        assert!(zone_carry_json("{\"anchorBefore\":1}").is_err());
        assert!(zone_carry_json("not json").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(rrule: &str, start: &str, tzid: Option<&str>) -> SeriesFacts {
        SeriesFacts {
            rrule: rrule.into(),
            start: start.into(),
            tzid: tzid.map(str::to_string),
        }
    }

    fn question(copy_tzid: Option<&str>) -> ZoneCarryQuestion {
        ZoneCarryQuestion {
            anchor_before: series("FREQ=WEEKLY;BYDAY=MO", "2027-01-04T08:00:00Z", None),
            anchor_after: series(
                "FREQ=WEEKLY;BYDAY=MO",
                "2027-01-04T08:00:00Z",
                Some("Europe/Berlin"),
            ),
            applied_start: "2027-01-04T08:00:00Z".into(),
            moved_minutes: 60,
            new_length_ms: None,
            copy: CopyFacts {
                rrule: Some("FREQ=WEEKLY;BYDAY=MO".into()),
                start: "2027-01-04T08:00:00Z".into(),
                end: "2027-01-04T09:00:00Z".into(),
                exceptions: Vec::new(),
                tzid: copy_tzid.map(str::to_string),
                all_day: false,
            },
            time_zones: SeriesTimeZones::Every,
            ends_by_date: false,
            today: "2026-11-28T12:00:00Z".parse().expect("an instant"),
        }
    }

    /// A shift no clock can hold answers rather than aborting.
    #[test]
    fn a_shift_no_clock_can_hold_is_refused() {
        for moved_minutes in [i64::MAX, i64::MIN, MAX_SHIFT_MINUTES + 1] {
            let q = ZoneCarryQuestion {
                moved_minutes,
                ..question(None)
            };
            assert_eq!(
                zone_carry(&q),
                ZoneCarry::Refused {
                    reason: ZoneCarryRefusal::RuleCannotMove,
                    rule: Some(ClockChangeRefusal::Unreadable),
                },
                "{moved_minutes}"
            );
        }
    }

    /// An empty rule is no series, as a missing one is; a blank one too.
    #[test]
    fn an_empty_rule_is_no_series() {
        for rule in ["", "  "] {
            let mut q = question(None);
            q.copy.rrule = Some(rule.into());
            assert_eq!(
                zone_carry(&q),
                ZoneCarry::Refused {
                    reason: ZoneCarryRefusal::NotASeries,
                    rule: None
                }
            );
        }
    }

    /// Row facts before calendar facts: an all-day copy in a calendar that
    /// stores no zone is refused for being all-day, as the offer says.
    #[test]
    fn row_facts_come_before_calendar_facts() {
        let mut q = question(None);
        q.copy.all_day = true;
        q.time_zones = SeriesTimeZones::NoZone;
        assert_eq!(
            zone_carry(&q),
            ZoneCarry::Refused {
                reason: ZoneCarryRefusal::AllDay,
                rule: None
            }
        );
    }

    /// Every UTC name and none are one clock, and an alias is its zone.
    #[test]
    fn one_clock_by_any_name() {
        let today: DateTime<Utc> = "2026-11-28T12:00:00Z".parse().expect("an instant");
        assert!(same_clock(None, Some("Etc/UTC"), today));
        assert!(same_clock(Some("UTC"), Some("GMT"), today));
        assert!(same_clock(
            Some("Asia/Calcutta"),
            Some("Asia/Kolkata"),
            today
        ));
        assert!(same_clock(
            Some("Europe/Vienna"),
            Some("Europe/Berlin"),
            today
        ));
        assert!(!same_clock(
            Some("Europe/London"),
            Some("Europe/Berlin"),
            today
        ));
        assert!(!same_clock(None, Some("Europe/London"), today));
        // A fixed zone on UTC's offset is not UTC: UTC is no zone (26a).
        assert!(!same_clock(None, Some("Africa/Abidjan"), today));
    }
}
