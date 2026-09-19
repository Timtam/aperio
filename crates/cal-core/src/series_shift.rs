//! Shifting a recurring series by whole days: the rule that dragging a whole
//! series onto another day writes.
//!
//! Moving a series by N days has to move every occurrence by N days. For the
//! rule that means: a weekday list moves round the week (a Monday rule becomes
//! a Tuesday rule), a day of the month moves within the month, and `UNTIL`
//! moves with the series. Days are counted on the series' own clock — its zone,
//! or UTC for a series without one — because that is the clock its rule is read
//! on. The start and the exceptions are instants and move in the shell, which
//! knows the zone; so does a UTC `UNTIL`, which the shell hands in already
//! moved. This module rewrites only the rule.
//!
//! Some rules cannot move every occurrence by the same number of days without
//! changing what they mean. Those are refused rather than bent (Toni,
//! 2026-09-14): the surface says so and offers to move only the occurrence.
//! Refused are an ordinal weekday ("the second Sunday"), `BYSETPOS`,
//! `BYYEARDAY`, `BYWEEKNO`, a negative day of the month, a day of the month
//! past the 28th or a shift that leaves the month (months differ in length), a
//! rule that recurs only in some months or years, a yearly rule whose shift
//! touches the end of February (leap years), time-of-day parts when the time
//! changes too, a part given twice, and any part this module does not know.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

// The rule is read once, for every module that asks (`rrule_parts`).
use crate::rrule_parts::{
    is_utc_date_time, parse_freq, parse_interval, parse_parts, part, Part, WEEKDAY_TOKENS,
};

/// Why a rule cannot move by whole days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum ShiftRefusal {
    /// The rule, its `FREQ`, a value in it or the start day cannot be read, or
    /// a part is given twice.
    Unreadable,
    /// `BYDAY` with an ordinal, such as `2SU` or `-1FR`.
    OrdinalWeekday,
    /// `BYSETPOS`.
    SetPosition,
    /// `BYYEARDAY`.
    YearDay,
    /// `BYWEEKNO`.
    WeekNumber,
    /// `BYHOUR`, `BYMINUTE` or `BYSECOND`, when the time of day changes too.
    TimeOfDay,
    /// A negative `BYMONTHDAY`, counted from the end of the month.
    NegativeMonthDay,
    /// A day of the month past the 28th, or a shift out of the month.
    MonthEnd,
    /// The rule recurs only in some months or years — `BYMONTH` on a daily or
    /// weekly rule or with weekdays, or an `INTERVAL` over months or years with
    /// weekdays or days of the month — and moved days would leave them.
    LimitedMonths,
    /// A yearly rule whose shift touches the end of February.
    LeapDay,
    /// A part this module does not know.
    UnknownPart,
}

/// What shifting a series by whole days writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum SeriesShift {
    /// The rule to store with the moved series.
    Shifted { rrule: String },
    /// The rule cannot move by whole days.
    Refused { reason: ShiftRefusal },
}

/// The question the door takes.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct SeriesShiftQuestion {
    /// The series' rule, with or without the `RRULE:` prefix.
    pub rrule: String,
    /// The day the series starts, `YYYY-MM-DD`, on the series' own clock: its
    /// zone, or UTC for a series without one.
    pub start: String,
    /// Whole days to move it by, on that clock; negative moves it earlier.
    pub days: i32,
    /// Whether the time of day changes as well.
    #[serde(default)]
    pub time_changes: bool,
    /// The moved bound for a rule whose `UNTIL` is a UTC date-time, written
    /// `YYYYMMDDTHHMMSSZ`. That bound is an instant: it moves on the series'
    /// clock by the days and by the change in time of day, which needs the zone
    /// the shell has. Without it a UTC `UNTIL` moves by whole UTC days. A date
    /// or a floating `UNTIL` moves by days here, and this is ignored for it.
    #[serde(default)]
    pub until: Option<String>,
}

/// The rule for a series that starts on `start` and moves by `days`, with the
/// time of day changing too when `time_changes`. `until` is the moved UTC bound,
/// see [`SeriesShiftQuestion::until`].
pub fn shift_series(
    rrule: &str,
    start: NaiveDate,
    days: i32,
    time_changes: bool,
    until: Option<&str>,
) -> SeriesShift {
    match shift(rrule, start, days, time_changes, until) {
        Ok(rrule) => SeriesShift::Shifted { rrule },
        Err(reason) => SeriesShift::Refused { reason },
    }
}

/// The door: a [`SeriesShiftQuestion`] as JSON in, a [`SeriesShift`] as JSON out.
pub fn series_shift_json(input_json: &str) -> Result<String, serde_json::Error> {
    let question: SeriesShiftQuestion = serde_json::from_str(input_json)?;
    let answer = match NaiveDate::parse_from_str(&question.start, "%Y-%m-%d") {
        Ok(start) => shift_series(
            &question.rrule,
            start,
            question.days,
            question.time_changes,
            question.until.as_deref(),
        ),
        Err(_) => SeriesShift::Refused {
            reason: ShiftRefusal::Unreadable,
        },
    };
    serde_json::to_string(&answer)
}

fn shift(
    rrule: &str,
    start: NaiveDate,
    days: i32,
    time_changes: bool,
    until: Option<&str>,
) -> Result<String, ShiftRefusal> {
    use ShiftRefusal::*;

    let trimmed = rrule.trim();
    let (prefix, parts) = parse_parts(trimmed).map_err(|_| Unreadable)?;
    let get = |key: &str| part(&parts, key);

    let freq = get("FREQ").ok_or(Unreadable)?.to_ascii_uppercase();
    if parse_freq(&freq).is_none() {
        return Err(Unreadable);
    }
    let has_time_parts = ["BYHOUR", "BYMINUTE", "BYSECOND"]
        .iter()
        .any(|k| get(k).is_some());
    if time_changes && has_time_parts {
        return Err(TimeOfDay);
    }
    if days == 0 {
        // No day moves. Only a UTC bound can, with the time of day.
        return match (get("UNTIL"), until) {
            (Some(value), Some(moved)) if is_utc_date_time(value) => {
                let moved = checked_utc_until(moved)?;
                Ok(write(prefix, &parts, &[("UNTIL", moved)]))
            }
            _ => Ok(trimmed.to_string()),
        };
    }
    for part in &parts {
        match part.key.as_str() {
            "FREQ" | "INTERVAL" | "COUNT" | "UNTIL" | "BYDAY" | "BYMONTHDAY" | "BYMONTH"
            | "WKST" | "BYHOUR" | "BYMINUTE" | "BYSECOND" => {}
            "BYSETPOS" => return Err(SetPosition),
            "BYYEARDAY" => return Err(YearDay),
            "BYWEEKNO" => return Err(WeekNumber),
            _ => return Err(UnknownPart),
        }
    }

    let interval = parse_interval(get("INTERVAL")).ok_or(Unreadable)?;
    let month_restricted = get("BYMONTH").is_some();
    let by_months = matches!(freq.as_str(), "MONTHLY" | "YEARLY");
    if month_restricted && !by_months {
        // "Every day in March" moved by a day would take March 31 into April
        // and leave March 1 empty.
        return Err(LimitedMonths);
    }
    let new_start = start
        .checked_add_signed(Duration::days(i64::from(days)))
        .ok_or(Unreadable)?;
    let mut replaced: Vec<(&str, String)> = Vec::new();

    let by_day = match get("BYDAY") {
        Some(value) => {
            let tokens: Vec<String> = value
                .split(',')
                .map(|t| t.trim().to_ascii_uppercase())
                .collect();
            let mut moved = Vec::with_capacity(tokens.len());
            for token in &tokens {
                if !WEEKDAY_TOKENS.contains(&token.as_str()) {
                    let ordinal = token.len() > 2
                        && token.is_ascii()
                        && WEEKDAY_TOKENS.contains(&&token[token.len() - 2..]);
                    return Err(if ordinal { OrdinalWeekday } else { Unreadable });
                }
                moved.push(shift_weekday(token, days));
            }
            if by_months && (month_restricted || interval > 1) {
                // "Every Monday in March", or Mondays every other month: a
                // Monday at the end of a month in the set would move into a
                // month outside it, and one outside would move in.
                return Err(LimitedMonths);
            }
            replaced.push(("BYDAY", moved.join(",")));
            Some(tokens)
        }
        None => None,
    };

    let by_month_day = get("BYMONTHDAY");
    if let Some(value) = by_month_day {
        let mut moved = Vec::new();
        for token in value.split(',') {
            let day: i32 = token.trim().parse().map_err(|_| Unreadable)?;
            if day == 0 {
                return Err(Unreadable);
            }
            if day < 0 {
                return Err(NegativeMonthDay);
            }
            let to = day + days;
            if !(1..=28).contains(&day) || !(1..=28).contains(&to) {
                return Err(MonthEnd);
            }
            moved.push(to.to_string());
        }
        if interval > 1 && leaves_period(&freq, start, new_start) {
            // Every other month (or year) is counted from the one the start is
            // in; a start moved into the next one would pick the other set.
            return Err(LimitedMonths);
        }
        replaced.push(("BYMONTHDAY", moved.join(",")));
    }

    let day_of_month_from_start = by_month_day.is_none()
        && by_day.is_none()
        && (freq == "MONTHLY" || (freq == "YEARLY" && month_restricted));
    if day_of_month_from_start {
        if start.day() > 28 || new_start.day() > 28 {
            return Err(MonthEnd);
        }
        if (start.year(), start.month()) != (new_start.year(), new_start.month()) {
            return Err(if month_restricted {
                LimitedMonths
            } else {
                MonthEnd
            });
        }
    }
    if freq == "YEARLY"
        && !month_restricted
        && by_month_day.is_none()
        && by_day.is_none()
        && touches_end_of_february(start, new_start)
    {
        return Err(LeapDay);
    }

    if freq == "WEEKLY" && interval > 1 {
        if let Some(week_start) = get("WKST") {
            let week_start = week_start.to_ascii_uppercase();
            if !WEEKDAY_TOKENS.contains(&week_start.as_str()) {
                return Err(Unreadable);
            }
            replaced.push(("WKST", shift_weekday(&week_start, days)));
        } else if by_day.is_some() {
            // Every other week on given weekdays: which weeks are "on" is
            // counted from the week the start falls in, and that week depends
            // on where weeks begin. The week's first day moves with the days.
            replaced.push(("WKST", shift_weekday("MO", days)));
        }
    }

    if let Some(value) = get("UNTIL") {
        let moved = match until {
            Some(moved) if is_utc_date_time(value) => checked_utc_until(moved)?,
            _ => shift_until(value, days)?,
        };
        replaced.push(("UNTIL", moved));
    }

    Ok(write(prefix, &parts, &replaced))
}

/// The rule written back: each part as it was, or its replacement, and a week
/// start appended when one was added.
fn write(prefix: &str, parts: &[Part], replaced: &[(&str, String)]) -> String {
    let mut out: Vec<String> = parts
        .iter()
        .map(|p| match replaced.iter().find(|(k, _)| *k == p.key) {
            Some((k, v)) => format!("{k}={v}"),
            None => p.raw.trim().to_string(),
        })
        .collect();
    if let Some((_, v)) = replaced.iter().find(|(k, _)| *k == "WKST") {
        if !parts.iter().any(|p| p.key == "WKST") {
            out.push(format!("WKST={v}"));
        }
    }
    format!("{prefix}{}", out.join(";"))
}

fn shift_weekday(token: &str, days: i32) -> String {
    let index = WEEKDAY_TOKENS.iter().position(|w| *w == token).unwrap_or(0) as i64;
    WEEKDAY_TOKENS[(index + i64::from(days)).rem_euclid(7) as usize].to_string()
}

/// Whether `a` and `b` fall in different months (`MONTHLY`) or years (`YEARLY`).
fn leaves_period(freq: &str, a: NaiveDate, b: NaiveDate) -> bool {
    match freq {
        "MONTHLY" => (a.year(), a.month()) != (b.year(), b.month()),
        "YEARLY" => a.year() != b.year(),
        _ => false,
    }
}

/// The moved UTC bound the shell handed in, checked to be one.
fn checked_utc_until(moved: &str) -> Result<String, ShiftRefusal> {
    let moved = moved.trim().to_ascii_uppercase();
    if !is_utc_date_time(&moved) || NaiveDateTime::parse_from_str(&moved, "%Y%m%dT%H%M%SZ").is_err()
    {
        return Err(ShiftRefusal::Unreadable);
    }
    Ok(moved)
}

/// `UNTIL` moved by whole days: its date changes, its time and `Z` stay.
fn shift_until(value: &str, days: i32) -> Result<String, ShiftRefusal> {
    if !value.is_ascii() {
        return Err(ShiftRefusal::Unreadable);
    }
    let date = value.get(..8).ok_or(ShiftRefusal::Unreadable)?;
    let rest = &value[8..];
    let rest_ok = rest.is_empty()
        || (rest.len() >= 7
            && rest.starts_with('T')
            && rest[1..7].bytes().all(|b| b.is_ascii_digit())
            && (rest.len() == 7 || &rest[7..] == "Z"));
    if !rest_ok {
        return Err(ShiftRefusal::Unreadable);
    }
    let parsed = NaiveDate::parse_from_str(date, "%Y%m%d").map_err(|_| ShiftRefusal::Unreadable)?;
    let moved = parsed
        .checked_add_signed(Duration::days(i64::from(days)))
        .ok_or(ShiftRefusal::Unreadable)?;
    Ok(format!("{}{rest}", moved.format("%Y%m%d")))
}

/// Whether moving a yearly date from `a` to `b` touches February 29 or crosses
/// from February into March: then leap and common years disagree on the gap.
fn touches_end_of_february(a: NaiveDate, b: NaiveDate) -> bool {
    let leap_day = |d: NaiveDate| d.month() == 2 && d.day() == 29;
    if leap_day(a) || leap_day(b) {
        return true;
    }
    let (early, late) = if a <= b { (a, b) } else { (b, a) };
    let mut year = early.year();
    while year <= late.year() {
        let end_of_feb = NaiveDate::from_ymd_opt(year, 3, 1)
            .and_then(|d| d.pred_opt())
            .unwrap_or(early);
        if early <= end_of_feb && end_of_feb < late {
            return true;
        }
        year += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("a test date")
    }

    fn shifted(rrule: &str, start: &str, days: i32) -> String {
        match shift_series(rrule, day(start), days, false, None) {
            SeriesShift::Shifted { rrule } => rrule,
            SeriesShift::Refused { reason } => panic!("{rrule} +{days} was refused: {reason:?}"),
        }
    }

    fn refused(rrule: &str, start: &str, days: i32, time_changes: bool) -> ShiftRefusal {
        match shift_series(rrule, day(start), days, time_changes, None) {
            SeriesShift::Refused { reason } => reason,
            SeriesShift::Shifted { rrule: out } => panic!("{rrule} +{days} was shifted to {out}"),
        }
    }

    #[test]
    fn weekdays_move_round_the_week() {
        assert_eq!(
            shifted("FREQ=WEEKLY;BYDAY=MO", "2026-05-04", 1),
            "FREQ=WEEKLY;BYDAY=TU"
        );
        assert_eq!(
            shifted("FREQ=WEEKLY;BYDAY=MO,WE", "2026-05-04", 1),
            "FREQ=WEEKLY;BYDAY=TU,TH"
        );
        assert_eq!(
            shifted("FREQ=WEEKLY;BYDAY=SU", "2026-05-03", 1),
            "FREQ=WEEKLY;BYDAY=MO"
        );
        assert_eq!(
            shifted("FREQ=WEEKLY;BYDAY=MO", "2026-05-04", -1),
            "FREQ=WEEKLY;BYDAY=SU"
        );
        assert_eq!(
            shifted("FREQ=WEEKLY;BYDAY=MO", "2026-05-04", 7),
            "FREQ=WEEKLY;BYDAY=MO"
        );
        assert_eq!(
            shifted("FREQ=MONTHLY;BYDAY=MO", "2026-05-04", 1),
            "FREQ=MONTHLY;BYDAY=TU"
        );
    }

    #[test]
    fn every_other_week_moves_its_week_start_with_its_days() {
        assert_eq!(
            shifted("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TU", "2026-05-04", 6),
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=SU,MO;WKST=SU"
        );
        assert_eq!(
            shifted(
                "FREQ=WEEKLY;INTERVAL=2;BYDAY=SU,TU;WKST=SU",
                "2026-05-03",
                1
            ),
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;WKST=MO"
        );
        assert_eq!(
            shifted("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO", "2026-05-04", 1),
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=TU;WKST=TU"
        );
        // A start that is not on the rule's weekday: which week it falls in
        // depends on where weeks begin, so one weekday needs the week start too.
        assert_eq!(
            shifted("FREQ=WEEKLY;INTERVAL=2;BYDAY=WE", "2026-05-04", 5),
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO;WKST=SA"
        );
        // Without weekdays every occurrence is on the start's weekday.
        assert_eq!(
            shifted("FREQ=WEEKLY;INTERVAL=3", "2026-05-04", 2),
            "FREQ=WEEKLY;INTERVAL=3"
        );
    }

    #[test]
    fn until_moves_with_the_series() {
        assert_eq!(
            shifted("FREQ=DAILY;UNTIL=20261231", "2026-05-04", 1),
            "FREQ=DAILY;UNTIL=20270101"
        );
        assert_eq!(
            shifted(
                "FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T235959Z",
                "2026-05-04",
                2
            ),
            "FREQ=WEEKLY;BYDAY=WE;UNTIL=20261202T235959Z"
        );
        assert_eq!(
            shifted("FREQ=DAILY;UNTIL=20260503T090000", "2026-05-01", -2),
            "FREQ=DAILY;UNTIL=20260501T090000"
        );
    }

    #[test]
    fn a_utc_until_takes_the_bound_the_shell_moved() {
        let moved = |rrule: &str, days: i32, time_changes: bool, until: &str| match shift_series(
            rrule,
            day("2026-03-20"),
            days,
            time_changes,
            Some(until),
        ) {
            SeriesShift::Shifted { rrule } => rrule,
            SeriesShift::Refused { reason } => panic!("{rrule} was refused: {reason:?}"),
        };
        // Across a clock change the bound moves by a day on the series' clock,
        // which is 23 hours in UTC.
        assert_eq!(
            moved(
                "FREQ=DAILY;UNTIL=20260328T075959Z",
                1,
                false,
                "20260329T065959Z"
            ),
            "FREQ=DAILY;UNTIL=20260329T065959Z"
        );
        // A new time of day moves it on the same day.
        assert_eq!(
            moved(
                "FREQ=DAILY;UNTIL=20260328T075959Z",
                0,
                true,
                "20260328T085959z"
            ),
            "FREQ=DAILY;UNTIL=20260328T085959Z"
        );
        // A date bound moves by days; a moved UTC bound does not apply to it.
        assert_eq!(
            moved("FREQ=DAILY;UNTIL=20260328", 1, false, "20260329T065959Z"),
            "FREQ=DAILY;UNTIL=20260329"
        );
        assert_eq!(
            shift_series(
                "FREQ=DAILY;UNTIL=20260328T075959Z",
                day("2026-03-20"),
                1,
                false,
                Some("tomorrow")
            ),
            SeriesShift::Refused {
                reason: ShiftRefusal::Unreadable
            }
        );
    }

    #[test]
    fn a_rule_that_follows_its_start_is_unchanged() {
        assert_eq!(
            shifted("FREQ=DAILY;COUNT=5", "2026-05-04", 3),
            "FREQ=DAILY;COUNT=5"
        );
        assert_eq!(shifted("FREQ=MONTHLY", "2026-05-10", 3), "FREQ=MONTHLY");
        assert_eq!(shifted("FREQ=YEARLY", "2026-03-30", 3), "FREQ=YEARLY");
    }

    #[test]
    fn a_day_of_the_month_moves_within_the_month() {
        assert_eq!(
            shifted("FREQ=MONTHLY;BYMONTHDAY=10", "2026-05-10", 2),
            "FREQ=MONTHLY;BYMONTHDAY=12"
        );
        assert_eq!(
            shifted("FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=10", "2026-03-10", 2),
            "FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=12"
        );
        assert_eq!(
            shifted("FREQ=MONTHLY;INTERVAL=2;BYMONTHDAY=10", "2026-05-10", 2),
            "FREQ=MONTHLY;INTERVAL=2;BYMONTHDAY=12"
        );
        assert_eq!(
            refused("FREQ=MONTHLY;BYMONTHDAY=27", "2026-05-27", 2, false),
            ShiftRefusal::MonthEnd
        );
        assert_eq!(
            refused("FREQ=MONTHLY;BYMONTHDAY=-1", "2026-05-31", 1, false),
            ShiftRefusal::NegativeMonthDay
        );
        assert_eq!(
            refused("FREQ=MONTHLY;BYMONTHDAY=0", "2026-05-04", 1, false),
            ShiftRefusal::Unreadable
        );
        assert_eq!(
            refused("FREQ=MONTHLY", "2026-05-27", 3, false),
            ShiftRefusal::MonthEnd
        );
        assert_eq!(
            refused("FREQ=MONTHLY", "2026-05-20", 15, false),
            ShiftRefusal::MonthEnd
        );
    }

    #[test]
    fn a_rule_limited_to_some_months_or_years_is_refused() {
        for (rrule, start, days) in [
            // Weekdays in March only, and every day or week in March only.
            ("FREQ=YEARLY;BYMONTH=3;BYDAY=MO", "2026-03-02", 1),
            ("FREQ=DAILY;BYMONTH=3", "2026-03-10", 1),
            ("FREQ=WEEKLY;BYDAY=MO;BYMONTH=3", "2026-03-02", 1),
            // Weekdays every other month or year.
            ("FREQ=MONTHLY;INTERVAL=2;BYDAY=MO", "2026-05-04", 1),
            ("FREQ=YEARLY;INTERVAL=2;BYDAY=MO", "2026-05-04", 1),
            // A start off the rule that moves into the next month or year.
            ("FREQ=MONTHLY;INTERVAL=2;BYMONTHDAY=10", "2026-05-04", -6),
            (
                "FREQ=YEARLY;INTERVAL=2;BYMONTH=3;BYMONTHDAY=10",
                "2026-12-30",
                3,
            ),
            // A yearly date in March moved out of March.
            ("FREQ=YEARLY;BYMONTH=3", "2026-03-25", 7),
        ] {
            assert_eq!(
                refused(rrule, start, days, false),
                ShiftRefusal::LimitedMonths,
                "{rrule} from {start} by {days}"
            );
        }
        assert_eq!(
            refused("FREQ=YEARLY;BYMONTH=3", "2026-03-30", 3, false),
            ShiftRefusal::MonthEnd
        );
    }

    #[test]
    fn the_end_of_february_is_refused_for_a_yearly_date() {
        assert_eq!(
            refused("FREQ=YEARLY", "2026-02-27", 3, false),
            ShiftRefusal::LeapDay
        );
        assert_eq!(
            refused("FREQ=YEARLY", "2028-02-29", 1, false),
            ShiftRefusal::LeapDay
        );
        assert_eq!(
            refused("FREQ=YEARLY", "2026-03-02", -3, false),
            ShiftRefusal::LeapDay
        );
    }

    #[test]
    fn rules_that_cannot_move_by_whole_days_are_refused() {
        assert_eq!(
            refused("FREQ=MONTHLY;BYDAY=2SU", "2026-05-10", 1, false),
            ShiftRefusal::OrdinalWeekday
        );
        assert_eq!(
            refused(
                "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1",
                "2026-05-29",
                1,
                false
            ),
            ShiftRefusal::SetPosition
        );
        assert_eq!(
            refused("FREQ=YEARLY;BYYEARDAY=100", "2026-04-10", 1, false),
            ShiftRefusal::YearDay
        );
        assert_eq!(
            refused("FREQ=YEARLY;BYWEEKNO=20;BYDAY=MO", "2026-05-11", 1, false),
            ShiftRefusal::WeekNumber
        );
        assert_eq!(
            refused("FREQ=DAILY;X-NAME=1", "2026-05-04", 1, false),
            ShiftRefusal::UnknownPart
        );
        assert_eq!(
            refused("BYDAY=MO", "2026-05-04", 1, false),
            ShiftRefusal::Unreadable
        );
        assert_eq!(
            refused("FREQ=FORTNIGHTLY", "2026-05-04", 1, false),
            ShiftRefusal::Unreadable
        );
        assert_eq!(
            refused("FREQ=DAILY;UNTIL=soon", "2026-05-04", 1, false),
            ShiftRefusal::Unreadable
        );
    }

    #[test]
    fn garbled_rules_are_unreadable_not_a_crash() {
        // A multi-byte character where the time should be.
        assert_eq!(
            refused("FREQ=DAILY;UNTIL=20261231T12345é", "2026-05-04", 1, false),
            ShiftRefusal::Unreadable
        );
        // A part given twice: readers disagree on which copy wins.
        assert_eq!(
            refused("FREQ=WEEKLY;BYDAY=MO;BYDAY=WE", "2026-05-04", 1, false),
            ShiftRefusal::Unreadable
        );
    }

    #[test]
    fn time_of_day_parts_block_only_a_time_change() {
        assert_eq!(
            shifted("FREQ=DAILY;BYHOUR=9,17", "2026-05-04", 1),
            "FREQ=DAILY;BYHOUR=9,17"
        );
        assert_eq!(
            refused("FREQ=DAILY;BYHOUR=9,17", "2026-05-04", 0, true),
            ShiftRefusal::TimeOfDay
        );
        assert_eq!(
            refused("FREQ=DAILY;BYMINUTE=30", "2026-05-04", 1, true),
            ShiftRefusal::TimeOfDay
        );
    }

    #[test]
    fn no_shift_leaves_the_rule_as_written() {
        let odd = "FREQ=MONTHLY;BYSETPOS=1;BYDAY=MO";
        assert_eq!(shifted(odd, "2026-05-04", 0), odd);
        assert_eq!(
            shifted("RRULE:FREQ=WEEKLY;BYDAY=MO;", "2026-05-04", 0),
            "RRULE:FREQ=WEEKLY;BYDAY=MO;"
        );
    }

    #[test]
    fn a_prefix_and_lower_case_are_read() {
        assert_eq!(
            shifted("RRULE:FREQ=WEEKLY;BYDAY=MO", "2026-05-04", 1),
            "RRULE:FREQ=WEEKLY;BYDAY=TU"
        );
        assert_eq!(
            shifted("freq=weekly;byday=mo;", "2026-05-04", 1),
            "freq=weekly;BYDAY=TU"
        );
    }

    #[test]
    fn the_door_answers_in_json() {
        assert_eq!(
            series_shift_json(r#"{"rrule":"FREQ=WEEKLY;BYDAY=MO","start":"2026-05-04","days":1}"#)
                .unwrap(),
            r#"{"outcome":"shifted","rrule":"FREQ=WEEKLY;BYDAY=TU"}"#
        );
        assert_eq!(
            series_shift_json(r#"{"rrule":"FREQ=MONTHLY;BYDAY=2SU","start":"2026-05-10","days":1,"time_changes":false}"#)
                .unwrap(),
            r#"{"outcome":"refused","reason":"ordinal_weekday"}"#
        );
        assert_eq!(
            series_shift_json(r#"{"rrule":"FREQ=DAILY;UNTIL=20260328T075959Z","start":"2026-03-20","days":1,"time_changes":false,"until":"20260329T065959Z"}"#)
                .unwrap(),
            r#"{"outcome":"shifted","rrule":"FREQ=DAILY;UNTIL=20260329T065959Z"}"#
        );
        assert_eq!(
            series_shift_json(r#"{"rrule":"FREQ=DAILY","start":"May 4","days":1}"#).unwrap(),
            r#"{"outcome":"refused","reason":"unreadable"}"#
        );
    }
}
