//! Dates and times on the clock a series repeats on (DESIGN-series-time-zone.md,
//! stage 8, decision 100a).
//!
//! A series' start and end are typed and shown on its own clock: the zone its
//! stored name names, or UTC (decisions 13b, 14a, 26a; [`crate::series_clock_zone`]).
//! The editors hold a date and a time of day as text; this rule turns them
//! into an instant and back, with the bundled tzdata the adapters write with,
//! so a typed 09:00 reaches the provider as 09:00.
//!
//! - A wall time the clock skips (the spring gap) has no instant: the answer
//!   says so, with the offsets before and after the change, and the editors
//!   refuse the save with it ("02:30 gibt es am 28. März in Berlin nicht").
//! - A wall time the clock shows twice (the autumn fold) takes the earlier
//!   reading; the answer names the later one too.
//! - The end of a repeat, a changed `UNTIL`, is 23:59:59 of its date on the
//!   clock.
//!
//! One JSON door ([`wall_clock_json`]) answers all three questions, for the
//! desktop's WebAssembly and the phone's UniFFI bridge. Pinned by
//! `tests/fixtures/wallClock.json`, computed from tzdata's rules by hand and
//! checked against Node's ICU data; the surfaces replay the same file.

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, SecondsFormat, Timelike, Utc};
use serde::{Deserialize, Serialize};

use cal_zone::{Reading, WallPolicy, Zone};

/// The clock a series' fields stand on: the zone its stored name names, by
/// the core's one rule, or UTC.
fn clock_of(tzid: Option<&str>) -> (Zone, Option<&'static str>) {
    match crate::series_clock_zone(tzid).and_then(crate::canonical_zone) {
        Some(name) => match Zone::named(name) {
            Some(zone) => (zone, Some(name)),
            None => (Zone::UTC, None),
        },
        None => (Zone::UTC, None),
    }
}

/// One question about a series' clock.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum WallClockQuestion {
    /// The date and time of day an instant shows on the clock.
    #[serde(rename_all = "camelCase")]
    WallOf {
        /// An RFC 3339 instant.
        at: String,
        /// The series' stored zone name, as stored; `None` for none.
        tzid: Option<String>,
    },
    /// The instant a date and time of day name on the clock.
    #[serde(rename_all = "camelCase")]
    InstantOf {
        /// `YYYY-MM-DD`.
        date: String,
        /// `HH:MM`, 24 hours.
        time: String,
        tzid: Option<String>,
    },
    /// The instant a repeat ending on `date` ends at: 23:59:59 on the clock.
    #[serde(rename_all = "camelCase")]
    UntilOn {
        /// `YYYY-MM-DD`.
        date: String,
        tzid: Option<String>,
    },
}

/// How a wall time reads on the clock.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(tag = "reading", rename_all = "camelCase")]
pub enum WallReading {
    /// Shown once, at `at`.
    Single { at: String },
    /// Shown twice: `at` is the earlier instant, which a typed time takes,
    /// `later` the second.
    Fold { at: String, later: String },
    /// Skipped by a clock change: no instant. The offsets in seconds east of
    /// UTC before and after the change.
    #[serde(rename_all = "camelCase")]
    Gap {
        offset_before: i32,
        offset_after: i32,
    },
}

/// An answer: the clock it was read on (the canonical zone name, `None` for
/// UTC) and what it read. `None` as a whole for a question whose text names
/// no instant, date or time.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct WallClockAnswer {
    /// The canonical zone the fields stand on, `None` for UTC.
    pub clock: Option<String>,
    /// For `wallOf`: the date the instant shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub date: Option<String>,
    /// For `wallOf`: its time of day, `HH:MM`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub time: Option<String>,
    /// For `wallOf`: its seconds past the minute, which `HH:MM` leaves out
    /// (a stored instant need not sit on a whole minute: Paris kept +00:09:21
    /// until 1911).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub second: Option<u32>,
    /// For `instantOf`: how the wall time reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub wall: Option<WallReading>,
    /// For `untilOn`: the day's last second; the one before the change where
    /// a change skips the day's end; `None` only where the clock skips the
    /// whole day (Samoa, 30 December 2011).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub at: Option<String>,
}

fn instant_text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse_time(text: &str) -> Option<NaiveTime> {
    let bytes = text.as_bytes();
    let shape = bytes.len() == 5
        && bytes[2] == b':'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 2 || b.is_ascii_digit());
    if !shape {
        return None;
    }
    NaiveTime::parse_from_str(text, "%H:%M").ok()
}

/// The date and time `at` shows on `tzid`'s clock.
pub fn wall_of(at: DateTime<Utc>, tzid: Option<&str>) -> WallClockAnswer {
    let (zone, clock) = clock_of(tzid);
    let wall = zone.wall(at);
    WallClockAnswer {
        clock: clock.map(str::to_string),
        date: Some(wall.date().format("%Y-%m-%d").to_string()),
        time: Some(wall.time().format("%H:%M").to_string()),
        second: Some(wall.time().second()),
        wall: None,
        at: None,
    }
}

/// How `date` at `time` reads on `tzid`'s clock.
pub fn instant_of(date: NaiveDate, time: NaiveTime, tzid: Option<&str>) -> WallClockAnswer {
    let (zone, clock) = clock_of(tzid);
    let wall_time = NaiveDateTime::new(date, time);
    let at = |offset: chrono::FixedOffset| {
        instant_text(DateTime::<Utc>::from_naive_utc_and_offset(
            wall_time - chrono::Duration::seconds(i64::from(offset.local_minus_utc())),
            Utc,
        ))
    };
    let wall = match zone.reading(wall_time) {
        Reading::Single(offset) => WallReading::Single { at: at(offset) },
        Reading::Fold { earlier, later } => WallReading::Fold {
            at: at(earlier),
            later: at(later),
        },
        Reading::Gap { before, after } => WallReading::Gap {
            offset_before: before.local_minus_utc(),
            offset_after: after.local_minus_utc(),
        },
    };
    WallClockAnswer {
        clock: clock.map(str::to_string),
        date: None,
        time: None,
        second: None,
        wall: Some(wall),
        at: None,
    }
}

/// The instant a repeat ending on `date` ends at: the day's last second on
/// `tzid`'s clock. That is 23:59:59, the earlier reading in a fold; where a
/// change skips the day's end (Nuuk's 23:00 to 00:00 on the last Saturday of
/// March), the second before the change; `None` in the answer only where
/// the clock skips the whole day (Samoa, 30 December 2011).
pub fn until_on(date: NaiveDate, tzid: Option<&str>) -> WallClockAnswer {
    let (zone, clock) = clock_of(tzid);
    let last = date.and_hms_opt(23, 59, 59).expect("23:59:59 is a time");
    let at = match zone.reading(last) {
        Reading::Gap { before, after } => last_second_before_gap(zone, last, before, after)
            .filter(|at| zone.wall(*at).date() == date),
        _ => zone.to_utc(last, WallPolicy::EarlierElseHourLater),
    };
    WallClockAnswer {
        clock: clock.map(str::to_string),
        date: None,
        time: None,
        second: None,
        wall: None,
        at: at.map(instant_text),
    }
}

/// The last instant before the change that skips `wall`: the second before
/// the first instant on the `after` offset. `wall` read on `after` lies
/// before the change and read on `before` at or after it, so the change is
/// found between the two, to the second.
fn last_second_before_gap(
    zone: Zone,
    wall: NaiveDateTime,
    before: chrono::FixedOffset,
    after: chrono::FixedOffset,
) -> Option<DateTime<Utc>> {
    let on = |offset: chrono::FixedOffset| {
        DateTime::<Utc>::from_naive_utc_and_offset(
            wall - chrono::Duration::seconds(i64::from(offset.local_minus_utc())),
            Utc,
        )
    };
    // `lo` still reads `before`, `hi` no longer does.
    let (mut lo, mut hi) = (on(after), on(before));
    if zone.offset_at(lo) != before || zone.offset_at(hi) == before {
        return None;
    }
    while hi - lo > chrono::Duration::seconds(1) {
        let mid = lo + (hi - lo) / 2;
        if zone.offset_at(mid) == before {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(lo)
}

/// The answer to one question, `None` where its text names no instant, date
/// or time.
pub fn wall_clock(question: &WallClockQuestion) -> Option<WallClockAnswer> {
    match question {
        WallClockQuestion::WallOf { at, tzid } => {
            let at = DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&Utc);
            Some(wall_of(at, tzid.as_deref()))
        }
        WallClockQuestion::InstantOf { date, time, tzid } => {
            let date = crate::parse_day_key(date).ok()?;
            let time = parse_time(time)?;
            Some(instant_of(date, time, tzid.as_deref()))
        }
        WallClockQuestion::UntilOn { date, tzid } => {
            let date = crate::parse_day_key(date).ok()?;
            Some(until_on(date, tzid.as_deref()))
        }
    }
}

/// [`wall_clock`] over the wire: a question as JSON in, the answer as JSON
/// out, JSON `null` for a question whose text names no instant, date or time
/// (a form whose fields are being typed asks with such text while it
/// renders). An error only for JSON that is no question at all: a fault of
/// the door.
pub fn wall_clock_json(question_json: &str) -> Result<String, serde_json::Error> {
    let question: WallClockQuestion = serde_json::from_str(question_json)?;
    serde_json::to_string(&wall_clock(&question))
}

#[cfg(test)]
mod contract {
    //! The shared fixture.

    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/wallClock.json"
        ));
        serde_json::from_str(text).expect("the fixture parses")
    }

    #[test]
    fn every_row_gives_its_answer() {
        let doc = fixture();
        let rows = doc["rows"].as_array().expect("rows");
        // Anti-silence: the rows the rule turns on, by name.
        for needed in [
            "berlin-spring-gap",
            "berlin-autumn-fold",
            "lord-howe-half-hour-gap",
            "apia-skipped-day",
            "paris-1900-seconds",
            "until-new-york",
            "until-nuuk-gap-at-day-end",
            "no-zone-is-utc",
            "alias-calcutta",
        ] {
            assert!(
                rows.iter().any(|r| r["name"] == needed),
                "the fixture lost {needed}"
            );
        }
        for row in rows {
            let answer: Value = serde_json::from_str(
                &wall_clock_json(&row["question"].to_string()).expect("a question"),
            )
            .expect("json");
            assert_eq!(answer, row["expect"], "{}", row["name"]);
        }
    }

    #[test]
    fn text_that_is_no_question_is_an_error() {
        assert!(wall_clock_json("{\"op\":\"teleport\"}").is_err());
        assert!(wall_clock_json("not json").is_err());
    }
}
