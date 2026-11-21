//! TEMPORARY: this fork against the registry rrule 0.14.0 it was forked from,
//! which repeats on chrono-tz 0.10.4's zones. It leaves together with
//! chrono-tz.
//!
//! The zones below hold the same data in tzdata 2025b and 2026f from 2020 to
//! 2040, so every answer must be the same: the occurrences (instant and wall
//! clock), the `limited` flag, and every validation and parse error. That
//! includes the drop of an occurrence whose wall time and local midnight a
//! clock change both skip or repeat.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};

const ZONES: &[&str] = &[
    "Europe/Berlin",
    "America/New_York",
    "America/Santiago",
    "America/Sao_Paulo",
    "Australia/Lord_Howe",
    "Pacific/Apia",
    "Europe/Dublin",
    "Asia/Kolkata",
    "Pacific/Chatham",
    "US/Pacific",
    "Etc/UTC",
    "UTC",
];

const RULES: &[&str] = &[
    "FREQ=DAILY",
    "FREQ=DAILY;COUNT=500",
    "FREQ=WEEKLY;BYDAY=MO,TH",
    "FREQ=MONTHLY;BYDAY=-1SU",
    "FREQ=YEARLY;BYMONTH=3,10;BYDAY=-1SU",
    "FREQ=HOURLY;INTERVAL=7",
    "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1",
    "FREQ=DAILY;UNTIL=20301231T235959Z",
    "FREQ=DAILY;BYHOUR=0,2,23;BYMINUTE=30",
];

const WALLS: &[(u32, u32)] = &[(0, 0), (0, 30), (2, 30), (23, 30)];

type Occurrences = (Vec<(DateTime<Utc>, NaiveDateTime)>, bool);

fn fork_zone(name: &str) -> rrule::Tz {
    rrule::Tz::named(name).expect("the fork knows the zone")
}

fn upstream_zone(name: &str) -> rrule_upstream::Tz {
    if name == "UTC" {
        return rrule_upstream::Tz::UTC;
    }
    rrule_upstream::Tz::Tz(name.parse().expect("chrono-tz knows the zone"))
}

fn fork(rule: &str, zone: &str, start: NaiveDateTime) -> Result<Option<Occurrences>, String> {
    let tz = fork_zone(zone);
    let Some(start) = tz.from_local_datetime(&start).earliest() else {
        return Ok(None);
    };
    let rule: rrule::RRule<rrule::Unvalidated> = rule.parse().map_err(|e| format!("{e}"))?;
    let set = rule.build(start).map_err(|e| format!("{e}"))?;
    let end = Utc.with_ymd_and_hms(2041, 1, 1, 0, 0, 0).unwrap();
    let result = set.before(end.with_timezone(&rrule::Tz::UTC)).all(u16::MAX);
    Ok(Some((
        result
            .dates
            .iter()
            .map(|d| (d.with_timezone(&Utc), d.naive_local()))
            .collect(),
        result.limited,
    )))
}

fn upstream(rule: &str, zone: &str, start: NaiveDateTime) -> Result<Option<Occurrences>, String> {
    let tz = upstream_zone(zone);
    let Some(start) = tz.from_local_datetime(&start).earliest() else {
        return Ok(None);
    };
    let rule: rrule_upstream::RRule<rrule_upstream::Unvalidated> =
        rule.parse().map_err(|e| format!("{e}"))?;
    let set = rule.build(start).map_err(|e| format!("{e}"))?;
    let end = Utc.with_ymd_and_hms(2041, 1, 1, 0, 0, 0).unwrap();
    let result = set
        .before(end.with_timezone(&rrule_upstream::Tz::UTC))
        .all(u16::MAX);
    Ok(Some((
        result
            .dates
            .iter()
            .map(|d| (d.with_timezone(&Utc), d.naive_local()))
            .collect(),
        result.limited,
    )))
}

/// The days a series starts on: the first of 2020, and the days of the
/// zone's clock changes in 2026, when a start or an occurrence meets them.
fn start_days(zone: &str) -> Vec<NaiveDate> {
    let mut days = vec![NaiveDate::from_ymd_opt(2020, 1, 1).unwrap()];
    if let Some(z) = cal_zone::Zone::named(zone) {
        let from = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        for change in z.offset_changes(from).take(2) {
            let day = z.wall(change.at).date();
            days.push(day);
            days.push(day - Duration::days(1));
        }
    }
    days
}

#[test]
fn the_same_occurrences_as_upstream() {
    let mut differences = Vec::new();
    let mut compared = 0usize;
    for zone in ZONES {
        for day in start_days(zone) {
            for (h, m) in WALLS {
                let start = day.and_hms_opt(*h, *m, 0).unwrap();
                for rule in RULES {
                    let (ours, theirs) = (fork(rule, zone, start), upstream(rule, zone, start));
                    if ours != theirs {
                        differences.push(format!("{zone} {start} {rule}"));
                    }
                    compared += 1;
                }
            }
        }
    }
    println!(
        "{compared} series compared, {} different",
        differences.len()
    );
    assert!(
        differences.is_empty(),
        "{} of {compared}: {differences:#?}",
        differences.len()
    );
}

#[test]
fn a_day_whose_midnight_and_wall_time_are_skipped_is_dropped_in_both() {
    // Santiago springs forward at 00:00 on 2026-09-06: 00:30 and midnight do
    // not exist, so add_time_to_date drops that day, and COUNT goes on.
    let start = NaiveDate::from_ymd_opt(2026, 9, 3)
        .unwrap()
        .and_hms_opt(0, 30, 0)
        .unwrap();
    let ours = fork("FREQ=DAILY;COUNT=5", "America/Santiago", start).expect("builds");
    let theirs = upstream("FREQ=DAILY;COUNT=5", "America/Santiago", start).expect("builds");
    assert_eq!(ours, theirs);
    let (dates, _) = ours.expect("the start exists");
    let days: Vec<u32> = dates
        .iter()
        .map(|(_, wall)| chrono::Datelike::day(&wall.date()))
        .collect();
    assert_eq!(days, [3, 4, 5, 7, 8]);
}

#[test]
fn the_same_errors_as_upstream() {
    // A zoned start with an UNTIL that is not UTC: the until's zone is Local.
    for rule in [
        "FREQ=DAILY;UNTIL=20301231T235959",
        "FREQ=DAILY;UNTIL=20301231",
    ] {
        let start = NaiveDate::from_ymd_opt(2026, 1, 1)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        assert_eq!(
            fork(rule, "Europe/Berlin", start).err(),
            upstream(rule, "Europe/Berlin", start).err(),
            "{rule}"
        );
        assert!(fork(rule, "Europe/Berlin", start).is_err(), "{rule}");
    }
    for text in [
        "DTSTART;TZID=Europe/Berlin:20200101T090000\nRRULE:FREQ=DAILY;COUNT=3",
        "DTSTART;TZID=europe/berlin:20200101T090000\nRRULE:FREQ=DAILY;COUNT=3",
        "DTSTART;TZID=Factory:20200101T090000\nRRULE:FREQ=DAILY;COUNT=3",
        "DTSTART;TZID=Etc/Unknown:20200101T090000\nRRULE:FREQ=DAILY;COUNT=3",
        "DTSTART;TZID=Etc/UTC:20200101T090000\nRRULE:FREQ=DAILY;COUNT=3",
        "DTSTART;TZID=Europe/Berlin:20260329T023000\nRRULE:FREQ=DAILY;COUNT=3",
        "DTSTART;TZID=Europe/Berlin:20261025T023000\nRRULE:FREQ=DAILY;COUNT=3",
        "DTSTART;TZID=America/New_York:19000201T093000Z\nRRULE:UNTIL=19990404T110000Z;FREQ=WEEKLY;BYDAY=TU,WE",
        "DTSTART:20120201T093000Z\nRRULE:FREQ=WEEKLY;INTERVAL=5;BYDAY=-2MO,FR",
    ] {
        let ours = text.parse::<rrule::RRuleSet>().map(|s| s.to_string());
        let theirs = text.parse::<rrule_upstream::RRuleSet>().map(|s| s.to_string());
        assert_eq!(
            ours.map_err(|e| e.to_string()),
            theirs.map_err(|e| e.to_string()),
            "{text}"
        );
    }
}

#[test]
fn only_utc_itself_writes_z() {
    let set: rrule::RRuleSet = "DTSTART:20200101T090000Z\nRRULE:FREQ=DAILY;COUNT=2"
        .parse()
        .expect("parses");
    assert!(set.to_string().starts_with("DTSTART:20200101T090000Z"));
    let set: rrule::RRuleSet = "DTSTART;TZID=Etc/UTC:20200101T090000\nRRULE:FREQ=DAILY;COUNT=2"
        .parse()
        .expect("parses");
    assert!(set
        .to_string()
        .starts_with("DTSTART;TZID=Etc/UTC:20200101T090000"));
    assert_eq!(rrule::Tz::named("UTC"), Some(rrule::Tz::UTC));
    assert_ne!(rrule::Tz::named("Etc/UTC"), Some(rrule::Tz::UTC));
}
