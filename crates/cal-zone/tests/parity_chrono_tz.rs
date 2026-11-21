//! TEMPORARY: cal-zone against chrono-tz 0.10.4 (IANA tzdata 2025b). It
//! leaves together with chrono-tz.
//!
//! Every difference is NAMED in `CHANGED_2025B_2026F` with the window of
//! instants in which it may show and the tzdata release that made it. Any
//! other difference fails, and so does an entry that no longer differs.

use std::collections::BTreeMap;

use cal_zone::{WallPolicy, Zone};
use chrono::{
    DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, Offset, TimeZone, Timelike, Utc,
};
use chrono_tz::{OffsetComponents, OffsetName, Tz};

const BRAZIL_1965: &[&str] = &[
    "America/Araguaina",
    "America/Bahia",
    "America/Belem",
    "America/Boa_Vista",
    "America/Campo_Grande",
    "America/Cuiaba",
    "America/Eirunepe",
    "America/Fortaleza",
    "America/Maceio",
    "America/Manaus",
    "America/Noronha",
    "America/Porto_Acre",
    "America/Porto_Velho",
    "America/Recife",
    "America/Rio_Branco",
    "America/Santarem",
    "America/Sao_Paulo",
    "Brazil/Acre",
    "Brazil/DeNoronha",
    "Brazil/East",
    "Brazil/West",
];

/// Names whose clocks tzdata 2025c..2026f moved: the window of instants
/// `[from, until)` in which the two releases may disagree (an empty `until`
/// is open), and why. Links are listed by their own names, as both libraries
/// know them.
const CHANGED_2025B_2026F: &[(&[&str], &str, &str, &str)] = &[
    (
        &["Africa/Casablanca", "Africa/El_Aaiun"],
        "2026-09-20T01:00:00Z",
        "",
        "2026c: Morocco moves to permanent +00 on 2026-09-20",
    ),
    (
        &["America/Vancouver", "Canada/Pacific"],
        "2026-11-01T09:00:00Z",
        "",
        "2026b: British Columbia keeps -07, no fall-back on 2026-11-01",
    ),
    (
        &[
            "America/Edmonton",
            "America/Inuvik",
            "America/Yellowknife",
            "Canada/Mountain",
        ],
        "2026-11-01T08:00:00Z",
        "",
        "2026c, 2026d: Alberta and the Northwest Territories keep -06",
    ),
    (
        &["America/Winnipeg", "America/Rainy_River", "Canada/Central"],
        "2026-11-01T07:00:00Z",
        "",
        "2026e: Manitoba moves to permanent -05",
    ),
    (
        &["America/Havana", "Cuba"],
        "2026-11-01T05:00:00Z",
        "2027-03-14T05:00:00Z",
        "2026f: Cuba stays on -04 through 2027-11-07",
    ),
    (
        &["Europe/Chisinau", "Europe/Tiraspol"],
        "2022-03-27T00:00:00Z",
        "",
        "2026a: Moldova has used EU transition times since 2022",
    ),
    (
        &["America/Bogota"],
        "1992-05-02T05:00:00Z",
        "1992-05-03T05:00:00Z",
        "2026d: Colombia's 1992-05-02 spring forward was at 00:00, not 24:00",
    ),
    (
        &["Asia/Tehran", "Iran"],
        "1979-05-25T20:30:00Z",
        "1979-05-26T20:30:00Z",
        "2026d: Iran's 1979-05-26 spring forward was at 00:00, not 24:00",
    ),
    (
        &["EST5EDT", "CST6CDT", "MST7MDT", "PST8PDT"],
        "1920-01-01T00:00:00Z",
        "1966-10-30T09:00:00Z",
        "2026d: the POSIX-style names are zones of their own and follow POSIX before 1967",
    ),
    (
        &["Europe/Dublin", "Eire"],
        "1925-09-20T02:00:00Z",
        "1925-10-04T02:00:00Z",
        "2026e: in 1925 Ireland fell back on 09-20, not 10-04",
    ),
    (
        &[
            "America/Tijuana",
            "America/Ensenada",
            "America/Santa_Isabel",
            "Mexico/BajaNorte",
        ],
        "1953-04-26T09:00:00Z",
        "1975-10-26T09:00:00Z",
        "2025c: Baja California kept California's daylight saving rules 1953-1975",
    ),
    (
        BRAZIL_1965,
        "1965-03-31T00:00:00Z",
        "1966-03-02T00:00:00Z",
        "2026f: Brazil's 1965/1966 data improved",
    ),
];

fn instant(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .expect("an RFC 3339 instant")
        .with_timezone(&Utc)
}

/// The window in which `name` may differ.
fn changed(name: &str) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    CHANGED_2025B_2026F
        .iter()
        .find(|(names, _, _, _)| names.contains(&name))
        .map(|(_, from, until, _)| {
            let until = if until.is_empty() {
                DateTime::<Utc>::MAX_UTC
            } else {
                instant(until)
            };
            (instant(from), until)
        })
}

fn may_differ(name: &str, at: DateTime<Utc>) -> bool {
    changed(name).is_some_and(|(from, until)| from <= at && at < until)
}

/// Whether the window touches `[from, until)` at all: a wall time read near a
/// change, or a whole year, may meet it.
fn may_differ_within(name: &str, from: DateTime<Utc>, until: DateTime<Utc>) -> bool {
    changed(name).is_some_and(|(a, b)| a < until && from < b)
}

fn ctz(zone: Zone) -> Tz {
    zone.name()
        .parse()
        .expect("chrono-tz knows every cal-zone name")
}

fn utc(y: i32, m: u32, d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, 0, 0, 0)
        .single()
        .expect("a real day")
}

fn total(tz: &Tz, at: DateTime<Utc>) -> i32 {
    tz.offset_from_utc_datetime(&at.naive_utc())
        .fix()
        .local_minus_utc()
}

/// Collects differences by zone; the first one per zone is what the report
/// shows.
#[derive(Default)]
struct Report(BTreeMap<&'static str, Vec<String>>);

impl Report {
    fn note(&mut self, zone: Zone, what: String) {
        self.0.entry(zone.name()).or_default().push(what);
    }

    fn assert_empty(self, check: &str) {
        if self.0.is_empty() {
            return;
        }
        let lines: Vec<String> = self
            .0
            .iter()
            .map(|(name, whats)| format!("{name}: {} ({} in all)", whats[0], whats.len()))
            .collect();
        panic!("{check}: unnamed differences\n{}", lines.join("\n"));
    }
}

#[test]
fn the_same_names_both_ways() {
    let mut ours: Vec<&str> = Zone::all().map(Zone::name).collect();
    let mut theirs: Vec<&str> = chrono_tz::TZ_VARIANTS.iter().map(|tz| tz.name()).collect();
    ours.sort_unstable();
    theirs.sort_unstable();
    assert_eq!(ours, theirs);
    for name in [
        "europe/berlin",
        "EUROPE/BERLIN",
        " Europe/Berlin",
        "Europe/Berlin ",
        "",
        "utc",
        "Utc",
        "Factory",
        "Etc/Unknown",
        "W. Europe Standard Time",
        "+05:30",
    ] {
        assert_eq!(Zone::named(name), None, "{name:?}");
        assert!(name.parse::<Tz>().is_err(), "{name:?}");
    }
    for (a, b) in [
        ("UTC", "Etc/UTC"),
        ("US/Pacific", "America/Los_Angeles"),
        ("Asia/Calcutta", "Asia/Kolkata"),
        ("Europe/Berlin", "Europe/Berlin"),
    ] {
        let ours = Zone::named(a) == Zone::named(b);
        let theirs = a.parse::<Tz>().ok() == b.parse::<Tz>().ok();
        assert_eq!(ours, theirs, "{a} == {b}");
    }
    assert_eq!(Zone::named("UTC").map(ctz), Some(chrono_tz::UTC));
}

#[test]
fn the_same_offsets_mid_month_1900_to_2099() {
    let mut report = Report::default();
    for zone in Zone::all() {
        let tz = ctz(zone);
        for year in 1900..=2099 {
            for month in 1..=12 {
                let at = utc(year, month, 15) + Duration::hours(12);
                let (ours, theirs) = (zone.offset_at(at).local_minus_utc(), total(&tz, at));
                if ours != theirs && !may_differ(zone.name(), at) {
                    report.note(zone, format!("{at}: {ours} vs {theirs}"));
                }
            }
        }
    }
    report.assert_empty("mid-month offsets");
}

#[test]
fn chrono_tz_agrees_on_both_sides_of_every_change_1900_to_2099() {
    let mut report = Report::default();
    let (from, until) = (utc(1900, 1, 1), utc(2100, 1, 1));
    for zone in Zone::all() {
        let tz = ctz(zone);
        for change in zone.offset_changes(from).take_while(|c| c.at < until) {
            let just_before = change.at - Duration::seconds(1);
            if !may_differ(zone.name(), just_before)
                && total(&tz, just_before) != change.from.local_minus_utc()
            {
                report.note(zone, format!("before {}: {}", change.at, change.from));
            }
            if !may_differ(zone.name(), change.at)
                && total(&tz, change.at) != change.to.local_minus_utc()
            {
                report.note(zone, format!("at {}: {}", change.at, change.to));
            }
        }
    }
    report.assert_empty("changes");
}

/// The wall times around each change 1970-2099: both edges of the gap or
/// fold to the second, and a quarter-hour grid three hours either side.
fn walls_around_changes(zone: Zone) -> Vec<(DateTime<Utc>, NaiveDateTime)> {
    let (from, until) = (utc(1970, 1, 1), utc(2100, 1, 1));
    let mut walls = Vec::new();
    for change in zone.offset_changes(from).take_while(|c| c.at < until) {
        let at = change.at.naive_utc();
        let (a, b) = (
            at + Duration::seconds(change.from.local_minus_utc().into()),
            at + Duration::seconds(change.to.local_minus_utc().into()),
        );
        for edge in [a, b] {
            for step in [-1, 0, 1] {
                walls.push((change.at, edge + Duration::seconds(step)));
            }
        }
        let start = a.min(b) - Duration::hours(3);
        let start = start
            - Duration::minutes(i64::from(start.minute() % 15))
            - Duration::seconds(start.second().into());
        let mut wall = start;
        while wall <= a.max(b) + Duration::hours(3) {
            walls.push((change.at, wall));
            wall += Duration::minutes(15);
        }
    }
    walls
}

#[test]
fn the_same_local_readings_around_every_change_1970_to_2099() {
    let mut report = Report::default();
    for zone in Zone::all() {
        let tz = ctz(zone);
        for (change, wall) in walls_around_changes(zone) {
            if may_differ_within(
                zone.name(),
                change - Duration::days(1),
                change + Duration::days(1),
            ) {
                continue;
            }
            let ours = zone.offset_from_local_datetime(&wall).map(|o| o.fix());
            let theirs = tz.offset_from_local_datetime(&wall).map(|o| o.fix());
            if ours != theirs {
                report.note(zone, format!("{wall}: {ours:?} vs {theirs:?}"));
            }
        }
    }
    report.assert_empty("local readings");
}

/// adapter-ews DayZone::instant_of and local_midnight_in, verbatim.
fn earliest_else_an_hour_later(tz: &Tz, local: NaiveDateTime) -> Option<DateTime<Utc>> {
    let at = |local: NaiveDateTime| {
        tz.from_local_datetime(&local)
            .earliest()
            .map(|l| l.with_timezone(&Utc))
    };
    at(local).or_else(|| at(local + chrono::Duration::hours(1)))
}

/// adapter-google wall_clock_in, verbatim.
fn wall_clock_in(tz: Tz, wall: NaiveDateTime) -> DateTime<Utc> {
    match tz.from_local_datetime(&wall) {
        LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => at.with_timezone(&Utc),
        LocalResult::None => {
            let before = tz
                .offset_from_utc_datetime(&(wall - Duration::days(1)))
                .fix();
            Utc.from_utc_datetime(&(wall - Duration::seconds(before.local_minus_utc().into())))
        }
    }
}

#[test]
fn the_policies_equal_the_sites_they_came_from() {
    let mut report = Report::default();
    for zone in Zone::all() {
        let tz = ctz(zone);
        for (change, wall) in walls_around_changes(zone) {
            let oracles = [
                (
                    WallPolicy::Strict,
                    tz.from_local_datetime(&wall)
                        .single()
                        .map(|l| l.with_timezone(&Utc)),
                ),
                (
                    WallPolicy::EarlierElseHourLater,
                    earliest_else_an_hour_later(&tz, wall),
                ),
                (WallPolicy::Rfc5545, Some(wall_clock_in(tz, wall))),
            ];
            for (policy, oracle) in oracles {
                // The moved code, on the very zone the site used.
                if policy.resolve(&tz, wall) != oracle {
                    report.note(zone, format!("{policy:?} on chrono-tz at {wall}"));
                }
                if !may_differ_within(
                    zone.name(),
                    change - Duration::days(1),
                    change + Duration::days(1),
                ) && zone.to_utc(wall, policy) != oracle
                {
                    report.note(zone, format!("{policy:?} at {wall}"));
                }
            }
        }
    }
    report.assert_empty("policies");
}

#[test]
fn after_2099_jiff_keeps_the_rule_chrono_tz_dropped() {
    let berlin = Zone::named("Europe/Berlin").expect("Berlin");
    let at = utc(2100, 7, 1);
    assert_eq!(berlin.offset_at(at).local_minus_utc(), 7200);
    assert_eq!(total(&chrono_tz::Europe::Berlin, at), 3600);
}

/// ical-mapping vtimezone.rs year_transitions on chrono-tz, verbatim but for
/// the result type.
fn year_transitions(tz: &Tz, year: i32) -> Option<Vec<(NaiveDateTime, i32, i32, bool, String)>> {
    let mut out = Vec::new();
    let mut day = NaiveDate::from_ymd_opt(year, 1, 1)?.and_hms_opt(0, 0, 0)?;
    let end = NaiveDate::from_ymd_opt(year + 1, 1, 1)?.and_hms_opt(0, 0, 0)?;
    let total_offset = |utc: NaiveDateTime| {
        let off = tz.offset_from_utc_datetime(&utc);
        (off.base_utc_offset() + off.dst_offset()).num_seconds() as i32
    };
    let mut prev = total_offset(day);
    while day < end {
        let next = day + Duration::days(1);
        let next_off = total_offset(next);
        if next_off != prev {
            let (mut lo, mut hi) = (day, next);
            while (hi - lo).num_seconds() > 1 {
                let mid = lo + Duration::seconds((hi - lo).num_seconds() / 2);
                if total_offset(mid) == prev {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let off = tz.offset_from_utc_datetime(&hi);
            out.push((
                hi,
                prev,
                next_off,
                off.dst_offset().num_seconds() != 0,
                off.abbreviation().unwrap_or_default().to_string(),
            ));
            prev = next_off;
        }
        day = next;
    }
    Some(out)
}

/// Zones whose daylight-saving flags differ between tzdata's main form
/// (chrono-tz) and its rearguard form (jiff-tzdb): a negative save.
const REARGUARD_FLAGS: &[&str] = &[
    "Europe/Dublin",
    "Eire",
    "Africa/Casablanca",
    "Africa/El_Aaiun",
    "Africa/Windhoek",
];

fn numeric(abbreviation: &str) -> bool {
    let digits = abbreviation.strip_prefix(['+', '-']).unwrap_or("");
    (digits.len() == 2 || digits.len() == 4) && digits.bytes().all(|b| b.is_ascii_digit())
}

#[test]
fn the_vtimezone_probe_sees_the_same_changes_1990_to_2045() {
    let mut instants = Report::default();
    let mut labels = Report::default();
    for zone in Zone::all() {
        let tz = ctz(zone);
        for year in 1990..=2045 {
            let start = utc(year, 1, 1);
            let end = utc(year + 1, 1, 1);
            if may_differ_within(zone.name(), start, end + Duration::seconds(1)) {
                continue;
            }
            let probe = year_transitions(&tz, year).expect("a year chrono builds");
            let ours: Vec<_> = zone
                .offset_changes(start)
                .take_while(|c| c.at <= end)
                .collect();
            let ours_short: Vec<_> = ours
                .iter()
                .map(|c| {
                    (
                        c.at.naive_utc(),
                        c.from.local_minus_utc(),
                        c.to.local_minus_utc(),
                    )
                })
                .collect();
            let probe_short: Vec<_> = probe.iter().map(|t| (t.0, t.1, t.2)).collect();
            if ours_short != probe_short {
                instants.note(zone, format!("{year}: {ours_short:?} vs {probe_short:?}"));
                continue;
            }
            for (change, (at, _, _, dst, name)) in ours.iter().zip(&probe) {
                if change.dst != *dst && !REARGUARD_FLAGS.contains(&zone.name()) {
                    labels.note(zone, format!("dst at {at}: {} vs {dst}", change.dst));
                }
                if change.abbreviation != *name
                    && !(name.is_empty() && numeric(&change.abbreviation))
                {
                    labels.note(
                        zone,
                        format!("name at {at}: {} vs {name:?}", change.abbreviation),
                    );
                }
            }
        }
    }
    instants.assert_empty("probe instants");
    labels.assert_empty("probe labels");
}

/// An entry that no longer differs is stale: the window says where, so
/// the first difference must lie inside it.
#[test]
fn every_named_change_still_differs() {
    let mut stale = Vec::new();
    for (names, _, _, why) in CHANGED_2025B_2026F {
        for name in *names {
            let zone = Zone::named(name).unwrap_or_else(|| panic!("{name} is bundled"));
            let tz = ctz(zone);
            let (from, until) = changed(name).expect("listed");
            let until = until.min(utc(2100, 1, 1));
            let differs_at =
                |at: DateTime<Utc>| zone.offset_at(at).local_minus_utc() != total(&tz, at);
            let mut at = from;
            let mut differs = false;
            while at < until && !differs {
                differs = differs_at(at);
                at += Duration::minutes(30);
            }
            if !differs {
                stale.push(format!("{name} ({from}, {why})"));
            }
        }
    }
    assert!(stale.is_empty(), "named but equal: {stale:?}");
}
