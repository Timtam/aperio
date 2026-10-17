//! The day a cut series ends on, for a provider that ends a series on a date
//! (decision 235).
//!
//! "This and all following" shortens the old series with an `UNTIL` one
//! second before the occurrence it is cut at, an instant. A provider that
//! stores a series' end as a date counts that whole date, on the clock the
//! series repeats on: Exchange does (live test 8a, step 8; live round 10a, M1
//! with a series at 00:30 Berlin, M2 at 20:00 New York). Written as the
//! `UNTIL`'s UTC date, the end kept the cut's own day, so it stood in the old
//! series and in the new one (a delete from one occurrence on kept it in the
//! only one, M5), and west of UTC in the evening the day after it too. Only
//! east of UTC just after midnight was the UTC date the day before already
//! (M1).
//!
//! Every occurrence of such a series stands at the same wall time on its
//! clock. So the end is the day of the `UNTIL` on that clock where that day's
//! occurrence starts at or before it, and the day before where it starts
//! after: no expansion of the rule, whatever its pattern. A weekly Monday
//! series cut on a Monday ends on the Sunday before, which keeps the same
//! occurrences (round 10a, M3: Exchange keeps such a date).
//!
//! The clock comes from the caller as two functions, so this crate names no
//! zone and no provider: the adapter knows its stored zones, custom ones
//! included.

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

use crate::rrule_parts::{is_utc_date_time, parse_parts, part};

/// The `UNTIL` of `rrule` as an instant, where it is a UTC date-time
/// (`YYYYMMDDTHHMMSSZ`), the form a cut of a timed series writes. A date (an
/// all-day series, whose cut already names the day before) or a floating
/// time is no instant, and gives `None`, as does a rule without an `UNTIL`.
pub fn utc_until(rrule: &str) -> Option<DateTime<Utc>> {
    let (_, parts) = parse_parts(rrule).ok()?;
    let value = part(&parts, "UNTIL")?;
    if !is_utc_date_time(value) {
        return None;
    }
    NaiveDateTime::parse_from_str(&value.to_ascii_uppercase(), "%Y%m%dT%H%M%SZ")
        .ok()
        .map(|naive| Utc.from_utc_datetime(&naive))
}

/// The last day whose occurrence starts at or before `until`, for a series
/// whose occurrences stand at `time` on its clock: the date the series ends
/// on so that it keeps exactly those occurrences.
///
/// `wall` reads an instant on the series' clock; `instant_of` places a wall
/// time on it, where the clock skips one as the series' occurrence is placed
/// there (pushed past the gap). `None` where either cannot say.
pub fn last_day_through(
    until: DateTime<Utc>,
    time: NaiveTime,
    wall: impl Fn(DateTime<Utc>) -> Option<NaiveDateTime>,
    instant_of: impl Fn(NaiveDateTime) -> Option<DateTime<Utc>>,
) -> Option<NaiveDate> {
    let day = wall(until)?.date();
    let occurrence = instant_of(day.and_time(time))?;
    if occurrence <= until {
        Some(day)
    } else {
        day.pred_opt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, FixedOffset};

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().expect("an instant")
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("a date")
    }

    fn hm(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).expect("a time")
    }

    /// A clock at a fixed offset from UTC, in hours.
    fn fixed(
        hours: i32,
    ) -> (
        impl Fn(DateTime<Utc>) -> Option<NaiveDateTime>,
        impl Fn(NaiveDateTime) -> Option<DateTime<Utc>>,
    ) {
        let offset = FixedOffset::east_opt(hours * 3600).expect("an offset");
        (
            move |when: DateTime<Utc>| Some(when.with_timezone(&offset).naive_local()),
            move |local: NaiveDateTime| {
                offset
                    .from_local_datetime(&local)
                    .single()
                    .map(|t| t.with_timezone(&Utc))
            },
        )
    }

    fn end(until: &str, time: NaiveTime, hours: i32) -> Option<NaiveDate> {
        let (wall, instant_of) = fixed(hours);
        last_day_through(at(until), time, wall, instant_of)
    }

    #[test]
    fn a_cut_ends_on_the_day_before_the_cut_on_the_series_clock() {
        // Daily 08:00 Berlin, cut on Thu 5.11. (round 10a, M5).
        assert_eq!(
            end("2026-11-05T06:59:59Z", hm(8, 0), 1),
            Some(day(2026, 11, 4))
        );
        // The 8a shape: 10:00 Berlin, cut on Wed 4.11.
        assert_eq!(
            end("2026-11-04T08:59:59Z", hm(10, 0), 1),
            Some(day(2026, 11, 3))
        );
        // 23:30 Berlin, cut on Wed 4.11.: the UNTIL is the 4.11. on the clock
        // as in UTC, and that day's occurrence starts after it, so the day
        // before; the UTC date kept the 4.11.
        assert_eq!(
            end("2026-11-04T22:29:59Z", hm(23, 30), 1),
            Some(day(2026, 11, 3))
        );
    }

    #[test]
    fn east_of_utc_after_midnight_the_utc_date_is_already_the_day_before() {
        // Daily 00:30 Berlin, cut on Wed 4.11. (round 10a, M1): 3.11. either way.
        assert_eq!(
            end("2026-11-03T23:29:59Z", hm(0, 30), 1),
            Some(day(2026, 11, 3))
        );
    }

    #[test]
    fn west_of_utc_in_the_evening_the_utc_date_was_the_day_after() {
        // Daily 20:00 New York (UTC-5), cut on Wed 4.11. (round 10a, M2): the
        // UTC date 5.11. kept the 4.11. and the 5.11. both.
        assert_eq!(
            end("2026-11-05T00:59:59Z", hm(20, 0), -5),
            Some(day(2026, 11, 3))
        );
    }

    #[test]
    fn a_weekly_cut_on_a_monday_ends_on_the_sunday_before() {
        // Mondays 08:00 Berlin, cut on Mon 16.11.: the same occurrences as an
        // end on 9.11. (round 10a, M3: Exchange keeps the Sunday).
        assert_eq!(
            end("2026-11-16T06:59:59Z", hm(8, 0), 1),
            Some(day(2026, 11, 15))
        );
    }

    #[test]
    fn an_until_on_an_occurrence_keeps_its_day() {
        // UNTIL is inclusive: an occurrence exactly at it stays.
        assert_eq!(
            end("2026-11-05T07:00:00Z", hm(8, 0), 1),
            Some(day(2026, 11, 5))
        );
    }

    #[test]
    fn a_skipped_wall_time_is_read_where_the_occurrence_stands() {
        // Daily 02:30 Berlin; on 29.3.2026 the clock skips 02:00-03:00 and the
        // occurrence stands at 03:30 CEST (01:30Z). Cut there: one second
        // before it, the day before.
        let wall = |when: DateTime<Utc>| {
            let hours = if when >= at("2026-03-29T01:00:00Z") {
                2
            } else {
                1
            };
            Some((when + Duration::hours(hours)).naive_utc())
        };
        let instant_of = |local: NaiveDateTime| {
            let gap_from = day(2026, 3, 29).and_time(hm(2, 0));
            let gap_to = day(2026, 3, 29).and_time(hm(3, 0));
            if local >= gap_from && local < gap_to {
                // Pushed past the gap, as the series' occurrence is.
                return Some(Utc.from_utc_datetime(&(local - Duration::hours(1))));
            }
            let hours = if local >= gap_to { 2 } else { 1 };
            Some(Utc.from_utc_datetime(&(local - Duration::hours(hours))))
        };
        assert_eq!(
            last_day_through(at("2026-03-29T01:29:59Z"), hm(2, 30), wall, instant_of),
            Some(day(2026, 3, 28))
        );
    }

    #[test]
    fn a_clock_that_cannot_say_gives_no_day() {
        let none_wall = |_: DateTime<Utc>| None;
        let (_, instant_of) = fixed(1);
        assert_eq!(
            last_day_through(at("2026-11-05T06:59:59Z"), hm(8, 0), none_wall, instant_of),
            None
        );
        let (wall, _) = fixed(1);
        let none_instant = |_: NaiveDateTime| None;
        assert_eq!(
            last_day_through(at("2026-11-05T06:59:59Z"), hm(8, 0), wall, none_instant),
            None
        );
    }

    #[test]
    fn only_a_utc_date_time_until_is_an_instant() {
        assert_eq!(
            utc_until("FREQ=DAILY;UNTIL=20261105T065959Z"),
            Some(at("2026-11-05T06:59:59Z"))
        );
        assert_eq!(
            utc_until("RRULE:FREQ=WEEKLY;BYDAY=MO;until=20261116t065959z"),
            Some(at("2026-11-16T06:59:59Z"))
        );
        // An all-day cut's date, a floating time, no end at all.
        assert_eq!(utc_until("FREQ=DAILY;UNTIL=20261104"), None);
        assert_eq!(utc_until("FREQ=DAILY;UNTIL=20261105T065959"), None);
        assert_eq!(utc_until("FREQ=DAILY;COUNT=6"), None);
        assert_eq!(utc_until("FREQ=DAILY"), None);
    }
}
