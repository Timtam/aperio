//! Changing the clock a series repeats on (DESIGN-series-time-zone.md, stage 9;
//! decisions 15a, 20a, 24a).
//!
//! A series repeats at one wall time on its clock: its zone, or UTC. Choosing
//! another zone keeps the start's instant, so the start can fall on another
//! day on the new clock. Where the rule names days (weekdays, a day of the
//! month, a month), they can then name a different day than the one the
//! series stood on, and the editors ask which days it should keep (15a):
//!
//! - **as they fall now:** the start stays; the rule names the days the
//!   occurrences fall on on the new clock, moved as a drag moves them
//!   (`series_shift`), in one step from the old clock to the new;
//! - **as the rule says:** the rule's text stays, and the start moves back
//!   onto its old day where the rule's days fit only that one.
//!
//! In both, every exception that cancels an occurrence moves with it, and one
//! that cancels nothing on the old clock stays as it is: a deletion from
//! before a zone was lost then deletes again once the zone is back. An answer
//! that would put the start or an exception into a time the new clock skips
//! is refused with that reason; so is one `series_shift` refuses (20a).
//!
//! A series whose days follow its start (24a) is not asked, unless it cannot
//! keep them on the new clock (monthly on the 31st): its answer is the first.
//! A rule that names only the day its start falls on anyway counts as one
//! whose days follow its start; the repeat field writes the day of the month
//! that way.
//!
//! Pinned by `tests/fixtures/seriesClockChange.json`; the surfaces replay it.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

use cal_zone::{Reading, Zone};

use crate::rrule_parts::{parse_freq, parse_parts, part, Freq, Part, WEEKDAY_TOKENS};
use crate::wall_clock::clock_of;
use crate::{EndReading, SeriesShift, ShiftRefusal};

/// A question: a series as it is stored, and the zone it is to repeat on.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ClockChangeQuestion {
    /// The series' rule, with or without `RRULE:`.
    pub rrule: String,
    /// The series' start, an RFC 3339 instant.
    pub start: String,
    /// The instants its exceptions cancel.
    #[serde(default)]
    pub exceptions: Vec<String>,
    /// The zone name the series stores now; `None` for none (UTC).
    pub from_tzid: Option<String>,
    /// The zone it is to repeat on; `None` for UTC.
    pub to_tzid: Option<String>,
}

/// Which clock's start the rule's named days fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum RuleFit {
    /// Only the start's day on the old clock.
    Before,
    /// Only the start's day on the new clock.
    After,
    /// Both days.
    Both,
    /// Neither day.
    Neither,
    /// The rule cannot say (a set position, a week number, a day of the year).
    Unknown,
}

/// Why an answer cannot be carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ClockChangeRefusal {
    /// The rule, a value in it, the start or an exception cannot be read.
    Unreadable,
    /// `BYDAY` with an ordinal, such as `2SU`.
    OrdinalWeekday,
    /// `BYSETPOS`.
    SetPosition,
    /// `BYYEARDAY`.
    YearDay,
    /// `BYWEEKNO`.
    WeekNumber,
    /// `BYHOUR`, `BYMINUTE` or `BYSECOND`, while the time of day changes.
    TimeOfDay,
    /// A negative `BYMONTHDAY`.
    NegativeMonthDay,
    /// A day of the month past the 28th, or a move out of the month.
    MonthEnd,
    /// The rule recurs only in some months or years.
    LimitedMonths,
    /// A yearly rule whose move touches the end of February.
    LeapDay,
    /// A part the rule reader does not know.
    UnknownPart,
    /// The start or an exception would fall on a time the new clock skips.
    SkippedTime,
}

impl From<ShiftRefusal> for ClockChangeRefusal {
    fn from(reason: ShiftRefusal) -> Self {
        match reason {
            ShiftRefusal::Unreadable => Self::Unreadable,
            ShiftRefusal::OrdinalWeekday => Self::OrdinalWeekday,
            ShiftRefusal::SetPosition => Self::SetPosition,
            ShiftRefusal::YearDay => Self::YearDay,
            ShiftRefusal::WeekNumber => Self::WeekNumber,
            ShiftRefusal::TimeOfDay => Self::TimeOfDay,
            ShiftRefusal::NegativeMonthDay => Self::NegativeMonthDay,
            ShiftRefusal::MonthEnd => Self::MonthEnd,
            ShiftRefusal::LimitedMonths => Self::LimitedMonths,
            ShiftRefusal::LeapDay => Self::LeapDay,
            ShiftRefusal::UnknownPart => Self::UnknownPart,
        }
    }
}

/// One answer to the question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum ClockChangeAnswer {
    /// The series as it is written on the new clock.
    Changed {
        rrule: String,
        /// RFC 3339.
        start: String,
        /// RFC 3339, in the order they came.
        exceptions: Vec<String>,
    },
    /// This answer cannot be carried out.
    Refused { reason: ClockChangeRefusal },
}

/// What changing a series' clock writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SeriesClockChange {
    /// Whether the editors ask which days to keep (15a, 24a).
    pub asks: bool,
    /// The start's day on the old clock, `YYYY-MM-DD`.
    pub start_day_before: String,
    /// The start's day on the new clock.
    pub start_day_after: String,
    /// Which of the two days the rule's named days fit.
    pub fit: RuleFit,
    /// "As they fall now": the start stays, the rule names the days the
    /// occurrences fall on on the new clock. The answer a series that is not
    /// asked gets.
    pub as_now: ClockChangeAnswer,
    /// "As the rule says": the rule's text stays, the start moves back onto
    /// its old day where the rule's days fit only that one.
    pub as_rule: ClockChangeAnswer,
}

/// The parts that name the days a rule repeats on.
const DAY_PARTS: [&str; 6] = [
    "BYDAY",
    "BYMONTHDAY",
    "BYMONTH",
    "BYYEARDAY",
    "BYWEEKNO",
    "BYSETPOS",
];

fn days_in_month(day: NaiveDate) -> i64 {
    let next = if day.month() == 12 {
        NaiveDate::from_ymd_opt(day.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(day.year(), day.month() + 1, 1)
    };
    next.and_then(|n| n.pred_opt())
        .map_or(31, |l| i64::from(l.day()))
}

fn days_in_year(day: NaiveDate) -> i64 {
    if NaiveDate::from_ymd_opt(day.year(), 2, 29).is_some() {
        366
    } else {
        365
    }
}

fn weekday_token(day: NaiveDate) -> &'static str {
    WEEKDAY_TOKENS[day.weekday().num_days_from_monday() as usize]
}

/// Whether the rule's named days include `day`: `None` where the rule names
/// them in a way this cannot follow (a set position, a week number, a day of
/// the year) or cannot be read, `Some(true)` for a rule that names none.
///
/// Counted weekdays follow RFC 5545: in a monthly rule, or a yearly one with
/// `BYMONTH`, `2SU` is the month's second Sunday and `-1FR` its last Friday;
/// in a yearly rule without `BYMONTH`, they count the weeks of the year. A
/// negative day of the month counts from its end.
pub fn rule_names_day(rrule: &str, day: NaiveDate) -> Option<bool> {
    let (_, parts) = parse_parts(rrule).ok()?;
    let freq = parse_freq(part(&parts, "FREQ")?)?;
    for key in ["BYYEARDAY", "BYWEEKNO", "BYSETPOS"] {
        if part(&parts, key).is_some() {
            return None;
        }
    }
    let numbers = |key: &str| -> Option<Option<Vec<i64>>> {
        match part(&parts, key) {
            None => Some(None),
            Some(value) => value
                .split(',')
                .map(|n| n.trim().parse::<i64>().ok())
                .collect::<Option<Vec<_>>>()
                .map(Some),
        }
    };
    let months = numbers("BYMONTH")?;
    if let Some(months) = &months {
        if !months.contains(&i64::from(day.month())) {
            return Some(false);
        }
    }
    if let Some(month_days) = numbers("BYMONTHDAY")? {
        let dom = i64::from(day.day());
        let last = days_in_month(day);
        let hit = month_days.iter().any(|d| {
            if *d > 0 {
                *d == dom
            } else {
                last + d + 1 == dom
            }
        });
        if !hit {
            return Some(false);
        }
    }
    if let Some(value) = part(&parts, "BYDAY") {
        let in_months =
            matches!(freq, Freq::Monthly) || (matches!(freq, Freq::Yearly) && months.is_some());
        let in_year = matches!(freq, Freq::Yearly) && months.is_none();
        let mut hit = false;
        for token in value.split(',') {
            let token = token.trim().to_ascii_uppercase();
            if token.len() < 2 || !token.is_ascii() {
                return None;
            }
            let (ordinal, code) = token.split_at(token.len() - 2);
            if !WEEKDAY_TOKENS.contains(&code) {
                return None;
            }
            if code != weekday_token(day) {
                continue;
            }
            if ordinal.is_empty() {
                hit = true;
                continue;
            }
            let n: i64 = ordinal.parse().ok()?;
            let (index, from_end) = if in_months {
                let dom = i64::from(day.day());
                ((dom - 1) / 7 + 1, (days_in_month(day) - dom) / 7 + 1)
            } else if in_year {
                let yday = i64::from(day.ordinal());
                ((yday - 1) / 7 + 1, (days_in_year(day) - yday) / 7 + 1)
            } else {
                // An ordinal in a daily or weekly rule means nothing to RFC 5545.
                return None;
            };
            if (n > 0 && n == index) || (n < 0 && -n == from_end) {
                hit = true;
            }
        }
        if !hit {
            return Some(false);
        }
    }
    Some(true)
}

/// Whether a rule takes its days from its start: it names none, or names only
/// the start's own day (`BYDAY=MO` from a Monday, `BYMONTHDAY=15` from the
/// 15th, `BYMONTH=3` from March).
fn days_follow_start(parts: &[Part], start: NaiveDate) -> bool {
    parts
        .iter()
        .filter(|p| DAY_PARTS.contains(&p.key.as_str()))
        .all(|p| {
            let value = p.value.trim();
            match p.key.as_str() {
                "BYDAY" => value.eq_ignore_ascii_case(weekday_token(start)),
                "BYMONTHDAY" => value.parse::<u32>().ok() == Some(start.day()),
                "BYMONTH" => value.parse::<u32>().ok() == Some(start.month()),
                _ => false,
            }
        })
}

/// The instant `wall` names on `zone`: a time it skips refuses, a time it
/// shows twice takes the earlier reading, as a typed time does.
fn instant_on(zone: Zone, wall: NaiveDateTime) -> Result<DateTime<Utc>, ClockChangeRefusal> {
    let on = |offset: chrono::FixedOffset| {
        DateTime::<Utc>::from_naive_utc_and_offset(
            wall - Duration::seconds(i64::from(offset.local_minus_utc())),
            Utc,
        )
    };
    match zone.reading(wall) {
        Reading::Single(offset) => Ok(on(offset)),
        Reading::Fold { earlier, .. } => Ok(on(earlier)),
        Reading::Gap { .. } => Err(ClockChangeRefusal::SkippedTime),
    }
}

fn instant_text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn rule_until_text(at: DateTime<Utc>) -> String {
    at.format("%Y%m%dT%H%M%SZ").to_string()
}

/// `rrule` with its `UNTIL` set to `value`, every other part as written.
fn with_until(rrule: &str, value: &str) -> String {
    let trimmed = rrule.trim();
    trimmed
        .split(';')
        .map(|part| match part.split_once('=') {
            Some((key, _)) if key.trim().eq_ignore_ascii_case("UNTIL") => {
                format!("{}={value}", key.trim())
            }
            _ => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// The answer to one question; `None` for a start that names no instant.
pub fn series_clock_change(question: &ClockChangeQuestion) -> Option<SeriesClockChange> {
    let start = DateTime::parse_from_rfc3339(&question.start)
        .ok()?
        .with_timezone(&Utc);
    let (before_zone, _) = clock_of(question.from_tzid.as_deref());
    let (after_zone, _) = clock_of(question.to_tzid.as_deref());
    let wall_before = before_zone.wall(start);
    let wall_after = after_zone.wall(start);
    let day_before = wall_before.date();
    let day_after = wall_after.date();
    let days = i32::try_from((day_after - day_before).num_days()).ok()?;
    let time_changes = wall_before.time() != wall_after.time();
    let rrule = question.rrule.trim();

    let parts = parse_parts(rrule).ok().map(|(_, parts)| parts);
    let follows = parts
        .as_ref()
        .is_some_and(|parts| days_follow_start(parts, day_before));
    let fit = match (
        rule_names_day(rrule, day_before),
        rule_names_day(rrule, day_after),
    ) {
        (Some(true), Some(true)) => RuleFit::Both,
        (Some(true), Some(false)) => RuleFit::Before,
        (Some(false), Some(true)) => RuleFit::After,
        (Some(false), Some(false)) => RuleFit::Neither,
        _ => RuleFit::Unknown,
    };

    // The end, read on the old clock (101b). An instant (a cut) moves with
    // the occurrences as the end of its last day on the new clock; through a
    // date and a wall time move by days in `series_shift` (as they fall now)
    // or stay (as the rule says).
    let end = crate::read_series_end(
        rrule,
        wall_before.time(),
        crate::expansion_clock(false, question.from_tzid.as_deref()),
        |at| before_zone.wall(at),
        |wall| crate::wall_clock::last_at_or_before(before_zone, wall),
        |wall| before_zone.to_utc(wall, cal_zone::WallPolicy::Rfc5545),
    );
    let instant_end_day = end
        .as_ref()
        .filter(|end| end.reading == EndReading::Instant)
        .and_then(|end| NaiveDate::parse_from_str(&end.end_day, "%Y-%m-%d").ok());
    let end_on_new_clock = |day: NaiveDate| -> Option<String> {
        let last = day.and_hms_opt(23, 59, 59)?;
        crate::wall_clock::last_at_or_before(after_zone, last).map(rule_until_text)
    };

    // Each exception that cancels an occurrence on the old clock: one at the
    // series' time of day on a day the rule names. Others cancel nothing and
    // stay as they are.
    let cancels = |at: DateTime<Utc>| -> bool {
        let wall = before_zone.wall(at);
        if wall.time() != wall_before.time() {
            return false;
        }
        if follows {
            return true;
        }
        rule_names_day(rrule, wall.date()) != Some(false)
    };
    let move_exceptions = |by_days: i64| -> Result<Vec<String>, ClockChangeRefusal> {
        question
            .exceptions
            .iter()
            .map(|text| {
                let at = DateTime::parse_from_rfc3339(text)
                    .map_err(|_| ClockChangeRefusal::Unreadable)?
                    .with_timezone(&Utc);
                if !cancels(at) {
                    return Ok(text.clone());
                }
                let day = before_zone.wall(at).date() + Duration::days(by_days);
                instant_on(after_zone, NaiveDateTime::new(day, wall_after.time())).map(instant_text)
            })
            .collect()
    };

    // As they fall now.
    let until_now = instant_end_day
        .and_then(|day| day.checked_add_signed(Duration::days(i64::from(days))))
        .and_then(end_on_new_clock);
    let as_now =
        match crate::shift_series(rrule, day_before, days, time_changes, until_now.as_deref()) {
            SeriesShift::Refused { reason } => ClockChangeAnswer::Refused {
                reason: reason.into(),
            },
            SeriesShift::Shifted { rrule: moved } => match move_exceptions(i64::from(days)) {
                Ok(exceptions) => ClockChangeAnswer::Changed {
                    rrule: moved,
                    start: question.start.clone(),
                    exceptions,
                },
                Err(reason) => ClockChangeAnswer::Refused { reason },
            },
        };

    // As the rule says.
    let has_time_parts = parts.as_ref().is_some_and(|parts| {
        ["BYHOUR", "BYMINUTE", "BYSECOND"]
            .iter()
            .any(|k| part(parts, k).is_some())
    });
    let as_rule = if parts.is_none() {
        ClockChangeAnswer::Refused {
            reason: ClockChangeRefusal::Unreadable,
        }
    } else if has_time_parts && time_changes {
        ClockChangeAnswer::Refused {
            reason: ClockChangeRefusal::TimeOfDay,
        }
    } else {
        let moves_back = days != 0 && (follows || fit == RuleFit::Before);
        let start_after = if moves_back {
            instant_on(
                after_zone,
                NaiveDateTime::new(day_before, wall_after.time()),
            )
            .map(instant_text)
        } else {
            Ok(question.start.clone())
        };
        let rule_after = match instant_end_day.and_then(end_on_new_clock) {
            Some(until) => with_until(rrule, &until),
            None => rrule.to_string(),
        };
        match (start_after, move_exceptions(0)) {
            (Ok(start), Ok(exceptions)) => ClockChangeAnswer::Changed {
                rrule: rule_after,
                start,
                exceptions,
            },
            (Err(reason), _) | (_, Err(reason)) => ClockChangeAnswer::Refused { reason },
        }
    };

    let asks = days != 0 && (!follows || matches!(as_now, ClockChangeAnswer::Refused { .. }));
    Some(SeriesClockChange {
        asks,
        start_day_before: day_before.format("%Y-%m-%d").to_string(),
        start_day_after: day_after.format("%Y-%m-%d").to_string(),
        fit,
        as_now,
        as_rule,
    })
}

/// [`series_clock_change`] over the wire: a question as JSON in, the answer as
/// JSON out, JSON `null` for a start that names no instant. An error only for
/// JSON that is no question at all.
pub fn series_clock_change_json(question_json: &str) -> Result<String, serde_json::Error> {
    let question: ClockChangeQuestion = serde_json::from_str(question_json)?;
    serde_json::to_string(&series_clock_change(&question))
}

#[cfg(test)]
mod contract {
    //! The shared fixture.

    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/seriesClockChange.json"
        ));
        serde_json::from_str(text).expect("the fixture parses")
    }

    #[test]
    fn every_row_gives_its_answer() {
        let doc = fixture();
        let rows = doc["rows"].as_array().expect("rows");
        // Anti-silence: the rows the rule turns on, by name.
        for needed in [
            "lost-berlin-zone-monday-rule",
            "a-series-that-means-utc-sundays",
            "same-day-moves-cancelling-exceptions-only",
            "monthly-on-the-31st-cannot-keep-its-days",
            "moved-start-in-the-spring-gap",
            "a-cut-moves-as-the-end-of-its-last-day",
        ] {
            assert!(
                rows.iter().any(|r| r["name"] == needed),
                "the fixture lost {needed}"
            );
        }
        for row in rows {
            let answer: Value = serde_json::from_str(
                &series_clock_change_json(&row["question"].to_string()).expect("a question"),
            )
            .expect("json");
            assert_eq!(answer, row["expect"], "{}", row["name"]);
        }
    }

    #[test]
    fn text_that_is_no_question_is_an_error() {
        assert!(series_clock_change_json("{\"rrule\":1}").is_err());
        assert!(series_clock_change_json("not json").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("a date")
    }

    #[test]
    fn counted_weekdays_and_days_from_the_end() {
        // The second Sunday of November 2026 is the 8th, the last Friday the 27th.
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYDAY=2SU", day(2026, 11, 8)),
            Some(true)
        );
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYDAY=2SU", day(2026, 11, 15)),
            Some(false)
        );
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYDAY=-1FR", day(2026, 11, 27)),
            Some(true)
        );
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYDAY=-1FR", day(2026, 11, 20)),
            Some(false)
        );
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYMONTHDAY=-1", day(2026, 2, 28)),
            Some(true)
        );
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYMONTHDAY=-2", day(2028, 2, 28)),
            Some(true)
        );
        // A yearly rule without a month counts the weeks of the year.
        assert_eq!(
            rule_names_day("FREQ=YEARLY;BYDAY=1MO", day(2026, 1, 5)),
            Some(true)
        );
        assert_eq!(
            rule_names_day("FREQ=YEARLY;BYDAY=-1SU", day(2026, 12, 27)),
            Some(true)
        );
        // With a month, within that month.
        assert_eq!(
            rule_names_day("FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU", day(2026, 3, 29)),
            Some(true)
        );
        assert_eq!(
            rule_names_day("FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU", day(2026, 10, 25)),
            Some(false)
        );
        // What it cannot follow.
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYDAY=MO;BYSETPOS=1", day(2026, 11, 2)),
            None
        );
    }
}
