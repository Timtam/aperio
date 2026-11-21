//! The rules cal-zone holds, on dates whose data no tzdata release will move.

use cal_zone::{Reading, WallPolicy, Zone, ZonedOffset, COVERED_YEARS};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};

fn zone(name: &str) -> Zone {
    Zone::named(name).unwrap_or_else(|| panic!("{name} is bundled"))
}

fn wall(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .and_then(|day| day.and_hms_opt(h, mi, s))
        .expect("a real wall time")
}

fn instant(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .expect("an RFC 3339 instant")
        .with_timezone(&Utc)
}

fn hours(h: i32) -> FixedOffset {
    FixedOffset::east_opt(h * 3600).expect("a whole-hour offset")
}

fn minutes(m: i32) -> FixedOffset {
    FixedOffset::east_opt(m * 60).expect("a whole-minute offset")
}

#[test]
fn a_name_matches_in_exact_case_and_nothing_is_trimmed() {
    assert_eq!(zone("Europe/Berlin").name(), "Europe/Berlin");
    for not_a_zone in [
        "europe/berlin",
        "EUROPE/BERLIN",
        " Europe/Berlin",
        "Europe/Berlin ",
        "",
        "utc",
        "W. Europe Standard Time",
        "+05:30",
        "Not/AZone",
    ] {
        assert_eq!(Zone::named(not_a_zone), None, "{not_a_zone:?}");
    }
}

#[test]
fn factory_and_etc_unknown_are_no_zones() {
    // Factory is bundled (tzdata's placeholder for an unset machine);
    // Etc/Unknown is not bundled at all. Neither names a zone.
    assert!(jiff_tzdb::get("Factory").is_some());
    assert_eq!(Zone::named("Factory"), None);
    assert_eq!(Zone::named("Etc/Unknown"), None);
    assert!(Zone::all().all(|z| z.name() != "Factory"));
}

#[test]
fn utc_is_its_own_name_and_links_stay_themselves() {
    assert_eq!(Zone::named("UTC"), Some(Zone::UTC));
    assert_eq!(Zone::UTC.name(), "UTC");
    for other in ["Etc/UTC", "GMT", "Zulu", "Etc/GMT", "Universal"] {
        let z = zone(other);
        assert_ne!(z, Zone::UTC, "{other}");
        assert_eq!(z.name(), other);
    }
    assert_ne!(zone("US/Pacific"), zone("America/Los_Angeles"));
    assert_ne!(zone("Asia/Calcutta"), zone("Asia/Kolkata"));
    assert_eq!(zone("US/Pacific"), zone("US/Pacific"));
}

#[test]
fn every_bundled_name_but_factory_parses_and_round_trips() {
    let mut bundled: Vec<&str> = jiff_tzdb::available()
        .filter(|name| *name != "Factory")
        .collect();
    bundled.sort_unstable();
    bundled.dedup();
    let all: Vec<Zone> = Zone::all().collect();
    let mut names: Vec<&str> = all.iter().map(|z| z.name()).collect();
    assert_eq!(names.first(), Some(&"UTC"));
    let rest = &names[1..];
    assert!(rest.windows(2).all(|w| w[0] < w[1]), "byte order after UTC");
    names.sort_unstable();
    assert_eq!(names, bundled);
    for z in all {
        assert_eq!(Zone::named(z.name()), Some(z));
        assert_eq!(z.name().parse::<Zone>(), Ok(z));
    }
    assert!("europe/berlin".parse::<Zone>().is_err());
}

#[test]
fn a_zone_is_two_bytes_copy_and_shareable() {
    fn shareable<T: Copy + Send + Sync + 'static>() {}
    shareable::<Zone>();
    shareable::<ZonedOffset>();
    shareable::<DateTime<Zone>>();
    assert_eq!(std::mem::size_of::<Zone>(), 2);
}

#[test]
fn debug_and_display_print_the_bare_name() {
    let berlin = zone("Europe/Berlin");
    assert_eq!(format!("{berlin:?}"), "Europe/Berlin");
    assert_eq!(berlin.to_string(), "Europe/Berlin");
    assert_eq!(format!("{:?}", Zone::UTC), "UTC");
}

#[test]
fn berlin_skips_two_oclock_in_spring_and_shows_it_twice_in_autumn() {
    let berlin = zone("Europe/Berlin");
    assert_eq!(
        berlin.reading(wall(2026, 3, 29, 1, 59, 59)),
        Reading::Single(hours(1))
    );
    assert_eq!(
        berlin.reading(wall(2026, 3, 29, 2, 0, 0)),
        Reading::Gap {
            before: hours(1),
            after: hours(2)
        }
    );
    assert_eq!(
        berlin.reading(wall(2026, 3, 29, 2, 59, 59)),
        Reading::Gap {
            before: hours(1),
            after: hours(2)
        }
    );
    assert_eq!(
        berlin.reading(wall(2026, 3, 29, 3, 0, 0)),
        Reading::Single(hours(2))
    );

    assert_eq!(
        berlin.reading(wall(2026, 10, 25, 1, 59, 59)),
        Reading::Single(hours(2))
    );
    assert_eq!(
        berlin.reading(wall(2026, 10, 25, 2, 0, 0)),
        Reading::Fold {
            earlier: hours(2),
            later: hours(1)
        }
    );
    assert_eq!(
        berlin.reading(wall(2026, 10, 25, 3, 0, 0)),
        Reading::Single(hours(1))
    );
}

#[test]
fn as_a_chrono_zone_a_gap_is_none_and_a_fold_is_earlier_then_later() {
    let berlin = zone("Europe/Berlin");
    assert!(berlin
        .from_local_datetime(&wall(2026, 3, 29, 2, 30, 0))
        .single()
        .is_none());
    assert!(berlin
        .from_local_datetime(&wall(2026, 3, 29, 2, 30, 0))
        .earliest()
        .is_none());
    let fold = berlin.from_local_datetime(&wall(2026, 10, 25, 2, 30, 0));
    let earliest = fold.earliest().expect("a fold has two readings");
    let latest = fold.latest().expect("a fold has two readings");
    assert_eq!(
        earliest.with_timezone(&Utc),
        instant("2026-10-25T00:30:00Z")
    );
    assert_eq!(latest.with_timezone(&Utc), instant("2026-10-25T01:30:00Z"));
    assert_eq!(earliest.timezone(), berlin);
    assert_eq!(earliest.offset().fix(), hours(2));
    // A date reads as one offset: midnight's, which is unambiguous.
    let day = berlin
        .offset_from_local_date(&NaiveDate::from_ymd_opt(2026, 3, 29).expect("a date"))
        .single()
        .expect("one offset for the day");
    assert_eq!(day.fix(), hours(1));
    assert_eq!(
        Utc.from_utc_datetime(&wall(2026, 7, 1, 12, 0, 0))
            .with_timezone(&berlin)
            .naive_local(),
        wall(2026, 7, 1, 14, 0, 0)
    );
}

#[test]
fn the_policies_on_lord_howes_half_hour_gap() {
    // Lord Howe moves from +10:30 to +11:00 at 02:00 local on the first
    // Sunday of October: 2020-10-04 02:15 does not exist.
    let lord_howe = zone("Australia/Lord_Howe");
    let skipped = wall(2020, 10, 4, 2, 15, 0);
    assert_eq!(
        lord_howe.reading(skipped),
        Reading::Gap {
            before: minutes(630),
            after: hours(11)
        }
    );
    assert_eq!(lord_howe.to_utc(skipped, WallPolicy::Strict), None);
    // An hour later: 03:15 at +11:00.
    assert_eq!(
        lord_howe.to_utc(skipped, WallPolicy::EarlierElseHourLater),
        Some(instant("2020-10-03T16:15:00Z"))
    );
    // Read with the offset from before the change: 02:15 at +10:30 (02:45 on the clock).
    assert_eq!(
        lord_howe.to_utc(skipped, WallPolicy::Rfc5545),
        Some(instant("2020-10-03T15:45:00Z"))
    );
}

#[test]
fn the_policies_on_berlins_hour_gap_and_fold() {
    let berlin = zone("Europe/Berlin");
    let skipped = wall(2026, 3, 29, 2, 30, 0);
    let twice = wall(2026, 10, 25, 2, 30, 0);
    assert_eq!(berlin.to_utc(skipped, WallPolicy::Strict), None);
    assert_eq!(berlin.to_utc(twice, WallPolicy::Strict), None);
    for policy in [WallPolicy::EarlierElseHourLater, WallPolicy::Rfc5545] {
        assert_eq!(
            berlin.to_utc(skipped, policy),
            Some(instant("2026-03-29T01:30:00Z")),
            "{policy:?}"
        );
        assert_eq!(
            berlin.to_utc(twice, policy),
            Some(instant("2026-10-25T00:30:00Z")),
            "{policy:?}"
        );
    }
    let once = wall(2026, 7, 1, 9, 0, 0);
    for policy in [
        WallPolicy::Strict,
        WallPolicy::EarlierElseHourLater,
        WallPolicy::Rfc5545,
    ] {
        assert_eq!(
            berlin.to_utc(once, policy),
            Some(instant("2026-07-01T07:00:00Z")),
            "{policy:?}"
        );
    }
}

#[test]
fn apia_skipped_a_whole_day() {
    // Samoa crossed the date line: 2011-12-30 never happened in Apia.
    let apia = zone("Pacific/Apia");
    let lost = wall(2011, 12, 30, 10, 0, 0);
    assert!(matches!(apia.reading(lost), Reading::Gap { .. }));
    assert_eq!(apia.to_utc(lost, WallPolicy::Strict), None);
    assert_eq!(apia.to_utc(lost, WallPolicy::EarlierElseHourLater), None);
    assert!(apia.to_utc(lost, WallPolicy::Rfc5545).is_some());
}

#[test]
fn sao_paulo_skipped_midnight() {
    let sao_paulo = zone("America/Sao_Paulo");
    assert_eq!(
        sao_paulo.reading(wall(2018, 11, 4, 0, 30, 0)),
        Reading::Gap {
            before: hours(-3),
            after: hours(-2)
        }
    );
    assert_eq!(
        sao_paulo.reading(wall(2018, 11, 4, 1, 0, 0)),
        Reading::Single(hours(-2))
    );
}

#[test]
fn the_bundled_data_is_tzdatas_rearguard_form() {
    // Dublin's winter GMT is standard time and its summer IST daylight
    // saving. tzdata's main form says the reverse (a negative save in winter).
    let dublin = zone("Europe/Dublin");
    let winter = dublin.offset_info(instant("2020-01-15T12:00:00Z"));
    assert_eq!(
        (winter.offset, winter.dst, winter.abbreviation.as_str()),
        (hours(0), false, "GMT")
    );
    let summer = dublin.offset_info(instant("2020-07-15T12:00:00Z"));
    assert_eq!(
        (summer.offset, summer.dst, summer.abbreviation.as_str()),
        (hours(1), true, "IST")
    );
}

#[test]
fn numeric_abbreviations_come_as_tzdata_writes_them() {
    let dubai = zone("Asia/Dubai").offset_info(instant("2020-07-01T00:00:00Z"));
    assert_eq!(
        (dubai.offset, dubai.dst, dubai.abbreviation.as_str()),
        (hours(4), false, "+04")
    );
    let utc = Zone::UTC.offset_info(instant("2020-07-01T00:00:00Z"));
    assert_eq!(
        (utc.offset, utc.dst, utc.abbreviation.as_str()),
        (hours(0), false, "UTC")
    );
}

#[test]
fn berlins_clock_changes_of_2020() {
    let changes: Vec<_> = zone("Europe/Berlin")
        .offset_changes(instant("2020-01-01T00:00:00Z"))
        .take(2)
        .map(|c| (c.at, c.from, c.to, c.dst, c.abbreviation))
        .collect();
    assert_eq!(
        changes,
        [
            (
                instant("2020-03-29T01:00:00Z"),
                hours(1),
                hours(2),
                true,
                "CEST".to_owned()
            ),
            (
                instant("2020-10-25T01:00:00Z"),
                hours(2),
                hours(1),
                false,
                "CET".to_owned()
            ),
        ]
    );
    // Strictly after: a change AT the start is not listed.
    let next = zone("Europe/Berlin")
        .offset_changes(instant("2020-03-29T01:00:00Z"))
        .next()
        .expect("Berlin keeps changing");
    assert_eq!(next.at, instant("2020-10-25T01:00:00Z"));
}

#[test]
fn every_listed_change_moves_the_clock_and_follows_the_last() {
    let from = instant("1900-01-01T00:00:00Z");
    let until = instant("2100-01-01T00:00:00Z");
    for z in Zone::all() {
        let mut offset = z.offset_at(from);
        for change in z.offset_changes(from).take_while(|c| c.at < until) {
            assert_eq!(change.from, offset, "{z} at {}", change.at);
            assert_ne!(change.from, change.to, "{z} at {}", change.at);
            assert_eq!(z.offset_at(change.at), change.to, "{z} at {}", change.at);
            offset = change.to;
        }
    }
    assert_eq!(Zone::UTC.offset_changes(from).next(), None);
    assert_eq!(zone("Etc/GMT-2").offset_changes(from).next(), None);
}

#[test]
fn every_offset_fits_chronos_fixed_offset() {
    // chrono's FixedOffset holds less than a day; tzdata's largest offset is
    // Asia/Manila's local mean time before 1845, +15:56:08.
    let mut largest = (0, "");
    for z in Zone::all() {
        let first = z.offset_at(DateTime::<Utc>::MIN_UTC).local_minus_utc();
        let offsets = std::iter::once(first).chain(
            z.offset_changes(DateTime::<Utc>::MIN_UTC)
                .take_while(|c| c.at.timestamp() < 4_102_444_800)
                .map(|c| c.to.local_minus_utc()),
        );
        for seconds in offsets {
            assert!(seconds.abs() < 86_399, "{z}: {seconds}");
            if seconds.abs() > largest.0 {
                largest = (seconds.abs(), z.name());
            }
        }
    }
    assert_eq!(largest, (57_368, "Asia/Manila"));
}

#[test]
fn outside_jiffs_years_every_answer_is_the_one_at_the_edge() {
    let berlin = zone("Europe/Berlin");
    assert!(COVERED_YEARS.contains(&2026));
    let first = berlin.offset_at(DateTime::<Utc>::MIN_UTC);
    let last = berlin.offset_at(DateTime::<Utc>::MAX_UTC);
    assert_eq!(
        first,
        berlin.offset_at(wall(-9999, 6, 1, 0, 0, 0).and_utc())
    );
    assert_eq!(
        last,
        berlin.offset_at(wall(9999, 12, 30, 12, 0, 0).and_utc())
    );
    let _ = berlin.wall(DateTime::<Utc>::MIN_UTC);
    assert_eq!(berlin.wall(DateTime::<Utc>::MAX_UTC), NaiveDateTime::MAX);
    assert!(matches!(
        berlin.reading(wall(200_000, 7, 1, 9, 0, 0)),
        Reading::Single(_)
    ));
    assert!(matches!(
        berlin.reading(wall(-200_000, 7, 1, 9, 0, 0)),
        Reading::Single(_)
    ));
    // rrule's counter reaches the year 10000, a leap year: no rebuilt
    // 9999-02-29.
    assert!(matches!(
        berlin.reading(wall(10_000, 2, 29, 9, 0, 0)),
        Reading::Single(_)
    ));
    assert!(berlin
        .to_utc(wall(10_000, 2, 29, 9, 0, 0), WallPolicy::Strict)
        .is_some());
    // A chrono leap second.
    let leap = NaiveDate::from_ymd_opt(2016, 12, 31)
        .and_then(|d| d.and_hms_nano_opt(23, 59, 59, 1_500_000_000))
        .expect("chrono's leap second");
    assert_eq!(
        zone("Europe/London").reading(leap),
        Reading::Single(hours(0))
    );
    for policy in [
        WallPolicy::Strict,
        WallPolicy::EarlierElseHourLater,
        WallPolicy::Rfc5545,
    ] {
        let _ = berlin.to_utc(NaiveDateTime::MIN, policy);
        let _ = berlin.to_utc(NaiveDateTime::MAX, policy);
    }
}

#[test]
fn utc_never_moves() {
    for at in [
        DateTime::<Utc>::MIN_UTC,
        instant("2026-03-29T01:00:00Z"),
        DateTime::<Utc>::MAX_UTC,
    ] {
        assert_eq!(Zone::UTC.offset_at(at), hours(0));
    }
    assert_eq!(
        Zone::UTC.reading(wall(2026, 3, 29, 2, 30, 0)),
        Reading::Single(hours(0))
    );
}
