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
//!
//! By place only when the rule is the same AND the new series starts on an
//! occurrence of its own rule. A rule that names its weekdays ("every Monday")
//! moved to a Tuesday keeps its Mondays, and its first occurrence is not the
//! one the user moved; counting places there would shift every deletion by
//! one. Such a deletion stays on its day until the rule itself moves with the
//! date (decision 189, a later change).
//!
//! The shell expands both series — the device's zone is the shell's to know,
//! and the core reads no clock — and hands over each occurrence with the day it
//! falls on, on its series' own clock. The decisions are made here: which old
//! occurrence a deletion names, by place or by day, what is dropped, and how a
//! kept deletion is spelled: as the instant of the new series' own occurrence,
//! which is what every writer and every reader matches.
//!
//! Pinned row by row in `tests/fixtures/tailExceptions.json`; the `contract`
//! module below reads it, and so do the phone's door test in cal-ffi and the
//! TypeScript contract test through the WebAssembly door.

use chrono::DateTime;
use serde::{Deserialize, Serialize};

use crate::series_clock::{expansion_clock, ExpansionClock};

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
    /// The new series' occurrences from its start on, in order, as its rule
    /// generates them with no exception.
    pub tail: Vec<TailSlot>,
    /// The instant the new series starts at.
    pub tail_start: String,
    /// Whether the new series is all-day, and its zone.
    pub tail_all_day: bool,
    pub tail_tzid: Option<String>,
    /// Whether the user gave the new series another repeat rule.
    pub rule_changed: bool,
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
    /// By their day (decision 188, and a series not starting on its own rule).
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

    // By place only when the places line up: the same rule, and the new series
    // starting on an occurrence of it.
    let aligned = match (question.tail.first(), instant_ms(&question.tail_start)) {
        (Some(first), Some(start)) => instant_ms(&first.at).is_some_and(|at| same_tail(at, start)),
        _ => false,
    };
    let carried_by = if !question.rule_changed && aligned {
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
                    let day = &question.old_slots[place].day;
                    let mut on_day = question.tail.iter().filter(|occ| &occ.day == day);
                    // Two occurrences on one day (a rule that repeats within
                    // it): which one was meant cannot be told, so neither.
                    match (on_day.next(), on_day.next()) {
                        (Some(only), None) => Some(only),
                        _ => None,
                    }
                }
            })
            .and_then(|occ| instant_ms(&occ.at).map(|at| (at, occ.at.clone())));
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

    fn slot(at: &str, day: &str) -> TailSlot {
        TailSlot {
            at: at.to_string(),
            day: day.to_string(),
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
            tail,
            tail_all_day: false,
            tail_tzid: None,
            rule_changed: false,
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
        q.rule_changed = true;
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
        q.tail_all_day = true;
        let answer = tail_exceptions(&q);
        assert_eq!(answer.carried_by, TailCarry::Place);
        assert_eq!(answer.exceptions, vec!["2026-08-26T22:00:00.000Z"]);
    }

    #[test]
    fn a_series_not_starting_on_its_own_rule_keeps_deletions_on_their_day() {
        // "Every Monday" moved to a Tuesday: the rule still has Mondays, its
        // first occurrence is not the moved one (decision 189). Counted by
        // place, the deletion would land a week early.
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
        q.rule_changed = true;
        let answer = tail_exceptions(&q);
        assert!(answer.exceptions.is_empty());
        assert_eq!(answer.dropped, vec!["2026-08-24T20:00:00.000Z"]);
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
