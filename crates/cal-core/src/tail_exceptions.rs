//! The deleted occurrences a series keeps when "this and all following" writes
//! it from the cut on.
//!
//! Splitting a series creates a new one from the occurrence the user picked; a
//! whole-series edit from there rewrites it in place. Either way the occurrences
//! the calendar shows nothing for (the shell's `deletedSlots`) have to stay
//! deleted, or they come back with the edit. An exception names one occurrence
//! by its instant, so it only keeps doing that while the occurrence keeps its
//! instant. Three edits move occurrences away from their old instants:
//!
//! - A new date for the occurrence the user opened moves every occurrence from
//!   there on. A deleted occurrence goes with its PLACE in the series: a deleted
//!   third Monday becomes a deleted third Tuesday (decision 152, as a copy is cut
//!   by its place, 141). Left on its old day it hit the day before the occurrence
//!   it was meant for, or nothing at all.
//! - A switch between all-day and a time of day. An all-day occurrence is the
//!   local midnight of its day, so an exception at 18:00 sat closer to the next
//!   day's midnight and deleted that day instead. By place, it stays on its own.
//! - A new repeat rule. Places no longer line up, so a deletion stays on its DAY
//!   — but only where the new rule still has an occurrence that day (decision
//!   188). One on a day the new rule never meets deletes nothing; written anyway,
//!   Exchange refuses to delete what it cannot find, and the whole save fails.
//!   A new end (`COUNT`, `UNTIL`) or another spelling of the same rule is no new
//!   rule: the occurrences are the same.
//!
//! By place only when every occurrence moves with the start: the pattern is
//! the same, the rule takes its days and times from the start rather than
//! naming them, and every occurrence handed over moved by the same number of
//! days. A rule that names its days — weekdays, days of the month, months, or
//! its times of day — keeps them when one occurrence moves: "every Monday"
//! moved to a Tuesday still repeats on Mondays, and "every weekday" moved from
//! Monday to Tuesday still has its Wednesday and its Thursday where they were.
//! And a monthly rule from the 31st skips the shorter months, so moved to the
//! 30th it meets months the old one skipped: its places stop lining up after
//! the first short month. Counting places there shifted a deletion onto an
//! occurrence nobody deleted. Such a deletion stays on its day, at the new
//! time of day if that changed, until the rule itself moves with the date
//! (decision 189, a later change); where the new series has no occurrence that
//! day it is dropped, which hides nothing.
//!
//! A day is read on the clock both series repeat on. When they have none in
//! common — one becomes all-day, or gets another zone — it is read on the
//! device's calendar, the days the user saw the occurrences on. And a deletion
//! never lands on the new series' first occurrence: that is the one the user
//! is saving.
//!
//! The shell expands both series — the device's zone is the shell's to know,
//! and the core reads no clock — and hands over each occurrence with the day it
//! falls on, on its series' own clock and on the device's calendar. The
//! decisions are made here: which old occurrence a deletion names, by place or
//! by day, what is dropped, and how a kept deletion is spelled: as the instant
//! of the new series' own occurrence, which is what every writer and every
//! reader matches.
//!
//! Pinned row by row in `tests/fixtures/tailExceptions.json`; the `contract`
//! module below reads it, and so do the phone's door test in cal-ffi and the
//! TypeScript contract test through the WebAssembly door.

use chrono::{DateTime, NaiveDate};
use serde::{Deserialize, Serialize};

// The rule is read once, for every module that asks (`rrule_parts`).
use crate::rrule_parts::parse_parts;
use crate::series_clock::{canonical_zone, expansion_clock, ExpansionClock};

/// Half a day: two instants this close name the same day of a series of days,
/// and two that far apart never can (decision 95; the shell's `sameSlot`).
const SAME_DAY_MS: i64 = 12 * 60 * 60 * 1000;

/// One occurrence as the shell expanded it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct TailSlot {
    /// The instant, RFC 3339, as the shell's expansion wrote it.
    pub at: String,
    /// The day it falls on, `YYYY-MM-DD`, on the clock its series repeats on:
    /// its zone, the device's days for an all-day series, or UTC.
    pub day: String,
    /// The day it falls on on the device's calendar: the day the user sees it.
    pub device_day: String,
}

/// The question the door takes.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct TailExceptionsQuestion {
    /// The old series' occurrences from the cut on, in order, as its rule
    /// generates them with no exception: the first is the one the cut names.
    /// They reach at least as far as the last deleted occurrence.
    pub old_slots: Vec<TailSlot>,
    /// Whether the old series is all-day, and its stored zone: they say how a
    /// deleted occurrence names its slot — exactly, or by day.
    pub old_all_day: bool,
    pub old_tzid: Option<String>,
    /// The old series' rule, as stored.
    pub old_rule: String,
    /// The new series' occurrences from its start on, in order, as its rule
    /// generates them with no exception.
    pub tail: Vec<TailSlot>,
    /// The new series' rule.
    pub tail_rule: String,
    /// The instant the new series starts at.
    pub tail_start: String,
    /// Whether the new series is all-day, and its zone.
    pub tail_all_day: bool,
    pub tail_tzid: Option<String>,
    /// The old series' occurrences from the cut on that the calendar shows
    /// nothing for, as the shell's `deletedSlots` spelled them.
    pub deleted: Vec<String>,
    /// Exceptions that stay as they are spelled: occurrences a row of the
    /// series still stands in for, kept when the series is rewritten in place.
    #[serde(default)]
    pub standing: Vec<String>,
}

/// How the deleted occurrences went over to the new series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum TailCarry {
    /// By their place in the series (decision 152).
    Place,
    /// By their day (decisions 188, 189).
    Day,
}

/// The new series' exceptions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct TailExceptions {
    /// The exceptions to write, earliest first, each instant once.
    pub exceptions: Vec<String>,
    /// How the deleted occurrences were carried over.
    pub carried_by: TailCarry,
    /// The deleted occurrences that name no occurrence of the new series, as
    /// they were given: past its end, on a day its rule does not meet, or not
    /// an occurrence of the old series at all.
    pub dropped: Vec<String>,
}

/// The parts that name the days or the times a rule repeats on, instead of
/// taking them from the start.
const NAMING_PARTS: [&str; 9] = [
    "BYDAY",
    "BYMONTHDAY",
    "BYYEARDAY",
    "BYWEEKNO",
    "BYSETPOS",
    "BYMONTH",
    "BYHOUR",
    "BYMINUTE",
    "BYSECOND",
];

/// What a rule repeats by, whatever it is spelled like and wherever it ends:
/// its parts without `COUNT` and `UNTIL`, the defaults `INTERVAL=1` and
/// `WKST=MO` left out, values uppercased and lists in order. `None` when it
/// cannot be read.
fn pattern(rrule: &str) -> Option<Vec<(String, String)>> {
    let (_, parts) = parse_parts(rrule).ok()?;
    let mut pattern: Vec<(String, String)> = parts
        .into_iter()
        .filter(|p| p.key != "COUNT" && p.key != "UNTIL")
        .map(|p| {
            let mut values: Vec<String> = p
                .value
                .split(',')
                .map(|v| v.trim().to_ascii_uppercase())
                .collect();
            values.sort();
            (p.key, values.join(","))
        })
        .filter(|(key, value)| {
            !(key == "INTERVAL" && value == "1") && !(key == "WKST" && value == "MO")
        })
        .collect();
    pattern.sort();
    Some(pattern)
}

/// Whether a rule takes its days and times from its start, so its occurrences
/// move when the start does. Unreadable reads as no: by day is the side that
/// cannot shift a deletion onto an occurrence the user never deleted.
fn days_follow_start(rrule: &str) -> bool {
    parse_parts(rrule)
        .is_ok_and(|(_, parts)| !parts.iter().any(|p| NAMING_PARTS.contains(&p.key.as_str())))
}

/// Whether the new series' occurrences are the old series' occurrences moved
/// by one and the same number of days, place for place, as far as both reach.
/// A rule that takes its days from the start still skips months or years by
/// its day: from the 31st every month without one, from 29 February every
/// year that is no leap year.
fn moved_alike(old: &[TailSlot], tail: &[TailSlot], day_of: impl Fn(&TailSlot) -> String) -> bool {
    let day = |slot: &TailSlot| NaiveDate::parse_from_str(&day_of(slot), "%Y-%m-%d").ok();
    let mut moved = None;
    for (before, after) in old.iter().zip(tail) {
        let (Some(before), Some(after)) = (day(before), day(after)) else {
            return false;
        };
        let by = (after - before).num_days();
        if *moved.get_or_insert(by) != by {
            return false;
        }
    }
    true
}

/// Whether two series repeat on the same clock, so a day on one is the same
/// day on the other.
fn same_clock(
    a_all_day: bool,
    a_tzid: Option<&str>,
    b_all_day: bool,
    b_tzid: Option<&str>,
) -> bool {
    let a = expansion_clock(a_all_day, a_tzid);
    a == expansion_clock(b_all_day, b_tzid)
        && (a != ExpansionClock::Zone
            || a_tzid.and_then(canonical_zone) == b_tzid.and_then(canonical_zone))
}

/// An RFC 3339 instant in milliseconds.
fn instant_ms(iso: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|at| at.timestamp_millis())
}

/// Whether two instants name the same occurrence of a series: exactly for a
/// timed series, the same day for a series of days (the shell's `sameSlot`).
fn same_slot(all_day: bool, tzid: Option<&str>) -> impl Fn(i64, i64) -> bool {
    let by_day = expansion_clock(all_day, tzid) == ExpansionClock::DeviceDays;
    move |a, b| {
        if by_day {
            (a - b).abs() < SAME_DAY_MS
        } else {
            a == b
        }
    }
}

/// The new series' exceptions for this question.
pub fn tail_exceptions(question: &TailExceptionsQuestion) -> TailExceptions {
    let same_old = same_slot(question.old_all_day, question.old_tzid.as_deref());
    let same_tail = same_slot(question.tail_all_day, question.tail_tzid.as_deref());

    // By place only when every occurrence moves with the start: the same
    // pattern, days taken from the start, and the new series starting on an
    // occurrence of its own rule.
    let tail_start = instant_ms(&question.tail_start);
    let aligned = match (question.tail.first(), tail_start) {
        (Some(first), Some(start)) => instant_ms(&first.at).is_some_and(|at| same_tail(at, start)),
        _ => false,
    };
    let same_pattern = matches!(
        (pattern(&question.old_rule), pattern(&question.tail_rule)),
        (Some(old), Some(tail)) if old == tail
    );
    // A day on the clock both series repeat on, or the device's when they have
    // none in common.
    let shared_clock = same_clock(
        question.old_all_day,
        question.old_tzid.as_deref(),
        question.tail_all_day,
        question.tail_tzid.as_deref(),
    );
    let day_of = |slot: &TailSlot| -> String {
        if shared_clock {
            slot.day.clone()
        } else {
            slot.device_day.clone()
        }
    };
    let carried_by = if same_pattern
        && days_follow_start(&question.tail_rule)
        && aligned
        && moved_alike(&question.old_slots, &question.tail, day_of)
    {
        TailCarry::Place
    } else {
        TailCarry::Day
    };

    let old: Vec<Option<i64>> = question
        .old_slots
        .iter()
        .map(|slot| instant_ms(&slot.at))
        .collect();
    let mut kept: Vec<(i64, String)> = Vec::new();
    let mut dropped = Vec::new();
    for deleted in &question.deleted {
        let target = instant_ms(deleted)
            .and_then(|at| {
                old.iter()
                    .position(|slot| slot.is_some_and(|s| same_old(s, at)))
            })
            .and_then(|place| match carried_by {
                TailCarry::Place => question.tail.get(place),
                TailCarry::Day => {
                    // An occurrence that did not move keeps its deletion as
                    // it is: the same instant in the new series.
                    let unmoved = old[place].and_then(|at| {
                        question
                            .tail
                            .iter()
                            .find(|occ| instant_ms(&occ.at) == Some(at))
                    });
                    unmoved.or_else(|| {
                        let day = day_of(&question.old_slots[place]);
                        let mut on_day = question.tail.iter().filter(|occ| day_of(occ) == day);
                        // Two occurrences on one day (a rule that repeats
                        // within it): which one was meant cannot be told, so
                        // neither.
                        match (on_day.next(), on_day.next()) {
                            (Some(only), None) => Some(only),
                            _ => None,
                        }
                    })
                }
            })
            .and_then(|occ| instant_ms(&occ.at).map(|at| (at, occ.at.clone())))
            // Never the occurrence the user is saving.
            .filter(|(at, _)| !tail_start.is_some_and(|start| same_tail(*at, start)));
        match target {
            Some(found) => kept.push(found),
            None => dropped.push(deleted.clone()),
        }
    }
    for standing in &question.standing {
        // A spelling that is no instant still stands: it is the provider's,
        // and the series is rewritten with it as it was.
        kept.push((instant_ms(standing).unwrap_or(i64::MAX), standing.clone()));
    }
    // Earliest first; two spellings of one instant are one exception.
    kept.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut exceptions: Vec<String> = Vec::new();
    let mut last: Option<i64> = None;
    for (at, iso) in kept {
        if at != i64::MAX && last == Some(at) {
            continue;
        }
        last = Some(at);
        exceptions.push(iso);
    }
    TailExceptions {
        exceptions,
        carried_by,
        dropped,
    }
}

/// [`tail_exceptions`] over JSON: the door the desktop's WebAssembly and the
/// phone's UniFFI binding take.
pub fn tail_exceptions_json(input_json: &str) -> Result<String, serde_json::Error> {
    let question: TailExceptionsQuestion = serde_json::from_str(input_json)?;
    serde_json::to_string(&tail_exceptions(&question))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slot whose day is the same on its series' clock and on the device's.
    fn slot(at: &str, day: &str) -> TailSlot {
        TailSlot {
            at: at.to_string(),
            day: day.to_string(),
            device_day: day.to_string(),
        }
    }

    fn question(
        old: Vec<TailSlot>,
        tail: Vec<TailSlot>,
        deleted: &[&str],
    ) -> TailExceptionsQuestion {
        TailExceptionsQuestion {
            tail_start: tail.first().map(|s| s.at.clone()).unwrap_or_default(),
            old_slots: old,
            old_all_day: false,
            old_tzid: None,
            old_rule: "FREQ=WEEKLY".into(),
            tail,
            tail_rule: "FREQ=WEEKLY".into(),
            tail_all_day: false,
            tail_tzid: None,
            deleted: deleted.iter().map(|d| d.to_string()).collect(),
            standing: Vec::new(),
        }
    }

    #[test]
    fn a_deletion_keeps_its_place_when_the_date_moves() {
        // Weekly on Mondays at 08:00 UTC, the 3rd deleted, moved to Tuesdays.
        let old = vec![
            slot("2026-08-24T08:00:00.000Z", "2026-08-24"),
            slot("2026-08-31T08:00:00.000Z", "2026-08-31"),
            slot("2026-09-07T08:00:00.000Z", "2026-09-07"),
        ];
        let tail = vec![
            slot("2026-08-25T08:00:00.000Z", "2026-08-25"),
            slot("2026-09-01T08:00:00.000Z", "2026-09-01"),
            slot("2026-09-08T08:00:00.000Z", "2026-09-08"),
        ];
        let answer = tail_exceptions(&question(old, tail, &["2026-09-07T08:00:00.000Z"]));
        assert_eq!(answer.carried_by, TailCarry::Place);
        assert_eq!(answer.exceptions, vec!["2026-09-08T08:00:00.000Z"]);
        assert!(answer.dropped.is_empty());
    }

    #[test]
    fn a_new_rule_keeps_a_deletion_only_on_a_day_it_still_meets() {
        // Weekly to every second week (decision 188): 31.08 is gone with the
        // new rule, 07.09 is still an occurrence of it.
        let old = vec![
            slot("2026-08-24T08:00:00.000Z", "2026-08-24"),
            slot("2026-08-31T08:00:00.000Z", "2026-08-31"),
            slot("2026-09-07T08:00:00.000Z", "2026-09-07"),
        ];
        let tail = vec![
            slot("2026-08-24T08:00:00.000Z", "2026-08-24"),
            slot("2026-09-07T08:00:00.000Z", "2026-09-07"),
        ];
        let mut q = question(
            old,
            tail,
            &["2026-08-31T08:00:00.000Z", "2026-09-07T08:00:00.000Z"],
        );
        q.tail_rule = "FREQ=WEEKLY;INTERVAL=2".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Day);
        assert_eq!(answer.exceptions, vec!["2026-09-07T08:00:00.000Z"]);
        assert_eq!(answer.dropped, vec!["2026-08-31T08:00:00.000Z"]);
    }

    #[test]
    fn an_evening_deletion_stays_on_its_day_when_the_series_becomes_all_day() {
        // Daily 18:00 Berlin (16:00 UTC); the 27th deleted; the new series is
        // all-day on a Berlin device. By place the 27th stays the 27th: its
        // local midnight, not the 28th's that 18:00 is closer to.
        let old = vec![
            slot("2026-08-24T16:00:00.000Z", "2026-08-24"),
            slot("2026-08-25T16:00:00.000Z", "2026-08-25"),
            slot("2026-08-26T16:00:00.000Z", "2026-08-26"),
            slot("2026-08-27T16:00:00.000Z", "2026-08-27"),
        ];
        let tail = vec![
            slot("2026-08-23T22:00:00.000Z", "2026-08-24"),
            slot("2026-08-24T22:00:00.000Z", "2026-08-25"),
            slot("2026-08-25T22:00:00.000Z", "2026-08-26"),
            slot("2026-08-26T22:00:00.000Z", "2026-08-27"),
        ];
        let mut q = question(old, tail, &["2026-08-27T16:00:00.000Z"]);
        q.old_tzid = Some("Europe/Berlin".into());
        q.old_rule = "FREQ=DAILY".into();
        q.tail_rule = "FREQ=DAILY".into();
        q.tail_all_day = true;
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Place);
        assert_eq!(answer.exceptions, vec!["2026-08-26T22:00:00.000Z"]);
    }

    #[test]
    fn a_series_not_starting_on_its_own_rule_keeps_deletions_on_their_day() {
        // "Every Monday" moved to a Tuesday: the rule still has Mondays, its
        // first occurrence is not the moved one (decision 189). Counted by
        // place, the deletion would land a week late.
        let old = vec![
            slot("2026-08-24T08:00:00.000Z", "2026-08-24"),
            slot("2026-08-31T08:00:00.000Z", "2026-08-31"),
            slot("2026-09-07T08:00:00.000Z", "2026-09-07"),
        ];
        let tail = vec![
            slot("2026-08-31T08:00:00.000Z", "2026-08-31"),
            slot("2026-09-07T08:00:00.000Z", "2026-09-07"),
        ];
        let mut q = question(old, tail, &["2026-09-07T08:00:00.000Z"]);
        q.old_rule = "FREQ=WEEKLY;BYDAY=MO".into();
        q.tail_rule = "FREQ=WEEKLY;BYDAY=MO".into();
        q.tail_start = "2026-08-25T08:00:00.000Z".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Day);
        assert_eq!(answer.exceptions, vec!["2026-09-07T08:00:00.000Z"]);
    }

    #[test]
    fn a_deletion_past_the_new_series_end_or_off_the_old_rule_is_dropped() {
        let old = vec![
            slot("2026-08-24T08:00:00.000Z", "2026-08-24"),
            slot("2026-08-25T08:00:00.000Z", "2026-08-25"),
        ];
        let tail = vec![slot("2026-08-25T08:00:00.000Z", "2026-08-25")];
        // The 25th is the old 2nd place; the new series has one occurrence.
        // The 30th is no occurrence of the old series at all.
        let answer = tail_exceptions(&question(
            old,
            tail,
            &["2026-08-25T08:00:00.000Z", "2026-08-30T08:00:00.000Z"],
        ));
        assert!(answer.exceptions.is_empty());
        assert_eq!(
            answer.dropped,
            vec!["2026-08-25T08:00:00.000Z", "2026-08-30T08:00:00.000Z"]
        );
    }

    #[test]
    fn standing_exceptions_stay_as_spelled_in_order_and_once() {
        let old = vec![
            slot("2026-08-24T08:00:00.000Z", "2026-08-24"),
            slot("2026-08-31T08:00:00.000Z", "2026-08-31"),
        ];
        let tail = old.clone();
        let mut q = question(old, tail, &["2026-08-31T08:00:00.000Z"]);
        q.standing = vec!["2026-08-31T08:00:00Z".into(), "2026-08-24T08:00:00Z".into()];
        let answer = tail_exceptions(&q);
        // The 31st is one instant in two spellings: once, the first in order.
        assert_eq!(
            answer.exceptions,
            vec!["2026-08-24T08:00:00Z", "2026-08-31T08:00:00.000Z"]
        );
    }

    #[test]
    fn two_occurrences_on_one_day_keep_neither_by_day() {
        let old = vec![
            slot("2026-08-24T08:00:00.000Z", "2026-08-24"),
            slot("2026-08-24T20:00:00.000Z", "2026-08-24"),
        ];
        let tail = vec![
            slot("2026-08-24T09:00:00.000Z", "2026-08-24"),
            slot("2026-08-24T21:00:00.000Z", "2026-08-24"),
        ];
        let mut q = question(old, tail, &["2026-08-24T20:00:00.000Z"]);
        q.old_rule = "FREQ=HOURLY;INTERVAL=12".into();
        q.tail_rule = "FREQ=DAILY;BYHOUR=9,21".into();
        let answer = tail_exceptions(&q);
        assert!(answer.exceptions.is_empty());
        assert_eq!(answer.dropped, vec!["2026-08-24T20:00:00.000Z"]);
    }

    #[test]
    fn a_rule_of_several_weekdays_moved_onto_another_keeps_its_deletions() {
        // "Every weekday" moved from Monday to Tuesday: Wednesday and Thursday
        // stay where they were, so the deleted Thursday stays deleted.
        let old = vec![
            slot("2026-08-24T06:00:00.000Z", "2026-08-24"),
            slot("2026-08-25T06:00:00.000Z", "2026-08-25"),
            slot("2026-08-26T06:00:00.000Z", "2026-08-26"),
            slot("2026-08-27T06:00:00.000Z", "2026-08-27"),
        ];
        let tail = vec![
            slot("2026-08-25T06:00:00.000Z", "2026-08-25"),
            slot("2026-08-26T06:00:00.000Z", "2026-08-26"),
            slot("2026-08-27T06:00:00.000Z", "2026-08-27"),
            slot("2026-08-28T06:00:00.000Z", "2026-08-28"),
        ];
        let mut q = question(old, tail, &["2026-08-27T06:00:00.000Z"]);
        q.old_rule = "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into();
        q.tail_rule = "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Day);
        assert_eq!(answer.exceptions, vec!["2026-08-27T06:00:00.000Z"]);
    }

    #[test]
    fn a_monthly_rule_from_the_31st_moved_to_the_30th_keeps_its_deletion_on_its_day() {
        // From the 31st the rule skips November and February; from the 30th it
        // skips only February. Counted by place, the deleted March landed on
        // 30 January, which nobody deleted.
        let old = vec![
            slot("2026-10-31T09:00:00.000Z", "2026-10-31"),
            slot("2026-12-31T09:00:00.000Z", "2026-12-31"),
            slot("2027-01-31T09:00:00.000Z", "2027-01-31"),
            slot("2027-03-31T08:00:00.000Z", "2027-03-31"),
        ];
        let tail = vec![
            slot("2026-10-30T09:00:00.000Z", "2026-10-30"),
            slot("2026-11-30T09:00:00.000Z", "2026-11-30"),
            slot("2026-12-30T09:00:00.000Z", "2026-12-30"),
            slot("2027-01-30T09:00:00.000Z", "2027-01-30"),
        ];
        let mut q = question(old, tail, &["2027-03-31T08:00:00.000Z"]);
        q.old_rule = "FREQ=MONTHLY".into();
        q.tail_rule = "FREQ=MONTHLY".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Day);
        assert!(answer.exceptions.is_empty());
    }

    #[test]
    fn a_rule_that_names_its_times_keeps_its_deletions() {
        // 09:00 and 21:00, the opened 09:00 moved to 21:00: the next 09:00 is
        // where it was, so its deletion stays.
        let old = vec![
            slot("2026-08-24T07:00:00.000Z", "2026-08-24"),
            slot("2026-08-24T19:00:00.000Z", "2026-08-24"),
            slot("2026-08-25T07:00:00.000Z", "2026-08-25"),
        ];
        let tail = vec![
            slot("2026-08-24T19:00:00.000Z", "2026-08-24"),
            slot("2026-08-25T07:00:00.000Z", "2026-08-25"),
            slot("2026-08-25T19:00:00.000Z", "2026-08-25"),
        ];
        let mut q = question(old, tail, &["2026-08-25T07:00:00.000Z"]);
        q.old_rule = "FREQ=DAILY;BYHOUR=9,21".into();
        q.tail_rule = "FREQ=DAILY;BYHOUR=9,21".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Day);
        assert_eq!(answer.exceptions, vec!["2026-08-25T07:00:00.000Z"]);
    }

    #[test]
    fn a_new_end_or_another_spelling_is_no_new_rule() {
        assert_eq!(
            pattern("FREQ=WEEKLY;COUNT=10"),
            pattern("RRULE:freq=weekly;COUNT=12")
        );
        assert_eq!(
            pattern("FREQ=WEEKLY;WKST=MO;COUNT=10"),
            pattern("FREQ=WEEKLY;COUNT=12")
        );
        assert_ne!(pattern("FREQ=WEEKLY;WKST=SU"), pattern("FREQ=WEEKLY"));
        assert_eq!(pattern("FREQ=DAILY"), pattern("FREQ=DAILY;INTERVAL=1"));
        assert_eq!(
            pattern("FREQ=WEEKLY;BYDAY=MO,WE"),
            pattern("BYDAY=WE,MO;FREQ=WEEKLY;UNTIL=20261231T000000Z")
        );
        assert_ne!(pattern("FREQ=WEEKLY"), pattern("FREQ=WEEKLY;INTERVAL=2"));
        assert!(days_follow_start("FREQ=MONTHLY"));
        assert!(!days_follow_start("FREQ=MONTHLY;BYMONTHDAY=10"));
        assert!(!days_follow_start("FREQ=WEEKLY;BYDAY=MO"));
        assert!(!days_follow_start("not a rule"));
    }

    #[test]
    fn by_day_never_deletes_the_occurrence_being_saved() {
        // Daily, the 25th deleted; moved to the 25th with a weekly rule. The
        // new series starts on the 25th: the user just put it there.
        let old = vec![
            slot("2026-08-24T08:00:00.000Z", "2026-08-24"),
            slot("2026-08-25T08:00:00.000Z", "2026-08-25"),
        ];
        let tail = vec![
            slot("2026-08-25T08:00:00.000Z", "2026-08-25"),
            slot("2026-09-01T08:00:00.000Z", "2026-09-01"),
        ];
        let mut q = question(old, tail, &["2026-08-25T08:00:00.000Z"]);
        q.old_rule = "FREQ=DAILY".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Day);
        assert!(answer.exceptions.is_empty());
        assert_eq!(answer.dropped, vec!["2026-08-25T08:00:00.000Z"]);
    }

    #[test]
    fn without_a_clock_in_common_a_day_is_the_device_s() {
        // 19:00 New York is 01:00 the next day in Berlin. Switched to all-day
        // under a new rule, the deleted Monday evening is the Tuesday the user
        // saw it on.
        let old = vec![TailSlot {
            at: "2026-09-07T23:00:00.000Z".into(),
            day: "2026-09-07".into(),
            device_day: "2026-09-08".into(),
        }];
        let tail = vec![
            slot("2026-09-06T22:00:00.000Z", "2026-09-07"),
            slot("2026-09-07T22:00:00.000Z", "2026-09-08"),
        ];
        let mut q = question(old, tail, &["2026-09-07T23:00:00.000Z"]);
        q.old_tzid = Some("America/New_York".into());
        q.old_rule = "FREQ=DAILY".into();
        q.tail_rule = "FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR,SA,SU".into();
        q.tail_all_day = true;
        q.tail_start = "2026-08-24T22:00:00.000Z".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Day);
        assert_eq!(answer.exceptions, vec!["2026-09-07T22:00:00.000Z"]);
    }

    #[test]
    fn the_door_names_a_question_it_cannot_read() {
        assert!(tail_exceptions_json("not json").is_err());
        assert!(tail_exceptions_json("{}").is_err());
    }
}

/// The rows of `tests/fixtures/tailExceptions.json`, through the JSON door the
/// surfaces take: the same file the phone's door test and the desktop's
/// contract test read, so core and surfaces cannot drift apart.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::Value;

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tailExceptions.json"
    ));

    #[test]
    fn every_row_answers_as_the_contract_says() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let rows = doc["rows"].as_array().expect("rows");
        // Anti-silence: the rows each decision turns on, by name.
        for name in [
            "A1 an evening deletion stays on its day when the series becomes all-day",
            "B2 every second week keeps the deletion it still meets",
            "C a deleted third Monday becomes a deleted third Tuesday",
            "C4 \"every Monday\" moved to a Tuesday keeps its deletion on Monday",
            "\"every weekday\" moved onto another of its days keeps its deletions",
            "without a clock in common a day is the one the device shows",
        ] {
            assert!(
                rows.iter().any(|row| row["name"] == name),
                "the contract lost the row {name}"
            );
        }
        for row in rows {
            let name = row["name"].as_str().expect("a name");
            let answered = tail_exceptions_json(&row["question"].to_string())
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let answered: Value = serde_json::from_str(&answered).expect("the answer parses");
            assert_eq!(answered, row["expected"], "{name}");
        }
    }
}
