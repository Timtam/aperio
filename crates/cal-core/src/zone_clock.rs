//! Which tzdata zone runs a given clock.
//!
//! A provider can describe a zone by its rules instead of naming it: Exchange
//! keeps a `Customized Time Zone` with its periods and transitions (decision
//! 232), an iCalendar file a `VTIMEZONE` under a name only it knows. A series
//! on such a zone has to repeat on that clock, and the expander only knows
//! tzdata's zones. So the clock is matched to the zone that reads the same
//! offset at every moment of a window, rather than repeated in UTC (an hour off
//! after every clock change) or matched by a guess at the name.
//!
//! ## The comparison
//!
//! Two clocks agree when they read the same UTC offset at the start of every
//! UTC day of the window, and at every whole hour of a day at whose start and
//! end either of them reads different offsets. A difference that starts and
//! ends between two whole hours goes unseen, and so does a clock that changes
//! and changes back within one day. The generator of the Exchange zone table
//! compares a zone with its Windows id's default zone by the same rule
//! (`cargo xtask windows-zones`, decision 22a).
//!
//! ## The window
//!
//! Five years ([`CLOCK_WINDOW_MONTHS`]) from the later of two days: the UTC day
//! of the series' first start, and the first day the table's guard compared
//! ([`crate::windows_zone::CLOCKS_COMPARED_FROM`], the pinned CLDR release's
//! publication). The clock is compared where Aperio reads the series now and
//! ahead, not over years long past, in which a zone may have run another
//! clock: Cairo was on +02:00 all year from 2014 to 2023, so a series begun in
//! 2017 on a fixed +02:00 definition would otherwise read as Cairo and ring an
//! hour early every summer since. The window depends on the build and the
//! series alone, never on today's date, so a reading does not change as the
//! years pass.
//!
//! ## The zone chosen
//!
//! Several zones often run one clock: Berlin, Paris, Budapest and Warsaw all
//! do. The first that agrees, in this order (Toni, 2026-11-09):
//!
//! 1. UTC, when the clock reads +00:00 throughout: no zone at all
//!    ([`ClockMatch::Utc`]).
//! 2. The device's own zone, when it runs that clock: a reader in Berlin sees
//!    Berlin, one in Paris sees Paris, and both see the same times.
//! 3. The default zone of each Windows id, in the table's order, so the zone
//!    also writes back to Exchange under an id of its own.
//! 4. Every listed zone ([`crate::listed_zones`]), in its order.
//!
//! A clock no zone runs is [`ClockMatch::NoZone`]: the caller says so, and the
//! series repeats as it did before.

use chrono::{DateTime, Duration, Months, NaiveDate, NaiveTime, Offset, TimeZone, Utc};

/// How long a window of comparison runs: five years, as the zone table's clock
/// guard looks ahead.
pub const CLOCK_WINDOW_MONTHS: u32 = 60;

/// A clock: the UTC offset in seconds (local minus UTC) it reads at an instant,
/// `None` where it cannot say.
pub trait Clock {
    fn offset_at(&self, at: DateTime<Utc>) -> Option<i32>;
}

impl<F: Fn(DateTime<Utc>) -> Option<i32>> Clock for F {
    fn offset_at(&self, at: DateTime<Utc>) -> Option<i32> {
        self(at)
    }
}

/// A tzdata zone as a [`Clock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TzClock(pub chrono_tz::Tz);

impl Clock for TzClock {
    fn offset_at(&self, at: DateTime<Utc>) -> Option<i32> {
        Some(
            self.0
                .offset_from_utc_datetime(&at.naive_utc())
                .fix()
                .local_minus_utc(),
        )
    }
}

/// Where two clocks first read different offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDifference {
    pub at: DateTime<Utc>,
    pub a: Option<i32>,
    pub b: Option<i32>,
}

/// The first instant in `[from, until)` at which `a` and `b` read different
/// offsets, by the rule in the module's documentation. A clock that cannot say
/// differs from one that can, and agrees with another that cannot.
pub fn first_clock_difference(
    a: &impl Clock,
    b: &impl Clock,
    (from, until): (DateTime<Utc>, DateTime<Utc>),
) -> Option<ClockDifference> {
    let differ = |at: DateTime<Utc>| {
        let (x, y) = (a.offset_at(at), b.offset_at(at));
        (x != y).then_some(ClockDifference { at, a: x, b: y })
    };
    let mut day = from;
    while day < until {
        let next = (day + Duration::days(1)).min(until);
        if let Some(difference) = differ(day) {
            return Some(difference);
        }
        if a.offset_at(next) != a.offset_at(day) || b.offset_at(next) != b.offset_at(day) {
            let mut hour = day + Duration::hours(1);
            while hour < next {
                if let Some(difference) = differ(hour) {
                    return Some(difference);
                }
                hour += Duration::hours(1);
            }
        }
        day = next;
    }
    None
}

/// The window a series' clock is compared over, by the module's documentation:
/// [`CLOCK_WINDOW_MONTHS`] from the later of `first_start`'s UTC day and the
/// first day the table's guard compared.
pub fn clock_window(first_start: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let midnight = |day: NaiveDate| day.and_time(NaiveTime::MIN).and_utc();
    let (year, month, day) = crate::windows_zone::CLOCKS_COMPARED_FROM;
    let guard = NaiveDate::from_ymd_opt(year, month, day).map(midnight);
    let from = midnight(first_start.date_naive()).max(guard.unwrap_or(first_start));
    let until = from
        .checked_add_months(Months::new(CLOCK_WINDOW_MONTHS))
        .unwrap_or(from);
    (from, until)
}

/// Which zone runs a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockMatch {
    /// It reads +00:00 throughout: no zone, the series repeats in UTC.
    Utc,
    /// This zone, in tzdata's canonical spelling.
    Zone(&'static str),
    /// No zone runs it.
    NoZone,
}

/// The zone that runs `clock` through the [`clock_window`] of a series whose
/// first start is `first_start`, by the order in the module's documentation.
/// `device` is the device's own zone, in any spelling tzdata knows; `None`, or
/// a name it does not know, skips that step.
pub fn zone_with_clock(
    clock: &impl Clock,
    first_start: DateTime<Utc>,
    device: Option<&str>,
) -> ClockMatch {
    let window = clock_window(first_start);
    let utc = |_: DateTime<Utc>| -> Option<i32> { Some(0) };
    if first_clock_difference(clock, &utc, window).is_none() {
        return ClockMatch::Utc;
    }
    let runs = |zone: &str| {
        zone.parse::<chrono_tz::Tz>()
            .is_ok_and(|tz| first_clock_difference(clock, &TzClock(tz), window).is_none())
    };
    candidates(device)
        .find(|zone| runs(zone))
        .map_or(ClockMatch::NoZone, ClockMatch::Zone)
}

/// The zones tried, in order, each once; never a UTC name.
fn candidates(device: Option<&str>) -> impl Iterator<Item = &'static str> {
    let device = device
        .and_then(|name| crate::canonical_zone(name.trim()))
        .into_iter();
    let windows = crate::windows_zone::WINDOWS_ZONES
        .iter()
        .map(|(_, zone)| *zone);
    let listed = crate::listed_zones().iter().copied();
    let mut seen = std::collections::BTreeSet::new();
    device
        .chain(windows)
        .chain(listed)
        .filter(|zone| crate::series_clock_zone(Some(zone)).is_some())
        .filter(move |zone| seen.insert(*zone))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn tz(name: &str) -> TzClock {
        TzClock(name.parse().unwrap())
    }

    /// A clock with a fixed offset.
    fn fixed(seconds: i32) -> impl Fn(DateTime<Utc>) -> Option<i32> {
        move |_| Some(seconds)
    }

    /// Berlin's clock as tzdata runs it, but still on winter time for the
    /// week from 29 March 01:00 UTC in one year, after Berlin has changed: a
    /// definition that is nearly, not exactly, Berlin.
    fn berlin_late_in(year: i32) -> impl Fn(DateTime<Utc>) -> Option<i32> {
        let berlin = tz("Europe/Berlin");
        move |at: DateTime<Utc>| {
            let late_from = Utc.with_ymd_and_hms(year, 3, 29, 1, 0, 0).unwrap();
            let late = at >= late_from && at < late_from + Duration::days(7);
            if late && berlin.offset_at(at) == Some(7200) {
                Some(3600)
            } else {
                berlin.offset_at(at)
            }
        }
    }

    #[test]
    fn the_same_zone_has_no_difference_and_another_names_its_first() {
        let window = (utc("2026-01-01T00:00:00Z"), utc("2031-01-01T00:00:00Z"));
        assert_eq!(
            first_clock_difference(&tz("Europe/Berlin"), &tz("Europe/Paris"), window),
            None
        );
        // London changes on the same instants but an hour behind.
        assert_eq!(
            first_clock_difference(&tz("Europe/Berlin"), &tz("Europe/London"), window),
            Some(ClockDifference {
                at: utc("2026-01-01T00:00:00Z"),
                a: Some(3600),
                b: Some(0),
            })
        );
        // Lagos is Berlin's winter all year: the first difference is the
        // first hour of summer time.
        assert_eq!(
            first_clock_difference(&tz("Europe/Berlin"), &tz("Africa/Lagos"), window),
            Some(ClockDifference {
                at: utc("2026-03-29T01:00:00Z"),
                a: Some(7200),
                b: Some(3600),
            })
        );
    }

    #[test]
    fn a_clock_that_cannot_say_differs_from_one_that_can() {
        let window = (utc("2026-01-01T00:00:00Z"), utc("2031-01-01T00:00:00Z"));
        let silent = |_: DateTime<Utc>| -> Option<i32> { None };
        assert_eq!(
            first_clock_difference(&silent, &fixed(0), window).map(|d| (d.a, d.b)),
            Some((None, Some(0)))
        );
        assert_eq!(first_clock_difference(&silent, &silent, window), None);
    }

    /// Five years from the guard's first day, or from a later series' own UTC
    /// day. A clock change a week off in the fifth year is still seen; one
    /// after the window is not.
    #[test]
    fn the_window_is_five_years_from_the_guard_or_a_later_start() {
        let from = utc("2026-01-01T00:00:00Z");
        assert_eq!(crate::windows_zone::CLOCKS_COMPARED_FROM, (2026, 3, 17));
        assert_eq!(
            clock_window(from),
            (utc("2026-03-17T00:00:00Z"), utc("2031-03-17T00:00:00Z"))
        );
        assert_eq!(
            clock_window(utc("2017-03-06T08:00:00Z")),
            clock_window(from)
        );
        assert_eq!(
            clock_window(utc("2027-05-20T08:15:00Z")),
            (utc("2027-05-20T00:00:00Z"), utc("2032-05-20T00:00:00Z"))
        );
        assert_eq!(
            first_clock_difference(
                &berlin_late_in(2030),
                &tz("Europe/Berlin"),
                clock_window(from)
            )
            .map(|d| d.at),
            Some(utc("2030-03-31T01:00:00Z"))
        );
        assert_eq!(
            first_clock_difference(
                &berlin_late_in(2031),
                &tz("Europe/Berlin"),
                clock_window(from)
            ),
            None
        );
    }

    #[test]
    fn the_device_zone_comes_first_when_it_runs_the_clock() {
        let from = utc("2026-01-01T00:00:00Z");
        let berlin = tz("Europe/Berlin");
        assert_eq!(
            zone_with_clock(&berlin, from, Some("Europe/Berlin")),
            ClockMatch::Zone("Europe/Berlin")
        );
        assert_eq!(
            zone_with_clock(&berlin, from, Some("Europe/Paris")),
            ClockMatch::Zone("Europe/Paris")
        );
        // Any spelling tzdata knows, in its canonical form.
        assert_eq!(
            zone_with_clock(&tz("Asia/Kolkata"), from, Some("Asia/Calcutta")),
            ClockMatch::Zone("Asia/Kolkata")
        );
        // A device on another clock, or one that names no zone, does not
        // count: the Windows table's first zone on that clock does.
        let first_in_the_table = zone_with_clock(&berlin, from, None);
        assert_eq!(
            zone_with_clock(&berlin, from, Some("America/New_York")),
            first_in_the_table
        );
        assert_eq!(
            zone_with_clock(&berlin, from, Some("Not/AZone")),
            first_in_the_table
        );
    }

    /// Without the device, the first Windows id's default zone on that clock,
    /// in the table's order: the same answer on every device.
    #[test]
    fn without_the_device_the_windows_table_decides() {
        let from = utc("2026-01-01T00:00:00Z");
        let expected = crate::windows_zone::WINDOWS_ZONES
            .iter()
            .map(|(_, zone)| *zone)
            .find(|zone| {
                first_clock_difference(&tz("Europe/Berlin"), &tz(zone), clock_window(from))
                    .is_none()
            })
            .unwrap();
        assert_eq!(
            zone_with_clock(&tz("Europe/Berlin"), from, None),
            ClockMatch::Zone(expected)
        );
        assert_eq!(expected, "Europe/Budapest", "Central Europe Standard Time");
        assert_eq!(
            zone_with_clock(&tz("America/New_York"), from, None),
            ClockMatch::Zone("America/New_York")
        );
    }

    #[test]
    fn a_clock_on_utc_throughout_is_no_zone() {
        let from = utc("2026-01-01T00:00:00Z");
        assert_eq!(zone_with_clock(&fixed(0), from, None), ClockMatch::Utc);
        // Even where the device's zone runs it too: Abidjan is +00:00 all
        // year.
        assert_eq!(
            zone_with_clock(&fixed(0), from, Some("Africa/Abidjan")),
            ClockMatch::Utc
        );
    }

    #[test]
    fn a_fixed_offset_finds_a_zone_that_never_changes() {
        let from = utc("2026-01-01T00:00:00Z");
        let found = zone_with_clock(&fixed(3 * 3600), from, None);
        let ClockMatch::Zone(zone) = found else {
            panic!("+03:00 is a clock several zones run: {found:?}");
        };
        assert_eq!(
            first_clock_difference(&fixed(3 * 3600), &tz(zone), clock_window(from)),
            None
        );
    }

    /// A series begun in 2017 on a fixed +02:00 definition is compared where it
    /// runs now: Cairo, fixed +02:00 from 2014 to 2023 and on summer time
    /// again since, does not run it, not even on a device in Cairo.
    #[test]
    fn an_old_series_is_compared_where_it_runs_now() {
        let first_start = utc("2017-03-06T08:00:00Z");
        let two = fixed(2 * 3600);
        for device in [None, Some("Africa/Cairo")] {
            let found = zone_with_clock(&two, first_start, device);
            let ClockMatch::Zone(zone) = found else {
                panic!("+02:00 is a clock several zones run: {found:?}");
            };
            assert_ne!(zone, "Africa/Cairo", "{device:?}");
            assert_eq!(
                first_clock_difference(&two, &tz(zone), clock_window(first_start)),
                None,
                "{zone}"
            );
        }
        // Cairo did run it in the series' own first five years.
        let then = (first_start, utc("2022-03-06T00:00:00Z"));
        assert_eq!(
            first_clock_difference(&two, &tz("Africa/Cairo"), then),
            None
        );
    }

    #[test]
    fn a_clock_no_zone_runs_is_said() {
        let from = utc("2026-01-01T00:00:00Z");
        // +03:17 is no zone's offset.
        assert_eq!(
            zone_with_clock(&fixed(3 * 3600 + 17 * 60), from, None),
            ClockMatch::NoZone
        );
        // Nearly Berlin: a week late into summer time in 2028.
        assert_eq!(
            zone_with_clock(&berlin_late_in(2028), from, Some("Europe/Berlin")),
            ClockMatch::NoZone
        );
    }
}
