//! The calendar week a displayed week is numbered by (DESIGN §5.2).
//!
//! One rule for every surface and every language: ISO 8601 week numbers, the
//! German convention (DIN 1355). A displayed week begins on the user's own
//! week start, which need not be Monday, and carries the number of the ISO
//! week its fourth day falls in: the ISO week that holds most of its seven
//! days. With the week starting on Monday that is exactly each day's own ISO
//! week. With another start, every day of the displayed week reads the same
//! number, so one week never shows as the tail of one and the head of the
//! next, and one day never carries two numbers in the app.
//!
//! Like every rule in this crate it takes no clock, no zone and no language:
//! a date and the setting. Pinned by `tests/fixtures/calendarWeek.json`,
//! computed independently of this code; `shared/calendarWeek.ts` replays it.

use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};

/// A displayed week and the number it carries.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct CalendarWeek {
    /// The ISO week number, 1 to 53.
    pub week: u32,
    /// The ISO week-numbering year the number counts in. It differs from the
    /// calendar year around the turn of the year: 1 January 2027 lies in week
    /// 53 of 2026, 30 December 2024 in week 1 of 2025.
    pub week_year: i32,
    /// First day of the displayed week, on the user's week start.
    pub first_day: NaiveDate,
    /// Last day of the displayed week, inclusive.
    pub last_day: NaiveDate,
    /// Whether first and last day lie in one month: how a range is written
    /// ("2. bis 8. November 2026", not "2. November bis 8. November 2026").
    pub same_month: bool,
    /// Whether the week touches two calendar years. The texts then name the
    /// year the number counts in, `week_year`.
    pub spans_years: bool,
}

/// The first day of the displayed week `day` falls in. `week_starts_on` is
/// the user's own setting, 0 = Sunday … 6 = Saturday; anything above wraps.
pub fn first_day_of_week(day: NaiveDate, week_starts_on: u32) -> NaiveDate {
    // How far back the week's first day lies. The +7 keeps the result positive
    // for every combination of weekday and setting.
    let back = (day.weekday().num_days_from_sunday() + 7 - week_starts_on % 7) % 7;
    day - Duration::days(i64::from(back))
}

/// The calendar week `day` is shown in, with its number.
pub fn calendar_week(day: NaiveDate, week_starts_on: u32) -> CalendarWeek {
    let first_day = first_day_of_week(day, week_starts_on);
    let last_day = first_day + Duration::days(6);
    let iso = (first_day + Duration::days(3)).iso_week();
    CalendarWeek {
        week: iso.week(),
        week_year: iso.year(),
        first_day,
        last_day,
        same_month: first_day.year() == last_day.year() && first_day.month() == last_day.month(),
        spans_years: first_day.year() != last_day.year(),
    }
}

/// The question [`calendar_weeks_json`] answers: the weeks of many days at
/// once, so a month grid or a list asks once per window.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct CalendarWeeksInput {
    pub days: Vec<NaiveDate>,
    /// The user's own setting: 0 = Sunday … 6 = Saturday.
    pub week_starts_on: u32,
}

/// One day's answer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DayCalendarWeek {
    pub day: NaiveDate,
    /// Whether `day` is the first day of its displayed week: where a list
    /// puts the week's number.
    pub starts_week: bool,
    pub week: CalendarWeek,
}

/// The calendar week of each of `days`, in their order.
pub fn calendar_weeks(days: &[NaiveDate], week_starts_on: u32) -> Vec<DayCalendarWeek> {
    days.iter()
        .map(|&day| {
            let week = calendar_week(day, week_starts_on);
            DayCalendarWeek {
                day,
                starts_week: week.first_day == day,
                week,
            }
        })
        .collect()
}

/// [`calendar_weeks`] over the wire, for the desktop's WebAssembly and the
/// phone's UniFFI bridge.
pub fn calendar_weeks_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: CalendarWeeksInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&calendar_weeks(&input.days, input.week_starts_on))
}

#[cfg(test)]
mod contract {
    //! The shared fixture, computed with Python's `date.isocalendar()`.

    use super::*;
    use serde_json::Value;

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/calendarWeek.json"
    ));

    fn doc() -> Value {
        serde_json::from_str(CONTRACT).expect("the fixture parses")
    }

    fn date(value: &Value) -> NaiveDate {
        NaiveDate::parse_from_str(value.as_str().expect("a date string"), "%Y-%m-%d")
            .expect("a date")
    }

    #[test]
    fn every_case_of_the_fixture() {
        let doc = doc();
        let cases = doc["cases"].as_array().expect("cases");
        assert!(!cases.is_empty());
        for case in cases {
            let week = calendar_week(
                date(&case["day"]),
                case["weekStartsOn"].as_u64().expect("weekStartsOn") as u32,
            );
            let expected: CalendarWeek =
                serde_json::from_value(case["expect"].clone()).expect("expect deserializes");
            assert_eq!(week, expected, "{}", case["name"]);
        }
    }

    #[test]
    fn the_batch_of_the_fixture_through_the_wire() {
        let doc = doc();
        let batch = &doc["batch"];
        let input = serde_json::json!({
            "days": batch["days"],
            "weekStartsOn": batch["weekStartsOn"],
        });
        let answer: Value = serde_json::from_str(
            &calendar_weeks_json(&input.to_string()).expect("the question parses"),
        )
        .expect("the answer parses");
        assert_eq!(answer, batch["expect"]);
    }

    #[test]
    fn a_setting_above_six_wraps_like_the_backlog_weeks() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        assert_eq!(calendar_week(day, 7), calendar_week(day, 0));
    }

    #[test]
    fn a_broken_question_is_an_error() {
        assert!(calendar_weeks_json("not json").is_err());
    }
}
