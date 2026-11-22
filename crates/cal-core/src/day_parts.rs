//! The parts a repeat rule takes from the day a series starts on.
//!
//! A rule without BYDAY, BYMONTHDAY or BYMONTH repeats on the weekday, the
//! day of the month and the month of its start, and the editors offer
//! "the 24th", "the fourth Tuesday" and, in a month's last seven days,
//! "the last Tuesday" from that day. The surfaces asked their device's clock
//! for those parts (`Date.getDay()`, `getDate()`), which named the device's
//! day; read from the day key itself they name the day the form shows,
//! whichever clock it stands on (DESIGN-series-time-zone.md, stage 8). Pure
//! date arithmetic: no zone, no clock.
//!
//! Pinned by `tests/fixtures/dayParts.json`, computed independently with
//! Python's `datetime`; the surfaces replay the same file through their
//! doors.

use chrono::{Datelike, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

/// The parts of one day a repeat rule can take.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DayParts {
    /// The weekday as an RRULE code, `MO` … `SU`.
    pub weekday: String,
    /// The day of the month, 1-31.
    pub day_of_month: u32,
    /// Which of the month's weekdays of its kind the day is, 1-5: the 24th
    /// of November 2026 is its fourth Tuesday.
    pub nth_weekday: u32,
    /// Whether the day lies in the month's last seven days, where "the last
    /// Tuesday" is offered beside "the fourth".
    pub in_last_week: bool,
    /// The month, 1-12.
    pub month: u32,
}

/// Why a day key gives no parts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DayPartsError {
    /// Not a `YYYY-MM-DD` day that exists.
    #[error("not a day: {0:?}")]
    NotADay(String),
}

/// The RRULE code of a weekday.
fn weekday_code(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

/// The number of days in `day`'s month.
fn days_in_month(day: NaiveDate) -> u32 {
    let (year, month) = if day.month() == 12 {
        (day.year() + 1, 1)
    } else {
        (day.year(), day.month() + 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1)
        .and_then(|first| first.pred_opt())
        .map_or(31, |last| last.day())
}

/// The parts of `day`.
pub fn day_parts(day: NaiveDate) -> DayParts {
    let day_of_month = day.day();
    DayParts {
        weekday: weekday_code(day.weekday()).to_string(),
        day_of_month,
        nth_weekday: (day_of_month - 1) / 7 + 1,
        in_last_week: day_of_month + 7 > days_in_month(day),
        month: day.month(),
    }
}

/// The day a strict `YYYY-MM-DD` key names: four-digit year, two-digit month
/// and day, and a day that exists.
pub fn parse_day_key(key: &str) -> Result<NaiveDate, DayPartsError> {
    let bytes = key.as_bytes();
    let shape = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    if !shape {
        return Err(DayPartsError::NotADay(key.to_string()));
    }
    NaiveDate::parse_from_str(key, "%Y-%m-%d").map_err(|_| DayPartsError::NotADay(key.to_string()))
}

/// [`day_parts`] over the wire, for the desktop's WebAssembly and the phone's
/// UniFFI bridge: a `YYYY-MM-DD` key in, the parts as JSON out, and JSON
/// `null` for a key that names no day. A form whose date is being typed asks
/// with such keys while it renders, so the answer is a value, not an error:
/// whatever a door still throws is a fault of the door, and stays loud.
pub fn day_parts_json(day_key: &str) -> String {
    match parse_day_key(day_key) {
        Ok(day) => serde_json::to_string(&day_parts(day)).expect("day parts serialize"),
        Err(_) => "null".to_string(),
    }
}

#[cfg(test)]
mod contract {
    //! The shared fixture, computed with Python's `datetime`.

    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/dayParts.json"
        ));
        serde_json::from_str(text).expect("the fixture parses")
    }

    #[test]
    fn every_row_gives_its_parts() {
        let doc = fixture();
        let rows = doc["rows"].as_array().expect("rows");
        // Anti-silence: the rows the rule turns on, by name.
        for needed in [
            "2026-01-31",
            "2028-02-29",
            "2026-11-22",
            "2026-11-24",
            "2026-12-01",
        ] {
            assert!(
                rows.iter().any(|r| r["day"] == needed),
                "the fixture lost {needed}"
            );
        }
        for row in rows {
            let key = row["day"].as_str().expect("a day");
            let answer: Value = serde_json::from_str(&day_parts_json(key)).expect("json");
            assert_eq!(answer, row["expect"], "{key}");
        }
    }

    #[test]
    fn a_key_that_names_no_day_answers_null() {
        for key in fixture()["refused"].as_array().expect("refused") {
            let key = key.as_str().expect("a string");
            assert_eq!(day_parts_json(key), "null", "{key:?}");
            assert_eq!(
                parse_day_key(key),
                Err(DayPartsError::NotADay(key.to_string())),
                "{key:?}"
            );
        }
    }
}
