//! Changing the clock a series repeats on (DESIGN-series-time-zone.md, stage 9;
//! decisions 15a, 20a, 24a).
//!
//! A series repeats at one wall time on its clock: its zone, or UTC. Choosing
//! another zone keeps the start's instant, so the start can fall on another
//! day on the new clock. Where the rule names days (weekdays, a day of the
//! month, a month), they can then name a different day than the one the
//! series stood on, and the editors ask which days it should keep (15a):
//!
//! - **as they fall now:** the start stays; the rule names the days the
//!   occurrences fall on on the new clock, moved as a drag moves them
//!   (`series_shift`), in one step from the old clock to the new;
//! - **as the rule says:** the rule's text stays, and the start moves back
//!   onto its old day where the rule's days fit only that one.
//!
//! In both, every exception that cancels an occurrence moves with it, and one
//! that cancels nothing on the old clock stays as it is: a deletion from
//! before a zone was lost then deletes again once the zone is back. An answer
//! that would put the start or an exception into a time the new clock skips
//! is refused with that reason; so is one `series_shift` refuses (20a).
//!
//! A series whose days follow its start (24a) is not asked, unless it cannot
//! keep them on the new clock (monthly on the 31st): its answer is the first.
//! A rule that names only the day its start falls on anyway counts as one
//! whose days follow its start; the repeat field writes the day of the month
//! that way.
//!
//! Either answer moves every occurrence by one wall shift (stage 13): as they
//! fall now by the start's own move, as the rule says by its change in time
//! of day alone. A written answer says how far (`moved_minutes`), and
//! [`moved_slots`] moves the slots of a series' changed occurrences (their
//! `RECURRENCE-ID`s) by it, matched as an exception is (94a, 95a).
//!
//! Pinned by `tests/fixtures/seriesClockChange.json` and
//! `tests/fixtures/movedSlots.json`; the surfaces replay them.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc};
use serde::{Deserialize, Serialize};

use cal_zone::{Reading, Zone};

use crate::rrule_parts::{
    parse_freq, parse_parts, part, rule_until_text, with_until, Freq, Part, WEEKDAY_TOKENS,
};
use crate::wall_clock::clock_of;
use crate::{SeriesShift, ShiftRefusal};

/// A question: a series as it is stored, and the zone it is to repeat on.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ClockChangeQuestion {
    /// The series' rule, with or without `RRULE:`.
    pub rrule: String,
    /// The series' start, an RFC 3339 instant.
    pub start: String,
    /// The instants its exceptions cancel.
    #[serde(default)]
    pub exceptions: Vec<String>,
    /// The zone name the series stores now; `None` for none (UTC).
    pub from_tzid: Option<String>,
    /// The zone it is to repeat on; `None` for UTC.
    pub to_tzid: Option<String>,
    /// The series' own calendar stores its end as a date (manifest
    /// `recurrence.ends_by_date`). An unflagged `…T235959Z` that the 105a
    /// guard reads as a cut ([`crate::SeriesEnd::guarded`]) then counts
    /// through its own date (DESIGN-series-time-zone.md, 140a, D1). JSON
    /// without the field reads `false`.
    #[serde(default)]
    pub ends_by_date: bool,
}

/// Which clock's start the rule's named days fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum RuleFit {
    /// Only the start's day on the old clock.
    Before,
    /// Only the start's day on the new clock.
    After,
    /// Both days.
    Both,
    /// Neither day.
    Neither,
    /// The rule cannot say (a set position, a week number, a day of the year).
    Unknown,
}

/// Why an answer cannot be carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ClockChangeRefusal {
    /// The rule, a value in it, the start or an exception cannot be read.
    Unreadable,
    /// `BYDAY` with an ordinal, such as `2SU`.
    OrdinalWeekday,
    /// `BYSETPOS`.
    SetPosition,
    /// `BYYEARDAY`.
    YearDay,
    /// `BYWEEKNO`.
    WeekNumber,
    /// `BYHOUR`, `BYMINUTE` or `BYSECOND`, while the time of day changes.
    TimeOfDay,
    /// A negative `BYMONTHDAY`.
    NegativeMonthDay,
    /// A day of the month past the 28th, or a move out of the month.
    MonthEnd,
    /// The rule recurs only in some months or years.
    LimitedMonths,
    /// A yearly rule whose move touches the end of February.
    LeapDay,
    /// A part the rule reader does not know.
    UnknownPart,
    /// The start or an exception would fall on a time the new clock skips.
    SkippedTime,
}

impl From<ShiftRefusal> for ClockChangeRefusal {
    fn from(reason: ShiftRefusal) -> Self {
        match reason {
            ShiftRefusal::Unreadable => Self::Unreadable,
            ShiftRefusal::OrdinalWeekday => Self::OrdinalWeekday,
            ShiftRefusal::SetPosition => Self::SetPosition,
            ShiftRefusal::YearDay => Self::YearDay,
            ShiftRefusal::WeekNumber => Self::WeekNumber,
            ShiftRefusal::TimeOfDay => Self::TimeOfDay,
            ShiftRefusal::NegativeMonthDay => Self::NegativeMonthDay,
            ShiftRefusal::MonthEnd => Self::MonthEnd,
            ShiftRefusal::LimitedMonths => Self::LimitedMonths,
            ShiftRefusal::LeapDay => Self::LeapDay,
            ShiftRefusal::UnknownPart => Self::UnknownPart,
        }
    }
}

/// One answer to the question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum ClockChangeAnswer {
    /// The series as it is written on the new clock.
    Changed {
        rrule: String,
        /// RFC 3339.
        start: String,
        /// RFC 3339, in the order they came.
        exceptions: Vec<String>,
        /// How far, in wall minutes, every occurrence moves from the old
        /// clock to the new one: the start's own move as they fall now, its
        /// change in time of day alone as the rule says. A changed
        /// occurrence's slot moves by it too ([`moved_slots`]). Whole
        /// minutes: a zone whose offset had seconds (before 1972, such as
        /// Monrovia's −00:44:30) loses them here, and its slots move by that
        /// many seconds less than its occurrences.
        #[serde(rename = "movedMinutes")]
        #[cfg_attr(feature = "ts-export", ts(type = "number"))]
        moved_minutes: i64,
    },
    /// This answer cannot be carried out.
    Refused { reason: ClockChangeRefusal },
}

/// What changing a series' clock writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SeriesClockChange {
    /// Whether the editors ask which days to keep (15a, 24a).
    pub asks: bool,
    /// The start's day on the old clock, `YYYY-MM-DD`.
    pub start_day_before: String,
    /// The start's day on the new clock.
    pub start_day_after: String,
    /// Which of the two days the rule's named days fit.
    pub fit: RuleFit,
    /// "As they fall now": the start stays, the rule names the days the
    /// occurrences fall on on the new clock. The answer a series that is not
    /// asked gets.
    pub as_now: ClockChangeAnswer,
    /// "As the rule says": the rule's text stays, the start moves back onto
    /// its old day where the rule's days fit only that one.
    pub as_rule: ClockChangeAnswer,
}

/// The parts that name the days a rule repeats on.
const DAY_PARTS: [&str; 6] = [
    "BYDAY",
    "BYMONTHDAY",
    "BYMONTH",
    "BYYEARDAY",
    "BYWEEKNO",
    "BYSETPOS",
];

fn days_in_month(day: NaiveDate) -> i64 {
    let next = if day.month() == 12 {
        NaiveDate::from_ymd_opt(day.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(day.year(), day.month() + 1, 1)
    };
    next.and_then(|n| n.pred_opt())
        .map_or(31, |l| i64::from(l.day()))
}

fn days_in_year(day: NaiveDate) -> i64 {
    if NaiveDate::from_ymd_opt(day.year(), 2, 29).is_some() {
        366
    } else {
        365
    }
}

fn weekday_token(day: NaiveDate) -> &'static str {
    WEEKDAY_TOKENS[day.weekday().num_days_from_monday() as usize]
}

/// Whether the rule's named days include `day`: `None` where the rule names
/// them in a way this cannot follow (a set position, a week number, a day of
/// the year) or cannot be read, `Some(true)` for a rule that names none.
///
/// Counted weekdays follow RFC 5545: in a monthly rule, or a yearly one with
/// `BYMONTH`, `2SU` is the month's second Sunday and `-1FR` its last Friday;
/// in a yearly rule without `BYMONTH`, they count the weeks of the year. A
/// negative day of the month counts from its end.
pub fn rule_names_day(rrule: &str, day: NaiveDate) -> Option<bool> {
    let (_, parts) = parse_parts(rrule).ok()?;
    let freq = parse_freq(part(&parts, "FREQ")?)?;
    for key in ["BYYEARDAY", "BYWEEKNO", "BYSETPOS"] {
        if part(&parts, key).is_some() {
            return None;
        }
    }
    let numbers = |key: &str| -> Option<Option<Vec<i64>>> {
        match part(&parts, key) {
            None => Some(None),
            Some(value) => value
                .split(',')
                .map(|n| n.trim().parse::<i64>().ok())
                .collect::<Option<Vec<_>>>()
                .map(Some),
        }
    };
    let months = numbers("BYMONTH")?;
    if let Some(months) = &months {
        if !months.contains(&i64::from(day.month())) {
            return Some(false);
        }
    }
    if let Some(month_days) = numbers("BYMONTHDAY")? {
        let dom = i64::from(day.day());
        let last = days_in_month(day);
        let hit = month_days.iter().any(|d| {
            if *d > 0 {
                *d == dom
            } else {
                last + d + 1 == dom
            }
        });
        if !hit {
            return Some(false);
        }
    }
    if let Some(value) = part(&parts, "BYDAY") {
        let in_months =
            matches!(freq, Freq::Monthly) || (matches!(freq, Freq::Yearly) && months.is_some());
        let in_year = matches!(freq, Freq::Yearly) && months.is_none();
        let mut hit = false;
        for token in value.split(',') {
            let token = token.trim().to_ascii_uppercase();
            if token.len() < 2 || !token.is_ascii() {
                return None;
            }
            let (ordinal, code) = token.split_at(token.len() - 2);
            if !WEEKDAY_TOKENS.contains(&code) {
                return None;
            }
            if code != weekday_token(day) {
                continue;
            }
            if ordinal.is_empty() {
                hit = true;
                continue;
            }
            let n: i64 = ordinal.parse().ok()?;
            let (index, from_end) = if in_months {
                let dom = i64::from(day.day());
                ((dom - 1) / 7 + 1, (days_in_month(day) - dom) / 7 + 1)
            } else if in_year {
                let yday = i64::from(day.ordinal());
                ((yday - 1) / 7 + 1, (days_in_year(day) - yday) / 7 + 1)
            } else {
                // An ordinal in a daily or weekly rule means nothing to RFC 5545.
                return None;
            };
            if (n > 0 && n == index) || (n < 0 && -n == from_end) {
                hit = true;
            }
        }
        if !hit {
            return Some(false);
        }
    }
    Some(true)
}

/// Whether a rule takes its days from its start: it names none, or names only
/// the day the start falls on anyway — `BYDAY=MO` from a Monday in a weekly
/// rule, `BYMONTHDAY=15` from the 15th in a monthly one (or a yearly one with
/// a month), `BYMONTH=3` from March in any rule, where it only limits the
/// months. In a monthly or yearly rule `BYDAY=MO` names every Monday, and a
/// yearly `BYMONTHDAY=15` without a month the 15th of every month: those name
/// days of their own.
fn days_follow_start(parts: &[Part], freq: Freq, start: NaiveDate) -> bool {
    let number = |key: &str| part(parts, key).and_then(|v| v.trim().parse::<u32>().ok());
    let has_month = part(parts, "BYMONTH").is_some();
    parts
        .iter()
        .filter(|p| DAY_PARTS.contains(&p.key.as_str()))
        .all(|p| match p.key.as_str() {
            "BYDAY" => {
                matches!(freq, Freq::Weekly)
                    && p.value.trim().eq_ignore_ascii_case(weekday_token(start))
            }
            "BYMONTHDAY" => {
                number("BYMONTHDAY") == Some(start.day())
                    && (matches!(freq, Freq::Monthly)
                        || (matches!(freq, Freq::Yearly) && has_month))
            }
            "BYMONTH" => number("BYMONTH") == Some(start.month()),
            _ => false,
        })
}

/// The instant `wall` names on `zone`: a time it skips refuses, a time it
/// shows twice takes the earlier reading, as a typed time does.
fn instant_on(zone: Zone, wall: NaiveDateTime) -> Result<DateTime<Utc>, ClockChangeRefusal> {
    let on = |offset: chrono::FixedOffset| {
        DateTime::<Utc>::from_naive_utc_and_offset(
            wall - Duration::seconds(i64::from(offset.local_minus_utc())),
            Utc,
        )
    };
    match zone.reading(wall) {
        Reading::Single(offset) => Ok(on(offset)),
        Reading::Fold { earlier, .. } => Ok(on(earlier)),
        Reading::Gap { .. } => Err(ClockChangeRefusal::SkippedTime),
    }
}

fn instant_text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Whether every part of a rule can be read: a `FREQ` this reader knows, a
/// whole `INTERVAL` of at least 1, weekday tokens (with or without an
/// ordinal) and numbers where numbers belong.
fn rule_is_readable(parts: &[Part]) -> bool {
    let Some(freq) = part(parts, "FREQ") else {
        return false;
    };
    if parse_freq(freq).is_none()
        || crate::rrule_parts::parse_interval(part(parts, "INTERVAL")).is_none()
    {
        return false;
    }
    let numbers = |key: &str| {
        part(parts, key)
            .is_none_or(|value| value.split(',').all(|n| n.trim().parse::<i64>().is_ok()))
    };
    let weekdays = part(parts, "BYDAY").is_none_or(|value| {
        value.split(',').all(|token| {
            let token = token.trim().to_ascii_uppercase();
            token.len() >= 2
                && token.is_ascii()
                && WEEKDAY_TOKENS.contains(&&token[token.len() - 2..])
                && (token.len() == 2 || token[..token.len() - 2].parse::<i64>().is_ok())
        })
    });
    weekdays
        && [
            "BYMONTHDAY",
            "BYMONTH",
            "BYHOUR",
            "BYMINUTE",
            "BYSECOND",
            "BYSETPOS",
            "BYYEARDAY",
            "BYWEEKNO",
            "COUNT",
        ]
        .iter()
        .all(|key| numbers(key))
}

/// Whether `day` is one of the days a rule that takes its days from its start
/// (`start`) repeats on, by its frequency alone.
fn on_the_start_pattern(freq: Freq, day: NaiveDate, start: NaiveDate) -> bool {
    match freq {
        Freq::Weekly => day.weekday() == start.weekday(),
        Freq::Monthly => day.day() == start.day(),
        Freq::Yearly => day.day() == start.day() && day.month() == start.month(),
        _ => true,
    }
}

/// Whether `day` lies in a period the rule's `INTERVAL` repeats in, counted
/// from the start's: every other week from the start's week, every third
/// month, and so on. Weeks begin on the rule's `WKST` (`week_start`, days
/// from Monday), as RFC 5545 counts them; months and years by the calendar.
fn in_an_interval(
    freq: Freq,
    interval: u32,
    week_start: u32,
    day: NaiveDate,
    start: NaiveDate,
) -> bool {
    if interval <= 1 {
        return true;
    }
    let n = i64::from(interval);
    let periods = match freq {
        Freq::Daily => (day - start).num_days(),
        Freq::Weekly => {
            let first = |d: NaiveDate| {
                let back = (i64::from(d.weekday().num_days_from_monday()) - i64::from(week_start))
                    .rem_euclid(7);
                d - Duration::days(back)
            };
            (first(day) - first(start)).num_days() / 7
        }
        Freq::Monthly => {
            i64::from(day.year() - start.year()) * 12 + i64::from(day.month())
                - i64::from(start.month())
        }
        Freq::Yearly => i64::from(day.year() - start.year()),
        _ => return true,
    };
    periods.rem_euclid(n) == 0
}

/// Where the start goes when a series moves onto the new clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartMove {
    /// The start keeps its instant: the question's own text is written.
    Keep,
    /// The start moves in wall time with every occurrence; a time the new
    /// clock skips refuses.
    Shift,
}

/// A series read for a move from its old clock onto a new one: everything
/// the answers share. [`ClockMove::moved_by`] moves it by any wall shift, so
/// a series and each copy of it can move by the shift that is theirs
/// ([`crate::group_zone_carry`]).
pub(crate) struct ClockMove<'a> {
    question: &'a ClockChangeQuestion,
    rrule: &'a str,
    start: DateTime<Utc>,
    before_zone: Zone,
    after_zone: Zone,
    wall_before: NaiveDateTime,
    wall_after: NaiveDateTime,
    day_before: NaiveDate,
    parts: Vec<Part>,
    freq: Freq,
    interval: u32,
    /// The rule's `WKST`, in days from Monday.
    week_start: u32,
    /// Whether the rule takes its days from its start (24a).
    follows: bool,
    /// The times of day the rule names on the old clock.
    times: Vec<NaiveTime>,
    /// More than one occurrence a day (hourly, or several times): its
    /// occurrences keep their distances from the start, so its end moves
    /// with the start's instant rather than as the end of a day.
    several_a_day: bool,
    /// A rule by the hour or faster: the unit it steps in, in seconds.
    sub_daily_unit: Option<i64>,
    by_hour: Option<Vec<u32>>,
    by_minute: Option<Vec<u32>>,
    by_second: Option<Vec<u32>>,
    /// The end's instant, read on the old clock (101b, 115a).
    end_at: Option<DateTime<Utc>>,
    /// The end's last day on the old clock.
    end_day: Option<NaiveDate>,
    /// The instant an exception counts within the end by, on the old clock.
    cancel_bound: Option<DateTime<Utc>>,
}

impl<'a> ClockMove<'a> {
    /// The series a question names; `None` for a start that names no
    /// instant and for a rule that cannot be read.
    pub(crate) fn read(question: &'a ClockChangeQuestion) -> Option<ClockMove<'a>> {
        use crate::rrule_parts::parse_interval;
        use cal_zone::WallPolicy;

        let start = DateTime::parse_from_rfc3339(&question.start)
            .ok()?
            .with_timezone(&Utc);
        let (before_zone, _) = clock_of(question.from_tzid.as_deref());
        let (after_zone, _) = clock_of(question.to_tzid.as_deref());
        let wall_before = before_zone.wall(start);
        let wall_after = after_zone.wall(start);
        let day_before = wall_before.date();
        let rrule = question.rrule.trim();
        let parts = parse_parts(rrule)
            .ok()
            .map(|(_, parts)| parts)
            .filter(|parts| rule_is_readable(parts))?;
        let freq = part(&parts, "FREQ").and_then(parse_freq)?;
        let interval = parse_interval(part(&parts, "INTERVAL"))?;
        let follows = days_follow_start(&parts, freq, day_before);
        let week_start = part(&parts, "WKST")
            .and_then(|w| {
                WEEKDAY_TOKENS
                    .iter()
                    .position(|t| t.eq_ignore_ascii_case(w.trim()))
            })
            .map_or(0, |i| i as u32);
        let times = crate::series_end::rule_times(&parts, wall_before.time())?;
        let several_a_day =
            matches!(freq, Freq::Hourly | Freq::Minutely | Freq::Secondly) || times.len() > 1;

        // The end, read on the old clock (101b, 115a).
        let end = crate::read_series_end(
            rrule,
            wall_before.time(),
            crate::expansion_clock(false, question.from_tzid.as_deref()),
            |at| before_zone.wall(at),
            |wall| crate::wall_clock::last_at_or_before(before_zone, wall),
            |wall| before_zone.to_utc(wall, WallPolicy::Rfc5545),
        );
        let end_at = end
            .as_ref()
            .and_then(|end| end.at.as_deref())
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
            .map(|at| at.with_timezone(&Utc));
        // A guarded end on a calendar that stores its end as a date was typed
        // for its own date there (D1): it ends through the UTC digits' date.
        let end_day = end.as_ref().and_then(|end| {
            if question.ends_by_date && end.guarded {
                crate::utc_until(rrule).map(|at| at.date_naive())
            } else {
                NaiveDate::parse_from_str(&end.end_day, "%Y-%m-%d").ok()
            }
        });
        // The instant an exception counts within the end by, on the old
        // clock: the end the answer writes. A guarded end counted through its
        // own date bounds them by that date's last instant there, so a
        // deletion on that day moves with its occurrence; any other end by
        // its own instant.
        let guarded_by_date =
            question.ends_by_date && !several_a_day && end.as_ref().is_some_and(|end| end.guarded);
        let cancel_bound = match end_day {
            Some(day) if guarded_by_date => Some(crate::until_through(
                day,
                rrule,
                start,
                false,
                question.from_tzid.as_deref(),
            )),
            _ => end_at,
        };

        let sub_daily_unit: Option<i64> = match freq {
            Freq::Hourly => Some(3600),
            Freq::Minutely => Some(60),
            Freq::Secondly => Some(1),
            _ => None,
        };
        let listed = |key: &str| -> Option<Vec<u32>> {
            part(&parts, key).map(|value| {
                value
                    .split(',')
                    .filter_map(|n| n.trim().parse::<u32>().ok())
                    .collect()
            })
        };
        let (by_hour, by_minute, by_second) =
            (listed("BYHOUR"), listed("BYMINUTE"), listed("BYSECOND"));

        Some(ClockMove {
            question,
            rrule,
            start,
            before_zone,
            after_zone,
            wall_before,
            wall_after,
            day_before,
            parts,
            freq,
            interval,
            week_start,
            follows,
            times,
            several_a_day,
            sub_daily_unit,
            by_hour,
            by_minute,
            by_second,
            end_at,
            end_day,
            cancel_bound,
        })
    }

    /// How far every occurrence moves in wall time as they fall now: by the
    /// start's own move, its days and its time of day. The expanders step
    /// through a rule in its clock's wall time, so an occurrence, a deletion
    /// and a cut's end move together this way on any clock.
    pub(crate) fn wall_shift(&self) -> Duration {
        self.wall_after - self.wall_before
    }

    /// How far every occurrence moves in wall time as the rule says: by the
    /// start's change in time of day alone, its days staying.
    pub(crate) fn rule_shift(&self) -> Duration {
        self.wall_shift() - (self.wall_after.date() - self.day_before)
    }

    /// Whether the rule repeats on `day` of the old clock: on its start's
    /// pattern where its days follow its start, on the days it names, and in
    /// a period its interval repeats in (by the hour or faster, the interval
    /// counts in units of wall time instead: [`ClockMove::on_the_grid`]).
    pub(crate) fn repeats_on(&self, day: NaiveDate) -> bool {
        let named = rule_names_day(self.rrule, day) != Some(false);
        named
            && (!self.follows || on_the_start_pattern(self.freq, day, self.day_before))
            && in_an_interval(
                self.freq,
                self.interval,
                self.week_start,
                day,
                self.day_before,
            )
    }

    /// The time of day the rule's occurrences start at on the old clock, for
    /// a rule with one time a day; `None` for several a day or by the hour or
    /// faster.
    pub(crate) fn occurrence_time(&self) -> Option<NaiveTime> {
        if self.several_a_day {
            None
        } else {
            self.times.first().copied()
        }
    }

    /// The start's day on the old clock.
    pub(crate) fn start_day(&self) -> NaiveDate {
        self.day_before
    }

    /// The rule's `INTERVAL`, at least 1.
    pub(crate) fn interval(&self) -> u32 {
        self.interval
    }

    /// The old clock.
    pub(crate) fn old_clock(&self) -> Zone {
        self.before_zone
    }

    /// The instant the rule's occurrence on `day` of the old clock stands
    /// at, for a rule with one time a day, placed as the expanders place it
    /// (past a gap, at the earlier of two readings); `None` where the rule
    /// does not repeat on `day` or repeats several times a day. The rule's
    /// days reach past its start and its end: this names where an
    /// occurrence would stand. A rule whose days this cannot follow (a set
    /// position, a week number, a day of the year) names none here: a
    /// pairing must not stand on a day the rule may never repeat on.
    pub(crate) fn occurrence_on(&self, day: NaiveDate) -> Option<DateTime<Utc>> {
        use cal_zone::WallPolicy;

        let time = self.occurrence_time()?;
        if rule_names_day(self.rrule, day) != Some(true) || !self.repeats_on(day) {
            return None;
        }
        self.before_zone
            .to_utc(NaiveDateTime::new(day, time), WallPolicy::Rfc5545)
    }

    /// By the hour or faster, whether `nominal` is one of the rule's times:
    /// every `interval` units of wall time from the start's, at the times of
    /// day it lists. A part it leaves out is the start's where it is finer
    /// than the unit, any where it is not (RFC 5545).
    fn on_the_grid(&self, nominal: NaiveDateTime, unit: i64) -> bool {
        let floor = |t: NaiveDateTime| {
            t - Duration::seconds(i64::from(t.num_seconds_from_midnight()) % unit)
        };
        let since = (floor(nominal) - floor(self.wall_before)).num_seconds();
        let fits = |list: &Option<Vec<u32>>, value: u32, own: Option<u32>| match list {
            Some(list) => list.contains(&value),
            None => own.is_none_or(|own| own == value),
        };
        let time = nominal.time();
        since >= 0
            && since % (unit * i64::from(self.interval)) == 0
            && fits(&self.by_hour, time.hour(), None)
            && fits(
                &self.by_minute,
                time.minute(),
                (unit > 60).then(|| self.wall_before.minute()),
            )
            && fits(
                &self.by_second,
                time.second(),
                (unit > 1).then(|| self.wall_before.second()),
            )
    }

    /// The occurrences an exception (or a changed occurrence's slot) at `at`
    /// names on the old clock, by the wall time the rule names them at: those
    /// that stand at its instant (past a gap that skips their time, as the
    /// expanders place them; two can stand there, the one pushed past the gap
    /// and the one after it), on a day the rule repeats on, within its
    /// interval and its end. Empty for one that names nothing.
    fn cancelled(&self, at: DateTime<Utc>) -> Vec<NaiveDateTime> {
        use cal_zone::WallPolicy;

        if at < self.start || self.cancel_bound.is_some_and(|end| at > end) {
            return Vec::new();
        }
        let wall = self.before_zone.wall(at);
        let stands = |nominal: &NaiveDateTime| {
            self.before_zone.to_utc(*nominal, WallPolicy::Rfc5545) == Some(at)
        };
        // A gap pushes a time by its length: an hour in most zones, half an
        // hour or two in a few.
        let pushed_by = [0, 30, 60, 120].map(Duration::minutes);
        let mut nominals: Vec<NaiveDateTime> = match self.sub_daily_unit {
            Some(unit) => pushed_by
                .iter()
                .map(|by| wall - *by)
                .filter(|nominal| {
                    self.on_the_grid(*nominal, unit) && self.repeats_on(nominal.date())
                })
                .filter(stands)
                .collect(),
            None => {
                let mut days: Vec<NaiveDate> =
                    pushed_by.iter().map(|by| (wall - *by).date()).collect();
                days.dedup();
                days.into_iter()
                    .filter(|day| self.repeats_on(*day))
                    .flat_map(|day| {
                        self.times
                            .iter()
                            .map(move |time| NaiveDateTime::new(day, *time))
                    })
                    .filter(stands)
                    .collect()
            }
        };
        nominals.sort();
        nominals.dedup();
        nominals
    }

    /// Every exception moved by `shift` in wall time, as every occurrence
    /// moves: one that cancels an occurrence moves with it onto the new
    /// clock, one that cancels nothing stays as written. A moved one in a
    /// time the new clock skips refuses.
    fn move_exceptions(&self, shift: Duration) -> Result<Vec<String>, ClockChangeRefusal> {
        let mut moved: Vec<String> = Vec::new();
        for text in &self.question.exceptions {
            let at = DateTime::parse_from_rfc3339(text)
                .map_err(|_| ClockChangeRefusal::Unreadable)?
                .with_timezone(&Utc);
            let nominals = self.cancelled(at);
            let texts = if nominals.is_empty() {
                vec![text.clone()]
            } else {
                nominals
                    .into_iter()
                    .map(|nominal| instant_on(self.after_zone, nominal + shift).map(instant_text))
                    .collect::<Result<Vec<_>, _>>()?
            };
            for text in texts {
                if !moved.contains(&text) {
                    moved.push(text);
                }
            }
        }
        Ok(moved)
    }

    /// The end a series moved by `shift` writes, `start_after` its start.
    /// One time a day: through its last day on the new clock, moved by as
    /// many days as the shift moves the start's, as `until_through` spells
    /// it (`…T235959Z` where the new clock reads that as through the day,
    /// else that day's last instant). Several a day: the old end's instant,
    /// moved in wall time as every occurrence is.
    fn end_text(&self, start_after: DateTime<Utc>, shift: Duration) -> Option<String> {
        use cal_zone::WallPolicy;

        let at = self.end_at?;
        if self.several_a_day {
            // The occurrence it ends before moves in wall time, as every
            // occurrence does, placed as the expanders place it (past a gap,
            // at the earlier of two readings), and the end stays a second
            // before it.
            let next = self.before_zone.wall(at + Duration::seconds(1)) + shift;
            let placed = self.after_zone.to_utc(next, WallPolicy::Rfc5545)?;
            return Some(rule_until_text(placed - Duration::seconds(1)));
        }
        let by_days = ((self.wall_before + shift).date() - self.day_before).num_days();
        let day = self.end_day?.checked_add_signed(Duration::days(by_days))?;
        Some(rule_until_text(crate::until_through(
            day,
            self.rrule,
            start_after,
            false,
            self.question.to_tzid.as_deref(),
        )))
    }

    /// `rule` with the end a series moved by `shift` from `start_after`
    /// writes; as it is for a series without an end.
    fn with_end(&self, rule: String, start_after: DateTime<Utc>, shift: Duration) -> String {
        match self.end_text(start_after, shift) {
            Some(until) => with_until(&rule, &until),
            None => rule,
        }
    }

    /// The series moved onto the new clock by one wall shift: its rule (as
    /// a drag moves it, `shift_series`), its start (kept, or moved with the
    /// occurrences), every exception that cancels an occurrence and its end.
    /// Refused with the rule's own reason, or for a start or an exception in
    /// a time the new clock skips.
    pub(crate) fn moved_by(&self, shift: Duration, start: StartMove) -> ClockChangeAnswer {
        let refused = |reason| ClockChangeAnswer::Refused { reason };
        let moved_wall = self.wall_before + shift;
        let Ok(by_days) = i32::try_from((moved_wall.date() - self.day_before).num_days()) else {
            return refused(ClockChangeRefusal::Unreadable);
        };
        let time_changes = moved_wall.time() != self.wall_before.time();
        let rule =
            match crate::shift_series(self.rrule, self.day_before, by_days, time_changes, None) {
                SeriesShift::Refused { reason } => return refused(reason.into()),
                SeriesShift::Shifted { rrule } => rrule,
            };
        let (start_after, start_text) = match start {
            StartMove::Keep => (self.start, self.question.start.clone()),
            StartMove::Shift => match instant_on(self.after_zone, moved_wall) {
                Ok(at) => (at, instant_text(at)),
                Err(reason) => return refused(reason),
            },
        };
        match self.move_exceptions(shift) {
            Ok(exceptions) => ClockChangeAnswer::Changed {
                rrule: self.with_end(rule, start_after, shift),
                start: start_text,
                exceptions,
                moved_minutes: shift.num_minutes(),
            },
            Err(reason) => refused(reason),
        }
    }
}

/// The answer to one question; `None` for a start that names no instant.
pub fn series_clock_change(question: &ClockChangeQuestion) -> Option<SeriesClockChange> {
    let start = DateTime::parse_from_rfc3339(&question.start)
        .ok()?
        .with_timezone(&Utc);
    let (before_zone, _) = clock_of(question.from_tzid.as_deref());
    let (after_zone, _) = clock_of(question.to_tzid.as_deref());
    let wall_before = before_zone.wall(start);
    let wall_after = after_zone.wall(start);
    let day_before = wall_before.date();
    let day_after = wall_after.date();
    let days = i32::try_from((day_after - day_before).num_days()).ok()?;
    let time_changes = wall_before.time() != wall_after.time();
    let rrule = question.rrule.trim();
    let day_text = |day: NaiveDate| day.format("%Y-%m-%d").to_string();
    let refused = |reason| ClockChangeAnswer::Refused { reason };

    let fit = match (
        rule_names_day(rrule, day_before),
        rule_names_day(rrule, day_after),
    ) {
        (Some(true), Some(true)) => RuleFit::Both,
        (Some(true), Some(false)) => RuleFit::Before,
        (Some(false), Some(true)) => RuleFit::After,
        (Some(false), Some(false)) => RuleFit::Neither,
        _ => RuleFit::Unknown,
    };
    let readable = parse_parts(rrule)
        .ok()
        .is_some_and(|(_, parts)| rule_is_readable(&parts));
    if !readable {
        // Nothing can be written for a rule no reader agrees on.
        return Some(SeriesClockChange {
            asks: days != 0,
            start_day_before: day_text(day_before),
            start_day_after: day_text(day_after),
            fit,
            as_now: refused(ClockChangeRefusal::Unreadable),
            as_rule: refused(ClockChangeRefusal::Unreadable),
        });
    }
    let series = ClockMove::read(question)?;

    // As they fall now: the start stays, every occurrence moves by the
    // start's own wall move.
    let as_now = series.moved_by(series.wall_shift(), StartMove::Keep);

    // As the rule says: the rule's text stays, every occurrence moves by the
    // change in time of day alone, and the start moves back onto its old day
    // where the rule's days fit only that one.
    let has_time_parts = ["BYHOUR", "BYMINUTE", "BYSECOND"]
        .iter()
        .any(|k| part(&series.parts, k).is_some());
    let as_rule = if has_time_parts && time_changes {
        refused(ClockChangeRefusal::TimeOfDay)
    } else {
        let moves_back = days != 0 && (series.follows || fit == RuleFit::Before);
        let start_after = if moves_back {
            instant_on(
                after_zone,
                NaiveDateTime::new(day_before, wall_after.time()),
            )
        } else {
            Ok(start)
        };
        let shift = series.rule_shift();
        match start_after {
            Err(reason) => refused(reason),
            Ok(start_after) => match series.move_exceptions(shift) {
                Ok(exceptions) => ClockChangeAnswer::Changed {
                    rrule: series.with_end(rrule.to_string(), start_after, shift),
                    start: if moves_back {
                        instant_text(start_after)
                    } else {
                        question.start.clone()
                    },
                    exceptions,
                    moved_minutes: shift.num_minutes(),
                },
                Err(reason) => refused(reason),
            },
        }
    };

    let asks =
        days != 0 && (!series.follows || matches!(as_now, ClockChangeAnswer::Refused { .. }));
    Some(SeriesClockChange {
        asks,
        start_day_before: day_text(day_before),
        start_day_after: day_text(day_after),
        fit,
        as_now,
        as_rule,
    })
}

/// [`series_clock_change`] over the wire: a question as JSON in, the answer as
/// JSON out, JSON `null` for a start that names no instant. An error only for
/// JSON that is no question at all.
pub fn series_clock_change_json(question_json: &str) -> Result<String, serde_json::Error> {
    let question: ClockChangeQuestion = serde_json::from_str(question_json)?;
    serde_json::to_string(&series_clock_change(&question))
}

/// A question: a series as it is stored, the zone it moves to, how far its
/// occurrences move ([`ClockChangeAnswer::Changed::moved_minutes`]), and the
/// slots of its changed occurrences (their `RECURRENCE-ID`s).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SlotMoveQuestion {
    /// The series' rule, with or without `RRULE:`, as stored on the old
    /// clock.
    pub rrule: String,
    /// The series' start on the old clock, an RFC 3339 instant.
    pub start: String,
    /// The zone name the series stores now; `None` for none (UTC).
    pub from_tzid: Option<String>,
    /// The zone it moves to; `None` for UTC.
    pub to_tzid: Option<String>,
    /// The series' own calendar stores its end as a date, as in
    /// [`ClockChangeQuestion::ends_by_date`]. JSON without the field reads
    /// `false`.
    #[serde(default)]
    pub ends_by_date: bool,
    /// How far, in wall minutes, every occurrence moves: the answer's
    /// `moved_minutes`.
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub moved_minutes: i64,
    /// The slots, RFC 3339 instants.
    pub slots: Vec<String>,
}

/// One slot that moves: from where it was given to where its occurrence now
/// stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SlotMove {
    /// The slot as it was given.
    pub from: String,
    /// The slot on the new clock, RFC 3339 in UTC.
    pub to: String,
}

/// Where the changed occurrences of a series moved onto a new clock now
/// stand (95a). Each slot is matched to the occurrence it names as an
/// exception is (on a day the rule repeats on, in its interval, within its
/// end; a time the old clock skipped as the expanders placed it) and moves to
/// that occurrence's wall time plus `moved_minutes` on the new clock, placed
/// as the expanders place it (past a gap, at the earlier of two readings), so
/// it is never refused. Where two occurrences stand at a slot (one pushed
/// past a gap onto the next), it names the one whose own time it is, the
/// later. A slot that names no occurrence, or that does not move, is left
/// out; a rule that cannot be read names none. `None` for a start that names
/// no instant, and for a shift no clock can hold (a caller's `moved_minutes`
/// beyond any calendar), which the door answers rather than aborting on.
pub fn moved_slots(q: &SlotMoveQuestion) -> Option<Vec<SlotMove>> {
    use cal_zone::WallPolicy;

    DateTime::parse_from_rfc3339(&q.start).ok()?;
    let question = ClockChangeQuestion {
        rrule: q.rrule.clone(),
        start: q.start.clone(),
        exceptions: Vec::new(),
        from_tzid: q.from_tzid.clone(),
        to_tzid: q.to_tzid.clone(),
        ends_by_date: q.ends_by_date,
    };
    let Some(series) = ClockMove::read(&question) else {
        return Some(Vec::new());
    };
    let shift = Duration::try_minutes(q.moved_minutes)?;
    let mut moved = Vec::new();
    for slot in &q.slots {
        let Ok(at) = DateTime::parse_from_rfc3339(slot) else {
            continue;
        };
        let at = at.with_timezone(&Utc);
        let Some(nominal) = series.cancelled(at).last().copied() else {
            continue;
        };
        let wall = nominal.checked_add_signed(shift)?;
        let Some(to) = series.after_zone.to_utc(wall, WallPolicy::Rfc5545) else {
            continue;
        };
        if to != at {
            moved.push(SlotMove {
                from: slot.clone(),
                to: instant_text(to),
            });
        }
    }
    Some(moved)
}

/// [`moved_slots`] over the wire: a question as JSON in, the moves as JSON
/// out, JSON `null` for a start that names no instant. An error only for
/// JSON that is no question at all.
pub fn moved_slots_json(question_json: &str) -> Result<String, serde_json::Error> {
    let question: SlotMoveQuestion = serde_json::from_str(question_json)?;
    serde_json::to_string(&moved_slots(&question))
}

#[cfg(test)]
mod contract {
    //! The shared fixture.

    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/seriesClockChange.json"
        ));
        serde_json::from_str(text).expect("the fixture parses")
    }

    #[test]
    fn every_row_gives_its_answer() {
        let doc = fixture();
        let rows = doc["rows"].as_array().expect("rows");
        // Anti-silence: the rows the rule turns on, by name.
        for needed in [
            "lost-berlin-zone-monday-rule",
            "a-series-that-means-utc-sundays",
            "same-day-moves-cancelling-exceptions-only",
            "monthly-on-the-31st-cannot-keep-its-days",
            "moved-start-in-the-spring-gap",
            "a-cut-moves-as-the-end-of-its-last-day",
            "new-york-1900-end-of-utc-day-to-chicago-ends-by-date",
            "new-york-2000-end-of-utc-day-to-berlin-ends-by-date-moves-the-last-deletion",
            "new-york-1900-end-of-utc-day-to-chicago-reads-as-a-cut",
        ] {
            assert!(
                rows.iter().any(|r| r["name"] == needed),
                "the fixture lost {needed}"
            );
        }
        for row in rows {
            let answer: Value = serde_json::from_str(
                &series_clock_change_json(&row["question"].to_string()).expect("a question"),
            )
            .expect("json");
            assert_eq!(answer, row["expect"], "{}", row["name"]);
        }
    }

    #[test]
    fn text_that_is_no_question_is_an_error() {
        assert!(series_clock_change_json("{\"rrule\":1}").is_err());
        assert!(series_clock_change_json("not json").is_err());
    }

    fn moved_slots_fixture() -> Value {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/movedSlots.json"
        ));
        serde_json::from_str(text).expect("the fixture parses")
    }

    #[test]
    fn every_moved_slots_row_gives_its_answer() {
        let doc = moved_slots_fixture();
        let rows = doc["rows"].as_array().expect("rows");
        // Anti-silence: the rows the rule turns on, by name.
        for needed in [
            "as-now-in-the-other-half-year",
            "lost-berlin-zone-monday-rule-as-the-rule-says",
            "every-other-week",
            "a-slot-that-names-no-occurrence",
            "a-slot-after-the-end",
            "a-slot-moved-into-the-spring-gap",
            "a-start-that-names-no-instant",
        ] {
            assert!(
                rows.iter().any(|r| r["name"] == needed),
                "the fixture lost {needed}"
            );
        }
        for row in rows {
            let answer: Value = serde_json::from_str(
                &moved_slots_json(&row["question"].to_string()).expect("a question"),
            )
            .expect("json");
            assert_eq!(answer, row["expect"], "{}", row["name"]);
        }
    }

    #[test]
    fn text_that_is_no_slot_question_is_an_error() {
        assert!(moved_slots_json("{\"rrule\":1}").is_err());
        assert!(moved_slots_json("not json").is_err());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn moved_slots_answers_none_for_a_shift_no_clock_can_hold() {
        let question = SlotMoveQuestion {
            rrule: "FREQ=DAILY".into(),
            start: "2026-11-02T08:00:00Z".into(),
            from_tzid: Some("Europe/Berlin".into()),
            to_tzid: None,
            ends_by_date: false,
            moved_minutes: 1_000_000_000_000,
            slots: vec!["2026-11-03T08:00:00Z".into()],
        };
        assert_eq!(moved_slots(&question), None);
        let question = SlotMoveQuestion {
            moved_minutes: i64::MAX,
            ..question
        };
        assert_eq!(moved_slots(&question), None);
    }

    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("a date")
    }

    fn question(
        rrule: &str,
        start: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> ClockChangeQuestion {
        ClockChangeQuestion {
            rrule: rrule.to_string(),
            start: start.to_string(),
            exceptions: Vec::new(),
            from_tzid: from.map(str::to_string),
            to_tzid: to.map(str::to_string),
            ends_by_date: false,
        }
    }

    /// As the rule says with its start moved back onto its old day is the
    /// series moved by the rule's shift, start and all.
    #[test]
    fn a_start_moved_back_is_a_start_moved_with_the_occurrences() {
        let q = question(
            "FREQ=WEEKLY;BYDAY=SU",
            "2026-11-01T23:30:00Z",
            None,
            Some("Europe/Berlin"),
        );
        let series = ClockMove::read(&q).expect("a series");
        assert_eq!(series.rule_shift(), Duration::minutes(-1380));
        assert_eq!(series.wall_shift(), Duration::minutes(60));
        let answer = series_clock_change(&q).expect("an answer");
        assert_eq!(
            series.moved_by(series.rule_shift(), StartMove::Shift),
            answer.as_rule
        );
        assert_eq!(
            answer.as_rule,
            ClockChangeAnswer::Changed {
                rrule: "FREQ=WEEKLY;BYDAY=SU".into(),
                start: "2026-10-31T23:30:00Z".into(),
                exceptions: Vec::new(),
                moved_minutes: -1380,
            }
        );
    }

    /// A start moved into a time the new clock skips refuses, as the
    /// answer does.
    #[test]
    fn a_start_moved_into_the_spring_gap_refuses() {
        let q = question(
            "FREQ=WEEKLY;BYDAY=SU",
            "2027-03-29T00:30:00Z",
            Some("America/Los_Angeles"),
            Some("Europe/Berlin"),
        );
        let series = ClockMove::read(&q).expect("a series");
        assert_eq!(
            series.moved_by(series.rule_shift(), StartMove::Shift),
            ClockChangeAnswer::Refused {
                reason: ClockChangeRefusal::SkippedTime
            }
        );
    }

    /// Any other shift moves the series as a whole, as a drag does: an hour
    /// later on the same day keeps the rule's days.
    #[test]
    fn any_shift_moves_the_series_as_a_whole() {
        let mut q = question(
            "FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T225959Z",
            "2026-11-02T08:00:00Z",
            Some("Europe/Berlin"),
            Some("Europe/Berlin"),
        );
        q.exceptions = vec!["2026-11-16T08:00:00Z".into()];
        let series = ClockMove::read(&q).expect("a series");
        assert_eq!(
            series.moved_by(Duration::minutes(60), StartMove::Shift),
            ClockChangeAnswer::Changed {
                // Through 30 November, as `until_through` spells it.
                rrule: "FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T235959Z".into(),
                start: "2026-11-02T09:00:00Z".into(),
                exceptions: vec!["2026-11-16T09:00:00Z".into()],
                moved_minutes: 60,
            }
        );
        // Fifteen hours later is the next day: the rule's days move with it.
        let ClockChangeAnswer::Changed { rrule, start, .. } =
            series.moved_by(Duration::minutes(15 * 60), StartMove::Shift)
        else {
            panic!("a move");
        };
        assert_eq!(start, "2026-11-02T23:00:00Z");
        assert!(rrule.starts_with("FREQ=WEEKLY;BYDAY=TU;"), "{rrule}");
    }

    #[test]
    fn the_time_of_day_of_a_rule_with_one() {
        let at = |rrule: &str| {
            let q = question(rrule, "2026-11-02T08:00:00Z", Some("Europe/Berlin"), None);
            ClockMove::read(&q).expect("a series").occurrence_time()
        };
        assert_eq!(at("FREQ=WEEKLY;BYDAY=MO"), NaiveTime::from_hms_opt(9, 0, 0));
        assert_eq!(
            at("FREQ=DAILY;BYHOUR=10"),
            NaiveTime::from_hms_opt(10, 0, 0)
        );
        assert_eq!(at("FREQ=DAILY;BYHOUR=9,17"), None);
        assert_eq!(at("FREQ=HOURLY"), None);
    }

    /// The rule's days on the old clock: the days it names, its start's
    /// pattern where its days follow it, and its interval.
    #[test]
    fn the_days_a_rule_repeats_on() {
        let q = question(
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=WE",
            "2026-11-04T08:00:00Z",
            Some("Europe/Berlin"),
            None,
        );
        let series = ClockMove::read(&q).expect("a series");
        assert!(series.repeats_on(day(2026, 11, 18)));
        assert!(!series.repeats_on(day(2026, 11, 11)));
        assert!(!series.repeats_on(day(2026, 11, 17)));
    }

    #[test]
    fn counted_weekdays_and_days_from_the_end() {
        // The second Sunday of November 2026 is the 8th, the last Friday the 27th.
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYDAY=2SU", day(2026, 11, 8)),
            Some(true)
        );
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYDAY=2SU", day(2026, 11, 15)),
            Some(false)
        );
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYDAY=-1FR", day(2026, 11, 27)),
            Some(true)
        );
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYDAY=-1FR", day(2026, 11, 20)),
            Some(false)
        );
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYMONTHDAY=-1", day(2026, 2, 28)),
            Some(true)
        );
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYMONTHDAY=-2", day(2028, 2, 28)),
            Some(true)
        );
        // A yearly rule without a month counts the weeks of the year.
        assert_eq!(
            rule_names_day("FREQ=YEARLY;BYDAY=1MO", day(2026, 1, 5)),
            Some(true)
        );
        assert_eq!(
            rule_names_day("FREQ=YEARLY;BYDAY=-1SU", day(2026, 12, 27)),
            Some(true)
        );
        // With a month, within that month.
        assert_eq!(
            rule_names_day("FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU", day(2026, 3, 29)),
            Some(true)
        );
        assert_eq!(
            rule_names_day("FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU", day(2026, 10, 25)),
            Some(false)
        );
        // What it cannot follow.
        assert_eq!(
            rule_names_day("FREQ=MONTHLY;BYDAY=MO;BYSETPOS=1", day(2026, 11, 2)),
            None
        );
    }
}
