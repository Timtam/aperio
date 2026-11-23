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
//! - A stored `UNTIL` reads on the clock the series repeats on
//!   ([`crate::read_series_end`]): through a date, an instant, or a wall
//!   time (decisions 101b and 105a to 116a).
//! - "Through day D" is spelled so that no reader needs the 105a guard
//!   ([`until_through`]): `DT235959Z` where that reads through D on the
//!   series' clock, else D's last instant there.
//! - "Just before an occurrence" is spelled one second before it, or two
//!   where one would spell the end of a UTC day ([`crate::cut_before`], 106a).
//!
//! One JSON door ([`wall_clock_json`]) answers all six questions, for the
//! desktop's WebAssembly and the phone's UniFFI bridge. Pinned by
//! `tests/fixtures/wallClock.json`, computed from tzdata's rules by hand and
//! checked against Node's ICU data; the surfaces replay the same file.

use chrono::{
    DateTime, NaiveDate, NaiveDateTime, NaiveTime, SecondsFormat, TimeZone, Timelike, Utc,
};
use serde::{Deserialize, Serialize};

use cal_zone::{Reading, WallPolicy, Zone};

/// The clock a series' fields stand on: the zone its stored name names, by
/// the core's one rule, or UTC.
pub(crate) fn clock_of(tzid: Option<&str>) -> (Zone, Option<&'static str>) {
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
    /// How a series' stored `UNTIL` reads on the clock it repeats on.
    #[serde(rename_all = "camelCase")]
    SeriesEnd {
        /// The series' rule, with or without `RRULE:`.
        rrule: String,
        /// The series' start, an RFC 3339 instant.
        start: String,
        /// Whether the series is all-day: it then repeats on the
        /// device's days (decision 48a).
        all_day: bool,
        tzid: Option<String>,
        /// The device's zone as its runtime reports it, read only for a
        /// series of days; the core reads no clock of its own.
        device_zone: Option<String>,
    },
    /// The `UNTIL` that reads "through `date`" on the series' clock and needs
    /// no guard ([`until_through`]).
    #[serde(rename_all = "camelCase")]
    UntilThrough {
        /// `YYYY-MM-DD`.
        date: String,
        /// The series' rule, with or without `RRULE:`.
        rrule: String,
        /// The series' start, an RFC 3339 instant.
        start: String,
        all_day: bool,
        tzid: Option<String>,
    },
    /// The `UNTIL` that ends a series just before its occurrence at `at`
    /// ([`crate::cut_before`], 106a).
    #[serde(rename_all = "camelCase")]
    CutBefore {
        /// The occurrence, an RFC 3339 instant.
        at: String,
        /// The series' rule, with or without `RRULE:`.
        rrule: String,
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
    /// whole day (Samoa, 30 December 2011). For `untilThrough` and
    /// `cutBefore`: the `UNTIL` as an instant.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub at: Option<String>,
    /// For `seriesEnd`: how the end reads; absent for a rule without an
    /// `UNTIL`, with a `COUNT`, or one that cannot be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(optional))]
    pub end: Option<crate::SeriesEnd>,
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
        end: None,
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
        end: None,
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
        end: None,
    }
}

/// How `rrule`'s stored `UNTIL` reads for a series starting at `start`, on
/// the clock [`crate::expansion_clock`] gives it: its zone, UTC, or for a
/// series of days the zone `device_zone` names (UTC where it names none
/// tzdata knows). The rule is [`crate::read_series_end`]'s; this gives it
/// the clock.
pub fn series_end_on_clock(
    rrule: &str,
    start: DateTime<Utc>,
    all_day: bool,
    tzid: Option<&str>,
    device_zone: Option<&str>,
) -> WallClockAnswer {
    let expansion = crate::expansion_clock(all_day, tzid);
    let (zone, clock) = match expansion {
        crate::ExpansionClock::DeviceDays => clock_of(device_zone),
        crate::ExpansionClock::Zone => clock_of(tzid),
        crate::ExpansionClock::Utc => (Zone::UTC, None),
    };
    let end = crate::read_series_end(
        rrule,
        zone.wall(start).time(),
        expansion,
        |at| zone.wall(at),
        |wall| last_at_or_before(zone, wall),
        |wall| zone.to_utc(wall, WallPolicy::Rfc5545),
    );
    WallClockAnswer {
        clock: clock.map(str::to_string),
        date: None,
        time: None,
        second: None,
        wall: None,
        at: None,
        end,
    }
}

/// The `UNTIL` that reads "through `day`" on the clock the series repeats on
/// and needs no guard: `DT235959Z` where that reads through `day` as the
/// rule and start stand (an all-day series, a series on UTC, and on a zone's
/// clock wherever the 105a guard does not turn it into a cut), else the last
/// instant of `day` on the series' zone. That is the later reading where the
/// clock shows 23:59:59 twice and the second before a change that skips the
/// day's end. Where that change also pushes `day`'s last occurrence past the
/// day's end (Nuuk, whose spring change skips 23:00-24:00, for a series at
/// 23:00 or later), the second before the change comes before that
/// occurrence, so the `UNTIL` is the instant the occurrence stands at
/// instead: the first instant that reads through `day`. A day the clock
/// skips whole falls back to `DT235959Z`.
///
/// That holds for a rule with one time a day, the only rules Aperio writes
/// for Exchange and Microsoft 365. A rule with several times a day
/// (`BYHOUR=10,23`) whose earlier time on `day` already reads through `day`
/// takes the marker or the last instant before the change and is not
/// carried on to the pushed occurrence: the day's 23:xx occurrence is lost.
/// A known limit, reachable only from a CalDAV or file rule with several
/// times a day in such a zone, ending on the day of its spring change.
///
/// So no `UNTIL` Aperio writes for a typed or carried day needs the guard to
/// be read right: the views, the repeat sentence and a calendar that stores
/// a date (`recurrence.ends_by_date`) all count it through `day`.
pub fn until_through(
    day: NaiveDate,
    rrule: &str,
    start: DateTime<Utc>,
    all_day: bool,
    tzid: Option<&str>,
) -> DateTime<Utc> {
    let last = day.and_hms_opt(23, 59, 59).expect("23:59:59 is a time");
    let marker = Utc.from_utc_datetime(&last);
    if !matches!(
        crate::expansion_clock(all_day, tzid),
        crate::ExpansionClock::Zone
    ) {
        return marker;
    }
    let marker_text = crate::rule_until_text(marker);
    let has_until = crate::rrule_parts::parse_parts(rrule)
        .ok()
        .is_some_and(|(_, parts)| crate::rrule_parts::part(&parts, "UNTIL").is_some());
    let probe = |text: &str| {
        if has_until {
            crate::with_until(rrule, text)
        } else {
            format!("{};UNTIL={text}", rrule.trim().trim_end_matches(';'))
        }
    };
    let day_text = day.format("%Y-%m-%d").to_string();
    let reads = |text: &str, reading: crate::EndReading| {
        series_end_on_clock(&probe(text), start, false, tzid, None)
            .end
            .is_some_and(|end| end.reading == reading && end.end_day == day_text)
    };
    if reads(&marker_text, crate::EndReading::ThroughDay) {
        return marker;
    }
    let (zone, _) = clock_of(tzid);
    let last_instant = last_at_or_before(zone, last).filter(|at| zone.wall(*at).date() == day);
    let reads_at =
        |at: DateTime<Utc>| reads(&crate::rule_until_text(at), crate::EndReading::Instant);
    if let Some(at) = last_instant.filter(|at| reads_at(*at)) {
        return at;
    }
    // A change that skips the day's end pushes an occurrence of `day` set
    // after the change past the day's last instant: the `UNTIL` reaches it
    // where it stands.
    let pushed = crate::rrule_parts::parse_parts(rrule)
        .ok()
        .and_then(|(_, parts)| crate::series_end::rule_times(&parts, zone.wall(start).time()))
        .and_then(|times| {
            times
                .into_iter()
                .filter_map(|time| zone.to_utc(day.and_time(time), WallPolicy::Rfc5545))
                .max()
        })
        .filter(|at| last_instant.is_some_and(|last| *at > last) && reads_at(*at));
    pushed.or(last_instant).unwrap_or(marker)
}

/// The last instant a series' stored `UNTIL` lets an occurrence start at,
/// read on the clock the series repeats on ([`crate::read_series_end`]'s
/// `at`, decision 101b): where an adapter drops a cut series' overrides
/// after (116a). `None` without an `UNTIL`, or for one that cannot be read.
pub fn series_end_instant(
    rrule: &str,
    start: DateTime<Utc>,
    all_day: bool,
    tzid: Option<&str>,
) -> Option<DateTime<Utc>> {
    let at = series_end_on_clock(rrule, start, all_day, tzid, None)
        .end?
        .at?;
    DateTime::parse_from_rfc3339(&at)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// The last instant whose wall time on `zone` is `wall` or earlier: `wall`
/// itself, at its later reading where the clock shows it twice, or the
/// second before a change that skips it.
pub(crate) fn last_at_or_before(zone: Zone, wall: NaiveDateTime) -> Option<DateTime<Utc>> {
    let on = |offset: chrono::FixedOffset| {
        DateTime::<Utc>::from_naive_utc_and_offset(
            wall - chrono::Duration::seconds(i64::from(offset.local_minus_utc())),
            Utc,
        )
    };
    match zone.reading(wall) {
        Reading::Single(offset) => Some(on(offset)),
        Reading::Fold { later, .. } => Some(on(later)),
        Reading::Gap { before, after } => last_second_before_gap(zone, wall, before, after),
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
        WallClockQuestion::SeriesEnd {
            rrule,
            start,
            all_day,
            tzid,
            device_zone,
        } => {
            let start = DateTime::parse_from_rfc3339(start)
                .ok()?
                .with_timezone(&Utc);
            Some(series_end_on_clock(
                rrule,
                start,
                *all_day,
                tzid.as_deref(),
                device_zone.as_deref(),
            ))
        }
        WallClockQuestion::UntilThrough {
            date,
            rrule,
            start,
            all_day,
            tzid,
        } => {
            let date = crate::parse_day_key(date).ok()?;
            let start = DateTime::parse_from_rfc3339(start)
                .ok()?
                .with_timezone(&Utc);
            let tzid = tzid.as_deref();
            let clock = match crate::expansion_clock(*all_day, tzid) {
                crate::ExpansionClock::Zone => clock_of(tzid).1.map(str::to_string),
                _ => None,
            };
            Some(WallClockAnswer {
                clock,
                date: None,
                time: None,
                second: None,
                wall: None,
                at: Some(instant_text(until_through(
                    date, rrule, start, *all_day, tzid,
                ))),
                end: None,
            })
        }
        WallClockQuestion::CutBefore { at, rrule } => {
            let at = DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&Utc);
            Some(WallClockAnswer {
                clock: None,
                date: None,
                time: None,
                second: None,
                wall: None,
                at: Some(instant_text(crate::cut_before(at, rrule))),
                end: None,
            })
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
            "end-berlin-0030-summer-marker-through-day",
            "end-new-york-1900-winter-cut-stays-an-instant",
            "end-all-day-marker-by-its-digits",
            "until-through-new-york-1900-winter-blind-spot",
            "until-through-berlin-0100-winter-east-blind-spot",
            "until-through-apia-skipped-day-falls-back",
            "until-through-nuuk-2300-spring-change-pushes-the-last-occurrence",
            "cut-before-blind-spot",
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

    /// Where an adapter drops a cut series' overrides after (116a): the end's
    /// own instant on the series' clock, not the UTC reading of its digits.
    #[test]
    fn the_end_instant_reads_on_the_series_clock() {
        let at = |s: &str| s.parse::<DateTime<Utc>>().expect("an instant");
        // Daily 20:00 New York through 5 November: its last evening is
        // 6 November 01:00 UTC, and the end reaches 04:59:59 UTC.
        assert_eq!(
            series_end_instant(
                "FREQ=DAILY;UNTIL=20261105T235959Z",
                at("2026-11-02T01:00:00Z"),
                false,
                Some("America/New_York"),
            ),
            Some(at("2026-11-06T04:59:59Z"))
        );
        // A cut stays the instant it is.
        assert_eq!(
            series_end_instant(
                "FREQ=DAILY;UNTIL=20261110T075959Z",
                at("2026-11-02T08:00:00Z"),
                false,
                Some("Europe/Berlin"),
            ),
            Some(at("2026-11-10T07:59:59Z"))
        );
        assert_eq!(
            series_end_instant("FREQ=DAILY", at("2026-11-02T08:00:00Z"), false, None),
            None
        );
    }

    /// A calendar that stores a series' end as a date writes the day the
    /// views read (`written_end_day`), on every one-time-a-day row of the
    /// shared fixture; a guarded row is the documented exception, where it
    /// writes the `UNTIL`'s own date (140a, D1).
    #[test]
    fn written_end_day_agrees_with_the_views_on_every_one_time_a_day_row() {
        use crate::rrule_parts::{parse_freq, parse_parts, part, Freq};

        let doc = fixture();
        let rows = doc["rows"].as_array().expect("rows");
        let mut agreed: Vec<String> = Vec::new();
        let mut own_date: Vec<String> = Vec::new();
        for row in rows {
            let question = &row["question"];
            let end = &row["expect"]["end"];
            if question["op"] != "seriesEnd" || question["allDay"] != false || end.is_null() {
                continue;
            }
            let name = row["name"].as_str().expect("a name");
            // A day the clock skips whole (Samoa, 30 December 2011): the
            // views count back to the day before, which has the same
            // occurrences; a calendar that stores the date keeps the date.
            if name == "end-apia-skipped-day" {
                continue;
            }
            let rrule = question["rrule"].as_str().expect("a rule");
            let Ok((_, parts)) = parse_parts(rrule) else {
                continue;
            };
            let Some(until) = part(&parts, "UNTIL") else {
                continue;
            };
            let sub_daily = matches!(
                part(&parts, "FREQ").and_then(parse_freq),
                Some(Freq::Hourly | Freq::Minutely | Freq::Secondly)
            );
            let one_time = ["BYHOUR", "BYMINUTE", "BYSECOND"]
                .iter()
                .all(|key| part(&parts, key).is_none_or(|v| !v.contains(',')));
            if sub_daily || !one_time {
                continue;
            }
            let tzid = question["tzid"].as_str();
            let (zone, _) = clock_of(tzid);
            let start = question["start"]
                .as_str()
                .expect("a start")
                .parse::<DateTime<Utc>>()
                .expect("an instant");
            let written = crate::written_end_day(
                rrule,
                zone.wall(start).time(),
                false,
                |w| Some(zone.wall(w)),
                |l| zone.to_utc(l, WallPolicy::EarlierElseHourLater),
            )
            .map(|day| day.format("%Y-%m-%d").to_string());
            if end["guarded"] == true {
                let digits = NaiveDate::parse_from_str(&until[..8], "%Y%m%d")
                    .expect("the UNTIL's date")
                    .format("%Y-%m-%d")
                    .to_string();
                assert_eq!(written, Some(digits), "{name}: its own date");
                own_date.push(name.to_string());
            } else {
                assert_eq!(
                    written.as_deref(),
                    end["endDay"].as_str(),
                    "{name}: the views' day"
                );
                agreed.push(name.to_string());
            }
        }
        // Anti-silence: both kinds were seen, by name.
        assert!(
            agreed
                .iter()
                .any(|n| n == "end-new-york-2000-winter-marker-through-day"),
            "no row agreed with the views by name: {agreed:?}"
        );
        assert!(
            own_date
                .iter()
                .any(|n| n == "end-new-york-1900-winter-cut-stays-an-instant"),
            "no guarded row kept its own date by name: {own_date:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().expect("an instant")
    }

    fn end_of(rrule: &str, start: &str, all_day: bool, tzid: Option<&str>) -> crate::SeriesEnd {
        series_end_on_clock(rrule, at(start), all_day, tzid, None)
            .end
            .expect("an end")
    }

    const NEW_YORK: Option<&str> = Some("America/New_York");

    #[test]
    fn guarded_is_set_only_by_the_guard() {
        // 19:00 New York in winter: the second after the end of the UTC day
        // is the next occurrence, so the guard reads a cut.
        let start = "2026-11-03T00:00:00Z";
        let rule = "FREQ=DAILY;UNTIL=20261110T235959Z";
        let guarded = end_of(rule, start, false, NEW_YORK);
        assert!(guarded.guarded);
        assert_eq!(guarded.reading, crate::EndReading::Instant);
        // At 18:00 it is no rule time: through the day, unguarded.
        let through = end_of(rule, "2026-11-02T23:00:00Z", false, NEW_YORK);
        assert!(!through.guarded);
        assert_eq!(through.reading, crate::EndReading::ThroughDay);
        // Another instant, a rule by the hour, no zone, a series of days,
        // and a 106a cut: never the guard.
        for (rule, all_day, tzid) in [
            ("FREQ=DAILY;UNTIL=20261111T045959Z", false, NEW_YORK),
            ("FREQ=HOURLY;UNTIL=20261110T235959Z", false, NEW_YORK),
            (rule, false, None),
            (rule, true, None),
            ("FREQ=DAILY;UNTIL=20261107T235958Z", false, NEW_YORK),
        ] {
            assert!(
                !end_of(rule, start, all_day, tzid).guarded,
                "{rule} all_day={all_day} {tzid:?}"
            );
        }
    }

    /// Every day `until_through` spells is read through that day by the
    /// views without the guard, and written as that day by a calendar that
    /// stores a date: around both clock changes of six zones and UTC, at
    /// every half hour. Nuuk's spring change skips the end of a day, and
    /// pushes that day's occurrences at 23:00 and later past it.
    #[test]
    fn until_through_spells_a_day_no_reader_needs_the_guard_for() {
        let day = |y: i32, m: u32, d: u32| NaiveDate::from_ymd_opt(y, m, d).expect("a date");
        let run = |from: NaiveDate, count: i64| -> Vec<NaiveDate> {
            (0..count)
                .map(|i| from + chrono::Duration::days(i))
                .collect()
        };
        let mut zones: Vec<(Option<&str>, Vec<NaiveDate>)> = vec![
            (
                Some("America/New_York"),
                [run(day(2026, 3, 7), 3), run(day(2026, 10, 31), 3)].concat(),
            ),
            (
                Some("America/Los_Angeles"),
                [run(day(2026, 3, 7), 3), run(day(2026, 10, 31), 3)].concat(),
            ),
            (
                Some("Europe/Berlin"),
                [run(day(2026, 3, 28), 3), run(day(2026, 10, 24), 3)].concat(),
            ),
            (
                Some("Pacific/Auckland"),
                [run(day(2026, 4, 4), 3), run(day(2026, 9, 26), 3)].concat(),
            ),
            (
                Some("America/Nuuk"),
                [run(day(2026, 3, 27), 3), run(day(2026, 10, 23), 3)].concat(),
            ),
            (Some("Asia/Kolkata"), vec![day(2026, 3, 29)]),
            (None, vec![day(2026, 3, 29)]),
        ];
        for (_, days) in &mut zones {
            days.push(day(2026, 11, 10));
        }
        let mut checked = 0;
        for (tzid, days) in &zones {
            let (zone, _) = clock_of(*tzid);
            for d in days {
                for half_hours in 0..48 {
                    let time = NaiveTime::from_hms_opt(half_hours / 2, (half_hours % 2) * 30, 0)
                        .expect("a time");
                    let first = *d - chrono::Duration::days(7);
                    let start = match instant_of(first, time, *tzid).wall {
                        Some(WallReading::Single { at: s })
                        | Some(WallReading::Fold { at: s, .. }) => at(&s),
                        _ => continue,
                    };
                    let text = crate::rule_until_text(until_through(
                        *d,
                        "FREQ=DAILY",
                        start,
                        false,
                        *tzid,
                    ));
                    let rule = format!("FREQ=DAILY;UNTIL={text}");
                    let read = series_end_on_clock(&rule, start, false, *tzid, None)
                        .end
                        .expect("an end");
                    let what = format!("{tzid:?} {d} {time}: {rule}");
                    assert_eq!(read.end_day, d.format("%Y-%m-%d").to_string(), "{what}");
                    assert!(!read.guarded, "{what}");
                    assert_eq!(
                        crate::written_end_day(
                            &rule,
                            zone.wall(start).time(),
                            false,
                            |w| Some(zone.wall(w)),
                            |l| zone.to_utc(l, WallPolicy::EarlierElseHourLater),
                        ),
                        Some(*d),
                        "{what}"
                    );
                    checked += 1;
                }
            }
        }
        // Anti-silence: 39 days at 48 times, less any start the clock skips.
        assert!(checked >= 1800, "only {checked} cases ran");
    }
}
