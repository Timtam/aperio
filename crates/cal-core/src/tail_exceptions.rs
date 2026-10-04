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
//! By place only when every occurrence moves with the start. Either the
//! pattern is the same and the rule takes its days and times from the start
//! rather than naming them, or the rule names its days and moved with the
//! start the way "this and all following" moves it (decision 189,
//! [`begin_series_anew`](crate::series_shift::begin_series_anew)): the old
//! rule shifted by the days between the cut and the new start reads as the new
//! one, "every Monday" become "every Tuesday". And every occurrence handed over
//! moved by the same step: in the rule's own unit for a rule that follows its
//! start — days for a daily or weekly rule, months for a monthly one, years
//! for a yearly one — and in days for a moved rule, which moves each by as
//! many days as the start.
//!
//! A rule that names its days — weekdays, days of the month, months, or its
//! times of day — and did not move keeps them: "every Monday" whose Wednesday
//! occurrence was moved there on its own earlier, now edited from there on,
//! still repeats on Mondays (decision 192), and "every weekday" with a new rule
//! that only adds Saturday still has its Wednesday where it was. And a monthly
//! rule from the 31st skips the shorter months, so moved to the 30th it meets
//! months the old one skipped: its places stop lining up after the first short
//! month. Counting places there shifted a deletion onto an occurrence nobody
//! deleted. Such a deletion stays on its day, at the new time of day if that
//! changed; where the new series has no occurrence that day it is dropped,
//! which hides nothing.
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

use chrono::{DateTime, Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

// The rule is read once, for every module that asks (`rrule_parts`).
use crate::rrule_parts::{parse_freq, parse_interval, parse_parts, part, Freq, WEEKDAY_TOKENS};
use crate::series_clock::{canonical_zone, expansion_clock, ExpansionClock};
use crate::series_shift::{days_follow_start, shift_series, week_start_matters, SeriesShift};

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

/// What a rule repeats by, whatever it is spelled like and wherever it ends:
/// its parts without `COUNT` and `UNTIL`, the default `INTERVAL=1` and a week
/// start that changes no date left out, values uppercased and lists in order.
/// A week start counts only for week numbers and for a weekly rule every other
/// week (or rarer) on more than one weekday; the core's own shift adds one
/// that does not, and the repeat field drops it. `None` when it cannot be read.
fn pattern(rrule: &str) -> Option<Vec<(String, String)>> {
    let (_, parts) = parse_parts(rrule).ok()?;
    let weekly = part(&parts, "FREQ").is_some_and(|f| f.eq_ignore_ascii_case("WEEKLY"));
    let interval = parse_interval(part(&parts, "INTERVAL")).unwrap_or(1);
    let weekdays = part(&parts, "BYDAY").map_or(0, |v| v.split(',').count());
    let week_start_counts =
        part(&parts, "BYWEEKNO").is_some() || (weekly && interval > 1 && weekdays > 1);
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
            !(key == "INTERVAL" && value == "1")
                && !(key == "WKST" && (value == "MO" || !week_start_counts))
        })
        .collect();
    pattern.sort();
    Some(pattern)
}

/// A rule's [`pattern`] with the days its start gives it written out: weekly
/// from a Tuesday is weekly on Tuesday, monthly from the 11th is monthly on the
/// 11th, yearly from 29 February is yearly on 29 February, and yearly in March
/// from the 15th is yearly on 15 March; a week start that moves no day from
/// the start is left out. Two spellings of one series read alike
/// then — the same series keeps the same deletions, however the user got there
/// (decision 196); the repeat field writes the day out as soon as it is
/// touched.
fn pattern_from(rrule: &str, start: NaiveDate) -> Option<Vec<(String, String)>> {
    let mut pattern = pattern(rrule)?;
    let named = |key: &str| pattern.iter().any(|(k, _)| k == key);
    let implied: Vec<(&str, String)> = match rule_freq(rrule)? {
        Freq::Weekly if !named("BYDAY") => vec![(
            "BYDAY",
            WEEKDAY_TOKENS[start.weekday().num_days_from_monday() as usize].to_string(),
        )],
        Freq::Monthly if !named("BYDAY") && !named("BYMONTHDAY") => {
            vec![("BYMONTHDAY", start.day().to_string())]
        }
        Freq::Yearly
            if !["BYDAY", "BYMONTHDAY", "BYYEARDAY", "BYWEEKNO"]
                .iter()
                .any(|key| named(key)) =>
        {
            let mut implied = vec![("BYMONTHDAY", start.day().to_string())];
            if !named("BYMONTH") {
                implied.push(("BYMONTH", start.month().to_string()));
            }
            implied
        }
        _ => Vec::new(),
    };
    pattern.extend(
        implied
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    );
    // And a week start that moves no day from this start is none, whatever
    // the weekdays (`week_start_matters`).
    if !week_start_matters(rrule, start) {
        pattern.retain(|(key, _)| key != "WKST");
    }
    pattern.sort();
    Some(pattern)
}

/// How often a rule recurs, when it can be read.
fn rule_freq(rule: &str) -> Option<Freq> {
    parse_parts(rule)
        .ok()
        .and_then(|(_, parts)| part(&parts, "FREQ").and_then(parse_freq))
}

/// The day a slot falls on, read on the clock the caller chose.
fn date_of(slot: &TailSlot, day_of: &impl Fn(&TailSlot) -> String) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(&day_of(slot), "%Y-%m-%d").ok()
}

/// The unit the occurrences moved in, when the new series' rule is the old one
/// moving with the start (`start`, the new series' first occurrence) — whoever
/// wrote it and however it is spelled (decision 196). `None` when it is not:
/// the deletions then keep their days.
///
/// - A rule that takes its days from its start begins anew there
///   ([`begin_series_anew`](crate::series_shift::begin_series_anew)): the new
///   rule is it when both read alike from the new start — "monthly" from the
///   2nd is "monthly on the 2nd" — and its occurrences move in its own unit:
///   days for a daily or weekly rule, months for a monthly one, years for a
///   yearly one.
/// - A rule that names its days moves them by the days from the cut (`cut`,
///   the first old slot) to the new start, as a drag does (decision 189): the
///   new rule is it when it reads as the old one shifted that far, and every
///   occurrence moves by those days. No days, no move: a rule that only got a
///   new time of day keeps its days, and so do its deletions.
fn moved_unit(
    old_rule: &str,
    tail_rule: &str,
    cut: Option<&TailSlot>,
    start: Option<&TailSlot>,
    day_of: &impl Fn(&TailSlot) -> String,
) -> Option<Freq> {
    let to = date_of(start?, day_of)?;
    if days_follow_start(old_rule) {
        let old = pattern_from(old_rule, to)?;
        return if Some(old) == pattern_from(tail_rule, to) {
            rule_freq(tail_rule)
        } else {
            None
        };
    }
    let from = date_of(cut?, day_of)?;
    let days = i32::try_from((to - from).num_days()).ok()?;
    if days == 0 {
        return None;
    }
    match shift_series(old_rule, from, days, false, None) {
        SeriesShift::Shifted { rrule } => {
            let moved = pattern_from(&rrule, to)?;
            (Some(moved) == pattern_from(tail_rule, to)).then_some(Freq::Daily)
        }
        SeriesShift::Refused { .. } => None,
    }
}

/// Whether the new series' occurrences are the old series' occurrences moved
/// by one and the same step, place for place, as far as both reach, counted in
/// `unit`: days for a daily or weekly rule (and finer) and for a rule moved by
/// days, months for a monthly one, years for a yearly one. A rule that takes
/// its days from the start still skips months or years by its day — from the
/// 31st every month without one, from 29 February every year that is no leap
/// year — so moved onto another day it can meet months the old one skipped.
/// Counted in days instead, a monthly move from the 25th to the 2nd of the next
/// month would look uneven only because the months differ in length.
fn moved_alike(
    unit: Freq,
    old: &[TailSlot],
    tail: &[TailSlot],
    day_of: &impl Fn(&TailSlot) -> String,
) -> bool {
    let step = |date: NaiveDate| -> i64 {
        match unit {
            Freq::Monthly => i64::from(date.year()) * 12 + i64::from(date.month0()),
            Freq::Yearly => i64::from(date.year()),
            _ => i64::from(date.num_days_from_ce()),
        }
    };
    let at = |slot: &TailSlot| date_of(slot, day_of).map(step);
    let mut moved = None;
    for (before, after) in old.iter().zip(tail) {
        let (Some(before), Some(after)) = (at(before), at(after)) else {
            return false;
        };
        let by = after - before;
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
    // pattern with days taken from the start, or a rule moved with it, and the
    // new series starting on an occurrence of its own rule.
    let tail_start = instant_ms(&question.tail_start);
    let aligned = match (question.tail.first(), tail_start) {
        (Some(first), Some(start)) => instant_ms(&first.at).is_some_and(|at| same_tail(at, start)),
        _ => false,
    };
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
    // The unit the occurrences are compared in: the rule's own for a rule
    // that follows its start, days for one moved by days. Neither: by day.
    let unit = moved_unit(
        &question.old_rule,
        &question.tail_rule,
        question.old_slots.first(),
        question.tail.first(),
        &day_of,
    );
    let carried_by = match unit {
        Some(unit)
            if aligned && moved_alike(unit, &question.old_slots, &question.tail, &day_of) =>
        {
            TailCarry::Place
        }
        _ => TailCarry::Day,
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
    fn a_monthly_rule_moved_into_the_next_month_keeps_its_places() {
        // The 25th moved to the 2nd of the next month: every occurrence moves
        // one month on, though the days between differ with the months.
        let old = vec![
            slot("2026-08-25T08:00:00.000Z", "2026-08-25"),
            slot("2026-09-25T08:00:00.000Z", "2026-09-25"),
            slot("2026-10-25T09:00:00.000Z", "2026-10-25"),
        ];
        let tail = vec![
            slot("2026-09-02T08:00:00.000Z", "2026-09-02"),
            slot("2026-10-02T08:00:00.000Z", "2026-10-02"),
            slot("2026-11-02T09:00:00.000Z", "2026-11-02"),
        ];
        let mut q = question(old, tail, &["2026-10-25T09:00:00.000Z"]);
        q.old_rule = "FREQ=MONTHLY".into();
        q.tail_rule = "FREQ=MONTHLY".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Place);
        assert_eq!(answer.exceptions, vec!["2026-11-02T09:00:00.000Z"]);
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
        // A week start counts only where it can change a date: every other
        // week on more than one weekday, or week numbers.
        assert_eq!(pattern("FREQ=WEEKLY;WKST=SU"), pattern("FREQ=WEEKLY"));
        assert_eq!(
            pattern("FREQ=WEEKLY;INTERVAL=2;BYDAY=TU;WKST=TU"),
            pattern("FREQ=WEEKLY;INTERVAL=2;BYDAY=TU")
        );
        assert_ne!(
            pattern("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,SU;WKST=SU"),
            pattern("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,SU")
        );
        assert_ne!(
            pattern("FREQ=YEARLY;BYWEEKNO=1;WKST=SU"),
            pattern("FREQ=YEARLY;BYWEEKNO=1")
        );
        assert_eq!(pattern("FREQ=DAILY"), pattern("FREQ=DAILY;INTERVAL=1"));
        assert_eq!(
            pattern("FREQ=WEEKLY;BYDAY=MO,WE"),
            pattern("BYDAY=WE,MO;FREQ=WEEKLY;UNTIL=20261231T000000Z")
        );
        assert_ne!(pattern("FREQ=WEEKLY"), pattern("FREQ=WEEKLY;INTERVAL=2"));
    }

    /// Mondays at 09:00 Berlin from 24 August, and the same moved by `days`.
    fn mondays(days: i64) -> Vec<TailSlot> {
        (0..3)
            .map(|week| {
                let date = NaiveDate::from_ymd_opt(2026, 8, 24).unwrap()
                    + chrono::Duration::days(week * 7 + days);
                slot(
                    &format!("{}T07:00:00.000Z", date.format("%Y-%m-%d")),
                    &date.format("%Y-%m-%d").to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn a_rule_moved_with_the_start_keeps_its_places() {
        // "Every Monday" moved to Tuesday from 24 August on became "every
        // Tuesday" (decision 189): the deleted third Monday is the deleted
        // third Tuesday.
        let mut q = question(mondays(0), mondays(1), &["2026-09-07T07:00:00.000Z"]);
        q.old_rule = "FREQ=WEEKLY;BYDAY=MO".into();
        q.tail_rule = "FREQ=WEEKLY;BYDAY=TU".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Place);
        assert_eq!(answer.exceptions, vec!["2026-09-08T07:00:00.000Z"]);

        // Every weekday, moved a day on, is Tuesday to Saturday: the deleted
        // Thursday is the deleted Friday, though Thursday still occurs.
        let old = vec![
            slot("2026-08-24T07:00:00.000Z", "2026-08-24"),
            slot("2026-08-25T07:00:00.000Z", "2026-08-25"),
            slot("2026-08-26T07:00:00.000Z", "2026-08-26"),
            slot("2026-08-27T07:00:00.000Z", "2026-08-27"),
        ];
        let tail = vec![
            slot("2026-08-25T07:00:00.000Z", "2026-08-25"),
            slot("2026-08-26T07:00:00.000Z", "2026-08-26"),
            slot("2026-08-27T07:00:00.000Z", "2026-08-27"),
            slot("2026-08-28T07:00:00.000Z", "2026-08-28"),
        ];
        let mut q = question(old, tail, &["2026-08-27T07:00:00.000Z"]);
        q.old_rule = "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into();
        q.tail_rule = "FREQ=WEEKLY;BYDAY=TU,WE,TH,FR,SA".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Place);
        assert_eq!(answer.exceptions, vec!["2026-08-28T07:00:00.000Z"]);

        // The 10th of each month moved to the 11th, an end and all.
        let old = vec![
            slot("2026-09-10T07:00:00.000Z", "2026-09-10"),
            slot("2026-10-10T07:00:00.000Z", "2026-10-10"),
            slot("2026-11-10T08:00:00.000Z", "2026-11-10"),
        ];
        let tail = vec![
            slot("2026-09-11T07:00:00.000Z", "2026-09-11"),
            slot("2026-10-11T07:00:00.000Z", "2026-10-11"),
            slot("2026-11-11T08:00:00.000Z", "2026-11-11"),
        ];
        let mut q = question(old, tail, &["2026-10-10T07:00:00.000Z"]);
        q.old_rule = "FREQ=MONTHLY;BYMONTHDAY=10;UNTIL=20261231T000000Z".into();
        q.tail_rule = "FREQ=MONTHLY;BYMONTHDAY=11;UNTIL=20270101T000000Z".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Place);
        assert_eq!(answer.exceptions, vec!["2026-10-11T07:00:00.000Z"]);
    }

    #[test]
    fn another_spelling_of_the_moved_rule_is_that_move() {
        // Weekly from Monday, moved to Tuesday and written "every Tuesday":
        // the same series as weekly from Tuesday, so the same deletions (196).
        let mut q = question(mondays(0), mondays(1), &["2026-09-07T07:00:00.000Z"]);
        q.old_rule = "FREQ=WEEKLY".into();
        q.tail_rule = "FREQ=WEEKLY;BYDAY=TU".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Place);
        assert_eq!(answer.exceptions, vec!["2026-09-08T07:00:00.000Z"]);

        // And the other way: "every Monday" moved, written as plain weekly.
        q.old_rule = "FREQ=WEEKLY;BYDAY=MO".into();
        q.tail_rule = "FREQ=WEEKLY".into();
        assert_eq!(tail_exceptions(&q).carried_by, TailCarry::Place);

        let tuesday = NaiveDate::from_ymd_opt(2026, 8, 25).unwrap();
        assert_eq!(
            pattern_from("FREQ=WEEKLY", tuesday),
            pattern_from("FREQ=WEEKLY;BYDAY=TU", tuesday)
        );
        assert_eq!(
            pattern_from("FREQ=MONTHLY;COUNT=3", tuesday),
            pattern_from("FREQ=MONTHLY;BYMONTHDAY=25", tuesday)
        );
        assert_eq!(
            pattern_from("FREQ=YEARLY", tuesday),
            pattern_from("FREQ=YEARLY;BYMONTH=8;BYMONTHDAY=25", tuesday)
        );
        assert_ne!(
            pattern_from("FREQ=MONTHLY;BYDAY=4TU", tuesday),
            pattern_from("FREQ=MONTHLY;BYMONTHDAY=25", tuesday)
        );
        assert_ne!(
            pattern_from("FREQ=DAILY", tuesday),
            pattern_from("FREQ=DAILY;BYDAY=TU", tuesday)
        );
        assert_eq!(
            pattern_from("FREQ=YEARLY;BYMONTH=8", tuesday),
            pattern_from("FREQ=YEARLY;BYMONTH=8;BYMONTHDAY=25", tuesday)
        );
    }

    #[test]
    fn a_rule_that_follows_its_start_written_out_by_the_field_is_that_move() {
        // Monthly from the 25th, "this and all following" moved to the 2nd of
        // the next month; touching the repeat field writes the day out. The
        // same series as monthly from the 2nd, so the same deletions, counted
        // in months (196).
        let old = vec![
            slot("2026-09-25T07:00:00.000Z", "2026-09-25"),
            slot("2026-10-25T08:00:00.000Z", "2026-10-25"),
        ];
        let tail = vec![
            slot("2026-10-02T07:00:00.000Z", "2026-10-02"),
            slot("2026-11-02T08:00:00.000Z", "2026-11-02"),
        ];
        let mut q = question(old, tail, &["2026-10-25T08:00:00.000Z"]);
        q.old_rule = "FREQ=MONTHLY".into();
        q.tail_rule = "FREQ=MONTHLY;BYMONTHDAY=2;COUNT=4".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Place);
        assert_eq!(answer.exceptions, vec!["2026-11-02T08:00:00.000Z"]);
    }

    #[test]
    fn a_week_start_that_moves_no_day_from_the_start_is_no_new_rule() {
        // Every other Monday and Thursday, moved a day on: Tuesdays and Fridays
        // share a week whether weeks begin on Monday or Tuesday, so the field's
        // spelling without a week start is the moved rule (196).
        let tuesday = NaiveDate::from_ymd_opt(2026, 8, 25).unwrap();
        assert_eq!(
            pattern_from("FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,FR;WKST=TU", tuesday),
            pattern_from("FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,FR;COUNT=6", tuesday)
        );
        let friday = NaiveDate::from_ymd_opt(2026, 8, 28).unwrap();
        assert_ne!(
            pattern_from("FREQ=WEEKLY;INTERVAL=2;BYDAY=FR,MO;WKST=FR", friday),
            pattern_from("FREQ=WEEKLY;INTERVAL=2;BYDAY=FR,MO", friday)
        );
    }

    #[test]
    fn a_week_start_that_changes_no_date_is_no_new_rule() {
        // Every other Monday moved to Tuesday, its end then set in the field,
        // which writes no week start: the same series as the shift's.
        let fortnights = |first: u32| {
            (0..3)
                .map(|n| {
                    let date = NaiveDate::from_ymd_opt(2026, 8, first).unwrap()
                        + chrono::Duration::days(i64::from(n) * 14);
                    slot(
                        &format!("{}T07:00:00.000Z", date.format("%Y-%m-%d")),
                        &date.format("%Y-%m-%d").to_string(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut q = question(
            fortnights(24),
            fortnights(25),
            &["2026-09-21T07:00:00.000Z"],
        );
        q.old_rule = "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO".into();
        q.tail_rule = "FREQ=WEEKLY;INTERVAL=2;BYDAY=TU;COUNT=3".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Place);
        assert_eq!(answer.exceptions, vec!["2026-09-22T07:00:00.000Z"]);
    }

    #[test]
    fn a_rule_that_did_not_move_with_the_start_keeps_its_days() {
        // A rule set anew on the moved day is no move of the old one: the
        // deleted Monday names no day of "Tuesdays and Fridays".
        let tail = vec![
            slot("2026-08-25T07:00:00.000Z", "2026-08-25"),
            slot("2026-08-28T07:00:00.000Z", "2026-08-28"),
            slot("2026-09-01T07:00:00.000Z", "2026-09-01"),
        ];
        let mut q = question(mondays(0), tail, &["2026-09-07T07:00:00.000Z"]);
        q.old_rule = "FREQ=WEEKLY;BYDAY=MO".into();
        q.tail_rule = "FREQ=WEEKLY;BYDAY=TU,FR".into();
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Day);
        assert!(answer.exceptions.is_empty());

        // Moved by two days, but the rule by one: the new series does not
        // start on its own rule, so nothing lines up.
        let mut q = question(mondays(0), mondays(1), &["2026-09-07T07:00:00.000Z"]);
        q.old_rule = "FREQ=WEEKLY;BYDAY=MO".into();
        q.tail_rule = "FREQ=WEEKLY;BYDAY=TU".into();
        q.tail_start = "2026-08-26T07:00:00.000Z".into();
        assert_eq!(tail_exceptions(&q).carried_by, TailCarry::Day);

        // "The second Tuesday" cannot move by a day, so no rule is that move.
        let old = vec![
            slot("2026-09-08T07:00:00.000Z", "2026-09-08"),
            slot("2026-10-13T07:00:00.000Z", "2026-10-13"),
        ];
        let tail = vec![
            slot("2026-09-09T07:00:00.000Z", "2026-09-09"),
            slot("2026-10-14T07:00:00.000Z", "2026-10-14"),
        ];
        let mut q = question(old, tail, &["2026-10-13T07:00:00.000Z"]);
        q.old_rule = "FREQ=MONTHLY;BYDAY=2TU".into();
        q.tail_rule = "FREQ=MONTHLY;BYDAY=2WE".into();
        assert_eq!(tail_exceptions(&q).carried_by, TailCarry::Day);
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
            "C4 \"every Monday\" kept by hand on a Tuesday start keeps its deletion on Monday",
            "\"every weekday\" kept by hand on a moved start keeps its deletions",
            "B a rule set to the moved weekday is the moved rule",
            "\"every weekday\" moved with the start keeps its places",
            "a rule that follows its start, written out by the repeat field, is the moved rule",
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
