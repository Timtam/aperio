//! Aperio's own cases for the fork's zones (cal-zone), on dates no tzdata
//! release will move. Not from upstream; see SOURCE.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use rrule::{RRule, RRuleSet, Tz, Unvalidated};

fn occurrences(rule: &str, zone: &str, wall: (i32, u32, u32, u32, u32)) -> Vec<DateTime<Utc>> {
    let tz = Tz::named(zone).expect("a bundled zone");
    let (y, mo, d, h, mi) = wall;
    let start = tz
        .from_local_datetime(
            &NaiveDate::from_ymd_opt(y, mo, d)
                .and_then(|day| day.and_hms_opt(h, mi, 0))
                .expect("a wall time"),
        )
        .earliest()
        .expect("the start exists");
    let rule: RRule<Unvalidated> = rule.parse().expect("a rule");
    rule.build(start)
        .expect("a valid rule")
        .all(100)
        .dates
        .iter()
        .map(|d| d.with_timezone(&Utc))
        .collect()
}

fn utc(text: &str) -> DateTime<Utc> {
    text.parse().expect("an RFC 3339 instant")
}

#[test]
fn a_skipped_wall_time_moves_past_the_gap_and_a_repeated_one_reads_first() {
    // Berlin 2018: 02:30 does not exist on 25 March and exists twice on 28
    // October.
    let spring = occurrences("FREQ=DAILY;COUNT=3", "Europe/Berlin", (2018, 3, 24, 2, 30));
    assert_eq!(spring[1], utc("2018-03-25T01:30:00Z"));
    let autumn = occurrences("FREQ=DAILY;COUNT=3", "Europe/Berlin", (2018, 10, 27, 2, 30));
    assert_eq!(autumn[1], utc("2018-10-28T00:30:00Z"));
}

#[test]
fn a_day_whose_wall_time_and_midnight_are_both_skipped_is_dropped() {
    // São Paulo sprang forward at 00:00 on 2018-11-04: 00:30 and midnight do
    // not exist, so that day has no occurrence, and COUNT goes on.
    let dates = occurrences(
        "FREQ=DAILY;COUNT=4",
        "America/Sao_Paulo",
        (2018, 11, 2, 0, 30),
    );
    let days: Vec<u32> = dates
        .iter()
        .map(|d| {
            chrono::Datelike::day(
                &d.with_timezone(&Tz::named("America/Sao_Paulo").unwrap())
                    .date_naive(),
            )
        })
        .collect();
    assert_eq!(days, [2, 3, 5, 6]);
}

#[test]
fn bysetpos_skips_a_dropped_occurrence() {
    // The first of the first Sunday's two times, monthly. In November 2018
    // São Paulo's 00:30 on the 4th does not exist and is dropped: the
    // position names nothing that month, it does not move on to 12:30.
    let dates = occurrences(
        "FREQ=MONTHLY;BYDAY=1SU;BYHOUR=0,12;BYMINUTE=30;BYSETPOS=1;COUNT=3",
        "America/Sao_Paulo",
        (2018, 10, 1, 0, 30),
    );
    assert_eq!(
        dates,
        [
            utc("2018-10-07T03:30:00Z"),
            utc("2018-12-02T02:30:00Z"),
            utc("2019-01-06T02:30:00Z"),
        ]
    );
}

#[test]
fn only_utc_itself_is_utc() {
    assert_eq!(Tz::named("UTC"), Some(Tz::UTC));
    assert_ne!(Tz::named("Etc/UTC"), Some(Tz::UTC));
    assert_eq!(Tz::named("europe/berlin"), None);
    assert_eq!(Tz::named("Factory"), None);
    // A zoned start wants its UNTIL in UTC itself; Etc/UTC is another zone.
    let set: RRuleSet = "DTSTART:20200101T090000Z\nRRULE:FREQ=DAILY;COUNT=2"
        .parse()
        .expect("parses");
    assert!(set.to_string().starts_with("DTSTART:20200101T090000Z"));
    let set: RRuleSet = "DTSTART;TZID=Etc/UTC:20200101T090000\nRRULE:FREQ=DAILY;COUNT=2"
        .parse()
        .expect("parses");
    assert!(set
        .to_string()
        .starts_with("DTSTART;TZID=Etc/UTC:20200101T090000"));
}
