//! Carrying a series' zone change to its copies (DESIGN-series-time-zone.md,
//! section 23b and stage 13; decisions 143a to 146a, 120a).
//!
//! After a whole-series save that changed the zone of a series timed before
//! and after, the one carry question names the zone first among the changes
//! (143a). Each writable copy that is a timed series then moves onto the new
//! clock by ONE wall shift `S_C`, start, rule, cancelling exceptions and end
//! together ([`crate::series_clock_change`]'s mover); a time the new clock
//! skips refuses the move. This rule answers, for one copy, what that write
//! is, or why there is none.
//!
//! # How far a copy moves
//!
//! The copy is paired with the series by instants, never by its start day,
//! in one of two ways:
//!
//! - **Before:** `y` is the series' first occurrence on its old pattern, and
//!   `x` the copy's occurrence on its own pattern that is nearest `y` and
//!   less than 12 hours away (both patterns reach past their starts and
//!   ends). Then `S_C = wall_zA(x) + S_A − wall_zC(x)`: `x` read on the
//!   series' old clock, moved as the series' occurrences moved (`S_A`, the
//!   applied zone answers' `movedMinutes` plus `Δ`, any date or time move made
//!   after the choice, on the new clock), minus `x` read on the copy's own old
//!   clock. A true copy lands on the series; one in the other half-year comes
//!   to the series' hour and day move (144a); one at another hour keeps its
//!   distance (145a).
//! - **After:** `y` is where the series' first NEW occurrence stood once its
//!   zone answers were applied (before `Δ`), and `x` the copy's occurrence
//!   nearest it, as above. The copy already sits where the series now sits:
//!   it keeps its instants and takes `Δ` alone,
//!   `S_C = wall_zT(x) + Δ − wall_zC(x)`.
//! - Where both pair, a week of each pattern decides first: the pairing
//!   under which fewer of the series' and the copy's occurrences stand on one
//!   side only, compared by wall days on each clock with two hours' room for
//!   a clock change. A rule that repeats on neighbouring days (Monday to
//!   Friday) stands near both patterns; only its whole week tells a copy of
//!   the old series from one already repaired. Where the weeks cannot tell (a
//!   rule on every day, one day a week), the copy's clock does: on the
//!   series' old clock it follows the old series, on the new one it already
//!   stands as the series does. Then the closer pairing, then the one that
//!   moves the copy less, then "after".
//! - Neither: its occurrences are not on the series' days, and it is refused.
//!   So is a copy whose days the reader cannot follow (a set position, a week
//!   number, a day of the year) beside a series whose days it can. The 12
//!   hours are Claude's technical choice (DESIGN, stage 13, for Toni's
//!   collected check).
//! - A rule with several times a day on either side, or a series whose days
//!   the reader cannot follow, pairs at the copy's own start, with no day
//!   check.
//! - The move must then land: the series' paired occurrence, moved, on the
//!   saved series, and the copy's on its new rule. Where an answer re-anchors
//!   a rule's weeks (every other week from a start that is no occurrence on
//!   one clock), it would not, and the copy is refused rather than written a
//!   week off. A copy identical to the series as it stood takes the series'
//!   own answer and ends as the series.
//!
//! # What it answers
//!
//! The copy's target clock is its own where that is the series' new clock
//! (both UTC, the same canonical zone, or the same rules from today on, 120a),
//! else the series' new zone. On its own clock with `S_C` zero and its length
//! right it is [`ZoneCarry::Unchanged`] and is not written. Otherwise its move
//! is composed as the series' was: for the zone part of its shift (without
//! `Δ`) it takes its own "as the rule says" or "as they fall now" where the
//! part is one of them, so an identical copy ends identical to the series,
//! and any other part moves it as a whole; then `Δ` drags it on its new clock,
//! as the editor dragged the series.
//!
//! A copy that is no series there, is all-day there, cannot store the zone,
//! is off the series' days or whose rule cannot move is refused with that
//! reason (146a); the surfaces still carry its title, location and
//! description. Row facts come before calendar facts, so the offer and the
//! report name the same reason.
//!
//! Pinned by `tests/fixtures/groupZoneCarry.json`; the surfaces replay it.

use cal_zone::WallPolicy;
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::series_clock_change::{ClockMove, StartMove};
use crate::wall_clock::clock_of;
use crate::{
    canonical_zone, series_clock_change, series_clock_zone, ClockChangeAnswer, ClockChangeQuestion,
    ClockChangeRefusal, SeriesTimeZones,
};

/// A series as stored: its rule, start and zone.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SeriesFacts {
    /// The rule, with or without `RRULE:`.
    pub rrule: String,
    /// The start, an RFC 3339 instant.
    pub start: String,
    /// The zone name it stores; `None` for none (UTC).
    pub tzid: Option<String>,
}

/// A copy as its calendar stores it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct CopyFacts {
    /// Its rule; `None` or empty where it is no series there (a single
    /// event, a device calendar's row, an occurrence row).
    pub rrule: Option<String>,
    /// RFC 3339 instants.
    pub start: String,
    pub end: String,
    /// The instants its exceptions cancel.
    #[serde(default)]
    pub exceptions: Vec<String>,
    /// The zone name it stores; `None` for none (UTC).
    pub tzid: Option<String>,
    pub all_day: bool,
}

/// One question: the series before and after its save, how its occurrences
/// moved, and one copy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ZoneCarryQuestion {
    /// The series as it was stored before the save.
    pub anchor_before: SeriesFacts,
    /// The series as it was saved; `tzid` `None` for UTC.
    pub anchor_after: SeriesFacts,
    /// The start the applied zone answers left (the session's zone answer's
    /// `start`), on the new clock.
    pub applied_start: String,
    /// How far the applied zone answers moved every occurrence, in wall
    /// minutes: the sum of their `movedMinutes`.
    #[cfg_attr(feature = "ts-export", ts(type = "number"))]
    pub moved_minutes: i64,
    /// The saved length in milliseconds, only where the save changed the end
    /// against the applied series; `None` keeps each copy's own length.
    #[cfg_attr(feature = "ts-export", ts(type = "number | null"))]
    pub new_length_ms: Option<i64>,
    pub copy: CopyFacts,
    /// The zones the copy's calendar stores for a series (97a), in the
    /// manifest's form.
    #[cfg_attr(feature = "ts-export", ts(type = "\"all\" | \"none\" | Array<string>"))]
    pub time_zones: SeriesTimeZones,
    /// The copy's calendar stores a series' end as a date (manifest
    /// `recurrence.ends_by_date`, 140a). JSON without the field reads
    /// `false`.
    #[serde(default)]
    pub ends_by_date: bool,
    /// Today, from the surface's clock: the rule groups are compared from it
    /// on (120a, 123a).
    pub today: DateTime<Utc>,
}

/// Why a copy does not get the zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ZoneCarryRefusal {
    /// It has no rule there: the zone does not apply to it.
    NotASeries,
    /// It is all-day there.
    AllDay,
    /// Its calendar cannot store the series' new zone.
    ZoneNotStorable,
    /// Its occurrences are not on the series' days.
    OffPattern,
    /// Its rule cannot move by its shift; `rule` says why.
    RuleCannotMove,
}

/// What a copy gets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum ZoneCarry {
    /// It already stands as the series does, on its own clock: nothing is
    /// written.
    Unchanged,
    /// The copy as it is written on its target clock.
    Changed {
        rrule: String,
        /// RFC 3339.
        start: String,
        /// RFC 3339: the new start plus the saved length, or the copy's own.
        end: String,
        /// RFC 3339, in the order they came.
        exceptions: Vec<String>,
        /// The zone it is written with, as spelled: its own where it keeps
        /// its clock, the series' otherwise; `None` for UTC.
        tzid: Option<String>,
        /// How far, in wall minutes, every occurrence moves; a changed
        /// occurrence's slot moves by it too (95a). Zero where only the end
        /// changes.
        #[serde(rename = "shiftMinutes")]
        #[cfg_attr(feature = "ts-export", ts(type = "number"))]
        shift_minutes: i64,
        /// How many days its first occurrence moves for the zone, read on
        /// the new clock before and after (a date move made after the zone
        /// choice left out): negative is earlier ("rücken vor", 138a).
        #[serde(rename = "zoneDays")]
        #[cfg_attr(feature = "ts-export", ts(type = "number"))]
        zone_days: i64,
    },
    /// The copy does not get the zone.
    Refused {
        reason: ZoneCarryRefusal,
        /// The rule's own reason, for [`ZoneCarryRefusal::RuleCannotMove`].
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts-export", ts(optional))]
        rule: Option<ClockChangeRefusal>,
    },
}

/// How close a copy's occurrence must stand to the series' to pair with it.
const PAIRING: i64 = 12;

/// How far a shift may reach: further than any date a stored series names,
/// short of any a clock cannot hold, so no wall time overflows.
const MAX_SHIFT_MINUTES: i64 = 1_000 * 366 * 24 * 60;

fn instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn instant_text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// The canonical zone a series' clock is, `None` for UTC (every UTC name,
/// none, and a name tzdata does not know).
fn named_clock(tzid: Option<&str>) -> Option<&'static str> {
    series_clock_zone(tzid).and_then(canonical_zone)
}

/// Whether two stored zones are one clock (120a): both UTC, the same
/// canonical zone, or the same rules from `today` on.
fn same_clock(a: Option<&str>, b: Option<&str>, today: DateTime<Utc>) -> bool {
    match (named_clock(a), named_clock(b)) {
        (None, None) => true,
        (Some(a), Some(b)) => crate::zone_list::same_rules(a, b, today),
        _ => false,
    }
}

/// A series' first occurrence on its old pattern: the first day from its
/// start's on which it repeats, at its time; `None` for a rule with several
/// times a day or none in eight of its periods.
fn first_occurrence(series: &ClockMove) -> Option<DateTime<Utc>> {
    series.occurrence_time()?;
    let days = (8 * 366 * i64::from(series.interval())).min(150_000);
    (0..=days).find_map(|n| {
        series
            .start_day()
            .checked_add_signed(Duration::days(n))
            .and_then(|day| series.occurrence_on(day))
    })
}

/// The copy's occurrence nearest `y` and less than [`PAIRING`] hours from
/// it, on the day `y` reads as on the copy's clock or the one before or
/// after; on a tie the earlier.
fn nearest(copy: &ClockMove, y: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let day: NaiveDate = copy.old_clock().wall(y).date();
    (-1..=1)
        .filter_map(|k| day.checked_add_signed(Duration::days(k)))
        .filter_map(|day| copy.occurrence_on(day))
        .map(|x| ((x - y).abs(), x))
        .filter(|(gap, _)| *gap < Duration::hours(PAIRING))
        .min()
        .map(|(_, x)| x)
}

/// How many wall days of two patterns a pairing compares.
const WEEK: i64 = 7;

/// How far two occurrences may stand apart and still count as one: a clock
/// change inside the week moves one pattern by up to an hour against another
/// (two at Antarctica/Troll), never by a day.
const SAME_OCCURRENCE_MINUTES: i64 = 120;

/// The instants `pattern` names on its clock's wall days `from_day` to
/// `from_day + days - 1`, each placed after moving its NOMINAL wall time back
/// by `back`: the series' new pattern as it stood before a move made after
/// the zone choice. Placing the nominal time, not the instant a gap pushed
/// it to, keeps an occurrence in the spring gap where it belongs.
fn occurrences_from(
    pattern: &ClockMove,
    from_day: NaiveDate,
    days: i64,
    back: Duration,
) -> Vec<DateTime<Utc>> {
    let clock = pattern.old_clock();
    let Some(time) = pattern.occurrence_time() else {
        return Vec::new();
    };
    (0..days)
        .filter_map(|k| from_day.checked_add_signed(Duration::days(k)))
        .filter(|day| pattern.occurrence_on(*day).is_some())
        .filter_map(|day| {
            let nominal = NaiveDateTime::new(day, time).checked_sub_signed(back)?;
            clock.to_utc(nominal, WallPolicy::Rfc5545)
        })
        .collect()
}

/// The day an instant `pattern` names stands on, as nominal wall time on its
/// clock: the day of `at` or, where a gap pushed it past midnight, the one
/// before.
fn occurrence_day(pattern: &ClockMove, at: DateTime<Utc>) -> Option<NaiveDate> {
    let day = pattern.old_clock().wall(at).date();
    [Some(day), day.pred_opt()]
        .into_iter()
        .flatten()
        .find(|day| pattern.occurrence_on(*day) == Some(at))
}

/// Whether `pattern`, moved back by `back`, names an occurrence within
/// [`SAME_OCCURRENCE_MINUTES`] of `at`.
fn stands_near(pattern: &ClockMove, at: DateTime<Utc>, back: Duration) -> bool {
    let clock = pattern.old_clock();
    let Some(day) = clock
        .wall(at)
        .checked_add_signed(back)
        .map(|wall| wall.date())
    else {
        return false;
    };
    let Some(first) = day.pred_opt() else {
        return false;
    };
    occurrences_from(pattern, first, 3, back)
        .into_iter()
        .any(|p| (p - at).num_minutes().abs() <= SAME_OCCURRENCE_MINUTES)
}

/// How many occurrences of one week stand in only one of two patterns: the
/// series' from `series_first` (moved back by `back`), moved by the distance
/// from `y` to `x`, and the copy's from `x`. Compared by wall days on each
/// clock and with room for a clock change, so a change inside the week counts
/// for neither.
fn one_sided(
    series: &ClockMove,
    series_first: DateTime<Utc>,
    back: Duration,
    copy: &ClockMove,
    y: DateTime<Utc>,
    x: DateTime<Utc>,
) -> Option<usize> {
    let apart = x - y;
    let series_week = occurrences_from(series, occurrence_day(series, series_first)?, WEEK, back);
    let copy_week = occurrences_from(copy, occurrence_day(copy, x)?, WEEK, Duration::zero());
    let lone_series = series_week
        .iter()
        .filter_map(|s| s.checked_add_signed(apart))
        .filter(|s| !stands_near(copy, *s, Duration::zero()))
        .count();
    let lone_copy = copy_week
        .iter()
        .filter_map(|c| c.checked_sub_signed(apart))
        .filter(|c| !stands_near(series, *c, back))
        .count();
    Some(lone_series + lone_copy)
}

/// One way the copy pairs with the series, and what it would write.
struct Pairing {
    /// How many occurrences of a week stand in only one of the two patterns:
    /// the series' (moved by the pairing's distance) or the copy's, compared
    /// by wall days, with room for a clock change.
    misses: usize,
    /// Whether the copy stands on the clock this pairing reads the series on:
    /// the series' old clock for "before", its new one for "after". Where the
    /// weeks cannot tell (a rule on every day, one day a week across a clock
    /// change), a copy on the old clock follows the old series, and one on the
    /// new clock already stands as the series does.
    own_clock: bool,
    /// How far the paired occurrences stand apart.
    gap: Duration,
    /// How far, in minutes, the copy's paired occurrence moves in time.
    moved: i64,
    /// The copy's paired occurrence, and its wall shift.
    x: DateTime<Utc>,
    shift: Duration,
    /// Where the series' paired occurrence stands once saved, for a "before"
    /// pairing: it must be one of the saved series' occurrences.
    series_after: Option<DateTime<Utc>>,
}

impl Pairing {
    /// The order in which two pairings are weighed: the smaller wins.
    fn weight(&self) -> (usize, bool, Duration, i64) {
        (self.misses, !self.own_clock, self.gap, self.moved)
    }
}

/// What one copy gets when a series' zone change is carried to it.
pub fn zone_carry(q: &ZoneCarryQuestion) -> ZoneCarry {
    let refused = |reason| ZoneCarry::Refused { reason, rule: None };
    let cannot_move = |rule| ZoneCarry::Refused {
        reason: ZoneCarryRefusal::RuleCannotMove,
        rule: Some(rule),
    };
    let unreadable = cannot_move(ClockChangeRefusal::Unreadable);

    // (1) Row facts first.
    let Some(copy_rule) = q
        .copy
        .rrule
        .as_deref()
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
    else {
        return refused(ZoneCarryRefusal::NotASeries);
    };
    if q.copy.all_day {
        return refused(ZoneCarryRefusal::AllDay);
    }

    // (2) The clocks: the series' old and new, the copy's own.
    let (series_old, _) = clock_of(q.anchor_before.tzid.as_deref());
    let (series_new, _) = clock_of(q.anchor_after.tzid.as_deref());
    let (copy_old, _) = clock_of(q.copy.tzid.as_deref());

    // (3) How far the series' occurrences moved: the zone answers, plus the
    // move made after the choice (Δ), read on the new clock.
    let (Some(applied), Some(saved), Some(copy_start), Some(copy_end)) = (
        instant(&q.applied_start),
        instant(&q.anchor_after.start),
        instant(&q.copy.start),
        instant(&q.copy.end),
    ) else {
        return unreadable;
    };
    if q.moved_minutes.unsigned_abs() > MAX_SHIFT_MINUTES.unsigned_abs() {
        return unreadable;
    }
    let delta = series_new.wall(saved) - series_new.wall(applied);
    let series_shift = Duration::minutes(q.moved_minutes) + delta;

    // (5) The copy's target clock: its own where that is the series' new
    // clock (120a), the series' otherwise. Asked first: the copy is read for
    // a move onto it.
    let keeps_clock = same_clock(
        q.copy.tzid.as_deref(),
        q.anchor_after.tzid.as_deref(),
        q.today,
    );
    let target_tzid = if keeps_clock {
        q.copy.tzid.as_deref()
    } else {
        q.anchor_after.tzid.as_deref()
    };
    let (target, _) = clock_of(target_tzid);
    let copy_question = ClockChangeQuestion {
        rrule: copy_rule.to_string(),
        start: q.copy.start.clone(),
        exceptions: q.copy.exceptions.clone(),
        from_tzid: q.copy.tzid.clone(),
        to_tzid: target_tzid.map(str::to_string),
        ends_by_date: q.ends_by_date,
    };
    let Some(copy) = ClockMove::read(&copy_question) else {
        return unreadable;
    };
    let before_question = ClockChangeQuestion {
        rrule: q.anchor_before.rrule.clone(),
        start: q.anchor_before.start.clone(),
        exceptions: Vec::new(),
        from_tzid: q.anchor_before.tzid.clone(),
        to_tzid: q.anchor_after.tzid.clone(),
        ends_by_date: false,
    };
    let Some(series) = ClockMove::read(&before_question) else {
        return unreadable;
    };

    // The series as saved, on its new clock: the "after" pairing reads its
    // pattern, and a "before" pairing must land on it.
    let after_question = ClockChangeQuestion {
        rrule: q.anchor_after.rrule.clone(),
        start: q.anchor_after.start.clone(),
        exceptions: Vec::new(),
        from_tzid: q.anchor_after.tzid.clone(),
        to_tzid: q.anchor_after.tzid.clone(),
        ends_by_date: false,
    };
    let saved_series = ClockMove::read(&after_question);
    // A copy that is the series as it stood before the save (same rule, same
    // start, same clock) ends as the series: it takes the series' own answer.
    let identical = copy_rule == q.anchor_before.rrule.trim()
        && instant(&q.anchor_before.start) == Some(copy_start)
        && same_clock(
            q.copy.tzid.as_deref(),
            q.anchor_before.tzid.as_deref(),
            q.today,
        );

    // (4) Pair the copy with the series by instants, before or after (see
    // the module's doc). A rule with several times a day on either side, or
    // a series whose days the reader cannot follow, pairs at the copy's own
    // start.
    let wall_shift_at = |x: DateTime<Utc>| series_old.wall(x) + series_shift - copy_old.wall(x);
    let pairs_at_start = series.occurrence_time().is_none()
        || copy.occurrence_time().is_none()
        || !series.follows_its_days();
    let paired: Option<Pairing> = if pairs_at_start {
        None
    } else {
        let moved_by = |x: DateTime<Utc>, shift: Duration| {
            copy_old
                .wall(x)
                .checked_add_signed(shift)
                .and_then(|wall| target.to_utc(wall, WallPolicy::Rfc5545))
                .map_or(i64::MAX, |to| (to - x).num_minutes().abs())
        };
        // Before: the copy follows the series' old occurrences, moved as they
        // moved.
        let before = first_occurrence(&series).and_then(|y| {
            let x = nearest(&copy, y)?;
            let shift = wall_shift_at(x);
            Some(Pairing {
                misses: one_sided(&series, y, Duration::zero(), &copy, y, x)?,
                own_clock: same_clock(
                    q.copy.tzid.as_deref(),
                    q.anchor_before.tzid.as_deref(),
                    q.today,
                ),
                gap: (x - y).abs(),
                moved: moved_by(x, shift),
                x,
                shift,
                series_after: series_old
                    .wall(y)
                    .checked_add_signed(series_shift)
                    .and_then(|wall| series_new.to_utc(wall, WallPolicy::Rfc5545)),
            })
        });
        // After: the copy already stands where the series stood once its zone
        // answers were applied; it keeps its instants and takes the move made
        // after the choice alone.
        let after = saved_series.as_ref().and_then(|saved_series| {
            let first_new = first_occurrence(saved_series)?;
            let day = occurrence_day(saved_series, first_new)?;
            let y = *occurrences_from(saved_series, day, 1, delta).first()?;
            let x = nearest(&copy, y)?;
            let shift = target.wall(x) + delta - copy_old.wall(x);
            Some(Pairing {
                misses: one_sided(saved_series, first_new, delta, &copy, y, x)?,
                own_clock: keeps_clock,
                gap: (x - y).abs(),
                moved: moved_by(x, shift),
                x,
                shift,
                series_after: None,
            })
        });
        match (before, after) {
            (Some(before), Some(after)) => Some(if before.weight() < after.weight() {
                before
            } else {
                after
            }),
            (Some(pairing), None) | (None, Some(pairing)) => Some(pairing),
            (None, None) => return refused(ZoneCarryRefusal::OffPattern),
        }
    };
    let shift = paired
        .as_ref()
        .map_or_else(|| wall_shift_at(copy_start), |pairing| pairing.shift);

    let own_length = (copy_end - copy_start).num_milliseconds().max(0);
    let length = q.new_length_ms.map_or(own_length, |ms| ms.max(0));
    let written_tzid = series_clock_zone(target_tzid).map(str::to_string);
    let end_after = |start: DateTime<Utc>| {
        Duration::try_milliseconds(length)
            .and_then(|length| start.checked_add_signed(length))
            .map(instant_text)
    };

    // (6) On its own clock, not moved, its length right: nothing to write.
    if keeps_clock && shift.is_zero() {
        if length == own_length {
            return ZoneCarry::Unchanged;
        }
        let Some(end) = end_after(copy_start) else {
            return unreadable;
        };
        return ZoneCarry::Changed {
            rrule: copy_rule.to_string(),
            start: q.copy.start.clone(),
            end,
            exceptions: q.copy.exceptions.clone(),
            tzid: written_tzid,
            shift_minutes: 0,
            zone_days: 0,
        };
    }

    // (7) Calendar facts: the series' zone where the copy takes it. UTC is
    // stored by every calendar that stores a rule.
    if !keeps_clock {
        if let Some(zone) = named_clock(q.anchor_after.tzid.as_deref()) {
            if !q.time_zones.stores(zone) {
                return refused(ZoneCarryRefusal::ZoneNotStorable);
            }
        }
    }

    // (8) The move, composed as the series' was: first the zone, then the
    // move made after the choice. For the zone part of the shift the copy
    // takes its own answer where the part is one of them ("as the rule says"
    // or "as they fall now"), so an identical copy ends identical to the
    // series; any other part moves it as a whole. The move made after the
    // choice then drags it on its new clock, as the editor dragged the
    // series. Compared as one shift, a drag of a day could equal the copy's
    // "as the rule says" and keep a start the series moved (review round 3).
    let zone_shift = shift - delta;
    let answer = if zone_shift == copy.rule_shift() {
        series_clock_change(&copy_question).map(|change| change.as_rule)
    } else if zone_shift == copy.wall_shift() {
        Some(copy.moved_by(zone_shift, StartMove::Keep))
    } else {
        Some(copy.moved_by(zone_shift, StartMove::Shift))
    };
    let (rrule, start, exceptions) = match answer {
        Some(ClockChangeAnswer::Changed {
            rrule,
            start,
            exceptions,
            ..
        }) => (rrule, start, exceptions),
        Some(ClockChangeAnswer::Refused { reason }) => return cannot_move(reason),
        None => return unreadable,
    };
    let (rrule, start, exceptions) = if delta.is_zero() {
        (rrule, start, exceptions)
    } else {
        let answered = ClockChangeQuestion {
            rrule,
            start,
            exceptions,
            from_tzid: target_tzid.map(str::to_string),
            to_tzid: target_tzid.map(str::to_string),
            ends_by_date: q.ends_by_date,
        };
        let Some(answered_copy) = ClockMove::read(&answered) else {
            return unreadable;
        };
        match answered_copy.moved_by(delta, StartMove::Shift) {
            ClockChangeAnswer::Changed {
                rrule,
                start,
                exceptions,
                ..
            } => (rrule, start, exceptions),
            ClockChangeAnswer::Refused { reason } => return cannot_move(reason),
        }
    };

    // (8b) The move must land the copy as the pairing says: the series'
    // paired occurrence, moved, on the saved series, and the copy's, moved,
    // on its new rule. Where a move re-anchors a rule's weeks (every other
    // week, from a start that is no occurrence on one clock), it would not,
    // and the copy is named instead of being written a week off. A copy
    // identical to the series ends as the series and is not checked.
    if let Some(pairing) = paired.as_ref().filter(|_| !identical) {
        let series_lands = pairing.series_after.is_none_or(|at| {
            saved_series
                .as_ref()
                .is_some_and(|saved| stands_near(saved, at, Duration::zero()))
        });
        let moved_copy = ClockChangeQuestion {
            rrule: rrule.clone(),
            start: start.clone(),
            exceptions: Vec::new(),
            from_tzid: written_tzid.clone(),
            to_tzid: written_tzid.clone(),
            ends_by_date: false,
        };
        let copy_lands = copy_old
            .wall(pairing.x)
            .checked_add_signed(shift)
            .and_then(|wall| target.to_utc(wall, WallPolicy::Rfc5545))
            .is_some_and(|at| {
                ClockMove::read(&moved_copy)
                    .is_some_and(|moved| stands_near(&moved, at, Duration::zero()))
            });
        if !series_lands || !copy_lands {
            return refused(ZoneCarryRefusal::OffPattern);
        }
    }

    // (9) The end: the saved length, or the copy's own.
    let Some(end) = instant(&start).and_then(end_after) else {
        return unreadable;
    };

    // (10) How far its first occurrence moves for the zone, read on the new
    // clock before and after.
    let first = first_occurrence(&copy).unwrap_or(copy_start);
    let moved = copy_old.wall(first) + shift - delta;
    let zone_days = (moved.date() - target.wall(first).date()).num_days();

    // (11) Written as the target clock is spelled; UTC as none.
    ZoneCarry::Changed {
        rrule,
        start,
        end,
        exceptions,
        tzid: written_tzid,
        shift_minutes: shift.num_minutes(),
        zone_days,
    }
}

/// [`zone_carry`] over the wire: a question as JSON in, the answer as JSON
/// out. An error only for JSON that is no question at all.
pub fn zone_carry_json(question_json: &str) -> Result<String, serde_json::Error> {
    let question: ZoneCarryQuestion = serde_json::from_str(question_json)?;
    serde_json::to_string(&zone_carry(&question))
}

#[cfg(test)]
mod contract {
    //! The shared fixture.

    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/groupZoneCarry.json"
        ));
        serde_json::from_str(text).expect("the fixture parses")
    }

    #[test]
    fn every_row_gives_its_answer() {
        let doc = fixture();
        let rows = doc["rows"].as_array().expect("rows");
        // Anti-silence: the rows the rule turns on, by name.
        for needed in NEEDED {
            assert!(
                rows.iter().any(|r| r["name"] == *needed),
                "the fixture lost {needed}"
            );
        }
        // Every row that differs, by name, rather than the first.
        let differing: Vec<String> = rows
            .iter()
            .filter_map(|row| {
                let answer: Value = serde_json::from_str(
                    &zone_carry_json(&row["question"].to_string()).expect("a question"),
                )
                .expect("json");
                (answer != row["expect"]).then(|| {
                    format!(
                        "{}: got {answer}, want {}",
                        row["name"].as_str().unwrap_or("?"),
                        row["expect"]
                    )
                })
            })
            .collect();
        assert!(
            differing.is_empty(),
            "{} row(s) differ:\n{}",
            differing.len(),
            differing.join("\n")
        );
    }

    /// The rows each decision and each closed hole turns on.
    pub(super) const NEEDED: &[&str] = &[
        "a-true-copy-as-they-fall-now",
        "a-true-copy-as-the-rule-says",
        "a-copy-in-the-other-half-year-comes-to-the-series-hour",
        "copy-in-june-anchor-in-january-as-they-fall-now",
        "copy-in-january-anchor-in-june-as-the-rule-says",
        "copy-in-january-anchor-in-june-as-they-fall-now",
        "a-copy-an-hour-later-keeps-its-distance",
        "a-copy-already-repaired-is-unchanged",
        "a-copy-already-repaired-gets-a-new-end",
        "a-copy-on-berlin-at-the-old-instants-moves-a-day-earlier",
        "a-copy-on-berlin-from-june-pairs-by-instants",
        "a-copy-in-vienna-keeps-vienna",
        "to-utc-a-copy-on-etc-utc-is-unchanged",
        "to-utc-a-copy-on-berlin-is-written-without-a-zone",
        "troll-on-a-calendar-that-cannot-store-it",
        "a-calendar-that-stores-no-zone",
        "a-copy-without-a-rule",
        "an-all-day-copy",
        "a-copy-whose-rule-cannot-be-read",
        "every-other-week-in-the-other-parity",
        "a-move-into-the-spring-gap",
        "a-rule-with-its-own-hour",
        "a-whole-move-with-a-deletion-and-an-end",
        "across-midnight-with-an-end-on-the-last-occurrence",
        "across-midnight-with-a-guarded-end-by-date",
        "across-midnight-with-a-guarded-end-as-a-cut",
        "a-deletion-that-cancels-nothing-stays",
        "zone-then-a-date-one-day-later",
        "zone-then-a-date-says-only-the-zone-days",
        "zone-and-a-new-length",
        "two-choices-summed",
        "an-hourly-copy",
        // Review round 1: the pairing after carries a later move; a whole
        // week tells the two pairings apart; days the reader cannot follow
        // pair on none.
        "a-repaired-copy-takes-a-later-time",
        "a-repaired-weekday-copy-is-unchanged",
        "a-daily-copy-already-there-is-unchanged",
        "a-utc-copy-at-the-new-instants-keeps-them",
        "a-true-weekday-copy-follows-the-series",
        "a-true-copy-from-midweek-follows-the-series",
        "a-repaired-copy-from-midweek-is-unchanged",
        "a-copy-equal-to-the-saved-series-across-autumn",
        "a-copy-equal-to-the-saved-series-across-spring",
        "a-copy-equal-to-the-saved-series-from-the-fold",
        "a-copy-on-a-day-of-the-year-is-off-pattern",
        "a-copy-on-a-set-position-is-off-pattern",
        // Review round 2: weeks compared by wall days, the copy's clock before
        // the distance, and a move that must land.
        "a-copy-an-hour-after-keeps-it-across-a-change",
        "a-copy-half-an-hour-after-keeps-it-across-a-change",
        "a-copy-on-the-new-clock-near-the-series-stays",
        "a-copy-at-the-applied-series-takes-a-day-earlier",
        "every-other-week-re-anchored-is-refused",
        "every-other-week-re-anchored-an-hour-later-is-refused",
        "every-other-week-a-true-copy-ends-as-the-series",
        "a-true-every-day-copy-takes-the-series-answer",
        "a-copy-three-hours-later-across-spring",
        "a-utc-copy-of-a-berlin-series-across-autumn",
        "a-copy-two-hours-later-across-autumn",
        "a-true-copy-of-a-set-position-series",
        "a-copy-at-the-applied-series-in-the-spring-gap",
        "a-true-yearly-copy-takes-the-series-answer",
        // Review round 3: the zone answer, then the move made after the
        // choice as a drag.
        "a-true-weekday-copy-as-now-then-a-day-earlier",
        "a-true-weekday-copy-to-new-york-then-a-day-later",
        "an-offset-weekday-copy-as-now-then-a-day-earlier",
        "every-other-week-a-true-copy-then-an-hour-later",
        "a-copy-from-the-other-half-year-then-an-hour-later",
    ];

    #[test]
    fn text_that_is_no_question_is_an_error() {
        assert!(zone_carry_json("{\"anchorBefore\":1}").is_err());
        assert!(zone_carry_json("not json").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(rrule: &str, start: &str, tzid: Option<&str>) -> SeriesFacts {
        SeriesFacts {
            rrule: rrule.into(),
            start: start.into(),
            tzid: tzid.map(str::to_string),
        }
    }

    fn question(copy_tzid: Option<&str>) -> ZoneCarryQuestion {
        ZoneCarryQuestion {
            anchor_before: series("FREQ=WEEKLY;BYDAY=MO", "2027-01-04T08:00:00Z", None),
            anchor_after: series(
                "FREQ=WEEKLY;BYDAY=MO",
                "2027-01-04T08:00:00Z",
                Some("Europe/Berlin"),
            ),
            applied_start: "2027-01-04T08:00:00Z".into(),
            moved_minutes: 60,
            new_length_ms: None,
            copy: CopyFacts {
                rrule: Some("FREQ=WEEKLY;BYDAY=MO".into()),
                start: "2027-01-04T08:00:00Z".into(),
                end: "2027-01-04T09:00:00Z".into(),
                exceptions: Vec::new(),
                tzid: copy_tzid.map(str::to_string),
                all_day: false,
            },
            time_zones: SeriesTimeZones::Every,
            ends_by_date: false,
            today: "2026-11-28T12:00:00Z".parse().expect("an instant"),
        }
    }

    /// A shift no clock can hold answers rather than aborting.
    #[test]
    fn a_shift_no_clock_can_hold_is_refused() {
        for moved_minutes in [i64::MAX, i64::MIN, MAX_SHIFT_MINUTES + 1] {
            let q = ZoneCarryQuestion {
                moved_minutes,
                ..question(None)
            };
            assert_eq!(
                zone_carry(&q),
                ZoneCarry::Refused {
                    reason: ZoneCarryRefusal::RuleCannotMove,
                    rule: Some(ClockChangeRefusal::Unreadable),
                },
                "{moved_minutes}"
            );
        }
    }

    /// An empty rule is no series, as a missing one is; a blank one too.
    #[test]
    fn an_empty_rule_is_no_series() {
        for rule in ["", "  "] {
            let mut q = question(None);
            q.copy.rrule = Some(rule.into());
            assert_eq!(
                zone_carry(&q),
                ZoneCarry::Refused {
                    reason: ZoneCarryRefusal::NotASeries,
                    rule: None
                }
            );
        }
    }

    /// Row facts before calendar facts: an all-day copy in a calendar that
    /// stores no zone is refused for being all-day, as the offer says.
    #[test]
    fn row_facts_come_before_calendar_facts() {
        let mut q = question(None);
        q.copy.all_day = true;
        q.time_zones = SeriesTimeZones::NoZone;
        assert_eq!(
            zone_carry(&q),
            ZoneCarry::Refused {
                reason: ZoneCarryRefusal::AllDay,
                rule: None
            }
        );
    }

    /// Every UTC name and none are one clock, and an alias is its zone.
    #[test]
    fn one_clock_by_any_name() {
        let today: DateTime<Utc> = "2026-11-28T12:00:00Z".parse().expect("an instant");
        assert!(same_clock(None, Some("Etc/UTC"), today));
        assert!(same_clock(Some("UTC"), Some("GMT"), today));
        assert!(same_clock(
            Some("Asia/Calcutta"),
            Some("Asia/Kolkata"),
            today
        ));
        assert!(same_clock(
            Some("Europe/Vienna"),
            Some("Europe/Berlin"),
            today
        ));
        assert!(!same_clock(
            Some("Europe/London"),
            Some("Europe/Berlin"),
            today
        ));
        assert!(!same_clock(None, Some("Europe/London"), today));
        // A fixed zone on UTC's offset is not UTC: UTC is no zone (26a).
        assert!(!same_clock(None, Some("Africa/Abidjan"), today));
    }
}
